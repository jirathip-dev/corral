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
//! store path. The watcher awaits the (bounded) delivery results so a
//! failed delivery is recorded in the shadow and re-attempted by the
//! periodic reconcile tick (N4) instead of being silently swallowed.
//!
//! ## Arming
//!
//! `main.rs` calls [`Notifier::from_env`] + [`Notifier::start`] once per
//! process. Unconfigured (`CORRAL_APNS_*` absent) → the daemon runs
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
use self::provider::{ApnsProvider, PushError, token_hash};

/// How long a single provider call may take before the notifier gives up
/// and treats it as transient (Apple's own guidance: fail fast, retry
/// later — never hang the notification pipeline).
const PROVIDER_CALL_TIMEOUT: Duration = Duration::from_secs(15);
/// Retry ladder for transient provider failures (network/429/5xx).
const RETRY_ATTEMPTS: usize = 3;
const RETRY_BASE: Duration = Duration::from_secs(1);
/// Reconcile cadence (N4): a Blocked agent whose last delivery failed (or
/// was dropped because no device was enrolled) is re-evaluated on this
/// tick, so a permanently-failed or missed delivery heals within one
/// interval instead of being recorded as handled forever.
const RECONCILE_TICK: Duration = Duration::from_secs(60);
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
    /// True once the last blocked push for `blocked_hash` was DELIVERED
    /// successfully to every eligible device (N4). The reconcile tick only
    /// re-pushes a Blocked agent whose shadow lacks this marker, so a
    /// failed delivery — or one skipped for lack of devices — is retried,
    /// never silently recorded as handled. Reset by any fresh claim.
    delivered_ok: bool,
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

    /// Test seam: completes once the watcher has subscribed AND run its
    /// boot seed (N14) — a caller can safely apply changes afterwards
    /// without racing the seed loop. `notify_one` stores a permit, so a
    /// waiter that registers late (after the watcher signalled) still
    /// wakes instead of hanging forever.
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
        let provider: Arc<dyn ApnsProvider> =
            match provider::RealApnsProvider::new(config.clone(), signing_key) {
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
    /// buffer) the shadow is cleared and re-seeded from the snapshot so an
    /// agent whose delta was dropped is re-evaluated instead of being
    /// silenced forever (N1); on `Closed` (store shutdown) the watcher
    /// exits quietly. A periodic reconcile tick re-pushes Blocked agents
    /// whose last delivery failed or never happened (N4).
    async fn run(&self) {
        let mut rx = self.store.subscribe();
        let mut shadow: HashMap<String, Shadow> = HashMap::new();
        // Boot seed (F6): the watcher only reacts to deltas, so an agent
        // that is ALREADY blocked when the daemon restarts would never get
        // its notification. Seed the shadow from the current snapshot and
        // push for already-blocked agents (fire-and-forget; deduped by
        // prompt_hash like any other blocked push).
        self.seed_from_snapshot(&mut shadow).await;
        // N14: ready() must mean "seed done", not "subscription held" —
        // notify_one stores a permit so a waiter that registered before
        // the seed loop still wakes, and a late waiter gets the permit too.
        self.ready.notify_one();
        let mut reconcile = tokio::time::interval(RECONCILE_TICK);
        reconcile.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        reconcile.tick().await; // discard the immediate first tick
        loop {
            tokio::select! {
                recv = rx.recv() => match recv {
                    Ok(delta) => self.handle_delta(&delta, &mut shadow).await,
                    Err(broadcast::error::RecvError::Lagged(skipped)) => {
                        warn!(skipped, "push watcher lagged; re-seeding from snapshot");
                        shadow.clear();
                        self.seed_from_snapshot(&mut shadow).await;
                    }
                    Err(broadcast::error::RecvError::Closed) => {
                        debug!("push watcher: store closed");
                        return;
                    }
                },
                _ = reconcile.tick() => {
                    self.reconcile(&mut shadow).await;
                }
            }
        }
    }

    /// Re-evaluate every currently-Blocked agent against the shadow and
    /// re-push those whose last delivery did not succeed (N4). Also seeds
    /// the shadow for agents never seen by a delta (boot/Lagged, N1).
    /// Trades a silent miss for a possible duplicate — the trade the module
    /// doc already endorses (dedupe is by prompt_hash on the phone).
    async fn reconcile(&self, shadow: &mut HashMap<String, Shadow>) {
        for agent in self.store.snapshot().await.agents.values() {
            if agent.state == AgentState::Blocked {
                let undelivered = match shadow.get(&agent.agent_id) {
                    Some(s) => {
                        s.state != AgentState::Blocked
                            || s.blocked_hash
                                != agent.waiting_on.as_ref().map(|w| w.prompt_hash.clone())
                            || !s.delivered_ok
                    }
                    None => true,
                };
                if undelivered {
                    self.process_agent(agent, true, shadow).await;
                }
            }
        }
    }

    /// Seed (or re-seed, after a Lagged drop) the shadow from the current
    /// snapshot: already-blocked agents get their notification now, exactly
    /// as if their delta had just arrived (N1/F6).
    async fn seed_from_snapshot(&self, shadow: &mut HashMap<String, Shadow>) {
        for agent in self.store.snapshot().await.agents.values() {
            if agent.state == AgentState::Blocked {
                self.process_agent(agent, false, shadow).await;
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
            self.process_agent(agent, false, shadow).await;
        }
        for removed in &delta.del {
            shadow.remove(removed);
        }
    }

    /// Decide + deliver for one agent, recording the outcome in the shadow
    /// (N4: a failed delivery leaves the Blocked agent re-eligible for the
    /// reconcile tick; a successful one sets `delivered_ok`).
    async fn process_agent(
        &self,
        agent: &Agent,
        force: bool,
        shadow: &mut HashMap<String, Shadow>,
    ) {
        let agent_id = agent.agent_id.clone();
        if let Some(push) = self.transition_push(agent, force, shadow) {
            let delivered = self.deliver(&push).await;
            if delivered
                && let Some(s) = shadow.get_mut(&agent_id)
                && s.state == agent.state
                && s.blocked_hash == agent.waiting_on.as_ref().map(|w| w.prompt_hash.clone())
            {
                s.delivered_ok = true;
            }
        }
    }

    /// Decide whether this upsert is a new notification (batching: at most
    /// one push per agent per state), and record the new shadow. The shadow
    /// is ALWAYS updated before returning — a fired push must mark its
    /// state as seen, or the next identical upsert would re-push. `force`
    /// (reconcile tick) re-pushes a Blocked agent whose claim is unchanged
    /// but whose last delivery failed or was skipped for lack of devices
    /// (N4); the shadow is still written, with `delivered_ok` reset so a
    /// fresh delivery outcome is what the next tick reads.
    fn transition_push(
        &self,
        agent: &Agent,
        force: bool,
        shadow: &mut HashMap<String, Shadow>,
    ) -> Option<Value> {
        let prev = shadow.get(&agent.agent_id);
        let mut current = Shadow {
            state: agent.state,
            blocked_hash: None,
            done_pushed: false,
            delivered_ok: false,
        };
        let push = match agent.state {
            AgentState::Blocked => {
                let hash = agent.waiting_on.as_ref().map(|w| w.prompt_hash.clone());
                let is_new = force
                    || match prev {
                        Some(p) => p.state != AgentState::Blocked || p.blocked_hash != hash,
                        None => true,
                    };
                current.blocked_hash = hash;
                // An unchanged, already-delivered claim keeps its marker
                // (the reconcile tick must NOT re-push a delivered block);
                // anything new starts undelivered.
                if !is_new {
                    current.delivered_ok = prev.is_some_and(|p| p.delivered_ok);
                }
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

    /// Deliver one notification to every push-eligible device. Each delivery
    /// runs in its own task with bounded retry, so a slow provider can
    /// never stall the store; the watcher awaits the bounded results so it
    /// can record whether the block was actually delivered (N4).
    ///
    /// Returns true when the notification reached every eligible device
    /// (or there were none to reach is NOT success: a device that enrolls
    /// one second after a block must still learn about the live block on
    /// the next reconcile tick). The shadow's `delivered_ok` is only set on
    /// a fully-successful delivery.
    async fn deliver(&self, payload: &Value) -> bool {
        let devices: Vec<DeviceRecord> = self
            .registry
            .records()
            .into_iter()
            .filter(DeviceRecord::push_eligible)
            .collect();
        if devices.is_empty() {
            debug!("push notification ready but no registered devices");
            return false;
        }
        let mut handles = Vec::with_capacity(devices.len());
        for device in devices {
            let provider = self.provider.clone();
            let registry = self.registry.clone();
            let token = device.device_token.clone().expect("filtered above");
            let payload = payload.clone();
            handles.push(tokio::spawn(async move {
                match deliver_with_retry(provider.as_ref(), &token, &payload, registry.as_ref())
                    .await
                {
                    Ok(()) => true,
                    Err(e) => {
                        warn!(
                            key_id = %device.key_id,
                            device_token = %token_hash(&token),
                            error = %e,
                            "apns delivery failed"
                        );
                        false
                    }
                }
            }));
        }
        let mut all_ok = true;
        for handle in handles {
            all_ok &= handle.await.unwrap_or(false);
        }
        all_ok
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
                warn!(
                    attempt,
                    device_token = %token_hash(device_token),
                    error = %e,
                    "apns retryable failure, backing off"
                );
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
                });
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
        assert_eq!(token, "a1b2c3d4e5f6"); // gitleaks:allow — fixture device token
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
            .apply(Change::upsert(blocked_agent(
                "herdr:ses-4",
                "sha256:1",
                "first?",
            )))
            .await;
        let (_, p1) = received.recv().await.unwrap();
        assert_eq!(p1["prompt_hash"], "sha256:1");

        // A NEW prompt while blocked: a fresh claim, a fresh push.
        store
            .apply(Change::upsert(blocked_agent(
                "herdr:ses-4",
                "sha256:2",
                "second?",
            )))
            .await;
        let (_, p2) = received.recv().await.unwrap();
        assert_eq!(p2["prompt_hash"], "sha256:2");

        // Re-upserts of the second prompt do not re-push.
        store
            .apply(Change::upsert(blocked_agent(
                "herdr:ses-4",
                "sha256:2",
                "second?",
            )))
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
            store
                .apply(Change::upsert(done_agent("herdr:ses-d1")))
                .await;
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

        store
            .apply(Change::upsert(blocked_agent(
                "herdr:ses-5",
                "sha256:ddd",
                "go?",
            )))
            .await;

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

    #[tokio::test]
    async fn boot_seed_pushes_agents_already_blocked_before_start() {
        // F6 regression: the watcher only reacts to deltas, so an agent
        // that is ALREADY blocked when the daemon (re)starts must be
        // notified by the boot seed — the store is populated BEFORE start().
        let (store, registry, notifier, mut received) = harness();
        device_with_token(&registry);
        store
            .apply(Change::upsert(blocked_agent(
                "herdr:ses-boot",
                "sha256:boot",
                "boot?",
            )))
            .await;
        notifier.clone().start();
        notifier.ready().await;

        let (_token, payload) = tokio::time::timeout(Duration::from_secs(5), received.recv())
            .await
            .expect("boot seed pushes the already-blocked agent")
            .expect("provider channel closed");
        assert_eq!(payload["type"], "blocked");
        assert_eq!(payload["agent_id"], "herdr:ses-boot");
        assert_eq!(payload["prompt_hash"], "sha256:boot");
    }

    /// A provider that parks every push on a semaphore until the test adds
    /// permits — lets a test stall the watcher mid-delivery while the store
    /// floods. Each permit admits one push; the test adds exactly as many
    /// as it expects to flow.
    struct GatedProvider {
        tx: mpsc::UnboundedSender<(String, Value)>,
        gate: Arc<tokio::sync::Semaphore>,
        in_flight: Arc<std::sync::atomic::AtomicUsize>,
    }

    /// What `GatedProvider::new` hands back: the provider, the delivery
    /// receiver, the gate that releases queued pushes, and the in-flight
    /// counter the concurrency assertions read.
    type GatedHarness = (
        Arc<GatedProvider>,
        mpsc::UnboundedReceiver<(String, Value)>,
        Arc<tokio::sync::Semaphore>,
        Arc<std::sync::atomic::AtomicUsize>,
    );

    impl GatedProvider {
        fn new() -> GatedHarness {
            let (tx, rx) = mpsc::unbounded_channel();
            let gate = Arc::new(tokio::sync::Semaphore::new(0));
            let in_flight = Arc::new(std::sync::atomic::AtomicUsize::new(0));
            (
                Arc::new(Self {
                    tx,
                    gate: gate.clone(),
                    in_flight: in_flight.clone(),
                }),
                rx,
                gate,
                in_flight,
            )
        }
    }

    impl ApnsProvider for GatedProvider {
        fn push<'a>(
            &'a self,
            token: &'a str,
            payload: &'a Value,
        ) -> futures::future::BoxFuture<'a, Result<(), PushError>> {
            Box::pin(async move {
                self.in_flight
                    .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                let _permit = self.gate.acquire().await.expect("gate never closed");
                self.tx
                    .send((token.to_string(), payload.clone()))
                    .map_err(|_| PushError::Rejected {
                        status: 500,
                        reason: "mock receiver dropped".to_string(),
                    })
            })
        }
    }

    #[tokio::test]
    async fn lagged_watcher_re_seeds_and_never_loses_the_block() {
        // N1: a burst that overflows the broadcast cap (256) drops queued
        // deltas. The Lagged arm must re-seed from the snapshot, or an
        // agent whose blocked delta was dropped stays Blocked forever with
        // no notification. Deterministic via the gate: the watcher parks
        // inside the FIRST delivery while the store floods past the cap.
        let store = Store::new();
        let (registry, _, _, _dir) = test_support::setup();
        let (provider, mut received, gate, in_flight) = GatedProvider::new();
        let notifier = Arc::new(Notifier::new(
            store.clone(),
            registry.clone(),
            provider,
            test_config(),
        ));
        notifier.clone().start();
        notifier.ready().await;
        device_with_token(&registry);

        // First block parks the watcher inside delivery (gate closed).
        store
            .apply(Change::upsert(blocked_agent(
                "herdr:lag-1",
                "sha256:lag",
                "go?",
            )))
            .await;
        store.flush().await;
        tokio::time::timeout(Duration::from_secs(5), async {
            while in_flight.load(std::sync::atomic::Ordering::SeqCst) == 0 {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("watcher parks inside the first delivery");

        // Flood past the broadcast cap while the watcher is parked: 300
        // more deltas, none of them the blocked agent.
        for i in 0..300 {
            let mut a = blocked_agent(&format!("herdr:flood-{i}"), "sha256:flood", "noop?");
            a.state = AgentState::Working;
            a.waiting_on = None;
            store.apply(Change::upsert(a)).await;
            store.flush().await;
        }

        // Open the gate for exactly two pushes: the delta-path push and the
        // post-Lagged re-seed push. A possible duplicate is the documented
        // trade for never missing the block.
        gate.add_permits(2);

        let (_t1, p1) = tokio::time::timeout(Duration::from_secs(5), received.recv())
            .await
            .expect("first push (delta path)")
            .expect("provider channel closed");
        assert_eq!(p1["agent_id"], "herdr:lag-1");
        let (_t2, p2) = tokio::time::timeout(Duration::from_secs(5), received.recv())
            .await
            .expect("second push (post-Lagged re-seed)")
            .expect("provider channel closed");
        assert_eq!(p2["agent_id"], "herdr:lag-1");
        assert_eq!(p1["prompt_hash"], p2["prompt_hash"], "same claim re-pushed");
    }

    #[tokio::test]
    async fn reconcile_retries_a_failed_delivery_until_it_succeeds() {
        // N4: a delivery that exhausts its retries must NOT be recorded as
        // handled — the reconcile tick re-pushes a Blocked agent whose
        // shadow lacks `delivered_ok`, so the block heals instead of being
        // silently swallowed. (Also covers N3's 400 PayloadTooLarge fallout:
        // a permanently Rejected delivery is retried the same way.)
        let store = Store::new();
        let (registry, _, _, _dir) = test_support::setup();
        device_with_token(&registry);
        let (provider, mut received) = MockProvider::new();
        // All three attempts fail (503), exhausting the retry ladder.
        provider.fail_next(3);
        let notifier = Arc::new(Notifier::new(
            store.clone(),
            registry.clone(),
            provider,
            test_config(),
        ));
        let mut shadow = HashMap::new();

        store
            .apply(Change::upsert(blocked_agent(
                "herdr:n4",
                "sha256:n4",
                "go?",
            )))
            .await;
        notifier.reconcile(&mut shadow).await;

        let s = shadow.get("herdr:n4").expect("shadow recorded");
        assert!(
            !s.delivered_ok,
            "a failed delivery is not recorded as handled"
        );

        // The next tick's provider is healthy (fail_next exhausted): the
        // block is re-pushed and the shadow marks the claim delivered.
        notifier.reconcile(&mut shadow).await;
        let (_token, p) = tokio::time::timeout(Duration::from_secs(5), received.recv())
            .await
            .expect("reconciled delivery lands")
            .expect("provider channel closed");
        assert_eq!(p["agent_id"], "herdr:n4");
        assert_eq!(p["prompt_hash"], "sha256:n4");
        assert!(
            shadow.get("herdr:n4").is_some_and(|s| s.delivered_ok),
            "a successful retry marks the claim handled"
        );
    }
}
