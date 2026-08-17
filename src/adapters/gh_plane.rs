//! gh plane: GitHub GraphQL poller (WS2).
//!
//! Polls the 8 tracked repos with ONE aliased GraphQL query (one HTTP
//! round-trip per poll; measured ~1.4s live against the real API — the
//! brief's 531ms estimate predates the 2026 schema's rollup `contexts`
//! pagination, so treat <2s as the budget target) and emits [`GhRepoState`]
//! facts into the [`PlaneSink`]: open PRs (state, mergeability, head oid,
//! head branch name, closing-issue refs, CI status collapsed from
//! `statusCheckRollup`), recent issue refs, default branch.
//!
//! Cadence rule (acceptance criterion 2):
//! - **Zero polling** until at least one SSE client has EVER connected this
//!   run (SWR-only: serve last-known state, fetch on first subscriber).
//! - **60s** while at least one SSE client is connected.
//! - **300s** after the first client ever connected, when none are live.
//!
//! The connection signal is [`Store::subscriber_count`] (the broadcast
//! receiver count: one receiver per live SSE connection, dropped on
//! disconnect); the plane holds no receiver itself, so a silent daemon reads
//! 0. Caveat: a receiver pinned for the daemon's whole life (e.g. a
//! store-integration task that never drops its subscription) keeps the count
//! permanently nonzero — the cadence then degrades to the safe 60s
//! foreground, never worse; WS3 is expected to use an explicit per-session
//! counter instead.
//!
//! Every sleep is sliced to ≤2s and `cadence_step` is re-derived on each
//! wake, so a subscriber (re)joining mid-sleep — including a reconnect during
//! a 300s background sleep — triggers the immediate SWR fetch instead of
//! waiting out the stale deadline.
//!
//! Token: `GITHUB_TOKEN` env, else `gh auth token` at startup (spawned in the
//! background task, so `start()` never blocks). If neither yields a token, a
//! warning is logged and the plane stays down — no crash loop, no retry. A
//! 401 during polling re-resolves the token (rotation/expiry support); if
//! re-resolution fails the plane stays down likewise.
//!
//! Failure handling: sustained poll failures back off exponentially from 5s,
//! capped at the current cadence, resetting on success. The poll's decode/
//! map/sort stage runs under `catch_unwind`: a panic is logged and the poll
//! is skipped (last-known state intact) instead of killing the plane. Only an
//! unconsumed sink (`SinkClosed`) exits the loop — by design: if the
//! integrator never consumes the channel, the plane stops rather than buffer
//! forever (WS3 owns the consumer).
//!
//! Read-only (D-083): GraphQL query only, never a mutation from the daemon.
//! No REST calls are made, so the ETag rule is vacuous by construction: the
//! whole poll is a single conditional-free GraphQL POST.
//!
//! Dedupe: a [`GhRepoState`] is emitted only when it differs from the last
//! successful poll for that repo; PRs and issues are sorted by number after
//! decode so `UPDATED_AT` ties cannot flip Vec order and cause spurious
//! re-emits. The first successful poll emits every repo. Failed polls emit
//! nothing and leave the last-known state intact.
//!
//! CI collapse policy: when rollup context items exist they decide — any
//! FAILURE/TIMED_OUT/CANCELLED etc. fails the PR; else any in-flight item ->
//! PENDING; else all green -> SUCCESS. Items GitHub reports as
//! ACTION_REQUIRED/STALE (or unknown types) decide nothing, so a rollup whose
//! items all collapse to nothing falls back to the rollup's aggregate `state`
//! — a genuinely broken PR must not read UNKNOWN indefinitely. An absent
//! rollup -> UNKNOWN.
//!
//! This plane is a poller BY DESIGN (the brief's acceptance criterion is one
//! round-trip per poll); the contract's zero-polling rule applies to the
//! never-connected state above.

use std::any::Any;
use std::collections::BTreeMap;
use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use serde::Deserialize;
use serde_json::{Value, json};
use tokio::time::Instant;
use tracing::{info, warn};

use crate::core::events::{GhIssueRef, GhPrState, GhRepoState, Plane, PlaneEvent, PlaneSink};
use crate::core::store::Store;

/// GitHub GraphQL endpoint (read-only query).
pub const GRAPHQL_ENDPOINT: &str = "https://api.github.com/graphql";

/// Poll cadence while at least one SSE client is connected.
pub const FOREGROUND_POLL: Duration = Duration::from_secs(60);
/// Poll cadence after the first client ever connected, while none are live.
pub const BACKGROUND_POLL: Duration = Duration::from_secs(300);
/// In-process wake interval: the subscriber-count re-check cadence while
/// never-connected AND the maximum sleep slice in active mode (so subscriber
/// joins cut background sleeps short); no network traffic in that state.
pub const SUBSCRIBER_WAKE: Duration = Duration::from_secs(2);
/// HTTP timeout for the single GraphQL round-trip per poll.
const HTTP_TIMEOUT: Duration = Duration::from_secs(30);
/// Starting exponential backoff on sustained poll failure (doubles, capped
/// at the current cadence).
pub const FAILURE_BACKOFF_BASE: Duration = Duration::from_secs(5);
/// Open PRs per repo per poll.
const PR_LIMIT: usize = 20;
/// Recent issues per repo per poll (open + closed, by updated).
const ISSUE_LIMIT: usize = 10;
/// Issues a PR closes, per PR per poll (the authoritative issue-linkage
/// surface, #23).
const CLOSING_ISSUES_LIMIT: usize = 10;
/// Rollup context items (check runs + commit statuses) per PR per poll.
const CONTEXTS_LIMIT: usize = 50;

/// One tracked repo: canonical name (the `GhRepoState.repo`) plus the actual
/// GitHub owner/repo pair. Owners verified against the live API: three repos
/// live under orgs, not `jirathip-k`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TrackedRepo {
    /// Canonical name used in the contract events.
    pub name: &'static str,
    /// GitHub owner (user or org).
    pub owner: &'static str,
    /// GitHub repository name (differs from `name` only for synergy-costing,
    /// which is the `synergy-apps` repo under synergy-services-cooling-tower).
    pub repo: &'static str,
}

/// The 8 tracked repos, in a fixed order (aliases `q0..q7` in one query).
pub const TRACKED_REPOS: &[TrackedRepo] = &[
    TrackedRepo {
        name: "sendmeter",
        owner: "sendmeter",
        repo: "sendmeter",
    },
    TrackedRepo {
        name: "project-hearthwild",
        owner: "jirathip-k",
        repo: "project-hearthwild",
    },
    TrackedRepo {
        name: "synergy-costing",
        owner: "synergy-services-cooling-tower",
        repo: "synergy-apps",
    },
    TrackedRepo {
        name: "dotfiles",
        owner: "jirathip-k",
        repo: "dotfiles",
    },
    TrackedRepo {
        name: "agent-ops",
        owner: "jirathip-k",
        repo: "agent-ops",
    },
    TrackedRepo {
        name: "herdr-board",
        owner: "jirathip-k",
        repo: "herdr-board",
    },
    TrackedRepo {
        name: "office-ops",
        owner: "jirathip-k",
        repo: "office-ops",
    },
    TrackedRepo {
        name: "synergy-services-website",
        owner: "synergy-services",
        repo: "synergy-services-website",
    },
];

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub enum GhError {
    /// Connection / TLS / decode-of-response-body failure.
    Transport(String),
    /// Non-2xx HTTP response.
    Http { status: u16, body: String },
    /// GraphQL-level failure (hard error: no usable `data` at all).
    GraphQl(Vec<String>),
    /// Response shape was not what the contract expects.
    Decode(String),
    /// A panic was caught in the poll's decode/map/sort stage.
    Panic(String),
    /// The sink receiver was dropped; the plane should stop.
    SinkClosed,
}

impl GhError {
    /// True for HTTP 401 — the token is expired/revoked and should be
    /// re-resolved (rotation support).
    fn is_unauthorized(&self) -> bool {
        matches!(self, Self::Http { status: 401, .. })
    }
}

impl fmt::Display for GhError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Transport(msg) => write!(f, "gh transport error: {msg}"),
            Self::Http { status, body } => write!(f, "gh HTTP {status}: {body}"),
            Self::GraphQl(msgs) => write!(f, "gh GraphQL error: {}", msgs.join("; ")),
            Self::Decode(msg) => write!(f, "gh decode error: {msg}"),
            Self::Panic(msg) => write!(f, "gh poll panicked: {msg}"),
            Self::SinkClosed => write!(f, "gh plane sink closed"),
        }
    }
}

impl std::error::Error for GhError {}

// ---------------------------------------------------------------------------
// HTTP transport (injectable for tests; never network in unit tests)
// ---------------------------------------------------------------------------

/// Minimal injectable HTTP seam. The real implementation talks to GitHub;
/// tests substitute a mock. No request must ever be issued without a token.
pub trait GhTransport: Send + Sync {
    fn post<'a>(
        &'a self,
        url: &'a str,
        token: &'a str,
        body: Value,
    ) -> BoxFuture<'a, Result<Value, GhError>>;
}

/// Boxed async response for the transport seam (async fn in traits is not
/// dyn-compatible, so the future is boxed explicitly).
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Real transport: one shared client, bearer token per request, hard timeout
/// so a hung connection cannot stall the poll loop forever.
struct ReqwestTransport {
    client: reqwest::Client,
}

impl ReqwestTransport {
    fn new() -> Self {
        Self {
            client: reqwest::Client::builder()
                .user_agent(concat!("corrald/", env!("CARGO_PKG_VERSION")))
                .timeout(HTTP_TIMEOUT)
                .build()
                .expect("reqwest client builds"),
        }
    }
}

impl GhTransport for ReqwestTransport {
    fn post<'a>(
        &'a self,
        url: &'a str,
        token: &'a str,
        body: Value,
    ) -> BoxFuture<'a, Result<Value, GhError>> {
        Box::pin(async move {
            let response = self
                .client
                .post(url)
                .bearer_auth(token)
                .json(&body)
                .send()
                .await
                .map_err(|e| GhError::Transport(e.to_string()))?;
            let status = response.status();
            let text = response
                .text()
                .await
                .map_err(|e| GhError::Transport(e.to_string()))?;
            if !status.is_success() {
                return Err(GhError::Http {
                    status: status.as_u16(),
                    body: text,
                });
            }
            serde_json::from_str(&text).map_err(|e| GhError::Decode(e.to_string()))
        })
    }
}

/// Resolve the API token: `GITHUB_TOKEN` env first, then `gh auth token`.
async fn resolve_token() -> Option<String> {
    if let Ok(token) = std::env::var("GITHUB_TOKEN") {
        let token = token.trim().to_string();
        if !token.is_empty() {
            return Some(token);
        }
    }
    let output = tokio::process::Command::new("gh")
        .args(["auth", "token"])
        .output()
        .await
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let token = String::from_utf8(output.stdout).ok()?.trim().to_string();
    (!token.is_empty()).then_some(token)
}

// ---------------------------------------------------------------------------
// Cadence configuration (defaults per the brief; tests shrink the durations)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct GhPlaneConfig {
    /// Cadence while at least one SSE client is connected.
    pub foreground: Duration,
    /// Cadence after the first client ever connected, while none are live.
    pub background: Duration,
    /// In-process wake for the subscriber-count check while never-connected
    /// and the maximum sleep slice in active mode.
    pub wake: Duration,
    /// Starting backoff on sustained poll failure (doubles, capped at the
    /// current cadence; resets on success).
    pub failure_backoff: Duration,
}

impl Default for GhPlaneConfig {
    fn default() -> Self {
        Self {
            foreground: FOREGROUND_POLL,
            background: BACKGROUND_POLL,
            wake: SUBSCRIBER_WAKE,
            failure_backoff: FAILURE_BACKOFF_BASE,
        }
    }
}

// ---------------------------------------------------------------------------
// Cadence state machine (pure: deterministically testable with a fake clock)
// ---------------------------------------------------------------------------

/// What the cadence loop should do next, derived purely from the connection
/// signal and the current time. Kept as a pure function so the 60s/300s rule
/// (acceptance criterion 2) is unit-testable without any timers.
///
/// Clock caveat: `now` is caller-provided, so the function is clock-agnostic;
/// the loop passes `tokio::time::Instant`. If tests ever adopt
/// `tokio::time::pause()`, the simulated clock diverges from
/// `std::time::Instant::now()` (which the test helpers build from via
/// `Instant::from_std`) — the cadence tests deliberately use std time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CadenceAction {
    /// No SSE client has EVER connected: stay SWR-only. Sleep the wake
    /// interval and re-check the subscriber count — no network polling.
    RecheckSubscribers,
    /// Active mode, not yet due: sleep a slice of the remaining time (at
    /// most the wake interval) so a subscriber join can cut the sleep short,
    /// then re-derive. `next_poll` is preserved across slices.
    SleepUntil(Instant),
    /// Poll now (scheduled poll due, first-ever subscriber, or a reconnect
    /// detected since the last decision), then schedule the next poll after
    /// `next` (foreground or background cadence).
    Poll { next: Duration },
}

fn cadence_step(
    ever_connected: bool,
    subscribers: usize,
    prev_subscribers: usize,
    next_poll: Option<Instant>,
    now: Instant,
    config: &GhPlaneConfig,
) -> (CadenceAction, bool) {
    if subscribers > 0 && prev_subscribers == 0 {
        // A client (re)joined since the last decision: immediate SWR fetch —
        // on the first-ever join AND on every reconnect, never wait out a
        // stale background deadline (F2).
        return (
            CadenceAction::Poll {
                next: config.foreground,
            },
            true,
        );
    }
    if !ever_connected {
        // Zero polling until the first client ever connects.
        return (CadenceAction::RecheckSubscribers, false);
    }
    if let Some(deadline) = next_poll
        && now < deadline
    {
        return (CadenceAction::SleepUntil(deadline), true);
    }
    let next = if subscribers > 0 {
        config.foreground
    } else {
        config.background
    };
    (CadenceAction::Poll { next }, true)
}

/// One exponential-backoff step for sustained poll failures: the next poll
/// waits `backoff` (capped at the cadence), and the backoff doubles for the
/// following failure, again capped at the cadence.
fn failure_backoff_step(backoff: Duration, cap: Duration) -> (Duration, Duration) {
    let delay = backoff.min(cap);
    (delay, (backoff * 2).min(cap))
}

// ---------------------------------------------------------------------------
// Plane
// ---------------------------------------------------------------------------

pub struct GhPlane {
    store: Arc<Store>,
    transport: Arc<dyn GhTransport>,
    token: Option<String>,
    config: GhPlaneConfig,
}

impl fmt::Debug for GhPlane {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("GhPlane")
            .field("config", &self.config)
            .field("token", &self.token.as_deref().map(|_| "<redacted>"))
            .finish_non_exhaustive()
    }
}

impl GhPlane {
    /// Production constructor: real transport; the token is resolved lazily
    /// in the background task so `start()` never blocks.
    pub fn new(store: Arc<Store>) -> Self {
        Self {
            store,
            transport: Arc::new(ReqwestTransport::new()),
            token: None,
            config: GhPlaneConfig::default(),
        }
    }

    /// Test/embedding constructor: injected transport, explicit token, custom
    /// cadence. Never issues network calls against the real API.
    pub fn with_config(
        store: Arc<Store>,
        transport: Arc<dyn GhTransport>,
        token: Option<String>,
        config: GhPlaneConfig,
    ) -> Self {
        Self {
            store,
            transport,
            token,
            config,
        }
    }

    /// Constructor with an explicit token: skips `GITHUB_TOKEN`/`gh auth
    /// token` resolution entirely (no subprocess spawn). Used by the live
    /// harness so the measured round-trip excludes token resolution, and by
    /// embedders that already hold a token.
    pub fn with_token(store: Arc<Store>, token: String) -> Self {
        Self {
            store,
            transport: Arc::new(ReqwestTransport::new()),
            token: Some(token),
            config: GhPlaneConfig::default(),
        }
    }

    async fn run_forever(self: Arc<Self>, sink: PlaneSink) {
        let mut token = match self.token.clone() {
            Some(token) => token,
            None => match resolve_token().await {
                Some(token) => token,
                None => {
                    warn!("gh plane staying down: no GITHUB_TOKEN and `gh auth token` unavailable");
                    return;
                }
            },
        };
        let query = build_query();
        let mut last: BTreeMap<String, GhRepoState> = BTreeMap::new();
        let mut ever_connected = false;
        let mut prev_subscribers = 0usize;
        let mut next_poll: Option<Instant> = None;
        let mut backoff = self.config.failure_backoff;
        loop {
            let subscribers = self.store.subscriber_count();
            let (action, ever_connected_now) = cadence_step(
                ever_connected,
                subscribers,
                prev_subscribers,
                next_poll,
                Instant::now(),
                &self.config,
            );
            prev_subscribers = subscribers;
            if !ever_connected && ever_connected_now {
                info!(subscribers, "gh plane live: first SSE subscriber connected");
            }
            ever_connected = ever_connected_now;
            match action {
                // SWR-only: zero network polling until a client connects.
                CadenceAction::RecheckSubscribers => {
                    tokio::time::sleep(self.config.wake).await;
                }
                CadenceAction::SleepUntil(deadline) => {
                    // Slice the sleep (at most the wake interval) and
                    // re-derive cadence on every wake, so a subscriber
                    // (re)joining mid-sleep — including a reconnect during a
                    // 300s background sleep — triggers the immediate SWR
                    // fetch instead of waiting out the stale deadline (F2).
                    let remaining = deadline.saturating_duration_since(Instant::now());
                    tokio::time::sleep(remaining.min(self.config.wake)).await;
                }
                CadenceAction::Poll { next } => {
                    match self.poll_once(&token, &query, &mut last, &sink).await {
                        Ok(()) => {
                            backoff = self.config.failure_backoff;
                            next_poll = Some(Instant::now() + next);
                        }
                        Err(GhError::SinkClosed) => return,
                        Err(e) => {
                            warn!(error = %e, "gh plane poll failed");
                            if e.is_unauthorized() {
                                // Token rotated/expired: re-resolve once.
                                match resolve_token().await {
                                    Some(new_token) => {
                                        info!("gh token re-resolved after 401");
                                        token = new_token;
                                    }
                                    None => {
                                        warn!(
                                            "gh plane staying down: token re-resolution failed after 401"
                                        );
                                        return;
                                    }
                                }
                            }
                            let (delay, next_backoff) = failure_backoff_step(backoff, next);
                            backoff = next_backoff;
                            next_poll = Some(Instant::now() + delay);
                        }
                    }
                }
            }
        }
    }

    /// One aliased GraphQL round-trip. Emits only changed repos; the first
    /// successful poll emits every repo. Per-alias failures (null data) skip
    /// that repo and keep its last-known state. The decode/map/sort stage is
    /// panic-guarded (F4): a panic is logged, the poll is skipped, and the
    /// plane keeps running with the last-known state intact.
    async fn poll_once(
        &self,
        token: &str,
        query: &str,
        last: &mut BTreeMap<String, GhRepoState>,
        sink: &PlaneSink,
    ) -> Result<(), GhError> {
        let started = Instant::now();
        let response = self
            .transport
            .post(GRAPHQL_ENDPOINT, token, json!({ "query": query }))
            .await?;
        let latency_ms = started.elapsed().as_millis() as u64;

        let Some(data) = response.get("data").and_then(|d| d.as_object()) else {
            let messages: Vec<String> = response
                .get("errors")
                .and_then(|e| e.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|e| e.get("message").and_then(|m| m.as_str()))
                        .map(String::from)
                        .collect()
                })
                .unwrap_or_default();
            return Err(GhError::GraphQl(messages));
        };

        let (new_last, changed) =
            match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                process_response(data, last)
            })) {
                Ok(result) => result,
                Err(payload) => return Err(GhError::Panic(panic_message(&payload))),
            };
        *last = new_last;
        for state in &changed {
            if sink.send(PlaneEvent::Gh(state.clone())).await.is_err() {
                return Err(GhError::SinkClosed);
            }
        }
        info!(
            latency_ms,
            repos = TRACKED_REPOS.len(),
            changed = changed.len(),
            "gh plane round-trip complete"
        );
        Ok(())
    }
}

/// Extract the message from a panic payload for logging.
fn panic_message(payload: &(dyn Any + Send)) -> String {
    if let Some(message) = payload.downcast_ref::<&str>() {
        (*message).to_string()
    } else if let Some(message) = payload.downcast_ref::<String>() {
        message.clone()
    } else {
        "unknown panic payload".to_string()
    }
}

/// Synchronous decode/map/dedupe stage of a poll, run under `catch_unwind`
/// (F4). `last` is borrowed immutably and cloned so a panic can never leave
/// the dedupe state half-updated. Returns the new last-known state and the
/// states that changed (sorted, in `TRACKED_REPOS` order).
fn process_response(
    data: &serde_json::Map<String, Value>,
    last: &BTreeMap<String, GhRepoState>,
) -> (BTreeMap<String, GhRepoState>, Vec<GhRepoState>) {
    let mut new_last = last.clone();
    let mut changed = Vec::new();
    for (i, repo) in TRACKED_REPOS.iter().enumerate() {
        let key = format!("q{i}");
        let Some(wire) = data
            .get(&key)
            .and_then(|v| serde_json::from_value::<RepoWire>(v.clone()).ok())
        else {
            warn!(
                repo = repo.name,
                "repo alias null or undecodable; skipping this poll"
            );
            continue;
        };
        let state = build_repo_state(*repo, &wire);
        if new_last.get(repo.name).is_some_and(|prev| *prev == state) {
            continue;
        }
        new_last.insert(repo.name.to_string(), state.clone());
        changed.push(state);
    }
    (new_last, changed)
}

impl Plane for GhPlane {
    fn source(&self) -> &'static str {
        "gh"
    }

    fn start(self: Arc<Self>, sink: PlaneSink) {
        tokio::spawn(async move { self.run_forever(sink).await });
    }
}

// ---------------------------------------------------------------------------
// GraphQL query + wire mapping
// ---------------------------------------------------------------------------

/// One aliased query for all repos: `q0..q7` each spread the shared fragment.
/// Literals are JSON-escaped so the query is valid for any owner/name.
fn build_query() -> String {
    let mut query = String::from("query {\n");
    for (i, repo) in TRACKED_REPOS.iter().enumerate() {
        let owner = serde_json::to_string(repo.owner).expect("static owner json-escapes");
        let name = serde_json::to_string(repo.repo).expect("static repo json-escapes");
        query.push_str(&format!(
            "  q{i}: repository(owner: {owner}, name: {name}) {{ ...GhPlaneRepo }}\n"
        ));
    }
    query.push_str(&format!(
        r#"}}
fragment GhPlaneRepo on Repository {{
  defaultBranchRef {{ name }}
  pullRequests(first: {PR_LIMIT}, states: OPEN, orderBy: {{field: UPDATED_AT, direction: DESC}}) {{
    nodes {{
      number
      title
      state
      mergeable
      headRefOid
      headRefName
      closingIssuesReferences(first: {CLOSING_ISSUES_LIMIT}) {{
        nodes {{ number title }}
      }}
      statusCheckRollup {{
        state
        contexts(first: {CONTEXTS_LIMIT}) {{
          nodes {{
            __typename
            ... on CheckRun {{ status conclusion }}
            ... on StatusContext {{ state }}
          }}
        }}
      }}
    }}
  }}
  issues(first: {ISSUE_LIMIT}, orderBy: {{field: UPDATED_AT, direction: DESC}}, states: [OPEN, CLOSED]) {{
    nodes {{ number state title }}
  }}
}}
"#,
    ));
    query
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct RepoWire {
    default_branch_ref: Option<DefaultBranchRefWire>,
    pull_requests: Option<NodesWire<PrWire>>,
    issues: Option<NodesWire<IssueWire>>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct DefaultBranchRefWire {
    name: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct NodesWire<T> {
    nodes: Option<Vec<T>>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct PrWire {
    number: u64,
    title: Option<String>,
    state: Option<String>,
    mergeable: Option<String>,
    head_ref_oid: Option<String>,
    head_ref_name: Option<String>,
    closing_issues_references: Option<NodesWire<ClosingIssueWire>>,
    status_check_rollup: Option<RollupWire>,
}

/// One node of a PR's `closingIssuesReferences` (the #23 authoritative
/// linkage): number + title only — the same-poll repo-level `issues` leg
/// enriches the state, so no extra fields hit the wire.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct ClosingIssueWire {
    number: u64,
    title: Option<String>,
}

/// `statusCheckRollup` (2026 schema): a single object carrying the aggregate
/// `state` plus a `contexts` connection whose nodes are the check-run /
/// commit-status union.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct RollupWire {
    /// Aggregate StatusState (SUCCESS/FAILURE/PENDING/ERROR/EXPECTED).
    state: Option<String>,
    contexts: Option<NodesWire<RollupItemWire>>,
}

/// One element of the rollup's `contexts` nodes: a check run OR a commit
/// status context. Which one is told by `__typename`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct RollupItemWire {
    #[serde(rename = "__typename")]
    typename: Option<String>,
    /// CheckRun lifecycle status (QUEUED/IN_PROGRESS/COMPLETED).
    status: Option<String>,
    /// CheckRun conclusion when COMPLETED.
    conclusion: Option<String>,
    /// StatusContext state (SUCCESS/FAILURE/ERROR/PENDING/EXPECTED).
    state: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct IssueWire {
    number: u64,
    state: Option<String>,
    title: Option<String>,
}

/// Collapse a PR's rollup into the canonical SUCCESS/FAILURE/PENDING/UNKNOWN.
///
/// Policy: when rollup context items are present they decide — any failing
/// item fails the PR, else any in-flight item -> PENDING, else all green ->
/// SUCCESS. Items GitHub reports as ACTION_REQUIRED/STALE (or unknown
/// types) decide nothing, so a rollup whose items all collapse to nothing —
/// or an empty contexts list — falls back to the rollup's aggregate `state`
/// (a genuinely broken PR must not read UNKNOWN indefinitely, F5). An absent
/// rollup -> UNKNOWN.
fn collapse_ci(rollup: Option<&RollupWire>) -> String {
    let Some(rollup) = rollup else {
        return "UNKNOWN".to_string();
    };
    if let Some(items) = rollup.contexts.as_ref().and_then(|c| c.nodes.as_ref())
        && let Some(collapsed) = collapse_items(items)
    {
        return collapsed;
    }
    aggregate_state(rollup.state.as_deref())
}

/// Map the rollup's aggregate StatusState to the canonical string.
fn aggregate_state(state: Option<&str>) -> String {
    match state {
        Some("SUCCESS") => "SUCCESS".to_string(),
        Some("FAILURE") | Some("ERROR") => "FAILURE".to_string(),
        Some("PENDING") | Some("EXPECTED") => "PENDING".to_string(),
        _ => "UNKNOWN".to_string(),
    }
}

/// Collapse the per-item contexts. `Some` when any item decided; `None` when
/// every item is unrecognized (ACTION_REQUIRED/STALE/unknown typename) or
/// the list is empty — the caller falls back to the aggregate state.
fn collapse_items(items: &[RollupItemWire]) -> Option<String> {
    let mut any_failure = false;
    let mut any_pending = false;
    let mut any_success = false;
    for item in items {
        match item.typename.as_deref() {
            Some("CheckRun") => match (item.status.as_deref(), item.conclusion.as_deref()) {
                (Some("COMPLETED"), Some(conclusion)) => match conclusion {
                    "SUCCESS" | "NEUTRAL" | "SKIPPED" => any_success = true,
                    "FAILURE" | "TIMED_OUT" | "STARTUP_FAILURE" | "CANCELLED" => any_failure = true,
                    // ACTION_REQUIRED / STALE: neither green nor in-flight.
                    _ => {}
                },
                // QUEUED / IN_PROGRESS / REQUESTED / WAITING / missing fields.
                _ => any_pending = true,
            },
            Some("StatusContext") => match item.state.as_deref() {
                Some("SUCCESS") => any_success = true,
                Some("FAILURE") | Some("ERROR") => any_failure = true,
                Some("PENDING") | Some("EXPECTED") => any_pending = true,
                _ => {}
            },
            _ => {}
        }
    }
    if any_failure {
        Some("FAILURE".to_string())
    } else if any_pending {
        Some("PENDING".to_string())
    } else if any_success {
        Some("SUCCESS".to_string())
    } else {
        None
    }
}

fn build_repo_state(repo: TrackedRepo, wire: &RepoWire) -> GhRepoState {
    let issues_wire: &[IssueWire] = wire
        .issues
        .as_ref()
        .and_then(|i| i.nodes.as_ref())
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    let mut prs: Vec<GhPrState> = wire
        .pull_requests
        .as_ref()
        .and_then(|p| p.nodes.as_ref())
        .map(|nodes| {
            nodes
                .iter()
                .map(|pr| normalize_pr(repo, pr, issues_wire))
                .collect()
        })
        .unwrap_or_default();
    let mut issues: Vec<GhIssueRef> = wire
        .issues
        .as_ref()
        .and_then(|i| i.nodes.as_ref())
        .map(|nodes| {
            nodes
                .iter()
                .map(|issue| GhIssueRef {
                    repo: repo.name.to_string(),
                    number: issue.number,
                    state: issue.state.clone().unwrap_or_else(|| "UNKNOWN".to_string()),
                    title: issue.title.clone().unwrap_or_default(),
                })
                .collect()
        })
        .unwrap_or_default();
    // Sort by number so UPDATED_AT ties cannot flip Vec order between polls
    // and defeat the dedupe (F7).
    prs.sort_by_key(|pr| pr.pr_number);
    issues.sort_by_key(|issue| issue.number);
    GhRepoState {
        repo: repo.name.to_string(),
        default_branch: wire
            .default_branch_ref
            .as_ref()
            .and_then(|b| b.name.clone())
            .unwrap_or_default(),
        // GitHub's API has no "ahead/behind vs default branch" concept; local
        // tracking info is WS1's job. Cheaply unavailable in-query -> 0.
        ahead: 0,
        behind: 0,
        prs,
        issues,
    }
}

fn normalize_pr(repo: TrackedRepo, pr: &PrWire, repo_issues: &[IssueWire]) -> GhPrState {
    // #23: the authoritative linkage is the PR's closingIssuesReferences.
    // `state` is not on the fragment — it is enriched from the SAME poll's
    // repo-level issues fetch (already fetched, zero extra requests) when
    // the issue is among the recent ones; otherwise the honest
    // "cannot tell" sentinel, never a guess.
    let closing_issues = pr
        .closing_issues_references
        .as_ref()
        .and_then(|c| c.nodes.as_ref())
        .map(|nodes| {
            nodes
                .iter()
                .map(|closing| {
                    let state = repo_issues
                        .iter()
                        .find(|issue| issue.number == closing.number)
                        .and_then(|issue| issue.state.clone())
                        .unwrap_or_else(|| "UNKNOWN".to_string());
                    GhIssueRef {
                        repo: repo.name.to_string(),
                        number: closing.number,
                        state,
                        title: closing.title.clone().unwrap_or_default(),
                    }
                })
                .collect()
        })
        .unwrap_or_default();
    GhPrState {
        repo: repo.name.to_string(),
        pr_number: pr.number,
        title: pr.title.clone().unwrap_or_default(),
        state: pr.state.clone().unwrap_or_else(|| "UNKNOWN".to_string()),
        mergeable: pr
            .mergeable
            .clone()
            .unwrap_or_else(|| "UNKNOWN".to_string()),
        ci_status: collapse_ci(pr.status_check_rollup.as_ref()),
        head_sha: pr.head_ref_oid.clone().unwrap_or_default(),
        head_branch: pr.head_ref_name.clone().unwrap_or_default(),
        closing_issues,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_covers_all_repos_once() {
        let query = build_query();
        for (i, repo) in TRACKED_REPOS.iter().enumerate() {
            assert!(
                query.contains(&format!(
                    "q{i}: repository(owner: \"{}\", name: \"{}\")",
                    repo.owner, repo.repo
                )),
                "alias q{i} for {} must be in the query",
                repo.name
            );
        }
        assert_eq!(
            query.matches("repository(owner:").count(),
            TRACKED_REPOS.len()
        );
        assert!(query.contains("fragment GhPlaneRepo on Repository"));
        // #22/#23: the branch-fallback and issue-linkage surfaces ride the
        // SAME fragment — one extra field each, never an extra request.
        assert!(
            query.contains("headRefName"),
            "branch-fallback key (issue #22)"
        );
        assert!(
            query.contains("closingIssuesReferences(first: 10)"),
            "authoritative issue linkage (issue #23)"
        );
        assert!(!query.contains("mutation"), "never a mutation (D-083)");
        assert_eq!(
            query.matches('{').count(),
            query.matches('}').count(),
            "query block must be brace-balanced (a missing close is a hard GraphQL parse error)"
        );
    }

    #[test]
    fn collapse_ci_priorities_and_unknowns() {
        let item = |typename: &str,
                    status: Option<&str>,
                    conclusion: Option<&str>,
                    state: Option<&str>| {
            RollupItemWire {
                typename: Some(typename.to_string()),
                status: status.map(String::from),
                conclusion: conclusion.map(String::from),
                state: state.map(String::from),
            }
        };
        let rollup = |items: Vec<RollupItemWire>, aggregate: Option<&str>| RollupWire {
            state: aggregate.map(String::from),
            contexts: Some(NodesWire { nodes: Some(items) }),
        };

        let success_run = item("CheckRun", Some("COMPLETED"), Some("SUCCESS"), None);
        let status_success = item("StatusContext", None, None, Some("SUCCESS"));

        assert_eq!(collapse_ci(None), "UNKNOWN", "absent rollup -> UNKNOWN");
        assert_eq!(
            collapse_ci(Some(&rollup(vec![], None))),
            "UNKNOWN",
            "empty rollup -> UNKNOWN"
        );
        assert_eq!(
            collapse_ci(Some(&rollup(
                vec![success_run.clone(), status_success.clone()],
                Some("SUCCESS")
            ))),
            "SUCCESS"
        );

        let failing = item("CheckRun", Some("COMPLETED"), Some("FAILURE"), None);
        assert_eq!(
            collapse_ci(Some(&rollup(
                vec![success_run.clone(), failing],
                Some("FAILURE")
            ))),
            "FAILURE"
        );
        let errored = item("StatusContext", None, None, Some("ERROR"));
        assert_eq!(
            collapse_ci(Some(&rollup(
                vec![success_run.clone(), errored],
                Some("FAILURE")
            ))),
            "FAILURE"
        );

        let in_flight = item("CheckRun", Some("IN_PROGRESS"), None, None);
        assert_eq!(
            collapse_ci(Some(&rollup(
                vec![success_run.clone(), in_flight],
                Some("PENDING")
            ))),
            "PENDING"
        );
        let context_pending = item("StatusContext", None, None, Some("PENDING"));
        assert_eq!(
            collapse_ci(Some(&rollup(
                vec![status_success.clone(), context_pending],
                Some("PENDING")
            ))),
            "PENDING"
        );

        let neutral = item("CheckRun", Some("COMPLETED"), Some("NEUTRAL"), None);
        assert_eq!(
            collapse_ci(Some(&rollup(vec![neutral], Some("SUCCESS")))),
            "SUCCESS"
        );
        let cancelled = item("CheckRun", Some("COMPLETED"), Some("CANCELLED"), None);
        assert_eq!(
            collapse_ci(Some(&rollup(vec![cancelled], Some("FAILURE")))),
            "FAILURE"
        );
        // A recognized success still decides even next to an ACTION_REQUIRED run.
        let action = item("CheckRun", Some("COMPLETED"), Some("ACTION_REQUIRED"), None);
        assert_eq!(
            collapse_ci(Some(&rollup(
                vec![success_run.clone(), action.clone()],
                Some("FAILURE")
            ))),
            "SUCCESS",
            "recognized items decide (defensible asymmetry)"
        );
        // All-ignored items (ACTION_REQUIRED only) fall back to the aggregate (F5):
        assert_eq!(
            collapse_ci(Some(&rollup(vec![action.clone()], Some("FAILURE")))),
            "FAILURE"
        );
        assert_eq!(
            collapse_ci(Some(&rollup(vec![action.clone()], Some("PENDING")))),
            "PENDING"
        );
        assert_eq!(collapse_ci(Some(&rollup(vec![action], None))), "UNKNOWN");
        // Unknown typename only -> aggregate too.
        let alien = item("SomethingElse", Some("COMPLETED"), Some("SUCCESS"), None);
        assert_eq!(
            collapse_ci(Some(&rollup(vec![alien.clone()], Some("PENDING")))),
            "PENDING"
        );
        assert_eq!(collapse_ci(Some(&rollup(vec![alien], None))), "UNKNOWN");

        // Empty contexts fall back to the rollup's aggregate state.
        assert_eq!(
            collapse_ci(Some(&rollup(vec![], Some("SUCCESS")))),
            "SUCCESS"
        );
        assert_eq!(
            collapse_ci(Some(&rollup(vec![], Some("PENDING")))),
            "PENDING"
        );
        assert_eq!(collapse_ci(Some(&rollup(vec![], Some("ERROR")))), "FAILURE");
        assert_eq!(
            collapse_ci(Some(&rollup(vec![], Some("EXPECTED")))),
            "PENDING"
        );
        assert_eq!(collapse_ci(Some(&rollup(vec![], Some("WEIRD")))), "UNKNOWN");
    }

    #[test]
    fn maps_repo_wire_into_contract_types() {
        let wire: RepoWire = serde_json::from_value(json!({
            "name": "herdr-board",
            "defaultBranchRef": { "name": "main" },
            "pullRequests": { "nodes": [
                {
                    "number": 7,
                    "title": "P2 three planes",
                    "state": "OPEN",
                    "mergeable": "CONFLICTING",
                    "headRefOid": "abc123",
                    "headRefName": "ws2/gh-plane",
                    "closingIssuesReferences": { "nodes": [
                        { "number": 4, "title": "P2 planes" },
                        { "number": 99, "title": "long-closed" }
                    ]},
                    "statusCheckRollup": {
                        "state": "SUCCESS",
                        "contexts": { "nodes": [
                            { "__typename": "CheckRun", "status": "COMPLETED", "conclusion": "SUCCESS" },
                            { "__typename": "StatusContext", "state": "SUCCESS" }
                        ]}
                    }
                },
                {
                    "number": 6,
                    "title": "head deleted",
                    "state": "OPEN",
                    "mergeable": "MERGEABLE",
                    "headRefOid": null,
                    "headRefName": null,
                    "closingIssuesReferences": null,
                    "statusCheckRollup": { "state": "PENDING", "contexts": { "nodes": [] } }
                }
            ]},
            "issues": { "nodes": [
                { "number": 4, "state": "OPEN", "title": "P2 planes" },
                { "number": 3, "state": "CLOSED", "title": "P1 shipped" }
            ]}
        }))
        .unwrap();
        let repo = TRACKED_REPOS
            .iter()
            .find(|r| r.name == "herdr-board")
            .expect("tracked");
        let state = build_repo_state(*repo, &wire);
        assert_eq!(state.repo, "herdr-board");
        assert_eq!(state.default_branch, "main");
        assert_eq!(state.ahead, 0);
        assert_eq!(state.behind, 0);
        assert_eq!(state.prs.len(), 2);
        // Wire order is 7,6 — decoded output is sorted by number (F7).
        assert_eq!(state.prs[0].pr_number, 6);
        assert_eq!(state.prs[0].mergeable, "MERGEABLE");
        assert_eq!(
            state.prs[0].ci_status, "PENDING",
            "empty contexts -> aggregate state"
        );
        assert_eq!(state.prs[0].head_sha, "", "null headRefOid -> empty string");
        assert_eq!(
            state.prs[0].head_branch, "",
            "null headRefName -> empty string"
        );
        assert!(
            state.prs[0].closing_issues.is_empty(),
            "null closing refs -> empty"
        );
        assert_eq!(state.prs[1].pr_number, 7);
        assert_eq!(state.prs[1].mergeable, "CONFLICTING");
        assert_eq!(state.prs[1].ci_status, "SUCCESS");
        assert_eq!(state.prs[1].head_sha, "abc123");
        assert_eq!(state.prs[1].head_branch, "ws2/gh-plane");
        // #23: closing refs are enriched with the state of the SAME poll's
        // repo-level issues fetch when the number is present; issues outside
        // the recent set keep the honest "UNKNOWN" (never a guess).
        assert_eq!(state.prs[1].closing_issues.len(), 2);
        assert_eq!(state.prs[1].closing_issues[0].number, 4);
        assert_eq!(state.prs[1].closing_issues[0].state, "OPEN");
        assert_eq!(state.prs[1].closing_issues[0].title, "P2 planes");
        assert_eq!(state.prs[1].closing_issues[1].number, 99);
        assert_eq!(state.prs[1].closing_issues[1].state, "UNKNOWN");
        assert_eq!(state.prs[1].closing_issues[1].title, "long-closed");
        assert_eq!(state.issues.len(), 2);
        assert_eq!(state.issues[0].number, 3, "issues sorted by number");
        assert_eq!(state.issues[0].state, "CLOSED");
        assert_eq!(state.issues[1].number, 4);
        assert_eq!(state.issues[1].state, "OPEN");
        assert_eq!(state.issues[1].title, "P2 planes");
    }

    #[test]
    fn tolerates_partial_repo_wire() {
        let wire: RepoWire = serde_json::from_value(json!({
            "name": "dotfiles",
            "pullRequests": null,
            "issues": null
        }))
        .unwrap();
        let repo = TRACKED_REPOS
            .iter()
            .find(|r| r.name == "dotfiles")
            .expect("tracked");
        let state = build_repo_state(*repo, &wire);
        assert_eq!(state.default_branch, "", "missing default branch -> empty");
        assert!(state.prs.is_empty());
        assert!(state.issues.is_empty());
    }

    // -----------------------------------------------------------------------
    // Cadence rule (acceptance criterion 2), tested with the PRODUCTION
    // 60s/300s constants on a fake clock — deterministic, no timers.
    // `fake_now` uses std time via `Instant::from_std`; that stays consistent
    // only while no test adopts `tokio::time::pause()` (whose simulated clock
    // diverges from std) — see the `cadence_step` doc caveat.
    // -----------------------------------------------------------------------

    fn fake_now() -> Instant {
        Instant::from_std(std::time::Instant::now())
    }

    #[test]
    fn cadence_swr_zero_polling_without_any_subscriber() {
        let t0 = fake_now();
        let config = GhPlaneConfig::default();
        let (mut ever_connected, mut next_poll) = (false, None);
        // 1000s of simulated uptime in 2s wake ticks: still no subscriber.
        for tick in 0..500 {
            let now = t0 + Duration::from_secs(tick * 2);
            let (action, ever) = cadence_step(ever_connected, 0, 0, next_poll, now, &config);
            ever_connected = ever;
            next_poll = None;
            assert_eq!(action, CadenceAction::RecheckSubscribers, "tick {tick}");
            assert!(!ever_connected, "never connected");
        }
    }

    #[test]
    fn cadence_first_subscriber_triggers_immediate_fetch() {
        let config = GhPlaneConfig::default();
        let (action, ever) = cadence_step(false, 1, 0, None, fake_now(), &config);
        assert!(ever, "marks ever-connected");
        assert_eq!(
            action,
            CadenceAction::Poll {
                next: FOREGROUND_POLL
            },
            "SWR: first subscriber fetches immediately, next poll in 60s"
        );
    }

    #[test]
    fn cadence_reconnect_mid_sleep_triggers_immediate_fetch() {
        let t0 = fake_now();
        let config = GhPlaneConfig::default();
        // A client reconnects while a 300s background sleep is pending: the
        // join must cut the sleep short with an immediate SWR fetch (F2)...
        let (action, ever) =
            cadence_step(true, 1, 0, Some(t0 + Duration::from_secs(290)), t0, &config);
        assert!(ever);
        assert_eq!(
            action,
            CadenceAction::Poll {
                next: FOREGROUND_POLL
            }
        );
        // ...whereas no join preserves the background sleep...
        let (action, _) =
            cadence_step(true, 0, 0, Some(t0 + Duration::from_secs(290)), t0, &config);
        assert_eq!(
            action,
            CadenceAction::SleepUntil(t0 + Duration::from_secs(290))
        );
        // ...and a steady subscriber (prev=1, now=1) does not spuriously poll.
        let (action, _) = cadence_step(true, 1, 1, Some(t0 + Duration::from_secs(10)), t0, &config);
        assert_eq!(
            action,
            CadenceAction::SleepUntil(t0 + Duration::from_secs(10))
        );
    }

    #[test]
    fn cadence_foreground_60s_while_connected() {
        let t0 = fake_now();
        let config = GhPlaneConfig::default();
        // Due now, subscriber live -> poll, 60s cadence.
        let (action, _) = cadence_step(true, 1, 1, Some(t0 - Duration::from_secs(1)), t0, &config);
        assert_eq!(
            action,
            CadenceAction::Poll {
                next: FOREGROUND_POLL
            }
        );
        // Not due -> sleep until the deadline.
        let deadline = t0 + Duration::from_secs(10);
        let (action, _) = cadence_step(true, 1, 1, Some(deadline), t0, &config);
        assert_eq!(action, CadenceAction::SleepUntil(deadline));
        // Due again exactly 60s later.
        let (action, _) = cadence_step(true, 1, 1, Some(t0), t0 + FOREGROUND_POLL, &config);
        assert_eq!(
            action,
            CadenceAction::Poll {
                next: FOREGROUND_POLL
            }
        );
    }

    #[test]
    fn cadence_background_300s_after_all_subscribers_disconnect() {
        let t0 = fake_now();
        let config = GhPlaneConfig::default();
        // Ever connected, subscriber just dropped, due -> poll, 300s cadence.
        let (action, _) = cadence_step(true, 0, 1, Some(t0 - Duration::from_secs(1)), t0, &config);
        assert_eq!(
            action,
            CadenceAction::Poll {
                next: BACKGROUND_POLL
            }
        );
        // Just polled with nobody watching -> next poll scheduled 300s out.
        let (action, _) = cadence_step(true, 0, 0, Some(t0 + Duration::from_secs(10)), t0, &config);
        assert_eq!(
            action,
            CadenceAction::SleepUntil(t0 + Duration::from_secs(10))
        );
        let (action, _) = cadence_step(true, 0, 0, Some(t0), t0 + BACKGROUND_POLL, &config);
        assert_eq!(
            action,
            CadenceAction::Poll {
                next: BACKGROUND_POLL
            }
        );
    }

    #[test]
    fn failure_backoff_doubles_then_caps() {
        let cap = Duration::from_secs(300);
        let mut backoff = Duration::from_secs(5);
        let mut delays = Vec::new();
        for _ in 0..12 {
            let (delay, next) = failure_backoff_step(backoff, cap);
            delays.push(delay);
            backoff = next;
        }
        let expected = [5, 10, 20, 40, 80, 160, 300, 300, 300, 300, 300, 300];
        for (delay, seconds) in delays.iter().zip(expected) {
            assert_eq!(*delay, Duration::from_secs(seconds));
        }
        // Capped at the cadence when the cadence is smaller than the backoff.
        let (delay, next) =
            failure_backoff_step(Duration::from_millis(400), Duration::from_millis(150));
        assert_eq!(delay, Duration::from_millis(150));
        assert_eq!(next, Duration::from_millis(150));
        // The first failure waits the base backoff, not the full cadence.
        let (delay, _) = failure_backoff_step(Duration::from_secs(5), Duration::from_secs(60));
        assert_eq!(delay, Duration::from_secs(5));
    }
}
