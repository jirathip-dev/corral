//! The eframe application: owns the fleet state, the background read
//! loop (SSE), the signed-drive dispatch, registration, and the four
//! workspace tabs (Board / Issues / Registry / Settings).

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
use crate::protocol::{self, ApplyMsg, GrantMutationMsg};
use crate::state::{
    AuditMsg, ConnState, DriveMsg, Fleet, GrantLedger, Level, RegistrationRecord, Toast,
};
use crate::theme;

/// The four top-level views in the persistent right-hand tab strip. Audit is
/// intentionally not a top-level destination; it is rendered below Settings
/// → Advanced device access when explicitly opened.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tab {
    Board,
    Issues,
    Registry,
    Settings,
}

const TAB_LABELS: [(&str, Tab); 4] = [
    ("Board", Tab::Board),
    ("Issues", Tab::Issues),
    ("Fleets", Tab::Registry),
    ("Settings", Tab::Settings),
];
const ISSUES_REFRESH_INTERVAL: std::time::Duration = std::time::Duration::from_secs(5);

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
        "issues" => Tab::Issues,
        "registry" => Tab::Registry,
        "settings" => Tab::Settings,
        _ => Tab::Board,
    }
}

/// Runtime-loaded + persisted app config (host URL, registration record).
#[derive(Debug, Clone, PartialEq)]
struct PersistedConfig {
    host_url: String,
    registration: Option<RegistrationRecord>,
    auto_reconnect: bool,
    group_by_repo: bool,
    show_idle_collapsed: bool,
    stick_to_bottom: bool,
    theme: String,
}

impl Default for PersistedConfig {
    fn default() -> Self {
        Self {
            host_url: protocol::DEFAULT_HOST_URL.to_string(),
            registration: None,
            auto_reconnect: true,
            group_by_repo: true,
            show_idle_collapsed: true,
            stick_to_bottom: true,
            theme: "dark".to_string(),
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
                group_by_repo: c.group_by_repo.unwrap_or(true),
                show_idle_collapsed: c.show_idle_collapsed.unwrap_or(true),
                stick_to_bottom: c.stick_to_bottom.unwrap_or(true),
                theme: c
                    .theme
                    .filter(|theme| theme == "dark")
                    .unwrap_or_else(|| "dark".to_string()),
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
            group_by_repo: Some(self.group_by_repo),
            show_idle_collapsed: Some(self.show_idle_collapsed),
            stick_to_bottom: Some(self.stick_to_bottom),
            theme: Some(self.theme.clone()),
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
    /// Monotonic identity epoch for the issue and registry read models.
    /// Results from an earlier connection or host are never allowed to fold
    /// into the current epoch.
    read_generation: u64,
    /// Generation of the currently spawned SSE/read loop. This is separate
    /// from `read_generation`: one loop can establish several connections.
    read_loop_generation: u64,

    // Audit view.
    audit: Option<Result<crate::protocol::AuditView, String>>,
    audit_loading: bool,
    issues_last_refresh: std::time::Instant,
    fleets_last_refresh: std::time::Instant,

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
        let (tx_audit, rx_audit) = tokio::sync::mpsc::unbounded_channel();
        let (tx_transcript, rx_transcript) = tokio::sync::mpsc::unbounded_channel();
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
            ledger: GrantLedger::default(),
            registration: config.registration.clone(),
            host_fingerprint: None,
            config: config.clone(),
            config_path,
            settings: crate::ui::register::SettingsState {
                host_url: host_url.clone(),
                auto_reconnect: config.auto_reconnect,
                group_by_repo: config.group_by_repo,
                show_idle_collapsed: config.show_idle_collapsed,
                stick_to_bottom: config.stick_to_bottom,
                theme: config.theme.clone(),
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
            read_generation: 0,
            read_loop_generation: 0,
            audit: None,
            audit_loading: false,
            // Permit the first Issues visit to fetch immediately; every
            // subsequent attempt records its start time, including errors.
            issues_last_refresh: std::time::Instant::now() - ISSUES_REFRESH_INTERVAL,
            fleets_last_refresh: std::time::Instant::now() - ISSUES_REFRESH_INTERVAL,
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
        self.read_loop_generation = generation;
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

    /// Start a new identity epoch for the read-only issue/fleets views.
    ///
    /// The old cache is deliberately removed before a reconnect or host
    /// switch starts new requests: a fresh `/issues` response must never be
    /// actionable through a previous catalog, and an old in-flight response
    /// must not clear the loading flag for the replacement request.
    fn invalidate_read_model(&mut self) -> u64 {
        self.read_generation = self
            .read_generation
            .checked_add(1)
            .expect("read model generation exhausted");
        self.fleet.issues.clear();
        self.fleet.issues_loaded = false;
        self.fleet.issues_loading = false;
        self.fleet.issues_error = None;
        self.fleet.fleets = None;
        self.fleet.fleets_loading = false;
        self.fleet.fleets_loaded = false;
        self.read_generation
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
                        self.invalidate_read_model();
                        self.conn = ConnState::Connected;
                        self.conn_detail = None;
                        // #113/#135: fetch both read-only views on connection.
                        // The invalidation above makes the current generation
                        // non-actionable until both read models catch up.
                        self.refresh_issues(true);
                        self.refresh_fleets(true);
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
            ApplyMsg::Issues { generation, result } => {
                if generation != self.read_generation {
                    tracing::debug!(
                        generation,
                        current = self.read_generation,
                        "ignored issue response from an obsolete identity generation"
                    );
                    return;
                }
                self.fleet.set_issues(result);
            }
            ApplyMsg::FleetIdentities { generation, result } => {
                if generation != self.read_generation {
                    tracing::debug!(
                        generation,
                        current = self.read_generation,
                        "ignored fleet-identities response from an obsolete identity generation"
                    );
                    return;
                }
                self.fleet.set_fleets(result);
            }
            ApplyMsg::GrantDevices(result) => self.handle_grant_devices(result),
            ApplyMsg::GrantMutation(msg) => self.handle_grant_mutation(msg),
        }
    }

    /// #113: fetch the daemon's read-only repo-level issue view on connect,
    /// manual refresh, and while the Issues tab is visible. A previous
    /// successful snapshot remains rendered while a retry is in flight; a
    /// transient failure is never converted into a permanent empty cache.
    fn refresh_issues(&mut self, force: bool) {
        if !issues_refresh_due(
            force,
            self.conn,
            self.fleet.issues_loading,
            self.issues_last_refresh,
        ) {
            return;
        }
        self.fleet.issues_loading = true;
        self.issues_last_refresh = std::time::Instant::now();
        let generation = self.read_generation;
        let client = self.client.clone();
        let base_url = self.config.host_url.clone();
        let tx = self.tx_apply.clone();
        self.rt.spawn(async move {
            let result = protocol::fetch_issues(&client, &base_url).await;
            if let Err(error) = &result {
                tracing::warn!(error, "GET /issues unavailable");
            }
            let _ = tx.send(ApplyMsg::Issues { generation, result });
        });
    }

    /// Hydrate the resolved visible Cards detail pane once the live snapshot
    /// and the persisted device grant are both ready. The board owns the
    /// visible/attention-ranked resolver; this method consumes that result,
    /// never selects a fallback and never writes `selected_agent`.
    ///
    /// Recent output is a composed surface: the bounded read_tail result is
    /// the immediate fallback while the signed transcript page supplies chat
    /// roles and the older cursor. Both requests share the same UI-owned
    /// caches, so either payload can make the surface useful without waiting
    /// for the other endpoint.
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

        self.request_transcript_page(crate::transcript::TranscriptRequest {
            agent_id: agent_id.clone(),
            cursor: None,
        });
        self.dispatch_drive_intents(vec![DriveIntent::read_tail(&agent_id, self.fleet.rev)]);
    }

    /// #237: fetch the daemon's fleet-ops CLI validated identity catalog
    /// once per connection, on manual refresh, and while the Fleets tab is
    /// visible. A previous successful catalog remains rendered while a retry
    /// is in flight; a transient failure is never converted into "no fleets".
    fn refresh_fleets(&mut self, force: bool) {
        if !issues_refresh_due(
            force,
            self.conn,
            self.fleet.fleets_loading,
            self.fleets_last_refresh,
        ) {
            return;
        }
        self.fleet.fleets_loading = true;
        self.fleets_last_refresh = std::time::Instant::now();
        let generation = self.read_generation;
        let client = self.client.clone();
        let base_url = self.config.host_url.clone();
        let tx = self.tx_apply.clone();
        self.rt.spawn(async move {
            let result = protocol::fetch_fleet_identities(&client, &base_url).await;
            if let Err(error) = &result {
                tracing::warn!(error, "GET /fleets unavailable");
            }
            let _ = tx.send(ApplyMsg::FleetIdentities { generation, result });
        });
    }

    /// #237: the Fleets tab is READ-ONLY — the only deferred action is a
    /// refresh. Mutations run through `herdr-fleet` on the host (corrald is
    /// configless; no fleets.json write path exists in the client).
    fn fleets_action(&mut self, action: crate::ui::registry::Action) {
        match action {
            crate::ui::registry::Action::Refresh => self.refresh_fleets(true),
        }
    }

    fn on_drive(&mut self, msg: DriveMsg) {
        let capability = msg.capability.clone();
        let state = crate::ui::board::classify_drive_state(&msg.outcome, &msg.capability);
        self.fleet.remember_drive(&msg.agent_id, state.clone());
        match &msg.outcome {
            DriveOutcome::Ok { rev, result } => {
                self.ledger.note_success(&capability);
                self.persist_ledger();
                // If the daemon ever grows a read_tail result, surface it.
                if capability == "read_tail" {
                    self.remember_tail_result(&msg);
                }
                let text = if capability == "start_worktree" {
                    let wt_state = result
                        .as_ref()
                        .and_then(|v| v.get("state"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("ok");
                    let branch = result
                        .as_ref()
                        .and_then(|v| v.get("branch"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    format!("start_worktree → {wt_state} (rev {rev}) {branch}")
                } else {
                    format!("{capability} → ok (rev {rev})")
                };
                self.toast(Level::Info, text);
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
        let outcome = self.fleet.fold_transcript(msg);
        let entries = self
            .fleet
            .transcripts
            .get(&agent_id)
            .map_or(0, |pane| pane.entries.len());
        let has_error = self
            .fleet
            .transcripts
            .get(&agent_id)
            .is_some_and(|pane| pane.error.is_some());
        tracing::info!(
            agent_id = %agent_id,
            ?outcome,
            entries,
            has_error,
            "transcript result folded into detail cache"
        );
        match outcome {
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
        tracing::info!(
            agent_id = %msg.agent_id,
            lines = lines.len(),
            "read_tail result applied to screenshot/detail cache"
        );
        fleet.remember_tail(&msg.agent_id, lines);
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
            "native screenshot evidence selected live agent; Cards hydration remains grant-gated"
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

    /// The admin token for host-side administration: an explicitly entered
    /// value wins for this session, otherwise the host's own token on
    /// localhost or the keychain-stored one.
    fn admin_token(&self) -> Option<String> {
        let entered = self.settings.admin_token_input.trim();
        if !entered.is_empty() {
            return Some(entered.to_string());
        }
        let fp = self.host_fingerprint.clone()?;
        crate::keys::load_admin_token(&fp).or_else(crate::keys::read_daemon_admin_token)
    }

    fn refresh_audit(&mut self) {
        let Some(token) = self.admin_token() else {
            self.audit = Some(Err(
                "no admin token available (Settings → Advanced device access → audit) — the log is host-admin".into(),
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

    fn refresh_grant_devices(&mut self) {
        if self.settings.grant_admin.loading || self.settings.grant_admin.saving {
            return;
        }
        let Some(token) = self.admin_token() else {
            self.settings.grant_admin.notice = Some((
                Level::Error,
                "no admin token available — save/paste it above before managing grants".into(),
            ));
            return;
        };
        self.settings.grant_admin.loading = true;
        self.settings.grant_admin.notice = None;
        let client = self.client.clone();
        let host_url = self.config.host_url.clone();
        let tx = self.tx_apply.clone();
        self.rt.spawn(async move {
            let view = protocol::fetch_admin_grants(&client, &host_url, &token, None).await;
            let _ = tx.send(ApplyMsg::GrantDevices(view));
        });
    }

    fn select_grant_device(&mut self, key_id: String) {
        let device = self
            .settings
            .grant_admin
            .view
            .as_ref()
            .and_then(|view| view.as_ref().ok())
            .and_then(|devices| devices.iter().find(|d| d.key_id == key_id))
            .cloned();
        match device {
            Some(device) => {
                self.settings.grant_admin.draft =
                    crate::ui::register::GrantDraft::for_device(&device);
                self.settings.grant_admin.notice = None;
            }
            None => {
                self.settings.grant_admin.draft.selected_key = key_id.clone();
                self.settings.grant_admin.notice = Some((
                    Level::Warn,
                    format!("{key_id} is not in the loaded device list — refresh"),
                ));
            }
        }
    }

    fn apply_grant_set(&mut self) {
        let key_id = self.settings.grant_admin.draft.selected_key.clone();
        if key_id.is_empty() {
            self.settings.grant_admin.notice = Some((
                Level::Error,
                "select a registered device key before applying grants".into(),
            ));
            return;
        }
        let grants = self.settings.grant_admin.draft.granted();
        if self.settings.grant_admin.loading || self.settings.grant_admin.saving {
            return;
        }
        let Some(token) = self.admin_token() else {
            self.settings.grant_admin.notice = Some((
                Level::Error,
                "no admin token available — save/paste it above before applying grants".into(),
            ));
            return;
        };
        self.settings.grant_admin.saving = true;
        self.settings.grant_admin.notice = None;
        let client = self.client.clone();
        let host_url = self.config.host_url.clone();
        let tx = self.tx_apply.clone();
        let result_key = key_id.clone();
        self.rt.spawn(async move {
            let result =
                protocol::set_admin_grants(&client, &host_url, &token, &result_key, &grants)
                    .await
                    .map(|_| ());
            let _ = tx.send(ApplyMsg::GrantMutation(GrantMutationMsg {
                key_id: result_key,
                grants,
                revoke: false,
                result,
            }));
        });
    }

    fn revoke_grant_device(&mut self) {
        let key_id = self.settings.grant_admin.draft.selected_key.clone();
        if key_id.is_empty() {
            self.settings.grant_admin.notice = Some((
                Level::Error,
                "select a registered device key before revoking".into(),
            ));
            return;
        }
        if self.settings.grant_admin.loading || self.settings.grant_admin.saving {
            return;
        }
        let Some(token) = self.admin_token() else {
            self.settings.grant_admin.notice = Some((
                Level::Error,
                "no admin token available — save/paste it above before revoking".into(),
            ));
            return;
        };
        self.settings.grant_admin.saving = true;
        self.settings.grant_admin.notice = None;
        let client = self.client.clone();
        let host_url = self.config.host_url.clone();
        let tx = self.tx_apply.clone();
        let result_key = key_id.clone();
        self.rt.spawn(async move {
            let result = protocol::revoke_admin_device(&client, &host_url, &token, &result_key)
                .await
                .map(|_| ());
            let _ = tx.send(ApplyMsg::GrantMutation(GrantMutationMsg {
                key_id: result_key,
                grants: Vec::new(),
                revoke: true,
                result,
            }));
        });
    }

    fn handle_grant_devices(&mut self, result: Result<protocol::AdminGrantsView, String>) {
        match result {
            Ok(view) if view.ok => {
                let own = self
                    .registration
                    .as_ref()
                    .map(|r| r.key_id.clone())
                    .unwrap_or_default();
                self.settings.grant_admin.set_view(view.devices, &own);
            }
            Ok(_) => {
                self.settings
                    .grant_admin
                    .set_error("GET /grants returned ok=false with a device list".to_string());
                self.settings.grant_admin.notice = Some((
                    Level::Error,
                    "grants view malformed: daemon returned ok=false".into(),
                ));
            }
            Err(error) => {
                self.settings.grant_admin.set_error(error.clone());
                self.settings.grant_admin.notice = Some((Level::Error, error));
            }
        }
    }

    fn handle_grant_mutation(&mut self, msg: GrantMutationMsg) {
        self.settings.grant_admin.saving = false;
        match msg.result {
            Ok(()) => {
                self.sync_own_grants(&msg.key_id, &msg.grants, msg.revoke);
                if msg.revoke {
                    self.toast(Level::Info, format!("revoked device {}", msg.key_id));
                } else {
                    self.toast(
                        Level::Info,
                        format!(
                            "updated grants for {}: {}",
                            msg.key_id,
                            if msg.grants.is_empty() {
                                "read-only".to_string()
                            } else {
                                msg.grants.join(", ")
                            }
                        ),
                    );
                }
                self.refresh_grant_devices();
            }
            Err(error) => {
                self.settings.grant_admin.notice =
                    Some((Level::Error, format!("grant update failed: {error}")));
            }
        }
    }

    /// Keep the board's local ledger honest when the selected managed device
    /// is this board's own registered key.
    fn sync_own_grants(&mut self, key_id: &str, grants: &[String], revoke: bool) {
        if let Some(reg) = &mut self.registration
            && reg.key_id == key_id
        {
            let effective = if revoke { Vec::new() } else { grants.to_vec() };
            reg.grants = effective.clone();
            reg.denied.clear();
            self.ledger = GrantLedger {
                base: effective,
                denied: Vec::new(),
            };
            self.config.registration = self.registration.clone();
            self.config.persist(&self.config_path);
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
            crate::ui::register::Request::LoadGrantDevices => self.refresh_grant_devices(),
            crate::ui::register::Request::SelectGrantDevice(key_id) => {
                self.select_grant_device(key_id);
            }
            crate::ui::register::Request::ApplyGrantSet => self.apply_grant_set(),
            crate::ui::register::Request::RevokeGrantDevice => self.revoke_grant_device(),
            crate::ui::register::Request::OpenAudit => {
                self.settings.audit_open = true;
                self.refresh_audit();
            }
            crate::ui::register::Request::CloseAudit => {
                self.settings.audit_open = false;
            }
            crate::ui::register::Request::RefreshAudit => self.refresh_audit(),
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
                self.config.group_by_repo = self.settings.group_by_repo;
                self.config.show_idle_collapsed = self.settings.show_idle_collapsed;
                self.config.stick_to_bottom = self.settings.stick_to_bottom;
                // The approved #206 surface currently ships one theme. Keep
                // the persisted compatibility key truthful instead of
                // exposing a selector whose alternate visuals do not exist.
                self.settings.theme = "dark".to_string();
                self.config.theme = "dark".to_string();
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

    /// Dispatch drive intents collected by Board or Issues after their
    /// immediate-mode render returns, so no frame holds overlapping
    /// borrows of the fleet/toast state while a network call is spawned.
    fn dispatch_drive_intents(&mut self, pending: Vec<DriveIntent>) {
        let registration = self.registration.clone();
        let signing = self.device_key.as_ref().map(|k| k.signing.clone());
        let client = self.client.clone();
        let host_url = self.config.host_url.clone();
        let tx_drive = self.tx_drive.clone();
        let rt = self.rt.clone();
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

        // Esc collapses expanded rows.
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.fleet.expanded.clear();
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
                let (state, decision) = self.screenshot_state.try_dispatch(
                    now,
                    window.as_ref().is_some_and(|state| state.visible),
                    window.as_ref().is_some_and(|state| state.frontmost),
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

    let mut left_ui = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(left_rect.shrink(1.0))
            .id(egui::Id::new("corral-ui-persistent-master-bar"))
            .layout(egui::Layout::top_down(egui::Align::Min)),
    );
    let resolved_selection = crate::ui::board::show_master(
        &mut left_ui,
        &mut app.fleet,
        app.settings.group_by_repo,
        app.settings.show_idle_collapsed,
    );

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
            let ledger = app.ledger.clone();
            let allowed = |cap: &str| ledger.allowed(cap);
            let mut pending: Vec<DriveIntent> = Vec::new();
            let mut pending_transcripts: Vec<crate::transcript::TranscriptRequest> = Vec::new();
            let mut pending_full_chat: Vec<String> = Vec::new();
            crate::ui::board::show_board_detail_with_options(
                &mut right_ui,
                &app.fleet,
                resolved_selection.as_deref(),
                &allowed,
                &mut crate::ui::board::BoardActions {
                    drive: &mut |intent| pending.push(intent),
                    transcript: &mut |request| pending_transcripts.push(request),
                    full_chat: &mut |agent_id| pending_full_chat.push(agent_id.to_string()),
                },
                app.settings.stick_to_bottom,
            );
            for agent_id in pending_full_chat {
                app.fleet.toggle_full_chat(&agent_id);
            }
            for request in pending_transcripts {
                app.request_transcript_page(request);
            }
            app.dispatch_drive_intents(pending);
            app.hydrate_recent_output(resolved_selection.as_deref());
        }
        Tab::Issues => {
            app.refresh_issues(false);
            if app.fleet.fleets.is_none() && app.conn == ConnState::Connected {
                // The Issues tab can win the first frame after connection
                // while the fleet-identities request is still racing the issue
                // request. The loading guard in refresh_registry prevents a
                // second request; a later manual Issues refresh retries a
                // failed projection as well.
                app.refresh_fleets(false);
            }
            let ledger = app.ledger.clone();
            let allowed = |cap: &str| ledger.allowed(cap);
            let mut pending: Vec<DriveIntent> = Vec::new();
            let mut refresh_requested = false;
            crate::ui::issues::show(
                &mut right_ui,
                &app.fleet,
                &allowed,
                &mut |intent| pending.push(intent),
                &mut || refresh_requested = true,
            );
            if refresh_requested {
                app.refresh_issues(true);
                if !app.fleet.fleets_ready() {
                    // A failed catalog must be recoverable
                    // from the surface that needs it. Force only here, after
                    // the user explicitly refreshed Issues; an in-flight
                    // request is still protected by refresh_registry's guard.
                    app.refresh_fleets(true);
                }
            }
            app.dispatch_drive_intents(pending);
        }
        Tab::Registry => {
            if app.fleet.fleets.is_none() && app.conn == ConnState::Connected {
                app.refresh_fleets(false);
            }
            let mut pending = None;
            crate::ui::registry::show(
                &mut right_ui,
                &app.fleet.fleets,
                app.fleet.fleets_loading,
                &mut |action| pending = Some(action),
            );
            if let Some(action) = pending {
                app.fleets_action(action);
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
            let admin_token_configured = app.admin_token().is_some();
            app.settings.admin_token_configured = admin_token_configured;
            if app.settings.grant_admin.view.is_none()
                && !app.settings.grant_admin.loading
                && !app.settings.grant_admin.saving
                && admin_token_configured
            {
                app.refresh_grant_devices();
            }
            crate::ui::register::settings_pane(
                &mut right_ui,
                &mut app.settings,
                crate::ui::register::SettingsPaneContext {
                    key_id: &key_id,
                    grants: &grants,
                    store: store.as_ref(),
                    conn: app.conn,
                    rev: app.fleet.rev,
                    audit: &app.audit,
                    audit_loading: app.audit_loading,
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

/// Return whether the app may start a non-forced Issues fetch. The timestamp
/// is recorded when every fetch starts, not only after a successful result,
/// so a folded transport error cannot turn the next frame into a retry loop.
fn issues_refresh_due(
    force: bool,
    conn: ConnState,
    loading: bool,
    last_refresh: std::time::Instant,
) -> bool {
    if loading || (!force && conn != ConnState::Connected) {
        return false;
    }
    force || last_refresh.elapsed() >= ISSUES_REFRESH_INTERVAL
}

/// Select the only agent eligible for automatic Recent-output hydration.
/// `resolved_selection` must come from the Cards surface's visible resolver;
/// no map-order fallback belongs here, and this helper deliberately never
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
    use std::collections::BTreeMap;
    use std::time::Instant;

    use super::*;
    use crate::model::{
        Agent, AgentState, FleetIdentities, FleetIdentityEntry, GhIssueRef, Workspace,
    };
    use crate::ui::board::{self, StateFilter};

    fn agent(id: &str, state: AgentState, capabilities: &[&str]) -> Agent {
        Agent {
            agent_id: id.into(),
            source: "herdr".into(),
            tool: "codex".into(),
            state,
            reason: None,
            seq: 1,
            ts: 1,
            capabilities: capabilities.iter().map(|cap| (*cap).into()).collect(),
            waiting_on: None,
            parent_id: None,
            host: None,
            workspace: Workspace::default(),
            attachment: None,
            display_name: None,
            title: None,
            issues: vec![],
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
        let (tx_audit, rx_audit) = tokio::sync::mpsc::unbounded_channel();
        let (tx_transcript, rx_transcript) = tokio::sync::mpsc::unbounded_channel();
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
            config,
            config_path: PathBuf::from("/tmp/corral-ui-read-model-test.json"),
            settings,
            tx_apply: tx_apply.clone(),
            rx_apply,
            rx_drive,
            tx_drive,
            tx_audit: tx_audit.clone(),
            rx_audit,
            tx_transcript,
            rx_transcript,
            stop_read: None,
            read_generation: 100,
            read_loop_generation: 7,
            audit: None,
            audit_loading: false,
            issues_last_refresh: Instant::now() - ISSUES_REFRESH_INTERVAL,
            fleets_last_refresh: Instant::now() - ISSUES_REFRESH_INTERVAL,
            tab: Tab::Issues,
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
            window_diagnostic_last_sample: None,
            evidence_visibility_requested: false,
            native_probe_tx,
            native_probe_rx,
            native_probe_in_flight: false,
        };
        (runtime, app)
    }

    fn test_fleet_identities(name: &str, gh_repo: &str) -> FleetIdentities {
        FleetIdentities {
            status: "ok".into(),
            error: None,
            fleets: vec![FleetIdentityEntry {
                name: name.into(),
                gh_repo: gh_repo.into(),
                orch: "orch".into(),
                workers: 0,
                paused: false,
            }],
        }
    }

    fn test_issue() -> GhIssueRef {
        GhIssueRef {
            repo: "foo".into(),
            number: 42,
            state: "OPEN".into(),
            title: "renamed fleet".into(),
            labels: vec![],
            url: "https://github.com/example/foo/issues/42".into(),
        }
    }

    #[test]
    fn workspace_navigation_has_exactly_four_tabs_and_demotes_audit() {
        let labels: Vec<&str> = TAB_LABELS.iter().map(|(label, _)| *label).collect();
        assert_eq!(labels, ["Board", "Issues", "Fleets", "Settings"]);
        assert!(!labels.contains(&"Audit"));
    }

    #[test]
    fn live_connected_orders_issue_and_registry_refreshes_by_identity_generation() {
        let (_runtime, mut app) = read_model_test_app();
        app.fleet
            .set_issues(Ok(BTreeMap::from([("foo".into(), vec![test_issue()])])));
        app.fleet
            .set_fleets(Ok(test_fleet_identities("alpha", "owner/foo")));
        assert!(app.fleet.fleets_ready());
        app.fleet.fleets_loading = true;
        app.fleet.issues_loading = true;
        let stale_generation = app.read_generation;
        let loop_generation = app.read_loop_generation;

        // Exercise the real application boundary: a live reconnect must
        // invalidate the old identity before it starts either read request.
        app.on_apply(ApplyMsg::Conn {
            loop_generation,
            event: protocol::Live::Connected,
        });
        let current_generation = app.read_generation;
        assert_ne!(current_generation, stale_generation);
        assert!(app.fleet.fleets.is_none());
        assert!(app.fleet.issues.is_empty());
        assert!(app.fleet.fleets_loading);
        assert!(app.fleet.issues_loading);

        // `/issues` can arrive first, but the old alpha registry is no longer
        // allowed to turn that category into an action.
        app.on_apply(ApplyMsg::Issues {
            generation: current_generation,
            result: Ok(BTreeMap::from([("foo".into(), vec![test_issue()])])),
        });
        let ctx = egui::Context::default();
        ctx.memory_mut(|memory| {
            memory.data.insert_temp(
                egui::Id::new("corral-ui-issues-selected"),
                Some(("foo".to_string(), "unregistered:foo".to_string(), 42_u64)),
            );
        });
        let intents = std::cell::RefCell::new(Vec::new());
        let mut frame = render_issues(&ctx, &app.fleet, issue_input(vec![]), &intents);
        let disabled = text_rect(&frame, "start worktree")
            .expect("the current issue remains visibly disabled during the gap")
            .center();
        frame.textures_delta.clear();
        for pressed in [true, false] {
            let mut attempted = render_issues(
                &ctx,
                &app.fleet,
                issue_pointer_input(disabled, pressed),
                &intents,
            );
            attempted.textures_delta.clear();
        }
        let mut frame = render_issues(&ctx, &app.fleet, issue_input(vec![]), &intents);
        assert!(text_rect(&frame, "✓ confirm create").is_none());
        frame.textures_delta.clear();
        assert!(intents.borrow().is_empty());

        app.on_apply(ApplyMsg::Issues {
            generation: stale_generation,
            result: Ok(BTreeMap::from([("alpha".into(), vec![test_issue()])])),
        });
        let issue_keys: Vec<String> = app.fleet.issues.keys().cloned().collect();
        assert_eq!(issue_keys, ["foo"]);
        assert!(!app.fleet.issues_loading);

        // A response from the pre-reconnect request cannot replace the
        // current empty/loading registry or clear its loading flag.
        app.on_apply(ApplyMsg::FleetIdentities {
            generation: stale_generation,
            result: Ok(test_fleet_identities("alpha", "owner/foo")),
        });
        assert!(app.fleet.fleets.is_none());
        assert!(app.fleet.fleets_loading);

        // The current response restores the exact current fleet action.
        app.on_apply(ApplyMsg::FleetIdentities {
            generation: current_generation,
            result: Ok(test_fleet_identities("foo", "owner/foo")),
        });
        assert_eq!(
            app.fleet
                .fleets
                .as_ref()
                .and_then(|result| result.as_ref().ok())
                .and_then(|fleets| fleets.fleets.first())
                .map(|entry| entry.name.as_str()),
            Some("foo")
        );
        ctx.memory_mut(|memory| {
            memory.data.insert_temp(
                egui::Id::new("corral-ui-issues-selected"),
                Some(("foo".to_string(), "owner/foo".to_string(), 42_u64)),
            );
        });
        let mut frame = render_issues(&ctx, &app.fleet, issue_input(vec![]), &intents);
        let start = text_rect(&frame, "start worktree")
            .expect("the current exact fleet action is enabled")
            .center();
        frame.textures_delta.clear();
        for pressed in [true, false] {
            let mut attempted = render_issues(
                &ctx,
                &app.fleet,
                issue_pointer_input(start, pressed),
                &intents,
            );
            attempted.textures_delta.clear();
        }
        let mut frame = render_issues(&ctx, &app.fleet, issue_input(vec![]), &intents);
        let confirm = text_rect(&frame, "✓ confirm create")
            .expect("the current fleet action asks for confirmation")
            .center();
        frame.textures_delta.clear();
        for pressed in [true, false] {
            let mut attempted = render_issues(
                &ctx,
                &app.fleet,
                issue_pointer_input(confirm, pressed),
                &intents,
            );
            attempted.textures_delta.clear();
        }
        assert_eq!(intents.borrow().len(), 1);
        assert_eq!(intents.borrow()[0].target, "foo");

        // Even after the current response, a late old response cannot restore
        // alpha or overwrite the current exact fleet.
        app.on_apply(ApplyMsg::FleetIdentities {
            generation: stale_generation,
            result: Ok(test_fleet_identities("alpha", "owner/foo")),
        });
        assert_eq!(
            app.fleet
                .fleets
                .as_ref()
                .and_then(|result| result.as_ref().ok())
                .and_then(|fleets| fleets.fleets.first())
                .map(|entry| entry.name.as_str()),
            Some("foo")
        );
    }

    #[test]
    fn obsolete_sse_events_cannot_overwrite_state_after_read_loop_replacement() {
        let (_runtime, mut app) = read_model_test_app();

        app.spawn_read_loop("http://127.0.0.1:1".into());
        let old_loop_generation = app.read_loop_generation;
        let old_agent = agent("old-agent", AgentState::Working, &[]);
        app.on_apply(ApplyMsg::Sse {
            loop_generation: old_loop_generation,
            event: protocol::SseEvent::Snapshot(crate::model::Snapshot {
                schema_version: 5,
                rev: 10,
                generated_at: 10,
                agents: BTreeMap::from([(old_agent.agent_id.clone(), old_agent)]),
            }),
        });
        assert_eq!(app.fleet.rev, Some(10));
        assert!(app.fleet.agents.contains_key("old-agent"));

        app.spawn_read_loop("http://127.0.0.1:2".into());
        let current_loop_generation = app.read_loop_generation;
        assert_ne!(current_loop_generation, old_loop_generation);

        let obsolete_agent = agent("obsolete-agent", AgentState::Blocked, &[]);
        app.on_apply(ApplyMsg::Sse {
            loop_generation: old_loop_generation,
            event: protocol::SseEvent::Snapshot(crate::model::Snapshot {
                schema_version: 5,
                rev: 99,
                generated_at: 99,
                agents: BTreeMap::from([(obsolete_agent.agent_id.clone(), obsolete_agent)]),
            }),
        });
        app.on_apply(ApplyMsg::Sse {
            loop_generation: old_loop_generation,
            event: protocol::SseEvent::Delta(crate::model::Delta {
                rev: 100,
                upd: vec![agent("obsolete-delta", AgentState::Working, &[])],
                del: vec![],
            }),
        });
        assert_eq!(app.fleet.rev, Some(10));
        assert!(app.fleet.agents.contains_key("old-agent"));
        assert!(!app.fleet.agents.contains_key("obsolete-agent"));
        assert!(!app.fleet.agents.contains_key("obsolete-delta"));

        let current_agent = agent("current-agent", AgentState::Idle, &[]);
        app.on_apply(ApplyMsg::Sse {
            loop_generation: current_loop_generation,
            event: protocol::SseEvent::Delta(crate::model::Delta {
                rev: 11,
                upd: vec![current_agent],
                del: vec![],
            }),
        });
        assert_eq!(app.fleet.rev, Some(11));
        assert!(app.fleet.agents.contains_key("old-agent"));
        assert!(app.fleet.agents.contains_key("current-agent"));
    }

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
                window_visible: Some(false),
                ..ready
            }),
            NativeProbeReason::DeferWindowHidden
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
                key_window: Some(false),
                ..ready
            }),
            NativeProbeReason::DeferNotFrontmost
        );
        assert_eq!(
            classify_native_probe(NativeProbeFacts {
                main_window: None,
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
        assert_eq!(
            classify_native_probe(NativeProbeFacts {
                cg_owner_pid_match: None,
                ..ready
            }),
            NativeProbeReason::DeferCgWindowMissing
        );
    }

    #[test]
    fn native_frontmost_gate_requires_key_and_main_window() {
        let observation = NativeProbeObservation {
            process_pid: Some(42),
            process_visible: Some(true),
            window_visible: Some(true),
            frontmost_observed: Some(true),
            key_window: Some(false),
            main_window: Some(true),
            frontmost_application_pid: Some(42),
            frontmost_application_matches_target: Some(true),
            exact_pid_match: true,
            cg_owner_pid_match: Some(true),
        };
        let state = NativeWindowState::from_facts(42, observation, vec![], None, true, None, None);
        assert!(state.visible);
        assert!(!state.frontmost);
        assert_eq!(state.reason_code, "defer_not_frontmost_or_unknown");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn native_probe_helper_timeout_reaps_a_hung_grandchild() {
        let dir = std::env::temp_dir().join(format!(
            "corrald-ui-native-probe-timeout-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("test clock is after the Unix epoch")
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let helper = dir.join("probe.sh");
        std::fs::write(&helper, b"#!/bin/sh\nsleep 30 &\nwait\n").unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&helper, std::fs::Permissions::from_mode(0o700)).unwrap();

        let started = Instant::now();
        let terminations = Arc::new(std::sync::Mutex::new(Vec::new()));
        let terminations_for_call = Arc::clone(&terminations);
        let error = run_native_probe_helper_with_timeout_using(
            &helper,
            42,
            std::time::Duration::from_millis(150),
            move |pid| {
                terminations_for_call.lock().unwrap().push(pid);
                terminate_native_probe_process_group(pid);
            },
        )
        .expect_err("a hung helper must fail closed");

        assert!(error.contains("timed out after 150ms"));
        assert_eq!(terminations.lock().unwrap().len(), 1);
        assert!(
            started.elapsed() < std::time::Duration::from_secs(2),
            "probe timeout must include descendant cleanup"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn native_probe_helper_success_does_not_terminate_reaped_child() {
        let dir = std::env::temp_dir().join(format!(
            "corrald-ui-native-probe-success-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("test clock is after the Unix epoch")
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let helper = dir.join("probe.sh");
        std::fs::write(&helper, b"#!/bin/sh\nprintf success\n").unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&helper, std::fs::Permissions::from_mode(0o700)).unwrap();

        let terminations = Arc::new(std::sync::Mutex::new(Vec::new()));
        let terminations_for_call = Arc::clone(&terminations);
        let output = run_native_probe_helper_with_timeout_using(
            &helper,
            42,
            std::time::Duration::from_secs(1),
            move |pid| terminations_for_call.lock().unwrap().push(pid),
        )
        .expect("a successful helper must return its output");

        assert!(output.status.success());
        assert_eq!(output.stdout, b"success");
        assert!(terminations.lock().unwrap().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
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
    fn screenshot_target_waits_for_the_configured_settle_delay() {
        let start = Instant::now();
        let settle = std::time::Duration::from_secs(12);
        let waiting = ScreenshotCaptureState::initial(true, true, start, settle);
        let (settling, armed) = waiting.target_ready_after(start, settle);
        assert!(armed);
        assert!(!settling.dispatch_due(start));
        assert!(settling.dispatch_due(start + settle));
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
            "activation must continue during the 12s settle"
        );
        assert!(
            !schedule.due(start + SCREENSHOT_WAKE_MAX_DURATION),
            "wake activation must have a hard lifetime bound"
        );
        schedule.activate(start + SCREENSHOT_WAKE_MAX_DURATION);
        assert!(
            !schedule.due(start + SCREENSHOT_WAKE_MAX_DURATION),
            "an expired schedule must not silently restart while activation remains requested"
        );
        schedule.deactivate();
        assert!(!schedule.due(start + SCREENSHOT_WAKE_MAX_DURATION));
    }

    #[cfg(unix)]
    #[test]
    fn exact_window_wake_command_has_a_bounded_owned_timeout() {
        let started = Instant::now();
        assert!(!invoke_exact_window_wake(
            "sleep 5",
            std::path::Path::new("/tmp/wake-test")
        ));
        assert!(
            started.elapsed() < std::time::Duration::from_secs(4),
            "a hung caller wake must not block the native capture indefinitely"
        );
    }

    #[cfg(unix)]
    #[test]
    fn exact_window_wake_command_cleans_up_successful_background_descendant() {
        let dir = std::env::temp_dir().join(format!(
            "corrald-ui-native-wake-success-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("test clock is after the Unix epoch")
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let descendant_pid_path = dir.join("descendant.pid");

        assert!(invoke_exact_window_wake(
            "sleep 20 & printf '%s\\n' \"$!\" > \"$CORRAL_UI_SCREENSHOT_PATH\"",
            &descendant_pid_path
        ));
        let descendant_pid = std::fs::read_to_string(&descendant_pid_path)
            .unwrap()
            .trim()
            .parse::<libc::pid_t>()
            .unwrap();
        let deadline = Instant::now() + std::time::Duration::from_secs(2);
        while unsafe { libc::kill(descendant_pid, 0) == 0 } && Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert_ne!(
            unsafe { libc::kill(descendant_pid, 0) },
            0,
            "a successful wake must not leave its background descendant alive"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn screenshot_state_retries_after_the_eight_second_deadline() {
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
        assert!(matches!(
            second,
            ScreenshotCaptureState::AwaitingScreenshot { attempt: 2, .. }
        ));
    }

    #[test]
    fn screenshot_state_exhausts_after_exactly_three_dispatch_attempts() {
        let start = Instant::now();
        let (ready, _) =
            ScreenshotCaptureState::initial(true, true, start, std::time::Duration::from_secs(2))
                .target_ready_after(start, std::time::Duration::ZERO);
        let (first, _) = ready.try_dispatch(start, true, true);
        let (second, decision) = first.try_dispatch(start + SCREENSHOT_RETRY_AFTER, true, true);
        assert_eq!(decision, ScreenshotDispatch::Dispatched { attempt: 2 });
        let (third, decision) = second.try_dispatch(start + SCREENSHOT_RETRY_AFTER * 2, true, true);
        assert_eq!(decision, ScreenshotDispatch::Dispatched { attempt: 3 });
        assert_eq!(third.attempts(), SCREENSHOT_MAX_ATTEMPTS);

        let (exhausted, decision) =
            third.try_dispatch(start + SCREENSHOT_RETRY_AFTER * 3, true, true);
        assert_eq!(decision, ScreenshotDispatch::Exhausted);
        assert_eq!(exhausted, ScreenshotCaptureState::Exhausted);
        let (still_exhausted, decision) =
            exhausted.try_dispatch(start + SCREENSHOT_RETRY_AFTER * 4, true, true);
        assert_eq!(decision, ScreenshotDispatch::NotDue);
        assert_eq!(still_exhausted, ScreenshotCaptureState::Exhausted);
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

    fn issue_input(events: Vec<egui::Event>) -> egui::RawInput {
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(900.0, 600.0),
            )),
            events,
            ..Default::default()
        }
    }

    fn issue_pointer_input(pos: egui::Pos2, pressed: bool) -> egui::RawInput {
        issue_input(vec![
            egui::Event::PointerMoved(pos),
            egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: Default::default(),
            },
        ])
    }

    fn render_issues(
        ctx: &egui::Context,
        fleet: &Fleet,
        input: egui::RawInput,
        intents: &std::cell::RefCell<Vec<crate::drive::DriveIntent>>,
    ) -> egui::FullOutput {
        ctx.run_ui(input, |ui| {
            let mut drive = |intent| intents.borrow_mut().push(intent);
            let mut refresh = || {};
            crate::ui::issues::show(ui, fleet, &|_| true, &mut drive, &mut refresh);
        })
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

    #[test]
    fn tab_strip_click_navigates_to_settings_without_an_audit_destination() {
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
        assert!(text_rect(&output, "Audit").is_none());
    }

    #[test]
    fn folded_issue_error_does_not_trigger_an_immediate_retry() {
        let mut fleet = Fleet {
            issues_loading: true,
            ..Default::default()
        };
        fleet.set_issues(Err("GET /issues unavailable".into()));
        assert!(!fleet.issues_loaded);
        assert!(!fleet.issues_loading);
        assert_eq!(
            fleet.issues_error.as_deref(),
            Some("GET /issues unavailable")
        );
        assert!(
            !issues_refresh_due(
                false,
                ConnState::Connected,
                fleet.issues_loading,
                Instant::now()
            ),
            "the frame after a folded error must remain inside the refresh interval"
        );
        assert!(issues_refresh_due(
            false,
            ConnState::Connected,
            false,
            Instant::now() - ISSUES_REFRESH_INTERVAL
        ));
    }

    #[test]
    fn hydration_uses_the_attention_ranked_visible_default_without_persisting_it() {
        let idle = agent("herdr:a-idle", AgentState::Idle, &["read_tail"]);
        let blocked = agent("herdr:z-blocked", AgentState::Blocked, &["read_tail"]);
        let fleet = Fleet {
            agents: [
                (idle.agent_id.clone(), idle),
                (blocked.agent_id.clone(), blocked),
            ]
            .into_iter()
            .collect(),
            ..Default::default()
        };

        let visible = board::visible_agent_ids(&fleet, StateFilter::All, "");
        assert_eq!(visible, ["herdr:z-blocked", "herdr:a-idle"]);
        let resolved = board::resolve_selection(&fleet, &visible);
        assert_eq!(resolved, Some("herdr:z-blocked"));
        assert_eq!(
            hydration_target(&fleet, resolved),
            Some("herdr:z-blocked".into())
        );
        assert_eq!(
            fleet.selected_agent, None,
            "default resolution is not persisted"
        );
    }

    #[test]
    fn hydration_follows_a_filtered_visible_card_and_ignores_hidden_pinned_agent() {
        let hidden_pinned = agent("herdr:a-pinned", AgentState::Blocked, &[]);
        let visible = agent("herdr:z-visible", AgentState::Working, &["read_tail"]);
        let fleet = Fleet {
            agents: [
                (hidden_pinned.agent_id.clone(), hidden_pinned),
                (visible.agent_id.clone(), visible),
            ]
            .into_iter()
            .collect(),
            selected_agent: Some("herdr:a-pinned".into()),
            ..Default::default()
        };

        let visible_ids = board::visible_agent_ids(&fleet, StateFilter::Working, "");
        let resolved = board::resolve_selection(&fleet, &visible_ids);
        assert_eq!(visible_ids, ["herdr:z-visible"]);
        assert_eq!(resolved, Some("herdr:z-visible"));
        assert_eq!(
            hydration_target(&fleet, resolved),
            Some("herdr:z-visible".into())
        );
        assert_eq!(
            hydration_target(&fleet, Some("herdr:a-pinned")),
            None,
            "a hidden pinned card without read_tail is never hydrated"
        );
        assert_eq!(fleet.selected_agent.as_deref(), Some("herdr:a-pinned"));
    }

    #[test]
    fn load_earlier_drive_response_reaches_the_app_tail_cache() {
        let intent = DriveIntent::read_tail("herdr:load-earlier", Some(42));
        let mut fleet = Fleet::default();
        let msg = DriveMsg {
            agent_id: intent.target.clone(),
            capability: intent.capability.to_string(),
            outcome: DriveOutcome::Ok {
                rev: 43,
                result: Some(serde_json::json!({
                    "lines": ["older line one", "older line two"]
                })),
            },
        };

        CorralApp::apply_read_tail_result(&mut fleet, &msg);
        assert_eq!(msg.capability, "read_tail");
        assert_eq!(
            fleet.tails["herdr:load-earlier"],
            ["older line one", "older line two"]
        );
    }
}
