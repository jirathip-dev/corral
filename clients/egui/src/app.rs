//! The eframe application: owns the fleet state, the background read
//! loop (SSE), the signed read-drive dispatch (read_tail), registration,
//! and the two workspace tabs (Board / Settings). #354 L3: no Issues tab,
//! no mutating drive, no grant admin, no audit — read-only board +
//! recents v1.

use std::collections::VecDeque;
use std::path::PathBuf;
// macos-only native probe helpers are the only non-test users of `Path`
// (#242 hosted CI red on Linux: an unconditional import is unused there).
#[cfg(target_os = "macos")]
use std::path::Path;
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU64, Ordering},
    mpsc::{Receiver, Sender},
};

use eframe::egui::{self, RichText};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};

use crate::drive::{DriveEndpoint, DriveIntent, DriveOutcome};
use crate::keys::{DeviceKey, KeyStore};
use crate::protocol::{self, ApplyMsg};
use crate::state::{ConnState, DriveMsg, Fleet, GrantLedger, Level, RegistrationRecord, Toast};
use crate::theme;

/// The two top-level views in the persistent right-hand tab strip (#354 L3:
/// Issues and the subordinate audit surface were removed with the cut).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tab {
    Board,
    Settings,
}

/// #249 device-identity recovery state. The board detects that its CURRENT
/// key material no longer matches the registered key_id (rebuild/reinstall
/// wiped or replaced the key while config.json kept the old record) and
/// re-registers the current key via the registration token (#354 L3: the
/// daemon is read-only, so there is no grant set to restore — the host
/// provisions grants out-of-band). States:
///
/// - `None` — identity consistent (steady state).
/// - `Mismatch` — recovery needed; only ever set by the USER-initiated
///   Settings recovery block (Restore saved identity), never at startup.
/// - `InFlight` — the user-initiated re-register / grant restore is running.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum IdentityRecovery {
    None,
    Mismatch,
    InFlight,
}

const TAB_LABELS: [(&str, Tab); 2] = [("Board", Tab::Board), ("Settings", Tab::Settings)];

/// #314: minimum spacing between automatic visible-agent Recent-output
/// refreshes. The refresh rides the existing frame cadence and this
/// cooldown paces it (no per-frame busy loop, no background task).
const RECENT_OUTPUT_REFRESH_COOLDOWN: std::time::Duration = std::time::Duration::from_secs(5);

const SCREENSHOT_RETRY_AFTER: std::time::Duration = std::time::Duration::from_secs(8);
const SCREENSHOT_MAX_ATTEMPTS: u8 = 3;
const SCREENSHOT_WAKE_RETRY_INTERVAL: std::time::Duration = std::time::Duration::from_secs(1);
const SCREENSHOT_WAKE_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(100);
const SCREENSHOT_WAKE_MAX_DURATION: std::time::Duration = std::time::Duration::from_secs(45);

/// Schedule exact-PID activation independently of eframe's `ui()` cadence.
///
/// macOS can stop delivering `ui()` frames while an eframe window is hidden or
/// occluded. The native evidence command therefore activates the window from
/// a bounded helper thread after target selection, rather than relying on the
/// first deferred probe to run the caller-supplied wake command.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct NativeWindowWakeSchedule {
    active_since: Option<std::time::Instant>,
    next_wake: Option<std::time::Instant>,
    expired: bool,
}

impl NativeWindowWakeSchedule {
    fn activate(&mut self, now: std::time::Instant) {
        if self.active_since.is_none() && !self.expired {
            self.active_since = Some(now);
            self.next_wake = Some(now);
        }
    }

    fn deactivate(&mut self) {
        self.active_since = None;
        self.next_wake = None;
        self.expired = false;
    }

    fn expire(&mut self) {
        self.active_since = None;
        self.next_wake = None;
        self.expired = true;
    }

    fn due(&mut self, now: std::time::Instant) -> bool {
        let Some(active_since) = self.active_since else {
            return false;
        };
        if now.saturating_duration_since(active_since) >= SCREENSHOT_WAKE_MAX_DURATION {
            self.expire();
            return false;
        }
        self.next_wake.is_some_and(|next_wake| now >= next_wake)
    }

    fn record_wake(&mut self, now: std::time::Instant) {
        let Some(active_since) = self.active_since else {
            return;
        };
        if now.saturating_duration_since(active_since) >= SCREENSHOT_WAKE_MAX_DURATION {
            self.expire();
        } else {
            self.next_wake = Some(now + SCREENSHOT_WAKE_RETRY_INTERVAL);
        }
    }

    fn sleep_for(self, now: std::time::Instant) -> std::time::Duration {
        self.next_wake
            .map(|next_wake| next_wake.saturating_duration_since(now))
            .unwrap_or(SCREENSHOT_WAKE_POLL_INTERVAL)
            .min(SCREENSHOT_WAKE_POLL_INTERVAL)
    }
}

/// The opt-in native evidence capture has an explicit readiness/settle state.
/// Target selection is immediately dispatchable, but every dispatch is
/// guarded by the native-window readiness probe in `ui`. A dispatch owns an
/// eight-second deadline; only a later egui Screenshot event containing a
/// successfully saved PNG can complete the capture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ScreenshotCaptureState {
    Disabled,
    WaitingForTarget,
    Ready,
    Settling {
        until: std::time::Instant,
    },
    AwaitingScreenshot {
        deadline: std::time::Instant,
        attempt: u8,
    },
    Exhausted,
    Complete,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ScreenshotDispatch {
    NotDue,
    DeferredForWindow,
    Dispatched { attempt: u8 },
    Exhausted,
}

impl ScreenshotCaptureState {
    fn initial(
        enabled: bool,
        target_required: bool,
        now: std::time::Instant,
        settle: std::time::Duration,
    ) -> Self {
        if !enabled {
            Self::Disabled
        } else if target_required {
            Self::WaitingForTarget
        } else {
            Self::Settling {
                until: now + settle,
            }
        }
    }

    fn target_ready_after(
        self,
        now: std::time::Instant,
        settle: std::time::Duration,
    ) -> (Self, bool) {
        match self {
            Self::WaitingForTarget => {
                let state = if settle.is_zero() {
                    Self::Ready
                } else {
                    Self::Settling {
                        until: now + settle,
                    }
                };
                (state, true)
            }
            state => (state, false),
        }
    }

    fn dispatch_due(self, now: std::time::Instant) -> bool {
        matches!(self, Self::Ready)
            || matches!(self, Self::Settling { until } if now >= until)
            || matches!(self, Self::AwaitingScreenshot { deadline, .. } if now >= deadline)
    }

    fn next_wake(self, now: std::time::Instant) -> std::time::Duration {
        match self {
            Self::Settling { until } => until
                .saturating_duration_since(now)
                .min(std::time::Duration::from_millis(100)),
            Self::AwaitingScreenshot { deadline, .. } => deadline
                .saturating_duration_since(now)
                .min(std::time::Duration::from_millis(100)),
            Self::WaitingForTarget => std::time::Duration::from_millis(100),
            Self::Ready => std::time::Duration::from_millis(100),
            Self::Disabled | Self::Exhausted | Self::Complete => std::time::Duration::from_secs(1),
        }
    }

    fn attempts(self) -> u8 {
        match self {
            Self::AwaitingScreenshot { attempt, .. } => attempt,
            Self::Exhausted => SCREENSHOT_MAX_ATTEMPTS,
            _ => 0,
        }
    }

    fn awaiting_screenshot(self) -> bool {
        matches!(self, Self::AwaitingScreenshot { .. })
    }

    fn try_dispatch(
        self,
        now: std::time::Instant,
        visible: bool,
        frontmost: bool,
    ) -> (Self, ScreenshotDispatch) {
        if !self.dispatch_due(now) {
            return (self, ScreenshotDispatch::NotDue);
        }
        if self.attempts() >= SCREENSHOT_MAX_ATTEMPTS {
            return (Self::Exhausted, ScreenshotDispatch::Exhausted);
        }
        if !visible || !frontmost {
            return (self, ScreenshotDispatch::DeferredForWindow);
        }

        let attempt = self.attempts() + 1;
        (
            Self::AwaitingScreenshot {
                deadline: now + SCREENSHOT_RETRY_AFTER,
                attempt,
            },
            ScreenshotDispatch::Dispatched { attempt },
        )
    }

    fn record_screenshot_event(self, valid_png: bool) -> Self {
        if valid_png && self.awaiting_screenshot() {
            Self::Complete
        } else {
            self
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NativeProbeReason {
    DispatchReady,
    DeferProbeFailed,
    DeferExactPidMismatch,
    DeferProcessHidden,
    DeferWindowHidden,
    DeferNotFrontmost,
    DeferCgWindowMissing,
}

impl NativeProbeReason {
    fn code(self) -> &'static str {
        match self {
            Self::DispatchReady => "dispatch_ready",
            Self::DeferProbeFailed => "defer_probe_failed",
            Self::DeferExactPidMismatch => "defer_exact_pid_mismatch",
            Self::DeferProcessHidden => "defer_process_hidden_or_unknown",
            Self::DeferWindowHidden => "defer_window_hidden_or_unknown",
            Self::DeferNotFrontmost => "defer_not_frontmost_or_unknown",
            Self::DeferCgWindowMissing => "defer_cg_window_missing",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct NativeProbeFacts {
    probe_ok: bool,
    exact_pid_match: bool,
    process_visible: Option<bool>,
    window_visible: Option<bool>,
    frontmost: Option<bool>,
    key_window: Option<bool>,
    main_window: Option<bool>,
    cg_owner_pid_match: Option<bool>,
}

#[derive(Debug, Clone)]
struct NativeProbeObservation {
    process_pid: Option<u32>,
    process_visible: Option<bool>,
    window_visible: Option<bool>,
    frontmost_observed: Option<bool>,
    key_window: Option<bool>,
    main_window: Option<bool>,
    frontmost_application_pid: Option<i64>,
    frontmost_application_matches_target: Option<bool>,
    exact_pid_match: bool,
    cg_owner_pid_match: Option<bool>,
}

fn classify_native_probe(facts: NativeProbeFacts) -> NativeProbeReason {
    if !facts.probe_ok {
        NativeProbeReason::DeferProbeFailed
    } else if !facts.exact_pid_match {
        NativeProbeReason::DeferExactPidMismatch
    } else if facts.process_visible != Some(true) {
        NativeProbeReason::DeferProcessHidden
    } else if facts.window_visible != Some(true) {
        NativeProbeReason::DeferWindowHidden
    } else if facts.frontmost != Some(true)
        || facts.key_window != Some(true)
        || facts.main_window != Some(true)
    {
        NativeProbeReason::DeferNotFrontmost
    } else if facts.cg_owner_pid_match != Some(true) {
        NativeProbeReason::DeferCgWindowMissing
    } else {
        NativeProbeReason::DispatchReady
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct CgWindowBounds {
    x: Option<f64>,
    y: Option<f64>,
    width: Option<f64>,
    height: Option<f64>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct CgWindowRecord {
    placement: usize,
    window_number: Option<i64>,
    layer: Option<i64>,
    onscreen: Option<bool>,
    bounds: Option<CgWindowBounds>,
}

#[derive(Debug, Clone, serde::Serialize)]
struct NativeWindowState {
    pid: u32,
    process_pid: Option<u32>,
    exact_pid_match: bool,
    process_visible: Option<bool>,
    window_visible: Option<bool>,
    frontmost_observed: Option<bool>,
    key_window: Option<bool>,
    main_window: Option<bool>,
    frontmost_application_pid: Option<i64>,
    frontmost_application_matches_target: Option<bool>,
    cg_owner_pid_match: Option<bool>,
    cg_windows: Vec<CgWindowRecord>,
    non_target_window_count: Option<usize>,
    probe_ok: bool,
    probe_error: Option<String>,
    cg_error: Option<String>,
    visible: bool,
    frontmost: bool,
    reason_code: &'static str,
}

static NATIVE_WINDOW_PROBE_SEQUENCE: AtomicU64 = AtomicU64::new(1);

impl NativeWindowState {
    fn from_facts(
        pid: u32,
        observation: NativeProbeObservation,
        cg_windows: Vec<CgWindowRecord>,
        non_target_window_count: Option<usize>,
        probe_ok: bool,
        probe_error: Option<String>,
        cg_error: Option<String>,
    ) -> Self {
        let reason = classify_native_probe(NativeProbeFacts {
            probe_ok,
            exact_pid_match: observation.exact_pid_match,
            process_visible: observation.process_visible,
            window_visible: observation.window_visible,
            frontmost: observation.frontmost_observed,
            key_window: observation.key_window,
            main_window: observation.main_window,
            cg_owner_pid_match: observation.cg_owner_pid_match,
        });
        let exact_process_ready = probe_ok && observation.exact_pid_match;
        let cg_window_ready = observation.cg_owner_pid_match == Some(true);
        Self {
            pid,
            process_pid: observation.process_pid,
            exact_pid_match: observation.exact_pid_match,
            process_visible: observation.process_visible,
            window_visible: observation.window_visible,
            frontmost_observed: observation.frontmost_observed,
            key_window: observation.key_window,
            main_window: observation.main_window,
            frontmost_application_pid: observation.frontmost_application_pid,
            frontmost_application_matches_target: observation.frontmost_application_matches_target,
            cg_owner_pid_match: observation.cg_owner_pid_match,
            cg_windows,
            non_target_window_count,
            probe_ok,
            probe_error,
            cg_error,
            // Keep the dispatch gates fail-closed when either exact process
            // identity or the optional CGWindow on-screen observation is
            // known to disagree. A missing optional CG helper remains
            // unknown rather than weakening the Accessibility gate.
            visible: exact_process_ready
                && cg_window_ready
                && observation.process_visible == Some(true)
                && observation.window_visible == Some(true),
            frontmost: exact_process_ready
                && observation.frontmost_observed == Some(true)
                && observation.key_window == Some(true)
                && observation.main_window == Some(true)
                && observation.frontmost_application_matches_target != Some(false),
            reason_code: reason.code(),
        }
    }
}

fn emit_native_window_probe(action: &str, state: &NativeWindowState) {
    let sample_id = NATIVE_WINDOW_PROBE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let timestamp_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or_default();
    let record = serde_json::json!({
        "sample_id": sample_id,
        "timestamp_unix_ms": timestamp_ms,
        "action": action,
        "pid": state.pid,
        "process_pid": state.process_pid,
        "exact_pid_match": state.exact_pid_match,
        "process_visible": state.process_visible,
        "window_visible": state.window_visible,
        "frontmost": state.frontmost_observed,
        "key_window": state.key_window,
        "main_window": state.main_window,
        "frontmost_application_pid": state.frontmost_application_pid,
        "frontmost_application_matches_target": state.frontmost_application_matches_target,
        "cg_owner_pid_match": state.cg_owner_pid_match,
        "cg_window_list": state.cg_windows,
        "non_target_window_count": state.non_target_window_count,
        "probe_ok": state.probe_ok,
        "probe_error": state.probe_error,
        "cg_error": state.cg_error,
        "visible_gate": state.visible,
        "frontmost_gate": state.frontmost,
        "reason_code": state.reason_code,
    });
    let record_text = record.to_string();
    tracing::info!(
        target: "corrald_ui::native_window_probe",
        sample_id,
        timestamp_unix_ms = timestamp_ms,
        action,
        pid = state.pid,
        process_pid = ?state.process_pid,
        exact_pid_match = state.exact_pid_match,
        process_visible = ?state.process_visible,
        window_visible = ?state.window_visible,
        frontmost = ?state.frontmost_observed,
        key_window = ?state.key_window,
        main_window = ?state.main_window,
        frontmost_application_pid = ?state.frontmost_application_pid,
        frontmost_application_matches_target = ?state.frontmost_application_matches_target,
        cg_owner_pid_match = ?state.cg_owner_pid_match,
        cg_window_count = state.cg_windows.len(),
        non_target_window_count = ?state.non_target_window_count,
        cg_windows = %record_text,
        reason_code = state.reason_code,
        "native window probe evaluation"
    );
    if let Ok(path) = std::env::var("CORRAL_UI_WINDOW_DIAGNOSTIC_LOG")
        && let Err(error) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .and_then(|mut file| {
                use std::io::Write;
                writeln!(file, "{record_text}")
            })
    {
        tracing::warn!(path = %path, error = %error, "could not persist native window probe record");
    }
}

/// Probe only the current corrald-ui process. The exact-PID helper combines
/// Accessibility properties with an on-screen CoreGraphics observation;
/// every evaluation is emitted and failure remains fail-closed. CoreGraphics
/// does not expose a public, independent Space membership query here, so the
/// capture gate makes no synthetic active-space claim.
fn native_window_state(action: &str) -> NativeWindowState {
    #[cfg(target_os = "macos")]
    {
        let pid = std::process::id();
        let (native_probe, native_error) = match macos_cg_window_probe(pid) {
            Ok(probe) => (Some(probe), None),
            Err(error) => (None, Some(error)),
        };
        let cg_windows = native_probe
            .as_ref()
            .map(|probe| probe.windows.clone())
            .unwrap_or_default();
        let non_target_window_count = native_probe
            .as_ref()
            .map(|probe| probe.non_target_window_count);
        let state = NativeWindowState::from_facts(
            pid,
            NativeProbeObservation {
                process_pid: native_probe.as_ref().and_then(|probe| probe.process_pid),
                process_visible: native_probe
                    .as_ref()
                    .and_then(|probe| probe.process_visible),
                window_visible: native_probe.as_ref().map(|probe| probe.window_visible),
                frontmost_observed: native_probe.as_ref().and_then(|probe| probe.frontmost),
                key_window: native_probe.as_ref().and_then(|probe| probe.key_window),
                main_window: native_probe.as_ref().and_then(|probe| probe.main_window),
                frontmost_application_pid: native_probe
                    .as_ref()
                    .and_then(|probe| probe.frontmost_application_pid),
                frontmost_application_matches_target: native_probe
                    .as_ref()
                    .and_then(|probe| probe.frontmost_matches_target),
                exact_pid_match: native_probe.as_ref().is_some_and(|probe| {
                    probe.accessibility_probe_ok && probe.process_pid == Some(pid)
                }),
                cg_owner_pid_match: native_probe.as_ref().map(|probe| probe.cg_owner_pid_match),
            },
            cg_windows,
            non_target_window_count,
            native_probe
                .as_ref()
                .is_some_and(|probe| probe.accessibility_probe_ok),
            native_probe
                .as_ref()
                .and_then(|probe| probe.accessibility_error.clone()),
            native_error,
        );
        emit_native_window_probe(action, &state);
        state
    }

    #[cfg(not(target_os = "macos"))]
    {
        // The native capture publisher is macOS-only. Keep unit tests and
        // other desktop builds deterministic while retaining the macOS
        // fail-closed probe above.
        let state = NativeWindowState::from_facts(
            std::process::id(),
            NativeProbeObservation {
                process_pid: Some(std::process::id()),
                process_visible: Some(true),
                window_visible: Some(true),
                frontmost_observed: Some(true),
                key_window: Some(true),
                main_window: Some(true),
                frontmost_application_pid: None,
                frontmost_application_matches_target: None,
                exact_pid_match: true,
                cg_owner_pid_match: Some(true),
            },
            Vec::new(),
            None,
            true,
            None,
            None,
        );
        emit_native_window_probe(action, &state);
        state
    }
}

#[cfg(target_os = "macos")]
#[derive(Debug, Clone, serde::Deserialize)]
struct MacosCgWindowProbe {
    target_pid: u32,
    accessibility_probe_ok: bool,
    accessibility_error: Option<String>,
    process_pid: Option<u32>,
    process_visible: Option<bool>,
    frontmost: Option<bool>,
    key_window: Option<bool>,
    main_window: Option<bool>,
    frontmost_application_pid: Option<i64>,
    frontmost_matches_target: Option<bool>,
    cg_owner_pid_match: bool,
    window_visible: bool,
    non_target_window_count: usize,
    windows: Vec<CgWindowRecord>,
}

#[cfg(target_os = "macos")]
const NATIVE_PROBE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);

#[cfg(target_os = "macos")]
const NATIVE_PROBE_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(10);

#[cfg(target_os = "macos")]
fn terminate_native_probe_process_group(pid: u32) {
    // The probe helper is placed in its own process group before exec. Killing
    // the group is necessary because a hung helper can leave a grandchild
    // holding stdout/stderr open after the direct child is gone.
    let process_group = -(pid as libc::pid_t);
    // SAFETY: `process_group` names only the process group created for this
    // helper, never the egui process group.
    unsafe {
        libc::kill(process_group, libc::SIGKILL);
    }
}

#[cfg(target_os = "macos")]
fn run_native_probe_helper_with_timeout(
    helper: &Path,
    pid: u32,
    timeout: std::time::Duration,
) -> Result<std::process::Output, String> {
    run_native_probe_helper_with_timeout_using(
        helper,
        pid,
        timeout,
        terminate_native_probe_process_group,
    )
}

#[cfg(target_os = "macos")]
fn run_native_probe_helper_with_timeout_using(
    helper: &Path,
    pid: u32,
    timeout: std::time::Duration,
    terminate_process_group: impl Fn(u32),
) -> Result<std::process::Output, String> {
    use std::io::Read;
    use std::os::unix::process::CommandExt;
    use std::process::{Command, Stdio};
    use std::thread;
    use std::time::Instant;

    let mut command = Command::new(helper);
    command
        .arg(pid.to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // SAFETY: the pre-exec closure performs only the async-signal-safe
    // process-group setup needed to make timeout cleanup cover descendants.
    unsafe {
        command.pre_exec(|| {
            if libc::setpgid(0, 0) == -1 {
                Err(std::io::Error::last_os_error())
            } else {
                Ok(())
            }
        });
    }
    let mut child = command
        .spawn()
        .map_err(|error| format!("could not spawn helper: {error}"))?;
    let child_pid = child.id();
    let mut stdout = child
        .stdout
        .take()
        .expect("piped native probe stdout is available after spawn");
    let mut stderr = child
        .stderr
        .take()
        .expect("piped native probe stderr is available after spawn");
    let stdout_reader = thread::spawn(move || {
        let mut bytes = Vec::new();
        stdout
            .read_to_end(&mut bytes)
            .map(|_| bytes)
            .map_err(|error| error.to_string())
    });
    let stderr_reader = thread::spawn(move || {
        let mut bytes = Vec::new();
        stderr
            .read_to_end(&mut bytes)
            .map(|_| bytes)
            .map_err(|error| error.to_string())
    });

    let deadline = Instant::now() + timeout;
    let mut timed_out = false;
    let mut wait_error = None;
    let mut status = None;
    loop {
        match child.try_wait() {
            Ok(Some(exit_status)) => {
                status = Some(exit_status);
                break;
            }
            Ok(None) if Instant::now() >= deadline => {
                timed_out = true;
                break;
            }
            Ok(None) => thread::sleep(NATIVE_PROBE_POLL_INTERVAL),
            Err(error) => {
                wait_error = Some(error.to_string());
                break;
            }
        }
    }

    if timed_out || wait_error.is_some() {
        // `try_wait` returning `Some` already reaped the child. Only the
        // timeout/error paths may still have a live or unreaped child, so
        // never signal a successfully exited PID or its possibly reused
        // process group.
        terminate_process_group(child_pid);
    }
    let reap_error = if status.is_none() {
        match child.wait() {
            Ok(reaped_status) => {
                status = Some(reaped_status);
                None
            }
            Err(error) => Some(error.to_string()),
        }
    } else {
        // `try_wait` reaps the child when it returns `Some`.
        None
    };
    let stdout = stdout_reader
        .join()
        .map_err(|_| "native probe stdout reader panicked".to_string())?
        .map_err(|error| format!("could not read helper stdout: {error}"))?;
    let stderr = stderr_reader
        .join()
        .map_err(|_| "native probe stderr reader panicked".to_string())?
        .map_err(|error| format!("could not read helper stderr: {error}"))?;

    if let Some(error) = wait_error {
        return Err(format!(
            "could not wait for helper; its process group was terminated: {error}"
        ));
    }
    if timed_out {
        return Err(format!(
            "helper timed out after {}ms; its process group was terminated",
            timeout.as_millis()
        ));
    }
    let status = status.ok_or_else(|| {
        format!(
            "helper exited without a status{}",
            reap_error
                .map(|error| format!(": {error}"))
                .unwrap_or_default()
        )
    })?;
    Ok(std::process::Output {
        status,
        stdout,
        stderr,
    })
}

#[cfg(target_os = "macos")]
fn macos_cg_window_probe(pid: u32) -> Result<MacosCgWindowProbe, String> {
    let helper = std::env::var_os("CORRAL_UI_WINDOW_PROBE_HELPER")
        .ok_or_else(|| "CORRAL_UI_WINDOW_PROBE_HELPER is not configured".to_string())?;
    let output =
        run_native_probe_helper_with_timeout(Path::new(&helper), pid, NATIVE_PROBE_TIMEOUT)
            .map_err(|error| format!("could not run CoreGraphics probe helper: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "CoreGraphics probe helper status={} stdout={:?} stderr={:?}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    let probe: MacosCgWindowProbe = serde_json::from_slice(&output.stdout)
        .map_err(|error| format!("invalid CoreGraphics probe JSON: {error}"))?;
    if probe.target_pid != pid {
        return Err(format!(
            "CoreGraphics probe target PID mismatch requested={} reported={}",
            pid, probe.target_pid
        ));
    }
    Ok(probe)
}

const SCREENSHOT_WAKE_COMMAND_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);

#[cfg(unix)]
fn terminate_wake_command_process_group(pid: u32) {
    let process_group = -(pid as libc::pid_t);
    // SAFETY: the wake command's pre-exec hook puts only that command in a
    // process group named by its own PID. Killing the group cannot target the
    // egui process group or an unrelated caller process.
    unsafe {
        libc::kill(process_group, libc::SIGKILL);
    }
}

fn cleanup_wake_command_process_group(
    child: &mut std::process::Child,
    child_pid: u32,
    child_reaped: bool,
) {
    // A shell can report success after starting `command &`. The direct
    // child is then already reaped, but its descendants remain in this exact
    // process group. Always terminate that group on every terminal outcome;
    // on timeout/wait-error paths also reap the direct child below.
    #[cfg(unix)]
    terminate_wake_command_process_group(child_pid);
    if !child_reaped {
        let _ = child.kill();
        let _ = child.wait();
    }
}

fn invoke_exact_window_wake(command: &str, path: &std::path::Path) -> bool {
    let pid = std::process::id().to_string();
    tracing::info!(
        pid = %pid,
        command = %command,
        path = %path.display(),
        "requesting exact-owned native window wake"
    );
    #[cfg(unix)]
    use std::os::unix::process::CommandExt;
    use std::process::{Command, Stdio};
    use std::thread;

    let mut wake_command = Command::new("bash");
    wake_command
        .args(["-c", command])
        .env("CORRAL_UI_SCREENSHOT_PID", &pid)
        .env("CORRAL_UI_SCREENSHOT_PATH", path)
        .stdin(Stdio::null())
        // A caller-owned wake helper has no trusted output channel. Discard
        // it so a noisy or descendant-holding command cannot block the
        // bounded scheduler on a pipe.
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(unix)]
    {
        // SAFETY: this pre-exec hook performs only async-signal-safe
        // process-group setup. The group gives timeout cleanup an exact
        // ownership boundary for the shell and any helper it launches.
        unsafe {
            wake_command.pre_exec(|| {
                if libc::setpgid(0, 0) == -1 {
                    Err(std::io::Error::last_os_error())
                } else {
                    Ok(())
                }
            });
        }
    }

    let mut child = match wake_command.spawn() {
        Ok(child) => child,
        Err(error) => {
            tracing::warn!(
                pid = %pid,
                command = %command,
                error = %error,
                "could not run exact-owned native window wake"
            );
            return false;
        }
    };
    let child_pid = child.id();
    let deadline = std::time::Instant::now() + SCREENSHOT_WAKE_COMMAND_TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                cleanup_wake_command_process_group(&mut child, child_pid, true);
                if status.success() {
                    tracing::info!(
                        pid = %pid,
                        command = %command,
                        "exact-owned native window wake completed"
                    );
                    return true;
                }
                tracing::warn!(
                    pid = %pid,
                    command = %command,
                    status = %status,
                    "exact-owned native window wake failed"
                );
                return false;
            }
            Ok(None) if std::time::Instant::now() >= deadline => {
                cleanup_wake_command_process_group(&mut child, child_pid, false);
                tracing::warn!(
                    pid = %pid,
                    command = %command,
                    timeout_ms = SCREENSHOT_WAKE_COMMAND_TIMEOUT.as_millis(),
                    "exact-owned native window wake timed out"
                );
                return false;
            }
            Ok(None) => thread::sleep(SCREENSHOT_WAKE_POLL_INTERVAL),
            Err(error) => {
                cleanup_wake_command_process_group(&mut child, child_pid, false);
                tracing::warn!(
                    pid = %pid,
                    command = %command,
                    error = %error,
                    "could not wait for exact-owned native window wake"
                );
                return false;
            }
        }
    }
}

fn tab_from_env() -> Tab {
    match std::env::var("CORRAL_UI_SCREENSHOT_TAB")
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "settings" => Tab::Settings,
        _ => Tab::Board,
    }
}

/// Runtime-loaded + persisted app config (host URL, registration record).
/// #354 L3: connection-only — the board/view toggles were removed with
/// their surfaces; older config keys are ignored on load.
#[derive(Debug, Clone, PartialEq)]
struct PersistedConfig {
    host_url: String,
    registration: Option<RegistrationRecord>,
    auto_reconnect: bool,
}

impl Default for PersistedConfig {
    fn default() -> Self {
        Self {
            host_url: protocol::DEFAULT_HOST_URL.to_string(),
            registration: None,
            auto_reconnect: true,
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
                auto_reconnect: c.auto_reconnect.unwrap_or(true),
            })
            .unwrap_or_default()
    }

    fn persist(&self, path: &PathBuf) {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::create_dir_all(path.parent().expect("config has parent"));
        let wire = crate::state::PersistedConfig {
            host_url: Some(self.host_url.clone()),
            registration: self.registration.clone(),
            auto_reconnect: Some(self.auto_reconnect),
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
    /// #249: key-vs-registration mismatch state (None = consistent).
    identity_recovery: IdentityRecovery,
    /// #310 r3: identity epoch for drive results. Bumped on every
    /// successful (re)registration; a drive initiated under an older epoch
    /// must not set or clear current recovery state when it lands late.
    identity_generation: u64,

    // Config + settings.
    config: PersistedConfig,
    config_path: PathBuf,
    settings: crate::ui::register::SettingsState,

    // Channels.
    tx_apply: UnboundedSender<ApplyMsg>,
    rx_apply: UnboundedReceiver<ApplyMsg>,
    rx_drive: UnboundedReceiver<DriveMsg>,
    tx_drive: UnboundedSender<DriveMsg>,
    stop_read: Option<tokio::sync::watch::Sender<bool>>,
    /// Generation of the currently spawned SSE/read loop.
    read_loop_generation: u64,
    /// #314: when the visible-agent Recent-output refresh last dispatched.
    /// `None` until the first paced refresh fires (the initial hydration is
    /// `hydrate_recent_output`'s job, not this pacing gate's).
    recent_output_last_refresh: Option<std::time::Instant>,

    // Tabs.
    tab: Tab,

    /// Evidence capture (env-gated): when `CORRAL_UI_SCREENSHOT` is set,
    /// request a viewport screenshot after a delay and write the PNG there
    /// before exiting. Never active by default.
    screenshot_path: Option<PathBuf>,
    screenshot_state: ScreenshotCaptureState,
    screenshot_settle: std::time::Duration,
    /// Optional native-evidence target. When set alongside the screenshot
    /// path, the app selects this live daemon agent so the operator can use
    /// the shipped Cards controls before capturing. Never active normally.
    screenshot_agent_id: Option<String>,
    screenshot_agent_selected: bool,
    screenshot_wake_stop: Option<Arc<AtomicBool>>,
    screenshot_wake_active: Option<Arc<AtomicBool>>,
    screenshot_wake_command: Option<String>,
    screenshot_last_wake: Option<std::time::Instant>,
    /// Diagnostic-only native window sampling. This is independent of the
    /// screenshot path and never enables ViewportCommand::Screenshot.
    window_diagnostic: bool,
    /// R3 #316: offline demo-evidence mode (env CORRAL_UI_DEMO_SEED).
    demo_seeded: bool,
    window_diagnostic_last_sample: Option<std::time::Instant>,
    evidence_visibility_requested: bool,
    native_probe_tx: Sender<(String, NativeWindowState)>,
    native_probe_rx: Receiver<(String, NativeWindowState)>,
    native_probe_in_flight: bool,
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
        let (native_probe_tx, native_probe_rx) = std::sync::mpsc::channel();
        let client_for_fp = client.clone();

        let screenshot_path = std::env::var("CORRAL_UI_SCREENSHOT")
            .ok()
            .map(PathBuf::from);
        let screenshot_agent_id = std::env::var("CORRAL_UI_SCREENSHOT_AGENT")
            .ok()
            .filter(|_| screenshot_path.is_some());
        let screenshot_wake_command = std::env::var("CORRAL_UI_SCREENSHOT_WAKE_COMMAND")
            .ok()
            .filter(|command| !command.trim().is_empty())
            .filter(|_| screenshot_path.is_some());
        let screenshot_wake_active = (screenshot_path.is_some()
            && screenshot_wake_command.is_some())
        .then(|| Arc::new(AtomicBool::new(false)));
        let window_diagnostic = std::env::var_os("CORRAL_UI_WINDOW_DIAGNOSTIC").is_some();
        // R3 #316: offline compiled-evidence seam. Seeds the bundled
        // synthetic fixture into the Fleet and renders the real board with
        // the normal capture pipeline — never connects to a daemon, and
        // never active without CORRAL_UI_SCREENSHOT also being set.
        let demo_seeded =
            std::env::var_os("CORRAL_UI_DEMO_SEED").is_some() && screenshot_path.is_some();
        let screenshot_settle = std::env::var("CORRAL_UI_SCREENSHOT_DELAY_MS")
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
            .map(std::time::Duration::from_millis)
            .unwrap_or_else(|| std::time::Duration::from_secs(6));
        let screenshot_state = ScreenshotCaptureState::initial(
            screenshot_path.is_some(),
            screenshot_agent_id.is_some(),
            std::time::Instant::now(),
            screenshot_settle,
        );

        let mut app = CorralApp {
            rt,
            client,
            fleet: Fleet::default(),
            conn: ConnState::Connecting,
            conn_detail: None,
            toasts: VecDeque::new(),
            device_key: None,
            device_key_store_warning: false,
            ledger: if demo_seeded {
                GrantLedger {
                    base: vec!["read_tail".into(), "read_diff".into()],
                    denied: Vec::new(),
                }
            } else {
                GrantLedger::default()
            },
            registration: if demo_seeded {
                Some(crate::state::RegistrationRecord {
                    host_fingerprint: "demo-seed".into(),
                    key_id: "demo-seed".into(),
                    grants: Vec::new(),
                    denied: Vec::new(),
                })
            } else {
                config.registration.clone()
            },
            host_fingerprint: None,
            identity_recovery: IdentityRecovery::None,
            identity_generation: 0,
            config: config.clone(),
            config_path,
            settings: crate::ui::register::SettingsState {
                host_url: host_url.clone(),
                auto_reconnect: config.auto_reconnect,
                ..Default::default()
            },
            tx_apply: tx_apply.clone(),
            rx_apply,
            rx_drive,
            tx_drive,
            stop_read: None,
            read_loop_generation: 0,
            // #314: the paced visible-agent refresh has not fired yet.
            recent_output_last_refresh: None,

            tab: tab_from_env(),
            screenshot_path,
            screenshot_state,
            screenshot_settle,
            screenshot_agent_id,
            screenshot_agent_selected: false,
            screenshot_wake_stop: None,
            screenshot_wake_active: screenshot_wake_active.clone(),
            screenshot_wake_command,
            screenshot_last_wake: None,
            window_diagnostic,
            demo_seeded,
            window_diagnostic_last_sample: None,
            evidence_visibility_requested: false,
            native_probe_tx,
            native_probe_rx,
            native_probe_in_flight: false,
        };

        // An un-targeted screenshot has no selection transition to arm the
        // exact-PID activation schedule. Targeted captures arm it only after
        // the requested live agent has been selected below.
        if app.screenshot_agent_id.is_none()
            && let Some(active) = &app.screenshot_wake_active
        {
            active.store(true, Ordering::Release);
        }

        // A native window can be quiet while an external wake command only
        // changes focus. Keep the env-gated evidence loop repainting until the
        // screenshot event arrives. For a screenshot capture with a caller
        // wake command, the same thread also performs repeated exact-PID
        // activation until dispatch or a bounded lifetime expires; normal app
        // instances never create this helper thread.
        if app.screenshot_path.is_some() || app.window_diagnostic {
            let stop = Arc::new(AtomicBool::new(false));
            let thread_stop = Arc::clone(&stop);
            let repaint_ctx = cc.egui_ctx.clone();
            let wake_active = app.screenshot_wake_active.clone();
            let wake_command = app.screenshot_wake_command.clone();
            let wake_path = app.screenshot_path.clone();
            let spawn_result = std::thread::Builder::new()
                .name("corral-screenshot-waker".into())
                .spawn(move || {
                    let mut wake_schedule = NativeWindowWakeSchedule::default();
                    while !thread_stop.load(std::sync::atomic::Ordering::Relaxed) {
                        // Request a repaint on every loop before any
                        // activation command. Either signal can be the event
                        // that gets a quiet eframe viewport back into `ui()`.
                        repaint_ctx.request_repaint();
                        let now = std::time::Instant::now();
                        if wake_active
                            .as_ref()
                            .is_some_and(|active| active.load(std::sync::atomic::Ordering::Acquire))
                        {
                            wake_schedule.activate(now);
                        } else {
                            wake_schedule.deactivate();
                        }
                        if let (Some(command), Some(path)) =
                            (wake_command.as_deref(), wake_path.as_deref())
                            && wake_schedule.due(now)
                        {
                            let _ = invoke_exact_window_wake(command, path);
                            wake_schedule.record_wake(std::time::Instant::now());
                        }
                        std::thread::sleep(wake_schedule.sleep_for(std::time::Instant::now()));
                    }
                });
            match spawn_result {
                Ok(_) => app.screenshot_wake_stop = Some(stop),
                Err(error) => {
                    app.screenshot_wake_active = None;
                    tracing::warn!(%error, "could not start screenshot wake helper");
                }
            }
        }

        if demo_seeded {
            // Offline demo evidence: the synthetic fixture is the ONLY data
            // source. No daemon connection, no fingerprint fetch, no read
            // loop is started for this process.
            let data = crate::demo::load();
            app.fleet.apply_snapshot(&data.snapshot);
            let blocks = crate::demo::recent_tail_blocks();
            let lines = crate::demo::recent_tail();
            let rev = data.snapshot.rev;
            for agent in data.snapshot.agents.values() {
                if agent
                    .capabilities
                    .iter()
                    .any(|capability| capability == "read_tail")
                {
                    app.fleet.remember_tail_full(
                        &agent.agent_id,
                        lines.clone(),
                        blocks.clone(),
                        Some(rev),
                    );
                }
            }
            if let Some(first) = data
                .snapshot
                .agents
                .values()
                .find(|agent| agent.capabilities.iter().any(|c| c == "read_tail"))
            {
                app.fleet.select_agent(&first.agent_id);
            }
            app.conn = ConnState::Connected;
            app.device_key = None;
            tracing::info!("demo seed applied; offline compiled-evidence mode");
            return app;
        }

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

    /// eframe 0.36.1 creates the root NSWindow hidden and normally reveals it
    /// after the first painted frame. On macOS, an early occlusion event can
    /// make eframe skip `App::ui` before that happens, leaving `App::logic` as
    /// the only recovery path. Keep this command strictly env-gated to native
    /// evidence/diagnostic runs; `Visible(true)` also makes the window key.
    fn request_evidence_window_visibility(&mut self, ctx: &egui::Context) {
        if (self.screenshot_path.is_some() || self.window_diagnostic)
            && !self.evidence_visibility_requested
        {
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
            self.evidence_visibility_requested = true;
        }
    }

    fn update_logic(&mut self, ctx: &egui::Context) {
        self.request_evidence_window_visibility(ctx);
    }

    /// Accessibility queries cannot synchronously inspect a process whose UI
    /// thread is waiting for that query. Run the exact-PID native helper off
    /// the egui thread and keep dispatch fail-closed until its fresh result
    /// returns.
    fn request_native_probe(
        &mut self,
        ctx: &egui::Context,
        action: &'static str,
    ) -> Option<NativeWindowState> {
        if let Ok((completed_action, state)) = self.native_probe_rx.try_recv() {
            self.native_probe_in_flight = false;
            if completed_action == action {
                ctx.request_repaint();
                return Some(state);
            }
        }

        if !self.native_probe_in_flight {
            let tx = self.native_probe_tx.clone();
            let action = action.to_string();
            match std::thread::Builder::new()
                .name("corral-native-window-probe".into())
                .spawn(move || {
                    let state = native_window_state(&action);
                    let _ = tx.send((action, state));
                }) {
                Ok(_) => self.native_probe_in_flight = true,
                Err(error) => tracing::warn!(%error, "could not start native window probe"),
            }
        }
        ctx.request_repaint_after(std::time::Duration::from_millis(100));
        None
    }

    fn spawn_read_loop(&mut self, host_url: String) {
        let generation = self.invalidate_read_model();
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
            generation,
            tx.clone(),
            stop_rx.clone(),
            self.settings.auto_reconnect,
        );
    }

    /// Start a new read-loop generation. Events from a superseded loop are
    /// dropped by the generation gate in `on_apply`.
    fn invalidate_read_model(&mut self) -> u64 {
        self.read_loop_generation = self
            .read_loop_generation
            .checked_add(1)
            .expect("read loop generation exhausted");
        self.read_loop_generation
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
        // #310: startup never mutates identity and never auto-re-registers.
        // A local key-ID mismatch gets a passive notice pointing at
        // Settings; the recovery block there renders only after an actual
        // server-side bad_signature rejection.
        self.passive_identity_mismatch_notice();
    }

    // -------------------------------------------------------------------
    // #249/#310 identity recovery: detect a device key that no longer
    // matches the registered key_id (rebuild/reinstall wiped or replaced
    // the key material while config.json kept the old record). Recovery is
    // USER-INITIATED only: the Settings recovery block renders after an
    // actual server-side bad_signature rejection, and its Restore /
    // Re-register actions drive `try_start_recovery` / `register`. There is
    // no unsigned fallback and no grant path beyond the existing
    // registration-token / admin-token mechanisms.
    // -------------------------------------------------------------------

    /// Does the current key material match the registered key_id?
    fn identity_mismatch(&self) -> bool {
        let Some(reg) = &self.registration else {
            return false;
        };
        let Some(key) = &self.device_key else {
            return false;
        };
        let current = crate::keys::device_key_id(&key.signing.verifying_key().to_bytes());
        current != reg.key_id
    }

    /// #310: passive, non-mutating notice for a local key-ID mismatch at
    /// startup. Sets a Settings-visible notice and changes nothing else —
    /// no IdentityRecovery state, no token read, no re-registration.
    fn passive_identity_mismatch_notice(&mut self) {
        if !self.identity_mismatch() {
            return;
        }
        self.settings.notice = Some((
            Level::Warn,
            "Device identity changed: the board's key no longer matches the registered \
             device. Open Settings to restore or re-register — no automatic \
             re-registration is performed."
                .to_string(),
        ));
    }

    /// Re-register the CURRENT key material (never rotates: the fresh key
    /// IS the identity the reinstall left behind) with the host's routing
    /// token. #354 L3: the daemon is read-only and grant administration is
    /// gone, so "Restore saved identity" is exactly this re-register — the
    /// recorded grants (if any) are re-learned from the register response.
    /// Returns true when the recovery was started.
    fn try_start_recovery(&mut self) -> bool {
        if self.identity_recovery != IdentityRecovery::Mismatch {
            return false;
        }
        self.identity_recovery = IdentityRecovery::InFlight;
        // Registration token: the pasted one wins (remote host), else the
        // localhost auto-register file (#249 auto-recovery path).
        let token = {
            let entered = self.settings.token_input.trim().to_string();
            if !entered.is_empty() {
                entered
            } else {
                match crate::keys::read_daemon_registration_token() {
                    Ok(token) => token,
                    Err(error) => {
                        self.identity_recovery = IdentityRecovery::Mismatch;
                        self.settings.notice = Some((
                            Level::Warn,
                            format!(
                                "device identity changed (#249) — re-register needs the \
                                 registration token: {error}. Paste it in Settings, then \
                                 use Restore saved identity."
                            ),
                        ));
                        return false;
                    }
                }
            }
        };
        self.register(token, false);
        true
    }

    /// A failed recovery leg leaves the user-initiated recovery retryable.
    fn mark_recovery_failed(&mut self) {
        if self.identity_recovery == IdentityRecovery::InFlight {
            self.identity_recovery = IdentityRecovery::Mismatch;
        }
    }

    /// #310 r3: drop the persisted recovery-guidance notice and its
    /// `settings.notice` twin, but ONLY when the current notice is still
    /// exactly that guidance — unrelated notices are never deleted.
    fn clear_recovery_notice(&mut self) {
        if let Some(previous) = self.settings.recovery_notice.take()
            && let Some((_, text)) = &self.settings.notice
            && text == &previous
        {
            self.settings.notice = None;
        }
    }

    fn on_apply(&mut self, msg: ApplyMsg) {
        match msg {
            ApplyMsg::Fingerprint(fp) => self.apply_fingerprint(fp),
            ApplyMsg::RegisterResult(result) => self.handle_register_result(result),
            ApplyMsg::Sse {
                loop_generation,
                event,
            } => {
                if loop_generation != self.read_loop_generation {
                    tracing::debug!(
                        loop_generation,
                        current = self.read_loop_generation,
                        "ignored SSE event from an obsolete read loop"
                    );
                    return;
                }
                match event {
                    protocol::SseEvent::Snapshot(snap) => {
                        self.fleet.apply_snapshot(&snap);
                    }
                    protocol::SseEvent::Delta(delta) => {
                        self.fleet.apply_delta(&delta);
                    }
                    protocol::SseEvent::Unknown { event, .. } => {
                        tracing::debug!(event, "ignored SSE event");
                    }
                }
            }
            ApplyMsg::Conn {
                loop_generation,
                event,
            } => {
                if loop_generation != self.read_loop_generation {
                    tracing::debug!(
                        loop_generation,
                        current = self.read_loop_generation,
                        "ignored connection event from an obsolete read loop"
                    );
                    return;
                }
                match event {
                    protocol::Live::Connected => {
                        self.conn = ConnState::Connected;
                        self.conn_detail = None;
                    }
                    protocol::Live::Disconnected => {
                        self.invalidate_read_model();
                        self.conn = ConnState::Connecting;
                        self.conn_detail = Some("disconnected — reconnecting".to_string());
                    }
                    protocol::Live::Reconnecting { backoff_ms, rev } => {
                        self.conn = ConnState::Reconnecting { backoff_ms };
                        self.conn_detail = Some(format!(
                            "resuming from rev {} (Last-Event-ID)",
                            rev.map(|r| r.to_string())
                                .unwrap_or_else(|| "none".to_string())
                        ));
                    }
                }
            }
            ApplyMsg::ConnError {
                loop_generation,
                error,
            } => {
                if loop_generation != self.read_loop_generation {
                    tracing::debug!(
                        loop_generation,
                        current = self.read_loop_generation,
                        "ignored connection error from an obsolete read loop"
                    );
                    return;
                }
                self.invalidate_read_model();
                self.conn = ConnState::Down;
                self.conn_detail = Some(error);
            }
        }
    }

    /// Hydrate the resolved visible agent's recents v1 tail once the live
    /// snapshot and the persisted device grant are both ready. This method
    /// consumes the visible selection, never selects a fallback and never
    /// writes `selected_agent`. The bounded read_tail result (200-line live
    /// tail: lines + canonical blocks) is the only output source.
    fn hydrate_recent_output(&mut self, resolved_selection: Option<&str>) {
        let Some(agent_id) = hydration_target(&self.fleet, resolved_selection) else {
            return;
        };
        if !self.ledger.allowed("read_tail")
            || self.registration.is_none()
            || self.device_key.is_none()
            || !self.fleet.needs_recent_output(&agent_id)
        {
            return;
        }

        let intent = self
            .fleet
            .tail_source_revs
            .get(&agent_id)
            .copied()
            .map(|source_rev| DriveIntent::read_tail_since(&agent_id, source_rev, self.fleet.rev))
            .unwrap_or_else(|| DriveIntent::read_tail(&agent_id, self.fleet.rev));
        self.dispatch_drive_intents(vec![intent]);
    }

    /// #314: pace the visible agent's Recent-output refresh through the
    /// existing frame cadence. A cached tail is refreshed with a
    /// revision-aware request carrying that cache's exact `source_rev`
    /// (single-flight: while a read_tail for the agent is in flight, no
    /// duplicate is sent; cooldown: the earliest a following refresh can
    /// fire is [`RECENT_OUTPUT_REFRESH_COOLDOWN`] after the last one).
    /// Hidden agents are never eligible — the caller passes only the
    /// resolved visible selection. Recents v1 always re-requests the
    /// daemon-capped 200-line live tail.
    fn refresh_recent_output(&mut self, resolved_selection: Option<&str>) {
        let Some(agent_id) = hydration_target(&self.fleet, resolved_selection) else {
            return;
        };
        if !self.ledger.allowed("read_tail")
            || self.registration.is_none()
            || self.device_key.is_none()
        {
            return;
        }
        // Single-flight + revision bookkeeping: a cached tail with no
        // read_tail drive in flight is the only refreshable shape.
        let Some(source_rev) = self.fleet.recent_output_refresh_candidate(&agent_id) else {
            return;
        };
        // Cooldown pacing: at most one automatic refresh per window. (The
        // initial hydration is `hydrate_recent_output`'s job and never
        // touches this gate.)
        let now = std::time::Instant::now();
        if self
            .recent_output_last_refresh
            .is_some_and(|last| now.duration_since(last) < RECENT_OUTPUT_REFRESH_COOLDOWN)
        {
            return;
        }
        self.recent_output_last_refresh = Some(now);
        // Recents v1 refreshes always re-request the daemon-capped 200-line
        // live tail, carrying the cached source revision.
        let intent = DriveIntent::read_tail_since(&agent_id, source_rev, self.fleet.rev);
        self.dispatch_drive_intents(vec![intent]);
    }

    fn on_drive(&mut self, msg: DriveMsg) {
        let capability = msg.capability.clone();
        // #310 r3: recovery-affecting drive results are scoped to the
        // identity generation that dispatched them. A result from a prior
        // generation (e.g. an in-flight drive that predates a rotation)
        // must never set or clear the CURRENT recovery latch/notice.
        let current_generation = msg.identity_generation == self.identity_generation;
        let state = match &msg.outcome {
            DriveOutcome::Ok { rev, .. } => crate::state::DriveState::Ok {
                rev: *rev,
                capability: capability.clone(),
            },
            DriveOutcome::Refused(failure) => crate::state::DriveState::Failed {
                failure: failure.clone(),
                capability: capability.clone(),
            },
        };
        self.fleet.remember_drive(&msg.agent_id, state);
        match &msg.outcome {
            DriveOutcome::Ok { rev, .. } => {
                self.ledger.note_success(&capability);
                self.persist_ledger();
                if current_generation {
                    // #310: a current-generation successful drive proves
                    // the current key is accepted — clear the bad-signature
                    // latch AND its persisted recovery guidance (leaving
                    // unrelated notices untouched).
                    self.settings.bad_signature = false;
                    self.clear_recovery_notice();
                }
                // read_tail is the only dispatched capability after the cut.
                if capability == "read_tail" {
                    self.remember_tail_result(&msg);
                }
                self.toast(Level::Info, format!("{capability} → ok (rev {rev})"));
            }
            DriveOutcome::Refused(failure) => {
                self.ledger.note_refusal(failure);
                self.persist_ledger();
                if current_generation {
                    // #249/#310: a bad_signature refusal is the live signal
                    // that the daemon rejected the CURRENT key — record it so
                    // the Settings recovery block renders. Recovery is
                    // user-initiated there (Restore / Re-register); no
                    // automatic re-registration. Stale-generation refusals
                    // never touch current recovery state.
                    if matches!(failure, crate::drive::DriveFailure::BadSignature(_)) {
                        self.settings.bad_signature = true;
                    }
                    if failure.suggests_re_registration() {
                        let text = format!(
                            "{failure} — open Settings to restore or re-register this device."
                        );
                        self.settings.recovery_notice = Some(text.clone());
                        self.settings.notice = Some((Level::Warn, text));
                    }
                }
                if matches!(failure, crate::drive::DriveFailure::StaleAgent(_)) {
                    // A stale tap is a read-model event, not a generic drive
                    // failure: remove the row before the next frame renders,
                    // then refresh once for the current identity.
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
                // #310 r4: re-registration guidance is written ONLY by the
                // generation-gated writer above — an unguarded duplicate
                // would let stale-generation refusals plant permanent
                // Restore/Re-register guidance on the healthy current key.
            }
        }
    }

    /// One-shot read-model refresh after a typed stale-target refusal. The
    /// live SSE loop remains authoritative; this closes the tap-to-refresh
    /// gap without creating a daemon poll loop.
    fn refresh_snapshot(&self) {
        let client = self.client.clone();
        let base_url = self.config.host_url.clone();
        let loop_generation = self.read_loop_generation;
        let tx = self.tx_apply.clone();
        self.rt.spawn(async move {
            if let Ok(snapshot) = protocol::fetch_snapshot(&client, &base_url).await {
                let _ = tx.send(ApplyMsg::Sse {
                    loop_generation,
                    event: protocol::SseEvent::Snapshot(snapshot),
                });
            }
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
        Self::apply_read_tail_result(&mut self.fleet, msg);
    }

    /// Apply the response half of the app's `read_tail` control path. Keeping
    /// this as a small app-layer operation makes the intent -> DriveMsg -> UI
    /// cache contract testable without starting an eframe window.
    fn apply_read_tail_result(fleet: &mut Fleet, msg: &DriveMsg) {
        let DriveOutcome::Ok { result, .. } = &msg.outcome else {
            return;
        };
        let Some(result) = result else {
            return;
        };
        let lines = crate::drive::parse_tail_lines(result);
        // #315: the canonical semantic blocks ride additively; when present
        // the Recent output view renders THEM (never re-classified lines).
        let blocks = crate::drive::parse_tail_blocks(result);
        tracing::info!(
            agent_id = %msg.agent_id,
            lines = lines.len(),
            blocks = blocks.len(),
            "read_tail result applied to screenshot/detail cache"
        );
        let source_rev = crate::drive::parse_tail_source_rev(result).or(match msg.outcome {
            DriveOutcome::Ok { rev, .. } => Some(rev),
            DriveOutcome::Refused(_) => None,
        });
        fleet.remember_tail_full(&msg.agent_id, lines, blocks, source_rev);
    }

    /// Native screenshot evidence helper. It is deliberately opt-in and
    /// targets an id observed in the live `/snapshot`; normal board hydration
    /// still owns the capability- and grant-gated content fetch.
    fn prepare_screenshot_evidence(&mut self) -> bool {
        let Some(agent_id) = self.screenshot_agent_id.clone() else {
            return false;
        };
        let Some(agent) = self.fleet.agents.get(&agent_id) else {
            return false;
        };
        if self.screenshot_agent_selected {
            return false;
        }
        let read_tail_advertised = agent.capabilities.iter().any(|cap| cap == "read_tail");
        self.fleet.select_agent(&agent_id);
        self.screenshot_agent_selected = true;
        let (state, target_ready) = self
            .screenshot_state
            .target_ready_after(std::time::Instant::now(), self.screenshot_settle);
        self.screenshot_state = state;
        tracing::info!(
            agent_id = %agent_id,
            read_tail_advertised,
            read_tail_granted = self.ledger.allowed("read_tail"),
            "native screenshot evidence selected live agent; board hydration remains grant-gated"
        );
        if let Some(active) = &self.screenshot_wake_active {
            active.store(true, Ordering::Release);
        }
        target_ready
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
        if rotate && let Err(error) = crate::keys::prepare_key_rotation(&fp) {
            self.settings.notice = Some((
                Level::Error,
                format!("cannot safely rotate device identity: {error}"),
            ));
            return;
        }
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
                        match protocol::register_device(
                            &client,
                            &host_url,
                            &token,
                            &pubkey_b64,
                            crate::keys::local_device_name().as_deref(),
                        )
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
                protocol::register_device(
                    &client,
                    &host_url,
                    &token,
                    &pubkey_b64,
                    crate::keys::local_device_name().as_deref(),
                )
                .await
            };
            let _ = tx.send(ApplyMsg::RegisterResult(registration));
        });
    }

    fn handle_register_result(&mut self, result: Result<(String, Vec<String>), String>) {
        match result {
            Ok((key_id, grants)) => {
                let fp = self.host_fingerprint.clone().unwrap_or_default();
                // #310 r3: any successful (re)registration opens a NEW
                // identity generation — in-flight drives dispatched before
                // this moment can no longer affect recovery state.
                self.identity_generation = self.identity_generation.wrapping_add(1);
                // #310: a successful (re)registration means the daemon
                // accepted the current key — any recorded bad_signature
                // rejection and its recovery guidance are resolved.
                self.settings.bad_signature = false;
                self.clear_recovery_notice();
                // A successful (re)registration refreshes the ledger from
                // the host's CURRENT grant set (F3): locally-demoted
                // capabilities the host re-granted are re-enabled, and
                // capabilities the host revoked are dropped.
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
                // F5 success path: a re-register rotated the persisted seed
                // BEFORE this result arrived — reload the in-memory signing
                // key so subsequent reads sign with the NEW key.
                if let Some(fp) = self.host_fingerprint.clone()
                    && let Ok(key) = crate::keys::load_or_create_key(&fp)
                {
                    self.device_key = Some(key);
                }
                self.config.registration = self.registration.clone();
                self.config.persist(&self.config_path);
                if self.identity_recovery == IdentityRecovery::InFlight {
                    // #249: the recovery re-register landed — the signature
                    // plane is live again (read-only; grants are provisioned
                    // by the host out-of-band).
                    self.identity_recovery = IdentityRecovery::None;
                    self.toast(
                        Level::Info,
                        "device identity recovered (#249) — re-registered with the current key",
                    );
                } else {
                    self.toast(
                        Level::Info,
                        format!("registered as {key_id} (grants: {})", grants.join(", ")),
                    );
                }
                self.settings.token_input.clear();
                self.tab = Tab::Board;
            }
            Err(e) => {
                self.mark_recovery_failed();
                self.toast(Level::Error, format!("registration failed: {e}"));
                self.settings.notice = Some((Level::Error, format!("registration failed: {e}")));
            }
        }
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
            crate::ui::register::Request::RecoverIdentity => {
                // #310 "Restore saved identity": re-register the CURRENT
                // key material — never mints a fresh key. Only offered
                // after an actual bad_signature rejection.
                if self.identity_recovery == IdentityRecovery::None {
                    self.identity_recovery = IdentityRecovery::Mismatch;
                }
                let _ = self.try_start_recovery();
            }
            crate::ui::register::Request::ReRegister => {
                // Fresh-key re-register ("Re-register…"). The fresh key runs
                // read-only; grants are provisioned by the host out-of-band.
                let token = if !self.settings.token_input.trim().is_empty() {
                    self.settings.token_input.trim().to_string()
                } else {
                    match crate::keys::read_daemon_registration_token() {
                        Ok(t) => t,
                        Err(e) => {
                            self.settings.notice = Some((
                                Level::Error,
                                format!("re-register needs the registration token: {e}"),
                            ));
                            return;
                        }
                    }
                };
                self.register(token, true);
            }
            crate::ui::register::Request::SaveSettings => {
                let url = self.settings.host_url.trim().to_string();
                if url.is_empty() {
                    self.settings.notice =
                        Some((Level::Error, "host URL must not be empty".into()));
                    return;
                }
                let host_changed = url != self.config.host_url;
                self.settings.host_url = url.clone();
                self.config.host_url = url.clone();
                self.config.auto_reconnect = self.settings.auto_reconnect;
                if host_changed {
                    // Registration keys and grants are scoped to the host
                    // fingerprint. A settings URL change must not carry a
                    // device identity or capability ledger across hosts.
                    self.config.registration = None;
                    self.registration = None;
                    self.ledger = GrantLedger::default();
                    self.host_fingerprint = None;
                }
                self.config.persist(&self.config_path);
                self.spawn_read_loop(url.clone());
                self.resolve_fingerprint(url);
                self.toast(Level::Info, "settings saved");
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

    /// Dispatch drive intents collected by the board after its
    /// immediate-mode render returns, so no frame holds overlapping
    /// borrows of the fleet/toast state while a network call is spawned.
    fn dispatch_drive_intents(&mut self, pending: Vec<DriveIntent>) {
        let registration = self.registration.clone();
        let signing = self.device_key.as_ref().map(|k| k.signing.clone());
        let identity_generation = self.identity_generation;
        let client = self.client.clone();
        let host_url = self.config.host_url.clone();
        let tx_drive = self.tx_drive.clone();
        let rt = self.rt.clone();
        for intent in pending {
            let Some(reg) = registration.clone() else {
                self.toasts.push_back(Toast {
                    text: "not registered — cannot read".into(),
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
                    identity_generation,
                });
            });
        }
    }
}

fn configure_fonts(ctx: &egui::Context) {
    let mut style = (*ctx.style_of(egui::Theme::Dark)).clone();
    style.spacing.item_spacing = egui::vec2(6.0, 4.0);
    style.spacing.button_padding = egui::vec2(8.0, 3.0);
    ctx.set_style_of(egui::Theme::Dark, style);
}

/// Write a viewport `ColorImage` as PNG (evidence capture only).
fn save_png(path: &std::path::Path, image: &egui::ColorImage) -> bool {
    let Some(pixel_count) = image.size[0].checked_mul(image.size[1]) else {
        tracing::error!(path = %path.display(), "screenshot dimensions overflow");
        return false;
    };
    if image.size[0] == 0 || image.size[1] == 0 || image.pixels.is_empty() {
        tracing::error!(
            path = %path.display(),
            width = image.size[0],
            height = image.size[1],
            pixels = image.pixels.len(),
            "screenshot event contained an empty image"
        );
        return false;
    }
    if image.pixels.len() != pixel_count {
        tracing::error!(
            path = %path.display(),
            expected = pixel_count,
            actual = image.pixels.len(),
            "screenshot event contained inconsistent image dimensions"
        );
        return false;
    }

    let size = [image.size[0] as u32, image.size[1] as u32];
    let mut png: image::RgbaImage = image::ImageBuffer::new(size[0], size[1]);
    for (pixel, color) in png.pixels_mut().zip(image.pixels.iter()) {
        *pixel = image::Rgba([color.r(), color.g(), color.b(), color.a()]);
    }
    if let Err(e) = png.save(path) {
        tracing::error!(path = %path.display(), error = %e, "screenshot save failed");
        let _ = std::fs::remove_file(path);
        return false;
    }
    match std::fs::metadata(path) {
        Ok(metadata) if metadata.len() > 0 => true,
        Ok(metadata) => {
            tracing::error!(
                path = %path.display(),
                bytes = metadata.len(),
                "screenshot save produced an empty file"
            );
            let _ = std::fs::remove_file(path);
            false
        }
        Err(error) => {
            tracing::error!(path = %path.display(), error = %error, "saved screenshot cannot be stat'ed");
            let _ = std::fs::remove_file(path);
            false
        }
    }
}

impl eframe::App for CorralApp {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.update_logic(ctx);
    }

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
        if got_messages {
            ctx.request_repaint();
        }

        if self.window_diagnostic {
            let now = std::time::Instant::now();
            let sample_due = self.window_diagnostic_last_sample.is_none_or(|last| {
                now.duration_since(last) >= std::time::Duration::from_millis(500)
            });
            if sample_due {
                self.request_native_probe(&ctx, "diagnostic_observation");
                self.window_diagnostic_last_sample = Some(now);
            }
            // A diagnostic run must keep producing observations even when the
            // native window has no input or screenshot work pending.
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }

        // Resolve the requested native-evidence target before the capture
        // command is issued. The target-ready state is immediately
        // dispatchable, subject to the native-window readiness assertion
        // below; the later Screenshot event remains the strict PNG boundary.
        if self.screenshot_path.is_some() && self.prepare_screenshot_evidence() {
            // Target readiness can be established while the native window is
            // idle between the SSE snapshot and this transition.
            ctx.request_repaint();
        }

        // Evidence capture: request the viewport screenshot once the target
        // is ready (or the un-targeted settle delay has elapsed). Each
        // request gets one later Screenshot event/deadline opportunity;
        // retries are bounded and never create a synthetic success artifact.
        if let Some(path) = self.screenshot_path.clone() {
            // A native window can become quiet while the screenshot command
            // is waiting for its readback event. Keep the eframe update loop
            // alive, but retain the strict Screenshot/PNG acceptance gate.
            let now = std::time::Instant::now();
            ctx.request_repaint_after(self.screenshot_state.next_wake(now));
            // The wgpu screenshot readback completes on a device poll; the
            // map callback needs it driven from here.
            if self.screenshot_state.awaiting_screenshot()
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
                tracing::info!(
                    path = %path.display(),
                    width = image.size[0],
                    height = image.size[1],
                    pixels = image.pixels.len(),
                    "screenshot event received"
                );
                let valid_png = save_png(&path, &image);
                self.screenshot_state = self.screenshot_state.record_screenshot_event(valid_png);
                if valid_png {
                    if let Some(active) = &self.screenshot_wake_active {
                        active.store(false, Ordering::Release);
                    }
                    if let Some(stop) = &self.screenshot_wake_stop {
                        stop.store(true, std::sync::atomic::Ordering::Relaxed);
                    }
                    tracing::info!(path = %path.display(), "screenshot saved — exiting");
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                } else {
                    tracing::warn!(
                        path = %path.display(),
                        "Screenshot event did not produce a valid non-empty PNG; capture remains pending"
                    );
                }
            }

            // Probe immediately before every possible dispatch. The probe is
            // fail-closed and is scoped to this process's exact native
            // window. If it is hidden or backgrounded, repeat the existing
            // exact-owned wake and defer without consuming an attempt.
            let now = std::time::Instant::now();
            if (self.screenshot_agent_id.is_none() || self.screenshot_agent_selected)
                && self.screenshot_state.dispatch_due(now)
            {
                let window = self.request_native_probe(&ctx, "dispatch_evaluation");
                // R3 demo-evidence relaxation: in offline demo-seed mode the
                // Accessibility subsystem is unavailable to this helper, so
                // the dispatch gate uses the CoreGraphics on-screen
                // observation (owner-PID match + on-screen layer-0 window),
                // which the probe reports even when AX queries fail. All
                // non-demo captures keep the full fail-closed gate.
                let demo_gate = self.demo_seeded
                    && window
                        .as_ref()
                        .is_some_and(|state| state.cg_owner_pid_match == Some(true))
                    && window
                        .as_ref()
                        .is_some_and(|state| state.window_visible == Some(true));
                let (state, decision) = self.screenshot_state.try_dispatch(
                    now,
                    window.as_ref().is_some_and(|state| state.visible) || demo_gate,
                    window.as_ref().is_some_and(|state| state.frontmost) || demo_gate,
                );
                self.screenshot_state = state;
                match decision {
                    ScreenshotDispatch::Dispatched { attempt } => {
                        let window =
                            window.expect("a dispatched screenshot requires a probe result");
                        // Exact-PID activation is needed to get us to this
                        // frame. Once the probe has authorized dispatch, stop
                        // sending input; the repaint helper remains alive
                        // until the Screenshot event is saved.
                        if let Some(active) = &self.screenshot_wake_active {
                            active.store(false, Ordering::Release);
                        }
                        tracing::info!(
                            path = %path.display(),
                            attempt,
                            visible = window.visible,
                            frontmost = window.frontmost,
                            reason_code = window.reason_code,
                            "requesting viewport screenshot"
                        );
                        ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(
                            egui::UserData::default(),
                        ));
                        // The screenshot readback event is delivered on a
                        // later frame. Explicitly wake eframe after issuing
                        // the command so a quiet native window cannot strand
                        // the pending GPU map callback.
                        ctx.request_repaint();
                    }
                    ScreenshotDispatch::DeferredForWindow => {
                        let visible = window.as_ref().is_some_and(|state| state.visible);
                        let frontmost = window.as_ref().is_some_and(|state| state.frontmost);
                        let reason_code = window
                            .as_ref()
                            .map_or("defer_probe_pending", |state| state.reason_code);
                        let should_wake = self.screenshot_last_wake.is_none_or(|last| {
                            now.duration_since(last) >= SCREENSHOT_WAKE_RETRY_INTERVAL
                        });
                        if should_wake {
                            if let Some(command) = self.screenshot_wake_command.as_deref() {
                                // The helper thread owns the repeated wake
                                // schedule. Keep the old direct call only as
                                // a bounded fallback if that thread could not
                                // be started.
                                if self.screenshot_wake_active.is_none() {
                                    let _ = invoke_exact_window_wake(command, &path);
                                }
                                self.screenshot_last_wake = Some(now);
                                tracing::info!(
                                    path = %path.display(),
                                    visible,
                                    frontmost,
                                    reason_code,
                                    "deferring viewport screenshot; exact-owned wake schedule remains active"
                                );
                            } else {
                                tracing::warn!(
                                    pid = std::process::id(),
                                    visible,
                                    frontmost,
                                    reason_code,
                                    "native window is not ready and no exact-owned wake command is configured"
                                );
                                self.screenshot_last_wake = Some(now);
                            }
                        }
                        ctx.request_repaint_after(std::time::Duration::from_millis(100));
                    }
                    ScreenshotDispatch::Exhausted => {
                        if let Some(active) = &self.screenshot_wake_active {
                            active.store(false, Ordering::Release);
                        }
                        if let Some(stop) = &self.screenshot_wake_stop {
                            stop.store(true, std::sync::atomic::Ordering::Relaxed);
                        }
                        tracing::error!(
                            path = %path.display(),
                            attempts = SCREENSHOT_MAX_ATTEMPTS,
                            "screenshot capture exhausted without a valid Screenshot PNG event"
                        );
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                    ScreenshotDispatch::NotDue => {}
                }
            }
        }

        self.update_settings_request();

        // Screenshot evidence is one deterministic tab per process. Keep the
        // requested tab locked while the env-gated capture is alive so a
        // stray native focus/click event cannot make a Board artifact claim a
        // different workspace surface.
        if self.screenshot_path.is_some() {
            self.tab = tab_from_env();
        }

        if self.registration.is_none() {
            egui::CentralPanel::default().show(ui, |ui| {
                crate::ui::register::register_screen(ui, &mut self.settings, self.conn);
            });
            crate::ui::toast_area(&ctx, &mut self.toasts);
            ctx.request_repaint_after(std::time::Duration::from_millis(500));
            return;
        }

        egui::CentralPanel::default().show(ui, |ui| {
            workspace(ui, self, &ctx);
        });

        crate::ui::toast_area(&ctx, &mut self.toasts);
        ctx.request_repaint_after(std::time::Duration::from_millis(500));
    }
}

impl Drop for CorralApp {
    fn drop(&mut self) {
        // The helper carries this process's exact PID in every wake command.
        // Stop it before the app can disappear so a late retry cannot act on
        // a reused PID after the capture process exits.
        if let Some(active) = &self.screenshot_wake_active {
            active.store(false, Ordering::Release);
        }
        if let Some(stop) = &self.screenshot_wake_stop {
            stop.store(true, Ordering::Release);
        }
    }
}

/// #310: no workspace-wide identity banner. Recovery guidance lives ONLY
/// inside the Settings recovery block, and only after an actual current-key
/// `bad_signature` rejection; a local fingerprint mismatch at startup is a
/// passive Settings notice, never a mutation or a re-registration prompt.
fn workspace(ui: &mut egui::Ui, app: &mut CorralApp, ctx: &egui::Context) {
    let available = ui.available_size();
    let (workspace_rect, _) = ui.allocate_exact_size(available, egui::Sense::hover());
    let left_width = (workspace_rect.width() * crate::ui::board::MASTER_DETAIL_RATIO.0).max(280.0);
    let right_width = (workspace_rect.width() - left_width - 1.0).max(0.0);
    let left_rect = egui::Rect::from_min_size(
        workspace_rect.min,
        egui::vec2(left_width, workspace_rect.height()),
    );
    let right_rect = egui::Rect::from_min_size(
        egui::pos2(left_rect.right() + 1.0, workspace_rect.top()),
        egui::vec2(right_width, workspace_rect.height()),
    );
    let painter = ui.painter();
    painter.rect_filled(workspace_rect, egui::CornerRadius::same(12), theme::ui::BG);
    painter.rect_stroke(
        workspace_rect,
        egui::CornerRadius::same(12),
        egui::Stroke::new(1.0, theme::ui::FRAME_BORDER),
        egui::StrokeKind::Outside,
    );
    painter.rect_filled(left_rect, egui::CornerRadius::ZERO, theme::ui::PANEL);
    painter.line_segment(
        [left_rect.right_top(), left_rect.right_bottom()],
        egui::Stroke::new(1.0, theme::ui::LINE),
    );

    // LEFT: the read-only board (repo groups + blocked pinned on top).
    let mut left_ui = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(left_rect.shrink(1.0))
            .id(egui::Id::new("corral-ui-persistent-master-bar"))
            .layout(egui::Layout::top_down(egui::Align::Min)),
    );
    let clicked = crate::ui::board::show_board(
        &mut left_ui,
        &app.fleet,
        app.conn,
        app.conn_detail.as_deref(),
        app.fleet.selected_agent.as_deref(),
        true,
        "board-row",
    );
    if let Some(agent_id) = clicked {
        app.fleet.select_agent(&agent_id);
    }

    // RIGHT: tab strip + the tab's content (Board = recents v1 drill-in for
    // the selected agent; Settings = connection only).
    let mut right_ui = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(right_rect.shrink(1.0))
            .id(egui::Id::new("corral-ui-persistent-detail-pane"))
            .layout(egui::Layout::top_down(egui::Align::Min)),
    );
    tab_strip(&mut right_ui, &mut app.tab);
    right_ui.separator();

    match app.tab {
        Tab::Board => {
            // Recents v1: LIVE TAIL ONLY for the selected agent. No
            // load-earlier, no Conversation/Harness partition, no composer.
            if let Some(agent_id) = app.fleet.selected_agent.clone() {
                let mut retry_requested = false;
                if let Some(agent) = app.fleet.agents.get(&agent_id) {
                    let can_read = agent.can_read_tail();
                    let lines = app
                        .fleet
                        .tails
                        .get(&agent_id)
                        .map(Vec::as_slice)
                        .unwrap_or(&[]);
                    let blocks = app
                        .fleet
                        .tail_blocks
                        .get(&agent_id)
                        .map(Vec::as_slice)
                        .unwrap_or(&[]);
                    let rows = crate::ui::board::tail_rows(lines, blocks);
                    let phase = crate::ui::board::recents_phase(&app.fleet, &agent_id, can_read);
                    let live = app.conn == ConnState::Connected
                        && matches!(
                            app.fleet.latest_drive(&agent_id),
                            Some(crate::state::DriveState::Ok { .. })
                        );
                    crate::ui::board::show_recents(
                        &mut right_ui,
                        agent,
                        &rows,
                        phase,
                        live,
                        &mut || retry_requested = true,
                    );
                }
                // Hydration + paced revision-aware refresh ride the frame
                // cadence exactly as pre-cut (single-flight + cooldown).
                app.hydrate_recent_output(Some(&agent_id));
                app.refresh_recent_output(Some(&agent_id));
                if retry_requested {
                    // Retry after an error/empty state: force the gates to
                    // re-evaluate by clearing the failed bookkeeping the
                    // phase reads, then hydrate once.
                    if !app.fleet.tails.contains_key(&agent_id)
                        && let Some(latest) = app.fleet.latest_drive(&agent_id)
                        && matches!(latest, crate::state::DriveState::Failed { .. })
                    {
                        app.fleet.recent_drives.remove(&agent_id);
                    }
                    app.hydrate_recent_output(Some(&agent_id));
                }
            } else {
                ui.vertical_centered(|ui| {
                    ui.add_space(12.0);
                    ui.label(
                        RichText::new("select an agent on the board to read its recent output")
                            .color(theme::ui::TEXT_MUTED),
                    );
                });
            }
        }
        Tab::Settings => {
            let store = app.device_key.as_ref().map(|key| key.store.clone());
            let key_id = app
                .registration
                .as_ref()
                .map(|registration| registration.key_id.clone())
                .unwrap_or_default();
            let grants = app
                .registration
                .as_ref()
                .map(|registration| registration.grants.clone())
                .unwrap_or_default();
            crate::ui::register::settings_pane(
                &mut right_ui,
                &mut app.settings,
                crate::ui::register::SettingsPaneContext {
                    key_id: &key_id,
                    grants: &grants,
                    store: store.as_ref(),
                    conn: app.conn,
                    rev: app.fleet.rev,
                },
            );
        }
    }
    ctx.request_repaint_after(std::time::Duration::from_millis(500));
}

fn tab_strip(ui: &mut egui::Ui, active: &mut Tab) {
    ui.horizontal(|ui| {
        for (label, tab) in TAB_LABELS {
            let selected = *active == tab;
            let response = ui.selectable_label(
                selected,
                RichText::new(label).strong().color(if selected {
                    theme::ui::INK
                } else {
                    theme::ui::MUTED
                }),
            );
            if selected {
                ui.painter().line_segment(
                    [response.rect.left_bottom(), response.rect.right_bottom()],
                    egui::Stroke::new(2.0, theme::ui::ACCENT),
                );
            }
            if response.clicked() {
                *active = tab;
            }
        }
    });
}

/// Select the only agent eligible for automatic Recent-output hydration.
/// `resolved_selection` must come from the board's visible resolver; no
/// map-order fallback belongs here, and this helper deliberately never
/// mutates `Fleet::selected_agent`.
fn hydration_target(fleet: &Fleet, resolved_selection: Option<&str>) -> Option<String> {
    let agent_id = resolved_selection?;
    let agent = fleet.agents.get(agent_id)?;
    agent
        .capabilities
        .iter()
        .any(|capability| capability == "read_tail")
        .then(|| agent_id.to_string())
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use axum::{Router, body::Body, http::Request, response::Response, routing::any};
    use tokio::sync::Mutex;

    use super::*;
    use crate::model::{Agent, AgentState, Workspace};
    use crate::state::DriveState;

    fn agent(id: &str, state: AgentState, capabilities: &[&str]) -> Agent {
        Agent {
            agent_id: id.into(),
            source: "herdr".into(),
            tool: "claude".into(),
            state,
            reason: None,
            seq: 1,
            ts: 1,
            capabilities: capabilities.iter().map(|cap| (*cap).into()).collect(),
            workspace: Workspace {
                repo: Some("corral".into()),
                branch: Some("g354-l3".into()),
                ..Default::default()
            },
            attachment: None,
            display_name: None,
            title: None,
        }
    }

    fn read_model_test_app() -> (tokio::runtime::Runtime, CorralApp) {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("test runtime");
        let client = reqwest::Client::builder().build().expect("test client");
        let (tx_apply, rx_apply) = tokio::sync::mpsc::unbounded_channel();
        let (tx_drive, rx_drive) = tokio::sync::mpsc::unbounded_channel();
        let (native_probe_tx, native_probe_rx) = std::sync::mpsc::channel();
        let config = PersistedConfig {
            host_url: "http://127.0.0.1:1".into(),
            ..Default::default()
        };
        let settings = crate::ui::register::SettingsState {
            host_url: config.host_url.clone(),
            ..Default::default()
        };
        let app = CorralApp {
            rt: runtime.handle().clone(),
            client,
            fleet: Fleet::default(),
            conn: ConnState::Connecting,
            conn_detail: None,
            toasts: VecDeque::new(),
            device_key: None,
            device_key_store_warning: false,
            ledger: GrantLedger::default(),
            registration: None,
            host_fingerprint: None,
            identity_recovery: IdentityRecovery::None,
            identity_generation: 0,
            config,
            config_path: PathBuf::from("/tmp/corral-ui-read-model-test.json"),
            settings,
            tx_apply: tx_apply.clone(),
            rx_apply,
            rx_drive,
            tx_drive,
            stop_read: None,
            read_loop_generation: 7,
            recent_output_last_refresh: None,

            tab: Tab::Board,
            screenshot_path: None,
            screenshot_state: ScreenshotCaptureState::initial(
                false,
                false,
                Instant::now(),
                std::time::Duration::ZERO,
            ),
            screenshot_settle: std::time::Duration::ZERO,
            screenshot_agent_id: None,
            screenshot_agent_selected: false,
            screenshot_wake_stop: None,
            screenshot_wake_active: None,
            screenshot_wake_command: None,
            screenshot_last_wake: None,
            window_diagnostic: false,
            demo_seeded: false,
            window_diagnostic_last_sample: None,
            evidence_visibility_requested: false,
            native_probe_tx,
            native_probe_rx,
            native_probe_in_flight: false,
        };
        (runtime, app)
    }

    /// #249 test app: a registered device whose key material was replaced
    /// (the reinstall state) — `registered_key_id` names the OLD key while
    /// `device_key` holds a FRESH seed.
    fn identity_test_app(
        registered_key_id: &str,
        device_key: DeviceKey,
    ) -> (tokio::runtime::Runtime, CorralApp) {
        let (runtime, mut app) = read_model_test_app();
        app.registration = Some(RegistrationRecord {
            host_fingerprint: "deadbeef00000000".to_string(),
            key_id: registered_key_id.to_string(),
            grants: vec!["read_tail".to_string()],
            denied: vec![],
        });
        app.host_fingerprint = Some("deadbeef00000000".to_string());
        app.device_key = Some(device_key);
        app.ledger = GrantLedger {
            base: app.registration.as_ref().unwrap().grants.clone(),
            denied: vec![],
        };
        (runtime, app)
    }

    fn fresh_device_key(seed: u8) -> DeviceKey {
        DeviceKey {
            signing: ed25519_dalek::SigningKey::from_bytes(&[seed; 32]),
            store: KeyStore::File {
                path: PathBuf::from("/tmp/corral-ui-identity-test.key"),
            },
        }
    }

    /// RAII env guard mirroring the daemon test suites' EnvRestore. Used to
    /// point the client config paths at scratch dirs and to force the
    /// file-store key mode: a test binary is not the keychain-authorized
    /// `corrald-ui` app, so a keyring read BLOCKS on an interactive
    /// Keychain prompt (the #209/#241 evidence-harness trap).
    struct EnvRestore {
        name: &'static str,
        previous: Option<String>,
    }

    impl EnvRestore {
        fn set(name: &'static str, value: impl Into<String>) -> Self {
            let previous = std::env::var(name).ok();
            unsafe { std::env::set_var(name, value.into()) };
            Self { name, previous }
        }
    }

    impl Drop for EnvRestore {
        fn drop(&mut self) {
            match &self.previous {
                Some(value) => unsafe { std::env::set_var(self.name, value) },
                None => unsafe { std::env::remove_var(self.name) },
            }
        }
    }

    fn scratch_dir(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "corral-ui-identity-test-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("test clock after epoch")
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).expect("scratch dir");
        dir
    }

    fn ready_read_app(app: &mut CorralApp) -> String {
        let key = fresh_device_key(11);
        let key_id = crate::keys::device_key_id(&key.signing.verifying_key().to_bytes());
        app.registration = Some(RegistrationRecord {
            host_fingerprint: "deadbeef00000000".into(),
            key_id: key_id.clone(),
            grants: vec!["read_tail".into()],
            denied: vec![],
        });
        app.host_fingerprint = Some("deadbeef00000000".into());
        app.device_key = Some(key);
        app.ledger = GrantLedger {
            base: vec!["read_tail".into()],
            denied: vec![],
        };
        key_id
    }

    // ------------------------------------------------------------------
    // #354 L3 probes: the read-only surface is closed.
    // ------------------------------------------------------------------

    /// #354 L3 probe helper: production code lines only (tests stripped,
    /// comments stripped) so probes never trip on their own needles.
    fn production_code_lines(source: &str) -> Vec<String> {
        let production = source.split("#[cfg(test)]").next().unwrap_or(source);
        production
            .lines()
            .map(|line| {
                let code = match line.find("//") {
                    Some(idx) => &line[..idx],
                    None => line,
                };
                code.trim().to_string()
            })
            .filter(|line| !line.is_empty())
            .collect()
    }

    #[test]
    fn workspace_navigation_has_exactly_two_tabs_and_no_issues_destination() {
        assert_eq!(TAB_LABELS.len(), 2);
        assert_eq!(
            TAB_LABELS
                .iter()
                .map(|(label, _)| *label)
                .collect::<Vec<_>>(),
            ["Board", "Settings"]
        );
        let source = include_str!("app.rs");
        let code_lines = production_code_lines(source);
        assert!(
            code_lines.iter().all(|line| !line.contains("Tab::Issues")),
            "the Issues tab variant must be gone from the client code"
        );
        assert_eq!(
            code_lines
                .iter()
                .filter(|line| line.contains("tab_strip(&mut right_ui, &mut app.tab);"))
                .count(),
            1
        );
    }

    /// Source-level probe: no mutating drive surface may survive anywhere in
    /// the egui client's production code (drive.rs keeps only the read
    /// capability, app.rs only read_tail intents, and no Issues/grant-admin
    /// UI text remains).
    #[test]
    fn mutating_drive_surface_is_absent_from_the_client() {
        let banned_in_drive = [
            "DriveIntent::prompt",
            "DriveIntent::interrupt",
            "DriveIntent::approve",
            "DriveIntent::kill",
            "DriveIntent::attach",
            "start_worktree",
            "read_diff_page",
            "mint_step_up",
            "sign_step_up",
            "approval_claim",
            "suggests_step_up",
        ];
        let drive_source = include_str!("../src/drive.rs");
        let drive_lines = production_code_lines(drive_source);
        for needle in banned_in_drive {
            assert!(
                drive_lines.iter().all(|line| !line.contains(needle)),
                "drive.rs production code must not contain {needle}"
            );
        }
        let app_source = include_str!("app.rs");
        let app_lines = production_code_lines(app_source);
        for needle in [
            "fetch_issues",
            "refresh_issues",
            "GrantDevices",
            "GrantMutation",
            "fetch_audit",
            "admin_token",
            "grant_admin",
            "Tab::Issues",
            "load_earlier",
        ] {
            assert!(
                app_lines.iter().all(|line| !line.contains(needle)),
                "app.rs production code must not contain {needle}"
            );
        }
        let board_source = include_str!("ui/board.rs");
        let board_lines = production_code_lines(board_source);
        for needle in [
            "drive_controls",
            "prompt_widget",
            "approve_choices",
            "ConversationPartition",
            "HarnessEntry",
            "recent_prompt_composer",
            "load_earlier",
            "Search",
        ] {
            assert!(
                board_lines.iter().all(|line| !line.contains(needle)),
                "ui/board.rs production code must not contain {needle}"
            );
        }
    }

    #[test]
    fn read_only_ui_copy_is_used_across_the_surfaces() {
        let app_source = include_str!("app.rs");
        assert!(
            app_source.contains("select an agent on the board to read its recent output"),
            "the board tab's empty state names the recents drill-in"
        );
        let board_source = include_str!("ui/board.rs");
        assert!(board_source.contains("daemon offline — showing the last-known board"));
        assert!(board_source.contains("blocked ("));
        assert!(!board_source.contains("Needs you"));
        assert!(!board_source.contains("Finished"));
    }

    // ------------------------------------------------------------------
    // Config persistence (connection-only).
    // ------------------------------------------------------------------

    #[test]
    fn config_round_trip_preserves_host_registration_and_tolerates_legacy_keys() {
        let dir = scratch_dir("config");
        let path = dir.join("config.json");
        // Legacy file with the pre-cut view toggles: extra keys are ignored.
        std::fs::write(
            &path,
            r#"{"host_url":"http://127.0.0.1:8474","group_by_repo":true,"completed_mode":"collapsed","stick_to_bottom":true,"theme":"dark"}"#,
        )
        .unwrap();
        let loaded = PersistedConfig::load(&path);
        assert_eq!(loaded.host_url, "http://127.0.0.1:8474");
        assert!(loaded.auto_reconnect);
        assert!(loaded.registration.is_none());

        let config = PersistedConfig {
            host_url: "http://127.0.0.1:9999".into(),
            registration: Some(RegistrationRecord {
                host_fingerprint: "fp".into(),
                key_id: "k1".into(),
                grants: vec!["read_tail".into()],
                denied: vec![],
            }),
            auto_reconnect: false,
        };
        config.persist(&path);
        let reloaded = PersistedConfig::load(&path);
        assert_eq!(reloaded, config);
        let wire: crate::state::PersistedConfig =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(wire.host_url.as_deref(), Some("http://127.0.0.1:9999"));
        assert_eq!(wire.auto_reconnect, Some(false));
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ------------------------------------------------------------------
    // Tabs.
    // ------------------------------------------------------------------

    fn navigation_input(events: Vec<egui::Event>) -> egui::RawInput {
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(900.0, 600.0),
            )),
            events,
            ..Default::default()
        }
    }

    fn navigation_pointer_input(pos: egui::Pos2, pressed: bool) -> egui::RawInput {
        navigation_input(vec![
            egui::Event::PointerMoved(pos),
            egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: Default::default(),
            },
        ])
    }

    fn text_rect(output: &egui::FullOutput, needle: &str) -> Option<egui::Rect> {
        fn walk(shape: &egui::epaint::Shape, needle: &str) -> Option<egui::Rect> {
            match shape {
                egui::epaint::Shape::Text(text) if text.galley.job.text.contains(needle) => {
                    Some(text.visual_bounding_rect())
                }
                egui::epaint::Shape::Vec(shapes) => {
                    shapes.iter().find_map(|shape| walk(shape, needle))
                }
                _ => None,
            }
        }
        output
            .shapes
            .iter()
            .find_map(|clipped| walk(&clipped.shape, needle))
    }

    #[test]
    fn tab_strip_click_navigates_to_settings_without_an_issues_destination() {
        let ctx = egui::Context::default();
        let mut active = Tab::Board;
        let mut output = ctx.run_ui(navigation_input(vec![]), |ui| {
            tab_strip(ui, &mut active);
        });
        let settings = text_rect(&output, "Settings").expect("Settings tab rendered");
        let pos = settings.center();
        output.textures_delta.clear();

        for pressed in [true, false] {
            let mut frame = ctx.run_ui(navigation_pointer_input(pos, pressed), |ui| {
                tab_strip(ui, &mut active);
            });
            frame.textures_delta.clear();
        }

        assert_eq!(active, Tab::Settings);
        assert!(text_rect(&output, "Issues").is_none());
        assert!(text_rect(&output, "Audit").is_none());
    }

    // ------------------------------------------------------------------
    // Read path: cache application + E2E through the production frame.
    // ------------------------------------------------------------------

    #[test]
    fn read_tail_drive_response_reaches_the_app_tail_cache() {
        let mut fleet = Fleet::default();
        let msg = DriveMsg {
            agent_id: "herdr:recents".into(),
            capability: "read_tail".into(),
            outcome: DriveOutcome::Ok {
                rev: 43,
                result: Some(serde_json::json!({
                    "lines": ["line one", "line two"],
                    "blocks": [
                        {"kind": "agent", "text": "line one"},
                        {"kind": "tool", "text": "line two"}
                    ],
                    "source_rev": 42
                })),
            },
            identity_generation: 0,
        };

        CorralApp::apply_read_tail_result(&mut fleet, &msg);
        assert_eq!(
            fleet.tails["herdr:recents"],
            ["line one".to_string(), "line two".to_string()]
        );
        assert_eq!(fleet.tail_source_revs["herdr:recents"], 42);
        assert_eq!(
            fleet.tail_blocks["herdr:recents"].len(),
            2,
            "canonical blocks ride additively into the same cache"
        );
    }

    fn sending_read_tail_drives(fleet: &Fleet, agent_id: &str) -> usize {
        fleet
            .recent_drives
            .get(agent_id)
            .map(|drives| {
                drives
                    .iter()
                    .filter(|state| matches!(state, DriveState::Sending { capability, .. } if capability == "read_tail"))
                    .count()
            })
            .unwrap_or(0)
    }

    /// The read path is INTACT end to end through the production Board
    /// frame: initial hydration dispatches the 200-line v1 tail without a
    /// cursor, the source advances, the next real frame sends the
    /// revision-aware refresh carrying the cached source_rev, and an
    /// unchanged revision settles without an immediate request loop.
    #[test]
    fn board_frame_drives_the_live_tail_hydration_and_revision_aware_refresh() {
        let (runtime, mut app) = read_model_test_app();

        let drives = std::sync::Arc::new(Mutex::new(Vec::<serde_json::Value>::new()));
        let canned_rev = std::sync::Arc::new(Mutex::new(4u64));
        let canned_lines = std::sync::Arc::new(Mutex::new(vec!["first window A1".to_string()]));
        let router_drives = drives.clone();
        let router_canned_rev = canned_rev.clone();
        let router_canned_lines = canned_lines.clone();
        let router = Router::new().fallback(any(move |request: Request<Body>| {
            let drives = router_drives.clone();
            let canned_rev = router_canned_rev.clone();
            let canned_lines = router_canned_lines.clone();
            async move {
                if request.uri().path() == "/drive" {
                    let body = axum::body::to_bytes(request.into_body(), 1 << 20)
                        .await
                        .expect("drive body");
                    let signed: serde_json::Value =
                        serde_json::from_slice(&body).expect("signed drive body");
                    let envelope = signed["envelope"].clone();
                    drives.lock().await.push(envelope);
                    let rev = *canned_rev.lock().await;
                    let lines = canned_lines.lock().await.clone();
                    let response = serde_json::json!({
                        "request_id": signed["envelope"]["request_id"],
                        "ok": true,
                        "rev": rev,
                        "result": { "lines": lines, "source_rev": rev },
                    });
                    return Response::builder()
                        .status(200)
                        .header("content-type", "application/json")
                        .body(Body::from(serde_json::to_vec(&response).unwrap()))
                        .unwrap();
                }
                Response::new(Body::from(r#"{"ok":true}"#))
            }
        }));

        runtime.block_on(async {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
                .await
                .expect("bind loopback");
            let address = listener.local_addr().unwrap();
            tokio::spawn(async move {
                axum::serve(listener, router).await.unwrap();
            });
            app.config.host_url = format!("http://{address}");
            let key_id = ready_read_app(&mut app);

            let mut selected = agent("herdr:l3", AgentState::Working, &["read_tail"]);
            selected.agent_id = "herdr:l3".into();
            selected.seq = 3;
            app.fleet = Fleet {
                agents: [(selected.agent_id.clone(), selected)]
                    .into_iter()
                    .collect(),
                rev: Some(7),
                selected_agent: Some("herdr:l3".into()),
                ..Default::default()
            };
            app.tab = Tab::Board;
            let _ = key_id;

            async fn pump_until(app: &mut CorralApp, cond: impl Fn(&CorralApp) -> bool) {
                let deadline = Instant::now() + std::time::Duration::from_secs(5);
                while Instant::now() < deadline {
                    while let Ok(msg) = app.rx_drive.try_recv() {
                        app.on_drive(msg);
                    }
                    if cond(app) {
                        return;
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
                }
                panic!(
                    "condition not met before the deadline; drives={:?} source_revs={:?}",
                    app.fleet.recent_drives.get("herdr:l3"),
                    app.fleet.tail_source_revs.get("herdr:l3"),
                );
            }

            let ctx = egui::Context::default();
            let board_frame = |app: &mut CorralApp, ctx: &egui::Context| {
                ctx.run_ui(egui::RawInput::default(), |ui| {
                    workspace(ui, app, ctx);
                })
                .drop_without_applying_deltas();
            };

            // Frame 1: initial hydration through the real arm.
            board_frame(&mut app, &ctx);
            pump_until(&mut app, |app| {
                app.fleet.tails.contains_key("herdr:l3")
                    && app.fleet.tail_source_revs.get("herdr:l3") == Some(&4)
            })
            .await;
            assert_eq!(drives.lock().await.len(), 1, "one hydration request");
            let first = drives.lock().await[0].clone();
            assert_eq!(first["capability"], "read_tail");
            assert_eq!(first["target"], "herdr:l3");
            assert_eq!(
                first["payload"]["lines"], 200,
                "recents v1 requests the daemon-capped 200-line live tail"
            );
            assert!(first["payload"].get("since_rev").is_none());
            assert_eq!(app.fleet.tails["herdr:l3"], ["first window A1".to_string()]);

            // The source advances; the NEXT real frame alone must send the
            // revision-aware refresh (no direct method call from the test).
            *canned_rev.lock().await = 5;
            *canned_lines.lock().await =
                vec!["newer window B1".to_string(), "newer window B2".to_string()];
            pump_until(&mut app, |app| {
                matches!(
                    app.fleet.latest_drive("herdr:l3"),
                    Some(DriveState::Ok { .. })
                )
            })
            .await;

            board_frame(&mut app, &ctx);
            let deadline = Instant::now() + std::time::Duration::from_secs(5);
            while drives.lock().await.len() < 2 && Instant::now() < deadline {
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
            let second = drives.lock().await[1].clone();
            assert_eq!(second["target"], "herdr:l3");
            assert_eq!(
                second["payload"]["since_rev"],
                serde_json::json!(4),
                "the frame-driven refresh carries the CACHED source_rev"
            );
            assert_eq!(second["payload"]["lines"], 200);
            pump_until(&mut app, |app| {
                app.fleet.tail_source_revs.get("herdr:l3") == Some(&5)
            })
            .await;
            assert_eq!(
                app.fleet.tails["herdr:l3"],
                ["newer window B1".to_string(), "newer window B2".to_string()]
            );

            // Unchanged source: no immediate loop through the real cadence.
            board_frame(&mut app, &ctx);
            let deadline = Instant::now() + std::time::Duration::from_millis(150);
            while Instant::now() < deadline {
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
            assert_eq!(drives.lock().await.len(), 2);
        });
    }

    #[test]
    fn unchanged_tail_settles_without_immediate_request_loop_and_single_flight_holds() {
        let (_runtime, mut app) = read_model_test_app();
        ready_read_app(&mut app);
        let working = agent("herdr:settle", AgentState::Working, &["read_tail"]);
        app.fleet = Fleet {
            agents: [(working.agent_id.clone(), working)].into_iter().collect(),
            rev: Some(7),
            selected_agent: Some("herdr:settle".into()),
            ..Default::default()
        };
        app.fleet
            .remember_tail_full("herdr:settle", vec!["window".into()], Vec::new(), Some(4));
        app.fleet.remember_drive(
            "herdr:settle",
            DriveState::Ok {
                rev: 4,
                capability: "read_tail".into(),
            },
        );

        app.refresh_recent_output(Some("herdr:settle"));
        assert_eq!(sending_read_tail_drives(&app.fleet, "herdr:settle"), 1);
        app.refresh_recent_output(Some("herdr:settle"));
        assert_eq!(
            sending_read_tail_drives(&app.fleet, "herdr:settle"),
            1,
            "single-flight: an in-flight refresh suppresses duplicates"
        );

        app.on_drive(DriveMsg {
            agent_id: "herdr:settle".into(),
            capability: "read_tail".into(),
            outcome: DriveOutcome::Ok {
                rev: 4,
                result: Some(serde_json::json!({
                    "lines": ["window"],
                    "source_rev": 4
                })),
            },
            identity_generation: 0,
        });
        for _ in 0..3 {
            app.refresh_recent_output(Some("herdr:settle"));
        }
        assert_eq!(
            sending_read_tail_drives(&app.fleet, "herdr:settle"),
            1,
            "an unchanged source_rev settles without an immediate request loop"
        );

        app.recent_output_last_refresh = Some(
            Instant::now() - RECENT_OUTPUT_REFRESH_COOLDOWN - std::time::Duration::from_millis(1),
        );
        app.refresh_recent_output(Some("herdr:settle"));
        assert_eq!(
            sending_read_tail_drives(&app.fleet, "herdr:settle"),
            2,
            "the paced refresh resumes after the cooldown"
        );
    }

    #[test]
    fn hidden_agents_are_never_auto_refreshed_even_when_cached_and_stale() {
        let (_runtime, mut app) = read_model_test_app();
        ready_read_app(&mut app);
        let visible = agent("herdr:a", AgentState::Working, &["read_tail"]);
        let hidden = agent("herdr:b", AgentState::Working, &["read_tail"]);
        app.fleet = Fleet {
            agents: [
                (visible.agent_id.clone(), visible),
                (hidden.agent_id.clone(), hidden),
            ]
            .into_iter()
            .collect(),
            rev: Some(9),
            selected_agent: Some("herdr:a".into()),
            ..Default::default()
        };
        app.fleet
            .remember_tail_full("herdr:a", vec!["a1".into()], Vec::new(), Some(4));
        app.fleet
            .remember_tail_full("herdr:b", vec!["stale-b".into()], Vec::new(), Some(2));

        app.refresh_recent_output(app.fleet.selected_agent.clone().as_deref());
        assert_eq!(sending_read_tail_drives(&app.fleet, "herdr:a"), 1);
        assert_eq!(
            sending_read_tail_drives(&app.fleet, "herdr:b"),
            0,
            "the non-selected agent is never prefetched"
        );
        assert_eq!(app.fleet.tails["herdr:b"], ["stale-b".to_string()]);
    }

    // ------------------------------------------------------------------
    // #249/#310 identity recovery on the read-only daemon.
    // ------------------------------------------------------------------

    #[test]
    fn startup_identity_mismatch_surfaces_passive_notice_without_mutating_identity() {
        let _lock = crate::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let ui_dir = scratch_dir("passive");
        let _env_ui = EnvRestore::set("CORRAL_UI_CONFIG_DIR", ui_dir.display().to_string());
        let _env_kr = EnvRestore::set("CORRAL_UI_DISABLE_KEYRING", "1");

        let (runtime, mut app) = identity_test_app("registered-old-key", fresh_device_key(3));
        let _ = runtime;
        // Startup shape: config.json names the old key id while the key
        // material on disk is the fresh one; the server fingerprint has not
        // been applied yet. Applying it must not mutate identity state — it
        // restores the grant ledger and raises a passive notice only.
        app.host_fingerprint = None;
        app.apply_fingerprint("deadbeef00000000".to_string());
        // The fresh key id differs from the registered old key id.
        let key_id = crate::keys::device_key_id(
            &app.device_key
                .as_ref()
                .unwrap()
                .signing
                .verifying_key()
                .to_bytes(),
        );
        assert_ne!(key_id, "registered-old-key");

        let (level, text) = app.settings.notice.as_ref().expect("passive notice");
        assert!(matches!(level, Level::Warn));
        assert!(text.contains("Device identity changed"));
        assert_eq!(
            app.identity_recovery,
            IdentityRecovery::None,
            "startup must never auto-enter recovery"
        );
        assert!(
            app.registration.is_some(),
            "startup must never mutate the stored registration"
        );
        let _ = std::fs::remove_dir_all(&ui_dir);
    }

    #[test]
    fn bad_signature_refusal_arms_recovery_and_stale_generation_results_never_touch_it() {
        let (runtime, mut app) = identity_test_app("registered-old-key", fresh_device_key(3));
        let _ = runtime;
        app.identity_generation = 7;

        // A stale-generation bad_signature must not arm recovery.
        app.on_drive(DriveMsg {
            agent_id: "herdr:a".into(),
            capability: "read_tail".into(),
            outcome: DriveOutcome::Refused(crate::drive::DriveFailure::BadSignature("sig".into())),
            identity_generation: 6,
        });
        assert!(!app.settings.bad_signature);
        assert!(app.settings.recovery_notice.is_none());

        // A current-generation bad_signature arms the Settings recovery block.
        app.on_drive(DriveMsg {
            agent_id: "herdr:a".into(),
            capability: "read_tail".into(),
            outcome: DriveOutcome::Refused(crate::drive::DriveFailure::BadSignature("sig".into())),
            identity_generation: 7,
        });
        assert!(app.settings.bad_signature);
        assert!(
            app.settings
                .recovery_notice
                .as_deref()
                .is_some_and(|t| t.contains("re-register")),
            "re-registration guidance names the Settings recovery path"
        );

        // A current-generation success clears the latch + its notice.
        app.on_drive(DriveMsg {
            agent_id: "herdr:a".into(),
            capability: "read_tail".into(),
            outcome: DriveOutcome::Ok {
                rev: 1,
                result: Some(serde_json::json!({ "lines": [], "source_rev": 1 })),
            },
            identity_generation: 7,
        });
        assert!(!app.settings.bad_signature);
        assert!(app.settings.recovery_notice.is_none());
    }

    #[test]
    fn successful_registration_clears_recovery_and_returns_to_the_board() {
        let (runtime, mut app) = read_model_test_app();
        let _ = runtime;
        app.identity_recovery = IdentityRecovery::InFlight;
        app.settings.bad_signature = true;
        app.settings.recovery_notice = Some("guidance".into());
        app.settings.notice = Some((Level::Warn, "guidance".into()));
        app.host_fingerprint = Some("fp".into());
        app.tab = Tab::Settings;

        app.handle_register_result(Ok(("new-key".into(), vec!["read_tail".into()])));
        assert_eq!(app.identity_recovery, IdentityRecovery::None);
        assert!(!app.settings.bad_signature);
        assert!(app.settings.recovery_notice.is_none());
        assert!(
            app.settings.notice.is_none(),
            "recovery guidance twin cleared"
        );
        assert_eq!(app.tab, Tab::Board);
        assert_eq!(
            app.registration.as_ref().unwrap().key_id,
            "new-key",
            "the register response refreshes the registration record"
        );
    }

    // ------------------------------------------------------------------
    // Retained evidence infra (env-gated capture machinery).
    // ------------------------------------------------------------------

    #[test]
    fn native_probe_reason_classifies_fail_closed_fields() {
        let ready = NativeProbeFacts {
            probe_ok: true,
            exact_pid_match: true,
            process_visible: Some(true),
            window_visible: Some(true),
            frontmost: Some(true),
            key_window: Some(true),
            main_window: Some(true),
            cg_owner_pid_match: Some(true),
        };
        assert_eq!(
            classify_native_probe(ready),
            NativeProbeReason::DispatchReady
        );
        assert_eq!(
            classify_native_probe(NativeProbeFacts {
                process_visible: Some(false),
                ..ready
            }),
            NativeProbeReason::DeferProcessHidden
        );
        assert_eq!(
            classify_native_probe(NativeProbeFacts {
                frontmost: Some(false),
                ..ready
            }),
            NativeProbeReason::DeferNotFrontmost
        );
        assert_eq!(
            classify_native_probe(NativeProbeFacts {
                exact_pid_match: false,
                ..ready
            }),
            NativeProbeReason::DeferExactPidMismatch
        );
        assert_eq!(
            classify_native_probe(NativeProbeFacts {
                probe_ok: false,
                ..ready
            }),
            NativeProbeReason::DeferProbeFailed
        );
    }

    #[test]
    fn screenshot_state_defers_until_the_native_window_is_visible_and_frontmost() {
        let start = Instant::now();
        let waiting =
            ScreenshotCaptureState::initial(true, true, start, std::time::Duration::from_secs(2));
        let (ready, armed) = waiting.target_ready_after(start, std::time::Duration::ZERO);
        assert!(armed);
        assert!(matches!(ready, ScreenshotCaptureState::Ready));

        let (deferred, decision) = ready.try_dispatch(start, false, false);
        assert_eq!(decision, ScreenshotDispatch::DeferredForWindow);
        assert_eq!(
            deferred, ready,
            "visibility deferral must not consume an attempt"
        );
        assert_eq!(deferred.attempts(), 0);

        let (dispatched, decision) = deferred.try_dispatch(start, true, true);
        assert_eq!(decision, ScreenshotDispatch::Dispatched { attempt: 1 });
        assert!(matches!(
            dispatched,
            ScreenshotCaptureState::AwaitingScreenshot { .. }
        ));
    }

    #[test]
    fn screenshot_state_retries_after_the_deadline_and_exhausts_at_three() {
        let start = Instant::now();
        let (ready, _) =
            ScreenshotCaptureState::initial(true, true, start, std::time::Duration::from_secs(2))
                .target_ready_after(start, std::time::Duration::ZERO);
        let (first, first_decision) = ready.try_dispatch(start, true, true);
        assert_eq!(
            first_decision,
            ScreenshotDispatch::Dispatched { attempt: 1 }
        );

        let before_deadline = start + SCREENSHOT_RETRY_AFTER - std::time::Duration::from_millis(1);
        let (still_waiting, decision) = first.try_dispatch(before_deadline, true, true);
        assert_eq!(decision, ScreenshotDispatch::NotDue);
        assert_eq!(still_waiting, first);

        let (second, decision) = first.try_dispatch(start + SCREENSHOT_RETRY_AFTER, true, true);
        assert_eq!(decision, ScreenshotDispatch::Dispatched { attempt: 2 });
        let (third, decision) = second.try_dispatch(start + SCREENSHOT_RETRY_AFTER * 2, true, true);
        assert_eq!(decision, ScreenshotDispatch::Dispatched { attempt: 3 });
        let (exhausted, decision) =
            third.try_dispatch(start + SCREENSHOT_RETRY_AFTER * 3, true, true);
        assert_eq!(decision, ScreenshotDispatch::Exhausted);
        assert_eq!(exhausted, ScreenshotCaptureState::Exhausted);
    }

    #[test]
    fn screenshot_state_completes_only_after_a_valid_saved_png_event() {
        let start = Instant::now();
        let (ready, _) =
            ScreenshotCaptureState::initial(true, true, start, std::time::Duration::from_secs(2))
                .target_ready_after(start, std::time::Duration::ZERO);
        let (awaiting, decision) = ready.try_dispatch(start, true, true);
        assert_eq!(decision, ScreenshotDispatch::Dispatched { attempt: 1 });
        assert_eq!(
            awaiting.record_screenshot_event(false),
            awaiting,
            "an empty or failed save cannot complete capture"
        );
        assert_eq!(
            awaiting.record_screenshot_event(true),
            ScreenshotCaptureState::Complete
        );
    }

    #[test]
    fn native_window_wake_schedule_repeats_through_the_settle_interval() {
        let start = Instant::now();
        let mut schedule = NativeWindowWakeSchedule::default();
        assert!(!schedule.due(start));

        schedule.activate(start);
        let mut wake_count = 0;
        for second in 0..=12 {
            let now = start + std::time::Duration::from_secs(second);
            if schedule.due(now) {
                wake_count += 1;
                schedule.record_wake(now);
            }
        }
        assert_eq!(
            wake_count, 13,
            "activation must continue during the settle interval"
        );
        assert!(
            !schedule.due(start + SCREENSHOT_WAKE_MAX_DURATION),
            "wake activation must have a hard lifetime bound"
        );
        schedule.deactivate();
        assert!(!schedule.due(start + SCREENSHOT_WAKE_MAX_DURATION));
    }

    // ------------------------------------------------------------------
    // #354 L3: board/recents wiring guards.
    // ------------------------------------------------------------------

    /// Structural guard for the highest seam: the production `Tab::Board`
    /// arm in `workspace()` must keep the exact ordered wiring
    /// `hydrate_recent_output` -> `refresh_recent_output` for the SELECTED
    /// agent (recents v1 live tail), and the Settings arm must own the only
    /// settings_pane call. The slice is comment-stripped per line.
    #[test]
    fn board_arm_keeps_the_ordered_recents_hydration_then_refresh_wiring() {
        let source = include_str!("app.rs");
        let board_arm = source
            .split("Tab::Board => {")
            .nth(1)
            .expect("a Tab::Board arm exists in workspace()")
            .split("Tab::Settings => {")
            .next()
            .expect("the Board arm is bounded by the Settings arm")
            .to_string();
        let code_lines: Vec<String> = board_arm
            .lines()
            .map(|line| {
                let code = match line.find("//") {
                    Some(idx) => &line[..idx],
                    None => line,
                };
                code.trim().to_string()
            })
            .filter(|line| !line.is_empty())
            .collect();
        let count_matches = |needle: &str| {
            code_lines
                .iter()
                .filter(|line| line.contains(needle))
                .count()
        };
        let hydrate = "app.hydrate_recent_output(Some(&agent_id))";
        let refresh = "app.refresh_recent_output(Some(&agent_id))";
        assert!(
            count_matches(hydrate) >= 1,
            "the Board arm must hydrate the selected agent's recents tail"
        );
        assert_eq!(
            count_matches(refresh),
            1,
            "exactly one (live, uncommented) refresh call in the Board arm"
        );
        assert!(
            code_lines
                .iter()
                .position(|line| line.contains(hydrate))
                .expect("hydration call present")
                < code_lines
                    .iter()
                    .position(|line| line.contains(refresh))
                    .expect("refresh call present"),
            "initial hydration must precede the revision-aware refresh"
        );
    }
}
