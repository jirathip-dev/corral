//! D33: per-agent daily digest, computed OFFLINE from the history ring.
//!
//! Blocked duration is derived, not stored: consecutive blocked ->
//! non-blocked event pairs on the same agent/pane. Conventions:
//! - An agent whose FIRST in-window event is Blocked was presumably blocked
//!   when the window opened — its span is counted from the window start.
//! - A span still open at the window end is reported as open ("still
//!   blocked"), with a duration so far; it is not included in the closed
//!   totals.
//! - Work per repo counts in-window transitions per `repo` (events without a
//!   repo reported as "(no repo)").

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};

use crate::core::model::AgentState;

use super::ring::HistoryEvent;

/// (start_ts, end_ts) of one closed blocked span, epoch millis.
pub type BlockedSpan = (u64, u64);

/// Per-agent summary within a digest window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentDigest {
    pub agent_id: String,
    pub source: String,
    /// In-window transitions.
    pub transitions: usize,
    /// Ordered statuses (each event's `new_status`).
    pub sequence: Vec<AgentState>,
    /// Closed blocked -> non-blocked spans.
    pub blocked_spans: Vec<BlockedSpan>,
    /// Start of a span still open at the window end.
    pub blocked_open: Option<u64>,
    /// Transition counts per repo (sorted by repo name).
    pub work_by_repo: BTreeMap<String, usize>,
}

impl AgentDigest {
    /// Sum of closed blocked spans.
    pub fn blocked_millis(&self) -> u64 {
        self.blocked_spans
            .iter()
            .map(|(start, end)| end.saturating_sub(*start))
            .sum()
    }

    /// Longest closed blocked span.
    pub fn longest_blocked_millis(&self) -> u64 {
        self.blocked_spans
            .iter()
            .map(|(start, end)| end.saturating_sub(*start))
            .max()
            .unwrap_or(0)
    }
}

/// Window digest: agents sorted by id, repos sorted by name — deterministic
/// for a given ring.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Digest {
    pub since: u64,
    pub until: u64,
    /// In-window events that contributed to the summary.
    pub events: usize,
    pub agents: BTreeMap<String, AgentDigest>,
}

impl Digest {
    /// Compute over `events` (assumed to be ring-ordered, oldest first).
    /// `since` is the window start and `until` the window end (open-span
    /// reporting).
    pub fn compute(events: &[HistoryEvent], since: u64, until: u64) -> Self {
        let mut by_agent: BTreeMap<String, Vec<&HistoryEvent>> = BTreeMap::new();
        for event in events.iter().filter(|e| e.ts >= since) {
            by_agent
                .entry(
                    event
                        .agent_id
                        .clone()
                        .unwrap_or_else(|| "<unknown>".to_string()),
                )
                .or_default()
                .push(event);
        }
        let mut agents = BTreeMap::new();
        for (agent_id, evs) in by_agent {
            let source = evs.first().map(|e| e.source.clone()).unwrap_or_default();
            let mut blocked_spans: Vec<BlockedSpan> = Vec::new();
            let mut open: Option<u64> = None;
            for (i, event) in evs.iter().enumerate() {
                match event.new_status {
                    AgentState::Blocked => {
                        if open.is_none() {
                            // First in-window event blocked: the span started
                            // before the window opened.
                            let start = if i == 0 { since } else { event.ts };
                            open = Some(start);
                        }
                    }
                    _ => {
                        if let Some(start) = open.take() {
                            blocked_spans.push((start, event.ts));
                        }
                    }
                }
            }
            let mut work_by_repo: BTreeMap<String, usize> = BTreeMap::new();
            for event in &evs {
                *work_by_repo
                    .entry(repo_label(event.repo.as_deref()))
                    .or_insert(0) += 1;
            }
            agents.insert(
                agent_id.clone(),
                AgentDigest {
                    agent_id,
                    source,
                    transitions: evs.len(),
                    sequence: evs.iter().map(|e| e.new_status).collect(),
                    blocked_spans,
                    blocked_open: open,
                    work_by_repo,
                },
            );
        }
        Self {
            since,
            until,
            events: events.iter().filter(|e| e.ts >= since).count(),
            agents,
        }
    }

    /// Render as deterministic plain text.
    pub fn render(&self) -> String {
        let mut out = String::new();
        let blocked_spans: usize = self.agents.values().map(|a| a.blocked_spans.len()).sum();
        let blocked_millis: u64 = self.agents.values().map(|a| a.blocked_millis()).sum();
        out.push_str(&format!(
            "corral digest — {} .. {} (window since {})\n",
            fmt_ts(self.since),
            fmt_ts(self.until),
            self.since,
        ));
        out.push_str(&format!(
            "events in window: {} | agents: {} | blocked spans: {} | blocked time: {}\n",
            self.events,
            self.agents.len(),
            blocked_spans,
            fmt_duration(blocked_millis),
        ));
        if self.agents.is_empty() {
            out.push_str("no transitions recorded in this window.\n");
            return out;
        }
        for agent in self.agents.values() {
            out.push('\n');
            out.push_str(&format!("{} ({})\n", agent.agent_id, agent.source));
            out.push_str(&format!(
                "  transitions ({}): {}\n",
                agent.transitions,
                seq_str(&agent.sequence),
            ));
            out.push_str(&self.blocked_line(agent));
            if !agent.blocked_spans.is_empty() {
                for (start, end) in &agent.blocked_spans {
                    out.push_str(&format!(
                        "    {} -> {} ({})\n",
                        fmt_time(*start),
                        fmt_time(*end),
                        fmt_duration(end.saturating_sub(*start)),
                    ));
                }
            }
            out.push_str(&self.repo_line(agent));
        }
        out
    }

    fn blocked_line(&self, agent: &AgentDigest) -> String {
        match (agent.blocked_spans.is_empty(), agent.blocked_open) {
            (true, None) => "  blocked: none\n".to_string(),
            (false, None) => format!(
                "  blocked: {} {}, total {}, longest {}\n",
                agent.blocked_spans.len(),
                span_word(agent.blocked_spans.len()),
                fmt_duration(agent.blocked_millis()),
                fmt_duration(agent.longest_blocked_millis()),
            ),
            (true, Some(open)) => format!(
                "  blocked: 1 span open (since {}, {} so far)\n",
                fmt_time(open),
                fmt_duration(self.until.saturating_sub(open)),
            ),
            (false, Some(open)) => format!(
                "  blocked: {} {}, total {}, longest {}; 1 open (since {}, {} so far)\n",
                agent.blocked_spans.len(),
                span_word(agent.blocked_spans.len()),
                fmt_duration(agent.blocked_millis()),
                fmt_duration(agent.longest_blocked_millis()),
                fmt_time(open),
                fmt_duration(self.until.saturating_sub(open)),
            ),
        }
    }

    fn repo_line(&self, agent: &AgentDigest) -> String {
        let parts: Vec<String> = agent
            .work_by_repo
            .iter()
            .map(|(repo, n)| format!("{repo} {n}"))
            .collect();
        format!("  work by repo: {}\n", parts.join(", "))
    }
}

fn span_word(n: usize) -> &'static str {
    if n == 1 { "span" } else { "spans" }
}

/// Status label matching the serde snake_case names (never assume a serial
/// repr elsewhere; model.rs stays untouched).
fn state_str(state: &AgentState) -> &'static str {
    match state {
        AgentState::Idle => "idle",
        AgentState::Working => "working",
        AgentState::Blocked => "blocked",
        AgentState::Done => "done",
        AgentState::Unknown => "unknown",
    }
}

fn seq_str(sequence: &[AgentState]) -> String {
    sequence
        .iter()
        .map(state_str)
        .collect::<Vec<_>>()
        .join(" -> ")
}

fn repo_label(repo: Option<&str>) -> String {
    repo.unwrap_or("(no repo)").to_string()
}

fn fmt_ts(ts: u64) -> String {
    DateTime::from_timestamp_millis(ts as i64)
        .map(|dt| {
            dt.with_timezone(&Utc)
                .format("%Y-%m-%dT%H:%M:%SZ")
                .to_string()
        })
        .unwrap_or_else(|| format!("{ts}"))
}

fn fmt_time(ts: u64) -> String {
    DateTime::from_timestamp_millis(ts as i64)
        .map(|dt| dt.with_timezone(&Utc).format("%H:%M:%SZ").to_string())
        .unwrap_or_else(|| format!("{ts}"))
}

/// Human duration, deterministic: "1h 23m 5s", "10m 56s", "45s", "2d 3h 4m".
pub fn fmt_duration(millis: u64) -> String {
    let total = millis / 1000;
    let (d, h, m, s) = (
        total / 86_400,
        (total / 3_600) % 24,
        (total / 60) % 60,
        total % 60,
    );
    if d > 0 {
        format!("{d}d {h}h {m}m")
    } else if h > 0 {
        format!("{h}h {m}m {s}s")
    } else if m > 0 {
        format!("{m}m {s}s")
    } else {
        format!("{s}s")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(ts: u64, agent: &str, new: AgentState, repo: Option<&str>) -> HistoryEvent {
        HistoryEvent {
            ts,
            pane_id: Some(format!("pane-{agent}")),
            agent_id: Some(agent.to_string()),
            old_status: None,
            new_status: new,
            source: "herdr".to_string(),
            repo: repo.map(str::to_string),
        }
    }

    #[test]
    fn fmt_duration_units() {
        assert_eq!(fmt_duration(0), "0s");
        assert_eq!(fmt_duration(45_000), "45s");
        assert_eq!(fmt_duration(656_000), "10m 56s");
        assert_eq!(fmt_duration(4_985_000), "1h 23m 5s");
        assert_eq!(fmt_duration(187_040_000), "2d 3h 57m");
    }

    #[test]
    fn blocked_pairs_from_consecutive_transitions() {
        let events = vec![
            event(1000, "a", AgentState::Working, Some("corral")),
            event(2000, "a", AgentState::Blocked, Some("corral")),
            event(3000, "a", AgentState::Working, Some("corral")),
            event(4000, "a", AgentState::Blocked, Some("corral")),
            event(5000, "a", AgentState::Done, Some("corral")),
        ];
        let digest = Digest::compute(&events, 0, 6000);
        let agent = &digest.agents["a"];
        assert_eq!(agent.blocked_spans, vec![(2000, 3000), (4000, 5000)]);
        assert_eq!(agent.blocked_millis(), 2000);
        assert_eq!(agent.longest_blocked_millis(), 1000);
        assert_eq!(agent.blocked_open, None);
        assert_eq!(digest.events, 5);
    }

    #[test]
    fn first_in_window_blocked_counts_from_window_start() {
        // Agent b's first in-window event is Blocked: the true start
        // predates the window, so the span is counted from `since`.
        let events = vec![
            event(1500, "b", AgentState::Blocked, None),
            event(2500, "b", AgentState::Working, None),
        ];
        let digest = Digest::compute(&events, 1000, 3000);
        let agent = &digest.agents["b"];
        assert_eq!(agent.blocked_spans, vec![(1000, 2500)]);
        assert_eq!(agent.blocked_open, None);
    }

    #[test]
    fn open_span_reported_without_joining_closed_total() {
        let events = vec![
            event(1000, "a", AgentState::Working, None),
            event(2000, "a", AgentState::Blocked, None),
            // b's first in-window event is Working, so its Blocked at 1500
            // is a normal span start (not backdated to the window).
            event(1200, "b", AgentState::Working, None),
            event(1500, "b", AgentState::Blocked, None),
            event(2500, "b", AgentState::Working, None),
            event(3500, "b", AgentState::Blocked, None),
        ];
        let digest = Digest::compute(&events, 0, 6000);
        let a = &digest.agents["a"];
        let b = &digest.agents["b"];
        // a's span opened at 2000 and no non-blocked event followed: it is
        // OPEN at the window end, never a closed span.
        assert!(a.blocked_spans.is_empty());
        assert_eq!(a.blocked_open, Some(2000));
        assert_eq!(b.blocked_spans, vec![(1500, 2500)]);
        assert_eq!(b.blocked_open, Some(3500));
        let text = digest.render();
        assert!(
            text.contains("blocked: 1 span open (since 00:00:02Z, 4s so far)"),
            "{text}"
        );
    }

    #[test]
    fn work_by_repo_and_ordering() {
        let events = vec![
            event(1000, "a", AgentState::Working, Some("corral")),
            event(2000, "a", AgentState::Blocked, Some("corral")),
            event(3000, "a", AgentState::Working, None),
            event(4000, "a", AgentState::Done, Some("herdr-board")),
        ];
        let digest = Digest::compute(&events, 0, 5000);
        let agent = &digest.agents["a"];
        assert_eq!(
            agent.work_by_repo,
            BTreeMap::from([
                ("corral".to_string(), 2),
                ("(no repo)".to_string(), 1),
                ("herdr-board".to_string(), 1),
            ])
        );
        assert!(
            digest
                .render()
                .contains("work by repo: (no repo) 1, corral 2, herdr-board 1")
        );
    }

    #[test]
    fn render_is_deterministic_and_sorted() {
        let events = vec![
            event(1000, "z", AgentState::Working, Some("corral")),
            event(1100, "z", AgentState::Blocked, Some("corral")),
            event(1200, "z", AgentState::Working, Some("corral")),
            event(1300, "a", AgentState::Blocked, None),
            event(1400, "a", AgentState::Done, None),
        ];
        let digest = Digest::compute(&events, 0, 2000);
        let text = digest.render();
        let a_at = text.find("\na (").expect("agent a section");
        let z_at = text.find("\nz (").expect("agent z section");
        assert!(a_at < z_at, "agents sorted by id");
        assert_eq!(text, digest.render(), "identical input renders identically");
        assert!(text.contains("transitions (2): blocked -> done"));
        assert!(
            text.contains("blocked: 1 span, total 1s, longest 1s"),
            "{text}"
        );
    }
}
