//! The eframe application: owns the fleet state, the background read
//! loop (SSE), the signed-drive dispatch, registration, and the three
//! tabs (Board / Audit / Settings).

use std::collections::VecDeque;
use std::path::PathBuf;

use eframe::egui::{self, RichText};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};

use crate::drive::{DriveEndpoint, DriveIntent, DriveOutcome};
use crate::keys::{DeviceKey, KeyStore};
use crate::protocol::{self, ApplyMsg};
use crate::state::{
    AuditMsg, ConnState, DriveMsg, Fleet, GrantLedger, Level, RegistrationRecord, Toast,
};
use crate::theme;

/// The three top-level views.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tab {
    Board,
    Audit,
    Settings,
}

/// Runtime-loaded + persisted app config (host URL, registration record).
#[derive(Debug, Clone, PartialEq)]
struct PersistedConfig {
    host_url: String,
    registration: Option<RegistrationRecord>,
}

impl Default for PersistedConfig {
    fn default() -> Self {
        Self {
            host_url: protocol::DEFAULT_HOST_URL.to_string(),
            registration: None,
        }
    }
}

impl PersistedConfig {
    fn load(path: &PathBuf) -> Self {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str::<crate::state::PersistedConfig>(&s).ok())
            .map(|c| PersistedConfig {
                host_url: c
                    .host_url
                    .unwrap_or_else(|| protocol::DEFAULT_HOST_URL.to_string()),
                registration: c.registration,
            })
            .unwrap_or_default()
    }

    fn persist(&self, path: &PathBuf) {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::create_dir_all(path.parent().expect("config has parent"));
        let wire = crate::state::PersistedConfig {
            host_url: Some(self.host_url.clone()),
            registration: self.registration.clone(),
        };
        if let Ok(json) = serde_json::to_string_pretty(&wire) {
            let tmp = path.with_extension("tmp");
            if std::fs::write(&tmp, json).is_ok() {
                let _ = std::fs::rename(&tmp, path);
                let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
            }
        }
    }
}

pub struct CorralApp {
    rt: tokio::runtime::Handle,
    client: reqwest::Client,

    // Fleet / connection state (UI-owned).
    fleet: Fleet,
    conn: ConnState,
    conn_detail: Option<String>,
    toasts: VecDeque<Toast>,

    // Device identity.
    device_key: Option<DeviceKey>,
    device_key_store_warning: bool,
    ledger: GrantLedger,
    registration: Option<RegistrationRecord>,
    host_fingerprint: Option<String>,

    // Config + settings.
    config: PersistedConfig,
    config_path: PathBuf,
    settings: crate::ui::register::SettingsState,

    // Channels.
    tx_apply: UnboundedSender<ApplyMsg>,
    rx_apply: UnboundedReceiver<ApplyMsg>,
    rx_drive: UnboundedReceiver<DriveMsg>,
    tx_drive: UnboundedSender<DriveMsg>,
    tx_audit: UnboundedSender<AuditMsg>,
    rx_audit: UnboundedReceiver<AuditMsg>,
    tx_transcript: UnboundedSender<crate::transcript::TranscriptMsg>,
    rx_transcript: UnboundedReceiver<crate::transcript::TranscriptMsg>,
    stop_read: Option<tokio::sync::watch::Sender<bool>>,

    // Audit view.
    audit: Option<Result<crate::protocol::AuditView, String>>,
    audit_loading: bool,
    audit_last_refresh: std::time::Instant,

    // Tabs.
    tab: Tab,

    /// Evidence capture (env-gated): when `CORRAL_UI_SCREENSHOT` is set,
    /// request a viewport screenshot after a delay and write the PNG there
    /// before exiting. Never active by default.
    screenshot_path: Option<PathBuf>,
    screenshot_sent: bool,
    screenshot_deadline: Option<std::time::Instant>,
}

impl CorralApp {
    pub fn new(cc: &eframe::CreationContext<'_>, rt: tokio::runtime::Handle) -> Self {
        cc.egui_ctx.set_visuals(theme::dark_dashboard());
        configure_fonts(&cc.egui_ctx);

        let config_path = crate::keys::client_config_dir().join("config.json");
        let config = PersistedConfig::load(&config_path);
        let host_url = config.host_url.clone();

        let client = reqwest::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(3))
            // #64 review F6: the corrald API never redirects, and a
            // redirect would forward the signed x-corral-drive header (a
            // replayable read credential) to wherever a hostile 302
            // points — reqwest only strips Authorization-class headers.
            // DELIBERATELY GLOBAL (R6): this is the shared client, so
            // every endpoint (/snapshot, /events, /drive, /audit) stops
            // following redirects too — a redirecting proxy in front of
            // corrald now fails loudly as HTTP 3xx everywhere instead of
            // silently forwarding credentials.
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .expect("reqwest client");

        let (tx_apply, rx_apply) = tokio::sync::mpsc::unbounded_channel();
        let (tx_drive, rx_drive) = tokio::sync::mpsc::unbounded_channel();
        let (tx_audit, rx_audit) = tokio::sync::mpsc::unbounded_channel();
        let (tx_transcript, rx_transcript) = tokio::sync::mpsc::unbounded_channel();
        let client_for_fp = client.clone();

        let mut app = CorralApp {
            rt,
            client,
            fleet: Fleet::default(),
            conn: ConnState::Connecting,
            conn_detail: None,
            toasts: VecDeque::new(),
            device_key: None,
            device_key_store_warning: false,
            ledger: GrantLedger::default(),
            registration: config.registration.clone(),
            host_fingerprint: None,
            config,
            config_path,
            settings: crate::ui::register::SettingsState {
                host_url: host_url.clone(),
                ..Default::default()
            },
            tx_apply: tx_apply.clone(),
            rx_apply,
            rx_drive,
            tx_drive,
            tx_audit: tx_audit.clone(),
            rx_audit,
            tx_transcript,
            rx_transcript,
            stop_read: None,
            audit: None,
            audit_loading: false,
            audit_last_refresh: std::time::Instant::now(),
            tab: Tab::Board,
            screenshot_path: std::env::var("CORRAL_UI_SCREENSHOT")
                .ok()
                .map(PathBuf::from),
            screenshot_sent: false,
            screenshot_deadline: std::env::var("CORRAL_UI_SCREENSHOT_DELAY_MS")
                .ok()
                .and_then(|s| s.parse::<u64>().ok())
                .map(|ms| std::time::Instant::now() + std::time::Duration::from_millis(ms))
                .or_else(|| {
                    std::env::var("CORRAL_UI_SCREENSHOT")
                        .ok()
                        .map(|_| std::time::Instant::now() + std::time::Duration::from_secs(6))
                }),
        };

        // Resolve the host identity so the device key can be scoped to it.
        let host_url_for_fp = host_url.clone();
        let tx_apply_clone = tx_apply.clone();
        let rt_handle = app.rt.clone();
        rt_handle.spawn(async move {
            let fingerprint = match protocol::fetch_host_key(&client_for_fp, &host_url_for_fp).await
            {
                Ok(host) => crate::keys::host_fingerprint(Some(&host.public_key), &host_url_for_fp),
                Err(_) => crate::keys::host_fingerprint(None, &host_url_for_fp),
            };
            let _ = tx_apply_clone.send(ApplyMsg::Fingerprint(fingerprint));
        });

        app.spawn_read_loop(host_url.clone());
        app
    }

    fn spawn_read_loop(&mut self, host_url: String) {
        if let Some(stop) = self.stop_read.take() {
            let _ = stop.send(true);
        }
        let (stop_tx, stop_rx) = tokio::sync::watch::channel(false);
        self.stop_read = Some(stop_tx);
        let tx = self.tx_apply();
        protocol::spawn_read_loop(
            self.rt.clone(),
            self.client.clone(),
            host_url.clone(),
            tx.clone(),
            stop_rx.clone(),
        );
    }

    fn tx_apply(&self) -> UnboundedSender<ApplyMsg> {
        self.tx_apply.clone()
    }

    fn toast(&mut self, level: Level, text: impl Into<String>) {
        self.toasts.push_back(Toast {
            text: text.into(),
            level,
            at: std::time::Instant::now(),
        });
        while self.toasts.len() > 6 {
            self.toasts.pop_front();
        }
    }

    fn apply_fingerprint(&mut self, fingerprint: String) {
        if self.host_fingerprint.as_deref() == Some(&fingerprint) {
            return;
        }
        self.host_fingerprint = Some(fingerprint.clone());
        match crate::keys::load_or_create_key(&fingerprint) {
            Ok(key) => {
                self.device_key_store_warning = matches!(key.store, KeyStore::File { .. });
                self.device_key = Some(key);
            }
            Err(e) => {
                self.toast(Level::Error, format!("device key init failed: {e}"));
            }
        }
        // The stored registration may belong to a different host fingerprint.
        if let Some(reg) = &self.registration {
            if reg.host_fingerprint != fingerprint {
                self.toast(
                    Level::Warn,
                    format!(
                        "stored registration is for host {}, not {} — re-register to drive this host",
                        reg.host_fingerprint, fingerprint
                    ),
                );
                self.registration = None;
            } else {
                // Restore the persisted grant ledger, including demotions
                // that survived a restart (F3).
                self.ledger = GrantLedger {
                    base: reg.grants.clone(),
                    denied: reg.denied.clone(),
                };
            }
        }
        if self.device_key_store_warning {
            self.toast(
                Level::Warn,
                "OS keychain unavailable — device key stored in a 0600 file (see Settings)",
            );
        }
        if self.registration.is_some() {
            let admin_token = crate::keys::load_admin_token(&fingerprint)
                .or_else(crate::keys::read_daemon_admin_token);
            self.settings.admin_token_input = admin_token.unwrap_or_default();
        }
    }

    fn on_apply(&mut self, msg: ApplyMsg) {
        match msg {
            ApplyMsg::Fingerprint(fp) => self.apply_fingerprint(fp),
            ApplyMsg::RegisterResult(result) => self.handle_register_result(result),
            ApplyMsg::Sse(protocol::SseEvent::Snapshot(snap)) => {
                self.fleet.apply_snapshot(&snap);
            }
            ApplyMsg::Sse(protocol::SseEvent::Delta(delta)) => {
                self.fleet.apply_delta(&delta);
            }
            ApplyMsg::Sse(protocol::SseEvent::Unknown { event, .. }) => {
                tracing::debug!(event, "ignored SSE event");
            }
            ApplyMsg::Conn(protocol::Live::Connected) => {
                self.conn = ConnState::Connected;
                self.conn_detail = None;
            }
            ApplyMsg::Conn(protocol::Live::Disconnected) => {
                self.conn = ConnState::Connecting;
                self.conn_detail = Some("disconnected — reconnecting".to_string());
            }
            ApplyMsg::Conn(protocol::Live::Reconnecting { backoff_ms, rev }) => {
                self.conn = ConnState::Reconnecting { backoff_ms };
                self.conn_detail = Some(format!(
                    "resuming from rev {} (Last-Event-ID)",
                    rev.map(|r| r.to_string())
                        .unwrap_or_else(|| "none".to_string())
                ));
            }
            ApplyMsg::ConnError(e) => {
                self.conn = ConnState::Down;
                self.conn_detail = Some(e);
            }
        }
    }

    fn on_drive(&mut self, msg: DriveMsg) {
        let capability = msg.capability.clone();
        let state = crate::ui::board::classify_drive_state(&msg.outcome, &msg.capability);
        self.fleet.remember_drive(&msg.agent_id, state.clone());
        match &msg.outcome {
            DriveOutcome::Ok { rev, .. } => {
                self.ledger.note_success(&capability);
                self.persist_ledger();
                // If the daemon ever grows a read_tail result, surface it.
                if capability == "read_tail" {
                    self.remember_tail_result(&msg);
                }
                self.toast(Level::Info, format!("{capability} → ok (rev {rev})"));
            }
            DriveOutcome::Refused(failure) => {
                self.ledger.note_refusal(failure);
                self.persist_ledger();
                if matches!(failure, crate::drive::DriveFailure::StaleAgent(_)) {
                    // A stale tap is a read-model event, not a generic drive
                    // failure: remove the row before the next frame renders
                    // controls, then refresh once for the current identity.
                    self.fleet.remove_agent(&msg.agent_id);
                    self.refresh_snapshot();
                    self.toast(
                        Level::Warn,
                        format!("{} disappeared; refreshing the fleet", msg.agent_id),
                    );
                    return;
                }
                let level = if failure.suggests_re_registration() {
                    Level::Warn
                } else {
                    Level::Error
                };
                self.toast(level, format!("{capability} refused: {failure}"));
                if failure.suggests_re_registration() {
                    self.settings.notice = Some((
                        Level::Warn,
                        format!("{failure} — re-register this device (Settings → re-register)."),
                    ));
                }
            }
        }
    }

    /// One-shot read-model refresh after a typed stale-target refusal. The
    /// live SSE loop remains authoritative; this closes the tap-to-refresh
    /// gap without creating a daemon poll loop.
    fn refresh_snapshot(&self) {
        let client = self.client.clone();
        let base_url = self.config.host_url.clone();
        let tx = self.tx_apply.clone();
        self.rt.spawn(async move {
            if let Ok(snapshot) = protocol::fetch_snapshot(&client, &base_url).await {
                let _ = tx.send(ApplyMsg::Sse(protocol::SseEvent::Snapshot(snapshot)));
            }
        });
    }

    /// #64: one transcript page arrived (or failed). Correlation and
    /// state transitions live in [`crate::state::Fleet::fold_transcript`]
    /// (review F1 — pure, generation-checked, never resurrects a deleted
    /// pane); this handler only acts on the outcome: refetch on the
    /// one-shot stale-cursor reload, and keep the grant ledger honest
    /// (review F5 — a transcript `not_granted` demotes read_tail exactly
    /// like a drive refusal; a served page proves the grant and clears
    /// the demotion).
    fn on_transcript(&mut self, msg: crate::transcript::TranscriptMsg) {
        use crate::transcript::FoldOutcome;
        let agent_id = msg.agent_id.clone();
        match self.fleet.fold_transcript(msg) {
            FoldOutcome::Dropped | FoldOutcome::Applied => {}
            FoldOutcome::AppliedOk => {
                self.ledger.note_success("read_tail");
                self.persist_ledger();
            }
            FoldOutcome::NotGranted => {
                self.ledger.note_denied("read_tail");
                self.persist_ledger();
            }
            FoldOutcome::NeedsReload => {
                self.toast(
                    Level::Info,
                    "transcript session changed — reloading from newest".to_string(),
                );
                self.request_transcript_page(crate::transcript::TranscriptRequest {
                    agent_id,
                    cursor: None,
                });
            }
        }
    }

    /// #64: spawn one signed `GET /transcript` page fetch, stamped with
    /// the pane's generation (review F1) so a late response can be
    /// dropped. `cursor: None` resets the pane and loads the newest
    /// page; `Some` extends with an older one (or retries the same
    /// cursor after a transient failure — review F7: dispatch clears
    /// the error so the spinner shows). No registration/device key is a
    /// pane-level error, not a toast storm.
    fn request_transcript_page(&mut self, request: crate::transcript::TranscriptRequest) {
        let (Some(reg), Some(signing)) = (
            self.registration.clone(),
            self.device_key.as_ref().map(|k| k.signing.clone()),
        ) else {
            let pane = self.fleet.transcript_pane_mut(&request.agent_id);
            pane.apply_failure(crate::transcript::TranscriptFailure {
                kind: "not_registered".to_string(),
                message: "register this device (Settings) to read transcripts".to_string(),
                candidates: Vec::new(),
            });
            return;
        };
        let generation = {
            let pane = self.fleet.transcript_pane_mut(&request.agent_id);
            if request.cursor.is_none() && !pane.loading {
                // Explicit newest-page loads reset under a fresh
                // generation (touched == the fleet clock the pane_mut
                // call just stamped, so it is new and unique); the
                // auto-reload path has already reset before we get here.
                let fresh = pane.touched;
                pane.user_reset(fresh);
            }
            pane.loading = true;
            pane.error = None;
            pane.generation
        };
        // Post-#88: the page parameters ride the SIGNED header payload,
        // so each page is signed with its own cursor/ts/limit — paging
        // re-signs per page with the new cursor.
        let header = crate::drive::transcript_auth_header(
            &reg.key_id,
            &signing,
            &request.agent_id,
            request.cursor.as_deref(),
            crate::transcript::PAGE_LIMIT,
        );
        let client = self.client.clone();
        let base_url = self.config.host_url.clone();
        let tx = self.tx_transcript.clone();
        let agent_id = request.agent_id.clone();
        self.rt.spawn(async move {
            let outcome =
                crate::protocol::fetch_transcript(&client, &base_url, &header, &agent_id).await;
            let _ = tx.send(crate::transcript::TranscriptMsg {
                agent_id,
                generation,
                outcome,
            });
        });
    }

    /// Persist the ledger's demoted capabilities alongside the
    /// registration record (F3): a `not_granted` demotion survives a
    /// restart, and a later grants refresh or successful drive clears it.
    fn persist_ledger(&mut self) {
        if let Some(reg) = &mut self.config.registration
            && let Some(mut current) = self.registration.clone()
        {
            current.denied = self.ledger.denied.clone();
            if current != *reg {
                *reg = current;
                self.config.persist(&self.config_path);
            }
        }
    }

    /// read_tail content path: the daemon's `DriveResponse.result` carries
    /// `{"lines": [...]}` (redacted + bounded before the bytes left it) —
    /// store into the tail cache for the detail view. An empty lines array
    /// (agent with no output) stores an empty tail so the view shows the
    /// clean empty state.
    fn remember_tail_result(&mut self, msg: &DriveMsg) {
        let DriveOutcome::Ok { result, .. } = &msg.outcome else {
            return;
        };
        let Some(result) = result else {
            return;
        };
        let lines = crate::drive::parse_tail_lines(result);
        self.fleet.remember_tail(&msg.agent_id, lines);
    }

    /// Register (or re-register with a fresh key, or refresh grants with
    /// the existing key) against the host.
    ///
    /// Rotation ordering (F5): the new pubkey is registered FIRST; the
    /// seed rotation is persisted only AFTER the daemon accepted it. A
    /// failed re-register therefore leaves the old seed + old key_id
    /// untouched and consistent — never an orphaned key_id that 401s.
    /// (First registration persists the seed up front: an unregistered
    /// seed is harmless and retries reuse it.)
    fn register(&mut self, token: String, rotate: bool) {
        let Some(fp) = self.host_fingerprint.clone() else {
            self.settings.notice = Some((
                Level::Error,
                "cannot register: host fingerprint unknown — is corrald reachable?".into(),
            ));
            return;
        };
        let client = self.client.clone();
        let host_url = self.config.host_url.clone();
        let rt = self.rt.clone();
        let tx = self.tx_apply();
        rt.spawn(async move {
            let registration: Result<(String, Vec<String>), String> = if rotate {
                // Fresh seed in memory only — nothing persisted yet.
                let mut seed = [0u8; 32];
                match getrandom::fill(&mut seed) {
                    Ok(()) => {
                        let signing = ed25519_dalek::SigningKey::from_bytes(&seed);
                        let pubkey_b64 = {
                            use base64::Engine;
                            base64::engine::general_purpose::STANDARD
                                .encode(signing.verifying_key().to_bytes())
                        };
                        match protocol::register_device(&client, &host_url, &token, &pubkey_b64)
                            .await
                        {
                            Ok(reg) => {
                                // The daemon accepted the new key: only NOW
                                // persist the rotation.
                                match crate::keys::rotate_key(&fp, &seed) {
                                    Ok(()) => Ok(reg),
                                    Err(e) => Err(e),
                                }
                            }
                            Err(e) => Err(e),
                        }
                    }
                    Err(e) => Err(format!("OS RNG failure: {e}")),
                }
            } else {
                // Existing key (or a fresh one for first registration):
                // re-register its pubkey to (re)learn current grants.
                let key = match crate::keys::load_or_create_key(&fp) {
                    Ok(key) => key,
                    Err(e) => {
                        let _ = tx.send(ApplyMsg::RegisterResult(Err(e)));
                        return;
                    }
                };
                let pubkey_b64 = {
                    use base64::Engine;
                    base64::engine::general_purpose::STANDARD
                        .encode(key.signing.verifying_key().to_bytes())
                };
                protocol::register_device(&client, &host_url, &token, &pubkey_b64).await
            };
            let _ = tx.send(ApplyMsg::RegisterResult(registration));
        });
    }

    fn handle_register_result(&mut self, result: Result<(String, Vec<String>), String>) {
        match result {
            Ok((key_id, grants)) => {
                let fp = self.host_fingerprint.clone().unwrap_or_default();
                // A successful (re)registration refreshes the ledger from
                // the host's CURRENT grant set: any locally-demoted
                // capability the host re-granted is re-enabled, and a
                // capability the host revoked is dropped (F3).
                self.registration = Some(RegistrationRecord {
                    host_fingerprint: fp.clone(),
                    key_id: key_id.clone(),
                    grants: grants.clone(),
                    denied: vec![],
                });
                self.ledger = GrantLedger {
                    base: grants.clone(),
                    denied: vec![],
                };
                // F5 success path: a re-register rotated the persisted
                // seed BEFORE this result arrived — reload the in-memory
                // signing key so subsequent drives sign with the NEW key
                // (otherwise the next drive presents the new key_id with
                // the old signature and 401s bad_signature until restart).
                if let Some(fp) = self.host_fingerprint.clone()
                    && let Ok(key) = crate::keys::load_or_create_key(&fp)
                {
                    self.device_key = Some(key);
                }
                self.config.registration = self.registration.clone();
                self.config.persist(&self.config_path);
                self.toast(
                    Level::Info,
                    format!("registered as {key_id} (grants: {})", grants.join(", ")),
                );
                self.settings.token_input.clear();
                self.tab = Tab::Board;
            }
            Err(e) => {
                self.toast(Level::Error, format!("registration failed: {e}"));
                self.settings.notice = Some((Level::Error, format!("registration failed: {e}")));
            }
        }
    }

    /// The admin token for the audit view: host's own token on localhost,
    /// or the keychain-stored one.
    fn admin_token(&self) -> Option<String> {
        let fp = self.host_fingerprint.clone()?;
        crate::keys::load_admin_token(&fp).or_else(crate::keys::read_daemon_admin_token)
    }

    fn refresh_audit(&mut self) {
        let Some(token) = self.admin_token() else {
            self.audit = Some(Err(
                "no admin token available (Settings → audit) — the log is host-admin".into(),
            ));
            return;
        };
        self.audit_loading = true;
        let client = self.client.clone();
        let host_url = self.config.host_url.clone();
        let tx = self.tx_audit.clone();
        self.rt.spawn(async move {
            let view = protocol::fetch_audit(&client, &host_url, &token).await;
            let _ = tx.send(AuditMsg { view });
        });
    }

    fn update_settings_request(&mut self) {
        let Some(request) = self.settings.requested.take() else {
            return;
        };
        match request {
            crate::ui::register::Request::Connect => {
                let url = self.settings.host_url.trim().to_string();
                if url.is_empty() {
                    self.settings.notice =
                        Some((Level::Error, "host URL must not be empty".into()));
                    return;
                }
                if url != self.config.host_url {
                    self.config.host_url = url.clone();
                    self.config.registration = None; // key scoping may change
                    self.registration = None;
                    self.ledger = GrantLedger::default();
                    self.host_fingerprint = None;
                    self.config.persist(&self.config_path);
                    self.spawn_read_loop(url.clone());
                    self.resolve_fingerprint(url);
                }
            }
            crate::ui::register::Request::Register => {
                let token = self.settings.token_input.trim().to_string();
                if token.is_empty() {
                    self.settings.notice =
                        Some((Level::Error, "registration token required".into()));
                } else {
                    self.register(token, false);
                }
            }
            crate::ui::register::Request::AutoRegister => {
                match crate::keys::read_daemon_registration_token() {
                    Ok(token) => self.register(token, false),
                    Err(e) => {
                        self.settings.notice = Some((
                            Level::Error,
                            format!("auto-register unavailable: {e} (paste the token instead)"),
                        ));
                    }
                }
            }
            crate::ui::register::Request::ReRegister => {
                match crate::keys::read_daemon_registration_token() {
                    Ok(token) => self.register(token, true),
                    Err(e) => {
                        self.settings.notice = Some((
                            Level::Error,
                            format!("re-register needs the token: {e} (paste it above)"),
                        ));
                    }
                }
            }
            crate::ui::register::Request::RefreshGrants => {
                // Re-register the SAME device key: the daemon returns the
                // key's CURRENT grant set, which re-enables capabilities
                // the host granted since registration and drops revoked
                // ones (F3/F4 recovery path).
                let token = if !self.settings.token_input.trim().is_empty() {
                    self.settings.token_input.trim().to_string()
                } else {
                    match crate::keys::read_daemon_registration_token() {
                        Ok(t) => t,
                        Err(e) => {
                            self.settings.notice = Some((
                                Level::Error,
                                format!("refresh grants needs the registration token: {e}"),
                            ));
                            return;
                        }
                    }
                };
                self.register(token, false);
            }
            crate::ui::register::Request::SaveAdminToken => {
                let token = self.settings.admin_token_input.trim().to_string();
                let fp = self.host_fingerprint.clone();
                match (token, fp) {
                    (t, Some(fp)) if !t.is_empty() => {
                        match crate::keys::store_admin_token(&fp, &t) {
                            Ok(_) => {
                                self.toast(Level::Info, "admin token stored in OS keychain");
                            }
                            Err(e) => {
                                self.toast(
                                    Level::Warn,
                                    format!("admin token not persisted ({e}); kept in memory for this session"),
                                );
                            }
                        }
                    }
                    _ => {
                        self.toast(Level::Warn, "empty admin token — nothing saved");
                    }
                }
            }
            crate::ui::register::Request::ClearAdminToken => {
                self.settings.admin_token_input.clear();
                self.audit = None;
            }
        }
    }

    fn resolve_fingerprint(&self, host_url: String) {
        let tx = self.tx_apply();
        let client = self.client.clone();
        self.rt.spawn(async move {
            let fingerprint = match protocol::fetch_host_key(&client, &host_url).await {
                Ok(host) => crate::keys::host_fingerprint(Some(&host.public_key), &host_url),
                Err(_) => crate::keys::host_fingerprint(None, &host_url),
            };
            let _ = tx.send(ApplyMsg::Fingerprint(fingerprint));
        });
    }
}

fn configure_fonts(ctx: &egui::Context) {
    let mut style = (*ctx.style_of(egui::Theme::Dark)).clone();
    style.spacing.item_spacing = egui::vec2(6.0, 4.0);
    style.spacing.button_padding = egui::vec2(8.0, 3.0);
    ctx.set_style_of(egui::Theme::Dark, style);
}

/// Write a viewport `ColorImage` as PNG (evidence capture only).
fn save_png(path: &std::path::Path, image: &egui::ColorImage) {
    let size = [image.size[0] as u32, image.size[1] as u32];
    let mut png: image::RgbaImage = image::ImageBuffer::new(size[0], size[1]);
    for (pixel, color) in png.pixels_mut().zip(image.pixels.iter()) {
        *pixel = image::Rgba([color.r(), color.g(), color.b(), color.a()]);
    }
    if let Err(e) = png.save(path) {
        tracing::error!(path = %path.display(), error = %e, "screenshot save failed");
    }
}

impl eframe::App for CorralApp {
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        // Drain background channels (and wake the UI when they deliver).
        let mut got_messages = false;
        while let Ok(msg) = self.rx_apply.try_recv() {
            got_messages = true;
            self.on_apply(msg);
        }
        while let Ok(msg) = self.rx_drive.try_recv() {
            got_messages = true;
            self.on_drive(msg);
        }
        while let Ok(msg) = self.rx_audit.try_recv() {
            got_messages = true;
            self.audit_loading = false;
            self.audit = Some(msg.view);
        }
        while let Ok(msg) = self.rx_transcript.try_recv() {
            got_messages = true;
            self.on_transcript(msg);
        }
        if got_messages {
            ctx.request_repaint();
        }

        // Esc collapses expanded rows.
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.fleet.expanded.clear();
        }

        // Evidence capture: request the viewport screenshot once the fleet
        // has had time to connect, then write the PNG and exit.
        if let Some(path) = self.screenshot_path.clone() {
            if !self.screenshot_sent
                && self
                    .screenshot_deadline
                    .is_some_and(|d| std::time::Instant::now() >= d)
            {
                self.screenshot_sent = true;
                tracing::info!(path = %path.display(), "requesting viewport screenshot");
                ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::UserData::default()));
            }
            // The wgpu screenshot readback completes on a device poll; the
            // map callback needs it driven from here.
            if self.screenshot_sent
                && let Some(rs) = frame.wgpu_render_state()
            {
                // Firing the capture map callback requires a device poll
                // that BLOCKS until the in-flight submissions complete;
                // bounded so a hung GPU cannot wedge the UI. Evidence
                // capture only (env-gated).
                let _ = rs.device.poll(wgpu::PollType::Wait {
                    submission_index: None,
                    timeout: Some(std::time::Duration::from_millis(500)),
                });
            }
            let mut captured: Option<std::sync::Arc<egui::ColorImage>> = None;
            ctx.input(|i| {
                for event in &i.events {
                    if let egui::Event::Screenshot { image, .. } = event {
                        captured = Some(image.clone());
                    }
                }
            });
            if let Some(image) = captured {
                save_png(&path, &image);
                tracing::info!(path = %path.display(), "screenshot saved — exiting");
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        }

        self.update_settings_request();

        egui::Panel::top("top").show(ui, |ui| {
            top_bar(ui, self);
        });

        if self.registration.is_none() {
            egui::CentralPanel::default().show(ui, |ui| {
                crate::ui::register::register_screen(ui, &mut self.settings, self.conn);
            });
            crate::ui::toast_area(&ctx, &mut self.toasts);
            ctx.request_repaint_after(std::time::Duration::from_millis(500));
            return;
        }

        egui::Panel::bottom("bottom").show(ui, |ui| {
            bottom_bar(ui, self);
        });

        egui::CentralPanel::default().show(ui, |ui| match self.tab {
            Tab::Board => {
                let ledger = self.ledger.clone();
                let allowed = |cap: &str| ledger.allowed(cap);
                // Split borrows: the drive closure needs &mut state while
                // board::show holds &mut fleet, so capture field copies.
                let registration = self.registration.clone();
                let signing = self.device_key.as_ref().map(|k| k.signing.clone());
                let client = self.client.clone();
                let host_url = self.config.host_url.clone();
                let tx_drive = self.tx_drive.clone();
                let rt = self.rt.clone();
                let mut pending: Vec<DriveIntent> = Vec::new();
                let mut pending_transcripts: Vec<crate::transcript::TranscriptRequest> = Vec::new();
                crate::ui::board::show(
                    ui,
                    &mut self.fleet,
                    &allowed,
                    &mut crate::ui::board::BoardActions {
                        drive: &mut |intent| pending.push(intent),
                        transcript: &mut |request| pending_transcripts.push(request),
                    },
                );
                for request in pending_transcripts {
                    self.request_transcript_page(request);
                }
                // Dispatch after board::show returns (no overlapping
                // borrows of fleet/toasts).
                for intent in pending {
                    let Some(reg) = registration.clone() else {
                        self.toasts.push_back(Toast {
                            text: "not registered — cannot drive".into(),
                            level: Level::Error,
                            at: std::time::Instant::now(),
                        });
                        continue;
                    };
                    let Some(signing) = signing.clone() else {
                        self.toasts.push_back(Toast {
                            text: "no device key — check Settings".into(),
                            level: Level::Error,
                            at: std::time::Instant::now(),
                        });
                        continue;
                    };
                    let endpoint = DriveEndpoint {
                        client: client.clone(),
                        base_url: host_url.clone(),
                        key_id: reg.key_id.clone(),
                        signing,
                    };
                    let agent_id = intent.target.clone();
                    let capability = intent.capability.to_string();
                    let tx = tx_drive.clone();
                    self.fleet.remember_drive(
                        &intent.target,
                        crate::state::DriveState::Sending {
                            request_id: intent.request_id.clone(),
                            capability: capability.clone(),
                        },
                    );
                    rt.spawn(async move {
                        let outcome = crate::drive::execute_drive(&endpoint, &intent).await;
                        let _ = tx.send(DriveMsg {
                            agent_id,
                            capability,
                            outcome,
                        });
                    });
                }
            }
            Tab::Audit => {
                let token_configured = self.admin_token().is_some();
                let mut refresh_requested = false;
                if self.audit.is_none() {
                    self.refresh_audit();
                }
                if self.audit_last_refresh.elapsed() >= std::time::Duration::from_secs(5) {
                    self.refresh_audit();
                    self.audit_last_refresh = std::time::Instant::now();
                }
                crate::ui::audit::show(
                    ui,
                    &self.audit,
                    token_configured,
                    self.audit_loading,
                    &mut || refresh_requested = true,
                );
                if refresh_requested {
                    self.refresh_audit();
                    self.audit_last_refresh = std::time::Instant::now();
                }
                ctx.request_repaint_after(std::time::Duration::from_secs(2));
            }
            Tab::Settings => {
                let store = self.device_key.as_ref().map(|k| k.store.clone());
                let key_id = self
                    .registration
                    .as_ref()
                    .map(|r| r.key_id.clone())
                    .unwrap_or_default();
                let grants = self
                    .registration
                    .as_ref()
                    .map(|r| r.grants.clone())
                    .unwrap_or_default();
                crate::ui::register::settings_pane(
                    ui,
                    &mut self.settings,
                    &key_id,
                    &grants,
                    store.as_ref(),
                    self.conn,
                    self.fleet.rev,
                );
            }
        });

        crate::ui::toast_area(&ctx, &mut self.toasts);
        ctx.request_repaint_after(std::time::Duration::from_millis(500));
    }
}

fn top_bar(ui: &mut egui::Ui, app: &mut CorralApp) {
    ui.horizontal(|ui| {
        ui.label(
            RichText::new("corral fleet")
                .strong()
                .color(theme::ui::ACCENT),
        );
        ui.separator();
        let host = app.config.host_url.clone();
        ui.label(
            RichText::new(host)
                .monospace()
                .small()
                .color(theme::ui::TEXT_MUTED),
        );
        crate::ui::connection_pill(ui, app.conn);
        if let Some(detail) = &app.conn_detail {
            ui.label(RichText::new(detail).small().color(theme::ui::TEXT_MUTED));
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.selectable_value(&mut app.tab, Tab::Settings, "settings");
            ui.selectable_value(&mut app.tab, Tab::Audit, "audit");
            ui.selectable_value(&mut app.tab, Tab::Board, "board");
        });
    });
}

fn bottom_bar(ui: &mut egui::Ui, app: &CorralApp) {
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(format!("agents {}", app.fleet.agents.len()))
                .small()
                .color(theme::ui::TEXT_MUTED),
        );
        ui.separator();
        let blocked = app
            .fleet
            .agents
            .values()
            .filter(|a| a.state == crate::model::AgentState::Blocked)
            .count();
        ui.label(
            RichText::new(format!("blocked {blocked}"))
                .small()
                .color(theme::ui::WARN),
        );
        ui.separator();
        let working = app
            .fleet
            .agents
            .values()
            .filter(|a| a.state == crate::model::AgentState::Working)
            .count();
        ui.label(
            RichText::new(format!("working {working}"))
                .small()
                .color(theme::ui::TEXT_MUTED),
        );
        if let Some(rev) = app.fleet.rev {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(
                    RichText::new(format!("rev {rev}"))
                        .monospace()
                        .small()
                        .color(theme::ui::TEXT_MUTED),
                );
            });
        }
    });
}
