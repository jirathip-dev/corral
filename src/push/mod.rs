//! The APNs push notifier (D16, issue #26) — the product.
//!
//! Watches the store's delta stream and pushes on exactly two transitions:
//!
//! - **blocked** → a notification carrying the (redacted) prompt text, its
//!   claim (`prompt_hash` + `approval_id`), the canned `choices[]`, kind
//!   and agent id. The iOS lock-screen actions bind their reply to the
//!   payload's `prompt_hash`; a stale notification's reply is refused
//!   (D13: whitelisted canned surface, no free-text from the lock screen).
//! - **done** → a plain completion notification (no reply surface).
//!
//! ## Architecture
//!
//! The notifier is a pure consumer of the store's broadcast channel — no
//! adapter code is touched (herdr's adapter keeps redacting at its own
//! boundary; this module re-redacts anyway, [`crate::push::payload`]).
//! Transition detection uses a per-agent shadow of the last-seen state, so
//! a burst of `output_matched` re-upserts on the same blocked prompt
//! produces exactly one notification (**at most one push per agent per
//! state**, D16 batching): blocked is keyed on `prompt_hash`, done on
//! having left the state.
//!
//! Delivery is per-device over the [`ApnsProvider`] seam (never real APNs
//! in tests), with retry + backoff for transient failures, tokens dropped
//! from the registry when Apple says the device is gone, and every failure
//! logged — the notifier never crashes the daemon and never blocks the
//! store path (pushes run in spawned tasks; a stalled provider only lags
//! this task, which re-arms from the next delta).
//!
//! ## Arming
//!
//! `router()` (crate::api) calls [`Notifier::from_env`] + [`Notifier::start`]
//! once per process. Unconfigured (`CORRAL_APNS_*` absent) → the daemon runs
//! as before, notifier disabled. See [`crate::push::config`] for the
//! provisioning inputs (the `.p8` push key is Guy's).

pub mod config;
pub mod payload;
pub mod provider;

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;
use tokio::sync::broadcast;
use tracing::{debug, info, warn};

use crate::auth::registry::{DeviceRecord, DeviceRegistry};
use crate::core::model::{Agent, AgentState, Delta};
use crate::core::store::Store;

use self::config::Config;
use self::payload::{blocked_payload, done_payload};
use self::provider::{ApnsProvider, PushError};

/// How long a single provider call may take before the notifier gives up
/// and treats it as transient (Apple's own guidance: fail fast, retry
/// later — never hang the notification pipeline).
const PROVIDER_CALL_TIMEOUT: Duration = Duration::from_secs(15);
/// Retry ladder for transient provider failures (network/429/5xx).
const RETRY_ATTEMPTS: usize = 3;
const RETRY_BASE: Duration = Duration::from_secs(1);
/// A lagged notifier clears its shadow so the next delta re-evaluates
/// every agent (a missed transition must not be silenced forever by stale
/// dedupe state).
struct Shadow {
    state: AgentState,
    /// The prompt_hash of the last blocked push (or None when not blocked).
    blocked_hash: Option<String>,
    /// True once a done push fired for this agent; cleared when the agent
    /// leaves Done (so a new done episode can notify again).
    done_pushed: bool,
}

/// The push notifier. Clone-safe; [`Notifier::start`] spawns the watcher.
#[derive(Clone)]
pub struct Notifier {
    store: Store,
    registry: Arc<DeviceRegistry>,
    provider: Arc<dyn ApnsProvider>,
    config: Config,
    /// Fired once the watcher holds a broadcast subscription — tests await
    /// it before applying changes so a flushed delta can never race the
    /// subscription (a delta broadcast before subscribe is missed forever).
    ready: Arc<tokio::sync::Notify>,
}

impl std::fmt::Debug for Notifier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // No secrets: only the topic/endpoint and registry size print.
        f.debug_struct("Notifier")
            .field("config", &self.config)
            .field("devices", &self.registry.device_count())
            .finish_non_exhaustive()
    }
}

impl Notifier {
    pub fn new(
        store: Store,
        registry: Arc<DeviceRegistry>,
        provider: Arc<dyn ApnsProvider>,
        config: Config,
    ) -> Self {
        Self {
            store,
            registry,
            provider,
            config,
            ready: Arc::new(tokio::sync::Notify::new()),
        }
    }

    /// Test seam: completes once the watcher holds its subscription.
    pub async fn ready(&self) {
        self.ready.notified().await;
    }

    /// Arm from `CORRAL_APNS_*` env. `None` when unconfigured; the daemon
    /// then runs with push disabled (documented first-run state).
    pub fn from_env(store: Store, registry: Arc<DeviceRegistry>) -> Option<Arc<Self>> {
        let config = Config::from_env()?;
        let signing_key = match Config::load_signing_key(&config.auth_key_path) {
            Ok(key) => key,
            Err(e) => {
                warn!(error = %e, "push notifier disabled: bad CORRAL_APNS_AUTH_KEY_PATH");
                return None;
            }
        };
        let provider: Arc<dyn ApnsProvider> = match provider::RealApnsProvider::new(
            config.clone(),
            signing_key,
        ) {
            Ok(p) => Arc::new(p),
            Err(e) => {
                warn!(error = %e, "push notifier disabled: apns provider init failed");
                return None;
            }
        };
        info!(
            endpoint = ?config.endpoint,
            topic = %config.topic,
            "push notifier armed (APNs)"
        );
        Some(Arc::new(Self::new(store, registry, provider, config)))
    }

    /// Spawn the watcher. Never panics and never blocks the store path:
    /// provider calls run in per-delivery tasks, the watcher itself only
    /// reads the broadcast channel.
    pub fn start(self: Arc<Self>) {
        tokio::spawn(async move { self.run().await });
    }

    /// Watch deltas forever. On `Lagged` (a burst the channel could not
    /// buffer) the shadow is cleared so the next delta re-evaluates every
    /// agent; on `Closed` (store shutdown) the watcher exits quietly.
    async fn run(&self) {
        let mut rx = self.store.subscribe();
        self.ready.notify_waiters();
        let mut shadow: HashMap<String, Shadow> = HashMap::new();
        // Boot seed (F6): the watcher only reacts to deltas, so an agent
        // that is ALREADY blocked when the daemon restarts would never get
        // its notification. Seed the shadow from the current snapshot and
        // push for already-blocked agents (fire-and-forget; deduped by
        // prompt_hash like any other blocked push).
        for agent in self.store.snapshot().await.agents.values() {
            if agent.state == AgentState::Blocked
                && let Some(push) = self.transition_push(agent, &mut shadow)
            {
                self.deliver(&push).await;
            }
        }
        loop {
            match rx.recv().await {
                Ok(delta) => self.handle_delta(&delta, &mut shadow).await,
                Err(broadcast::error::RecvError::Lagged(skipped)) => {
                    warn!(skipped, "push watcher lagged; re-evaluating from next delta");
                    shadow.clear();
                }
                Err(broadcast::error::RecvError::Closed) => {
                    debug!("push watcher: store closed");
                    return;
                }
            }
        }
    }

    /// One coalesced delta: last-record-wins per agent (the store already
    /// deduped within the window), then transition detection, then
    /// delivery to every registered device.
    async fn handle_delta(&self, delta: &Delta, shadow: &mut HashMap<String, Shadow>) {
        let mut last_seen: HashMap<&str, &Agent> = HashMap::with_capacity(delta.upd.len());
        for agent in &delta.upd {
            last_seen.insert(agent.agent_id.as_str(), agent);
        }
        for agent in last_seen.values() {
            if let Some(push) = self.transition_push(agent, shadow) {
                self.deliver(&push).await;
            }
        }
        for removed in &delta.del {
            shadow.remove(removed);
        }
    }

    /// Decide whether this upsert is a new notification (batching: at most
    /// one push per agent per state), and record the new shadow. The shadow
    /// is ALWAYS updated before returning — a fired push must mark its
    /// state as seen, or the next identical upsert would re-push.
    fn transition_push(&self, agent: &Agent, shadow: &mut HashMap<String, Shadow>) -> Option<Value> {
        let prev = shadow.get(&agent.agent_id);
        let mut current = Shadow {
            state: agent.state,
            blocked_hash: None,
            done_pushed: false,
        };
        let push = match agent.state {
            AgentState::Blocked => {
                let hash = agent
                    .waiting_on
                    .as_ref()
                    .map(|w| w.prompt_hash.clone());
                let is_new = match prev {
                    Some(p) => p.state != AgentState::Blocked || p.blocked_hash != hash,
                    None => true,
                };
                current.blocked_hash = hash;
                if is_new {
                    if let Some(waiting) = agent.waiting_on.as_ref() {
                        let approval_id = if waiting.approval_id.is_empty() {
                            crate::approve::approval_id_for(&agent.agent_id, &waiting.prompt_hash)
                        } else {
                            waiting.approval_id.clone()
                        };
                        Some(blocked_payload(
                            &agent.agent_id,
                            agent.display_name.as_deref(),
                            &agent.workspace,
                            waiting.kind,
                            &waiting.prompt,
                            &waiting.prompt_hash,
                            &approval_id,
                            &waiting.choices,
                        ))
                    } else {
                        None
                    }
                } else {
                    None
                }
            }
            AgentState::Done => {
                let is_new = match prev {
                    Some(p) => p.state != AgentState::Done || !p.done_pushed,
                    None => true,
                };
                // Sticky once the agent is seen in Done: consecutive done
                // re-upserts must not re-push (F1 — the flag records that a
                // done episode was already notified, not that THIS upsert
                // pushed). Cleared when the agent leaves Done via the fresh
                // shadow, so a new episode can notify again.
                current.done_pushed = true;
                if is_new {
                    Some(done_payload(
                        &agent.agent_id,
                        agent.display_name.as_deref(),
                        &agent.workspace,
                    ))
                } else {
                    None
                }
            }
            _ => None,
        };
        shadow.insert(agent.agent_id.clone(), current);
        push
    }

    /// Deliver one notification to every registered, non-revoked,
    /// non-expired device that has a push token. Each delivery runs in its
    /// own task with bounded retry, so a slow provider can never stall the
    /// watcher or the store.
    async fn deliver(&self, payload: &Value) {
        let devices: Vec<DeviceRecord> = self
            .registry
            .records()
            .into_iter()
            .filter(|rec| !rec.revoked && rec.device_token.is_some() && !rec.expired())
            .collect();
        if devices.is_empty() {
            debug!("push notification ready but no registered devices");
            return;
        }
        let payload = payload.clone();
        for device in devices {
            let provider = self.provider.clone();
            let registry = self.registry.clone();
            let token = device.device_token.clone().expect("filtered above");
            let payload = payload.clone();
            tokio::spawn(async move {
                if let Err(e) =
                    deliver_with_retry(provider.as_ref(), &token, &payload, registry.as_ref())
                        .await
                {
                    warn!(key_id = %device.key_id, error = %e, "apns delivery failed");
                }
            });
        }
    }
}

/// Bounded-retry delivery for one device: transient failures retry with
/// doubling backoff; `Unregistered` (Apple: bad/removed token) drops the
/// token from the registry — the device re-registers on next launch
/// (revocation, D16); everything else is logged and dropped.
async fn deliver_with_retry(
    provider: &dyn ApnsProvider,
    device_token: &str,
    payload: &Value,
    registry: &DeviceRegistry,
) -> Result<(), PushError> {
    let mut backoff = RETRY_BASE;
    for attempt in 0..RETRY_ATTEMPTS {
        if attempt > 0 {
            tokio::time::sleep(backoff).await;
            backoff *= 2;
        }
        match tokio::time::timeout(PROVIDER_CALL_TIMEOUT, provider.push(device_token, payload))
            .await
        {
            Ok(Ok(())) => return Ok(()),
            Ok(Err(PushError::Unregistered)) => {
                // Apple no longer knows this device: drop the token so the
                // next block does not keep failing against a dead address.
                if let Err(e) = registry.set_device_token_by_token(device_token, None) {
                    warn!(error = %e, "failed to drop unregistered device token");
                }
                return Err(PushError::Unregistered);
            }
            Ok(Err(e)) if e.is_retryable() && attempt + 1 < RETRY_ATTEMPTS => {
                warn!(attempt, error = %e, "apns retryable failure, backing off");
                continue;
            }
            Ok(Err(e)) => return Err(e),
            Err(_) if attempt + 1 < RETRY_ATTEMPTS => {
                warn!(attempt, "apns call timed out, backing off");
                continue;
            }
            Err(_) => {
                return Err(PushError::Retryable {
                    status: None,
                    reason: "provider call timed out".to_string(),
                })
            }
        }
    }
    unreachable!("loop always returns");
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use tokio::sync::mpsc;

    use super::*;
    use crate::auth::registry::DeviceRegistry;
    use crate::auth::test_support;
    use crate::core::model::{Change, WaitingOn, WaitingOnKind, Workspace};
    use crate::push::provider::MockProvider;

    fn test_config() -> Config {
        Config {
            team_id: "t".to_string(),
            key_id: "k".to_string(),
            auth_key_path: String::new(),
            endpoint: config::Endpoint::Sandbox,
            topic: "com.corral.fleetnotifier".to_string(),
        }
    }

    /// A blocked agent whose prompt carries a secret-shaped token: the
    /// stored record is deliberately NOT redacted to prove the push
    /// boundary redacts independently of the adapter.
    fn blocked_agent(id: &str, hash: &str, prompt: &str) -> Agent {
        Agent {
            agent_id: id.to_string(),
            source: "herdr".to_string(),
            tool: "claude".to_string(),
            state: AgentState::Blocked,
            reason: None,
            seq: 1,
            ts: 1,
            capabilities: vec!["approve".to_string()],
            waiting_on: Some(WaitingOn {
                kind: WaitingOnKind::Menu,
                prompt: prompt.to_string(),
                prompt_hash: hash.to_string(),
                approval_id: format!("{id}:{hash}"),
                choices: vec!["y".to_string(), "n".to_string()],
            }),
            cost: None,
            parent_id: None,
            host: None,
            workspace: Workspace::default(),
            attachment: None,
            display_name: Some("builder".to_string()),
            title: None,
        }
    }

    fn done_agent(id: &str) -> Agent {
        Agent {
            agent_id: id.to_string(),
            source: "herdr".to_string(),
            tool: "claude".to_string(),
            state: AgentState::Done,
            reason: None,
            seq: 2,
            ts: 2,
            capabilities: vec![],
            waiting_on: None,
            cost: None,
            parent_id: None,
            host: None,
            workspace: Workspace::default(),
            attachment: None,
            display_name: Some("builder".to_string()),
            title: None,
        }
    }

    /// Register one device with a push token; returns its key_id.
    fn device_with_token(registry: &DeviceRegistry) -> String {
        let (signing, pubkey) = test_support::keypair();
        let token = registry.registration_token();
        let rec = registry
            .register(&token, pubkey, std::time::Duration::from_secs(3600))
            .expect("register");
        registry
            .set_device_token(&rec.key_id, Some("a1b2c3d4e5f6"))
            .expect("set token");
        let _ = signing;
        rec.key_id
    }

    fn workspace_repo() -> Workspace {
        Workspace {
            repo: Some("jirathip-k/corral".to_string()),
            branch: Some("g26/apns-push".to_string()),
            ..Default::default()
        }
    }

    /// Build a notifier over a fresh store + registry + mock provider.
    type Harness = (
        Store,
        Arc<DeviceRegistry>,
        Arc<Notifier>,
        mpsc::UnboundedReceiver<(String, Value)>,
    );

    fn harness() -> Harness {
        let store = Store::new();
        let (registry, _, _, _dir) = test_support::setup();
        let (provider, received) = MockProvider::new();
        let notifier = Arc::new(Notifier::new(
            store.clone(),
            registry.clone(),
            provider,
            test_config(),
        ));
        (store, registry, notifier, received)
    }

    #[tokio::test]
    async fn blocked_transition_pushes_within_budget() {
        // Acceptance #1: blocked -> push well inside ~10s of the
        // transition. The store coalescer (250ms foreground tick) + the
        // watcher + the mock provider must deliver within the budget.
        let (store, registry, notifier, mut received) = harness();
        device_with_token(&registry);
        let coalescer = store.clone();
        tokio::spawn(async move { coalescer.run_coalescer().await });
        notifier.clone().start();

        let mut agent = blocked_agent("herdr:ses-1", "sha256:aaa", "proceed? [y/n]");
        agent.workspace = workspace_repo();
        store.apply(Change::upsert(agent)).await;

        let (token, payload) = tokio::time::timeout(Duration::from_secs(10), received.recv())
            .await
            .expect("push must arrive within 10s of the blocked transition")
            .expect("provider channel closed");
        assert_eq!(token, "a1b2c3d4e5f6");
        assert_eq!(payload["type"], "blocked");
        assert_eq!(payload["agent_id"], "herdr:ses-1");
        assert_eq!(payload["prompt_hash"], "sha256:aaa");
        assert_eq!(payload["aps"]["alert"]["body"], "proceed? [y/n]");
        assert_eq!(payload["repo"], "jirathip-k/corral");
    }

    #[tokio::test]
    async fn done_transition_pushes_plain_completion() {
        let (store, registry, notifier, mut received) = harness();
        device_with_token(&registry);
        let coalescer = store.clone();
        tokio::spawn(async move { coalescer.run_coalescer().await });
        notifier.clone().start();

        store.apply(Change::upsert(done_agent("herdr:ses-9"))).await;

        let (_token, payload) = tokio::time::timeout(Duration::from_secs(10), received.recv())
            .await
            .expect("done push within 10s")
            .expect("provider channel closed");
        assert_eq!(payload["type"], "done");
        assert_eq!(payload["agent_id"], "herdr:ses-9");
        assert!(payload.get("category").is_none(), "done: no reply surface");
    }

    #[tokio::test]
    async fn prompt_text_is_redacted_before_leaving_the_machine() {
        let (store, registry, notifier, mut received) = harness();
        device_with_token(&registry);
        let coalescer = store.clone();
        tokio::spawn(async move { coalescer.run_coalescer().await });
        notifier.clone().start();

        // Raw secret-shaped text in the STORE (bypassing the adapter's
        // redaction) must not reach the payload.
        let agent = blocked_agent(
            "herdr:ses-2",
            "sha256:bbb",
            "approve: token ghp_AbCdEf1234567890XyZ and sk-ant-api03-0123456789abcdef0123456789abcdef?",
        );
        store.apply(Change::upsert(agent)).await;

        let (_token, payload) = tokio::time::timeout(Duration::from_secs(10), received.recv())
            .await
            .expect("push within 10s")
            .expect("channel closed");
        let body = payload["aps"]["alert"]["body"].as_str().unwrap();
        assert!(!body.contains("ghp_"), "PAT leaked in push payload: {body}");
        assert!(!body.contains("sk-ant-"), "Anthropic key leaked: {body}");
        assert!(body.contains("[REDACTED]"));
    }

    #[tokio::test]
    async fn batching_a_burst_on_the_same_prompt_pushes_once() {
        // D16 batching: output_matched re-upserts the same blocked prompt
        // repeatedly — exactly one notification per (agent, prompt_hash).
        let (store, registry, notifier, mut received) = harness();
        device_with_token(&registry);
        notifier.clone().start();
        notifier.ready().await;

        let agent = || blocked_agent("herdr:ses-3", "sha256:ccc", "same prompt");
        for _ in 0..6 {
            store.apply(Change::upsert(agent())).await;
            store.flush().await;
        }

        let (_token, payload) = tokio::time::timeout(Duration::from_secs(5), received.recv())
            .await
            .expect("first push arrives")
            .expect("channel closed");
        assert_eq!(payload["prompt_hash"], "sha256:ccc");
        assert!(
            tokio::time::timeout(Duration::from_millis(300), received.recv())
                .await
                .is_err(),
            "no second push for the same blocked prompt"
        );
    }

    #[tokio::test]
    async fn new_prompt_while_blocked_pushes_again_then_done_pushes_once() {
        let (store, registry, notifier, mut received) = harness();
        device_with_token(&registry);
        let coalescer = store.clone();
        tokio::spawn(async move { coalescer.run_coalescer().await });
        notifier.clone().start();
        notifier.ready().await;

        store
            .apply(Change::upsert(blocked_agent("herdr:ses-4", "sha256:1", "first?")))
            .await;
        let (_, p1) = received.recv().await.unwrap();
        assert_eq!(p1["prompt_hash"], "sha256:1");

        // A NEW prompt while blocked: a fresh claim, a fresh push.
        store
            .apply(Change::upsert(blocked_agent("herdr:ses-4", "sha256:2", "second?")))
            .await;
        let (_, p2) = received.recv().await.unwrap();
        assert_eq!(p2["prompt_hash"], "sha256:2");

        // Re-upserts of the second prompt do not re-push.
        store
            .apply(Change::upsert(blocked_agent("herdr:ses-4", "sha256:2", "second?")))
            .await;
        store.flush().await;
        assert!(
            tokio::time::timeout(Duration::from_millis(300), received.recv())
                .await
                .is_err(),
            "same prompt re-upsert must not re-push"
        );

        // Done: one completion push, and staying done does not re-push.
        store.apply(Change::upsert(done_agent("herdr:ses-4"))).await;
        let (_t, p3) = received.recv().await.unwrap();
        assert_eq!(p3["type"], "done");
        store.apply(Change::upsert(done_agent("herdr:ses-4"))).await;
        store.flush().await;
        assert!(
            tokio::time::timeout(Duration::from_millis(300), received.recv())
                .await
                .is_err(),
            "staying done must not re-push"
        );
    }

    #[tokio::test]
    async fn repeated_done_upserts_push_exactly_once() {
        // F1 regression: the done_pushed shadow flipped on every done
        // re-upsert, so 5 identical done upserts pushed 3 times. The flag
        // must be sticky once the agent is seen in Done.
        let (store, registry, notifier, mut received) = harness();
        device_with_token(&registry);
        notifier.clone().start();
        notifier.ready().await;

        for _ in 0..5 {
            store.apply(Change::upsert(done_agent("herdr:ses-d1"))).await;
            store.flush().await;
        }

        let (_token, payload) = tokio::time::timeout(Duration::from_secs(5), received.recv())
            .await
            .expect("first done push arrives")
            .expect("channel closed");
        assert_eq!(payload["type"], "done");
        assert!(
            tokio::time::timeout(Duration::from_millis(300), received.recv())
                .await
                .is_err(),
            "five identical done upserts must produce exactly one push"
        );
    }

    #[tokio::test]
    async fn rejected_400_keeps_device_token_in_registry() {
        // F2: a 400 PayloadTooLarge (our bug) is Rejected, never
        // Unregistered — the device token must survive in the registry.
        let (provider, _received) = MockProvider::new();
        provider.fail_with(PushError::Rejected {
            status: 400,
            reason: "PayloadTooLarge".to_string(),
        });
        let (registry, _, _, _dir) = test_support::setup();
        let key_id = device_with_token(&registry);
        let payload = serde_json::json!({"type": "blocked"});
        let delivery_registry = Arc::clone(&registry);
        let handle = tokio::spawn(async move {
            deliver_with_retry(
                provider.as_ref(),
                "a1b2c3d4e5f6",
                &payload,
                delivery_registry.as_ref(),
            )
            .await
        });
        let result = tokio::time::timeout(Duration::from_secs(10), handle)
            .await
            .expect("delivery completes")
            .expect("no panic");
        assert!(
            matches!(result, Err(PushError::Rejected { status: 400, .. })),
            "PayloadTooLarge must map to Rejected, got {result:?}"
        );
        assert_eq!(
            registry.get(&key_id).unwrap().device_token.as_deref(),
            Some("a1b2c3d4e5f6"),
            "a config bug must NOT deregister the device"
        );
    }

    #[tokio::test]
    async fn unregistered_drops_device_token_from_registry() {
        // The flip side of F2: a genuinely dead device still drops the
        // token so the next block does not fail against a dead address.
        let (provider, _received) = MockProvider::new();
        provider.fail_with(PushError::Unregistered);
        let (registry, _, _, _dir) = test_support::setup();
        let key_id = device_with_token(&registry);
        let payload = serde_json::json!({"type": "blocked"});
        let delivery_registry = Arc::clone(&registry);
        let handle = tokio::spawn(async move {
            deliver_with_retry(
                provider.as_ref(),
                "a1b2c3d4e5f6",
                &payload,
                delivery_registry.as_ref(),
            )
            .await
        });
        let result = tokio::time::timeout(Duration::from_secs(10), handle)
            .await
            .expect("delivery completes")
            .expect("no panic");
        assert_eq!(result, Err(PushError::Unregistered));
        assert_eq!(
            registry.get(&key_id).unwrap().device_token,
            None,
            "a dead device's token is dropped (re-register on next launch)"
        );
    }

    #[tokio::test]
    async fn devices_without_tokens_are_skipped_and_revoked_never_push() {
        let (store, registry, notifier, mut received) = harness();
        let (signing, pubkey) = test_support::keypair();
        let _ = signing;
        let token = registry.registration_token();
        let rec = registry
            .register(&token, pubkey, std::time::Duration::from_secs(3600))
            .expect("register");
        // Device registered but NO push token, plus a revoked device with a
        // token: neither may receive anything.
        let rec2_key = device_with_token(&registry);
        registry.set_revoked(&rec2_key, true).expect("revoke");
        let _ = rec;
        let coalescer = store.clone();
        tokio::spawn(async move { coalescer.run_coalescer().await });
        notifier.clone().start();

        store.apply(Change::upsert(blocked_agent("herdr:ses-5", "sha256:ddd", "go?"))).await;

        assert!(
            tokio::time::timeout(Duration::from_millis(500), received.recv())
                .await
                .is_err(),
            "no push: only a token-less and a revoked device exist"
        );
    }

    #[tokio::test]
    async fn transient_failures_retry_with_backoff_and_succeed() {
        // Retry ladder: two transient failures then success — the push must
        // still be delivered exactly once (never dropped, never duplicated).
        let (provider, mut received) = MockProvider::new();
        provider.fail_next(2);
        let registry = {
            let (registry, _, _, _dir) = test_support::setup();
            registry
        };
        let payload = serde_json::json!({"type": "blocked"});
        let handle = tokio::spawn(async move {
            deliver_with_retry(provider.as_ref(), "tok", &payload, registry.as_ref()).await
        });
        let result = tokio::time::timeout(Duration::from_secs(10), handle)
            .await
            .expect("retry ladder completes in time")
            .expect("no panic");
        assert!(result.is_ok(), "transient failures retry to success");
        let (_token, p) = received.recv().await.unwrap();
        assert_eq!(p["type"], "blocked");
    }
}
