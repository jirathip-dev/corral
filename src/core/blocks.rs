//! #167 (D7): pane-tail block segmentation — ONE segmentation pass in
//! `corrald` so clients stay dumb renderers.
//!
//! A raw pane-tail string (read_tail delivery) is cleaned mechanically and
//! then split into [`TranscriptBlock`]s. The five cleaning passes (D10) run
//! BEFORE any grouping:
//!
//! 1. Strip ANSI/OSC escape sequences (`ESC [ …` CSI, `ESC ] … BEL|ST` OSC,
//!    charset-select escapes) — the codex/claude TUI leaves these in pane
//!    text and they render as `^[[` garbage on a phone.
//! 2. Resolve `\r` overdraw — a progress spinner overwrites the same line, so
//!    keep the segment after the FINAL `\r` per line (and drop a trailing
//!    `\r` that is just a CRLF).
//! 3. Normalize whitespace — right-trim lines, tabs→4 spaces, and collapse any
//!    run of ≥3 consecutive blank lines down to two.
//! 4. Lift `... +N lines (ctrl+t …)` markers into `truncated_before` on the
//!    following block and REMOVE the marker text — the `ctrl+t` hint is a lie
//!    on a phone, and a raw `+N` in the log is not a truncation the client can
//!    render.
//! 5. NO hard-wrap. Soft wrapping is the client's job — long lines pass
//!    through intact (D10 pass 5).
//!
//! ## Kinds
//!
//! [`TranscriptBlockKind`] is the client's only vocabulary (D7): User (the
//! operator's prompt), Agent (model output), Tool (a command/result), System
//! (diagnostic chrome). AC7: stray artifacts like `Missing environment
//! variable…`, `"Test"`, and `• Ask Codex to do anything` are NOT filtered —
//! they are demoted to System blocks so a phone renders them dim/mono and
//! collapsible instead of deleting diagnostic signal.
//!
//! ## Ordering
//!
//! Blocks are emitted in NATURAL (oldest→newest, top→bottom) reading order
//! for a given input text.
//!
//! ## Additive on the wire
//!
//! `read_tail` gains a `blocks` field alongside `lines`. Both retain the
//! existing text surface so egui (and the signed-envelope contract) keep
//! working until #168.

use serde::{Deserialize, Serialize};

/// Redaction marker reused so block text stays consistent with `lines`.
pub use crate::core::redact::REDACTED;

/// The block kinds a client may render (D7 + #315). No chat bubbles, no
/// per-message timestamps — the client renders these with minimal contrast.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TranscriptBlockKind {
    /// The operator's prompt / message, provenance-backed (a recorded
    /// Corral Prompt dispatch). Rendered trailing-aligned + tinted.
    User,
    /// Model/agent output. Rendered leading-aligned plain.
    Agent,
    /// A tool invocation or result (`$ cargo build` …). Collapsed to a one-line
    /// summary `▸ ran cargo test (exit 0)`, expanding on tap.
    Tool,
    /// Diagnostic chrome / stray artifacts (`Missing environment variable…`,
    /// `• Ask Codex to do anything`, box-drawing rules). Dim, mono, collapsible
    /// — never deleted (AC7).
    System,
    /// #315: terminal text with NO reliable provenance (direct terminal
    /// input, unrecognised activity). Preserved but never falsely
    /// attributed — a client must not render it as user/system/assistant.
    Unknown,
}

impl TranscriptBlockKind {
    /// Map a role string onto a block kind. Everything not clearly
    /// user/agent/tool collapses to System — that is the whole AC7 demotion
    /// rule.
    pub fn from_role(role: &str) -> Self {
        match role {
            "user" | "human" => Self::User,
            "assistant" | "ai" => Self::Agent,
            "tool" | "tool_result" | "tool_use" | "function" => Self::Tool,
            _ => Self::System,
        }
    }
}

/// One segmented block (D7). `text` is the cleaned, non-hard-wrapped text;
/// `at` is epoch millis when the source carried one (absent = unlabelled);
/// `truncated_before` is the count lifted from a `... +N lines` marker that
/// immediately precedes this block (absent = no marker);
/// `prompt_request_id` (#315) is the signed request id of the recorded
/// Prompt dispatch this user block is provenance-backed by (absent = none).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TranscriptBlock {
    pub kind: TranscriptBlockKind,
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub truncated_before: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_request_id: Option<String>,
}

/// Run all five cleaning passes over a raw string. Exposed so the table-driven
/// fixture tests can pin each pass independently.
pub fn clean(raw: &str) -> String {
    normalize_whitespace(&resolve_cr_overdraw(&strip_escapes(raw)))
}

/// Pass 1: strip ANSI/OSC escape sequences.
///
/// Byte-level but UTF-8 safe: every byte copied is a byte of the original
/// string, and the bytes we SKIP are ASCII escape-sequence bytes, so the
/// resulting `Vec<u8>` is always valid UTF-8.
fn strip_escapes(raw: &str) -> String {
    fn is_csi_final(b: u8) -> bool {
        (0x40..=0x7E).contains(&b)
    }
    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if b != 0x1B {
            out.push(b);
            i += 1;
            continue;
        }
        // b == ESC. Look at the next byte to choose the sequence shape.
        let Some(&next) = bytes.get(i + 1) else {
            i += 1; // trailing ESC
            continue;
        };
        match next {
            // CSI: ESC [ params... final byte (0x40..=0x7E)
            b'[' => {
                i += 2;
                while i < bytes.len() && !is_csi_final(bytes[i]) {
                    i += 1;
                }
                if i < bytes.len() && is_csi_final(bytes[i]) {
                    i += 1;
                }
            }
            // OSC: ESC ] ... BEL(0x07) | ST(ESC \)
            b']' => {
                i += 2;
                while i < bytes.len() {
                    match bytes[i] {
                        0x07 => {
                            i += 1;
                            break;
                        }
                        0x1B if bytes.get(i + 1) == Some(&b'\\') => {
                            i += 2;
                            break;
                        }
                        _ => i += 1,
                    }
                }
            }
            // Charset-select / single-char introducers: ESC ( X, ESC ) X,
            // ESC # X, ESC % X, ESC = X, ESC > X. Each is ESC + introducer
            // + one final character.
            b'(' | b')' | b'#' | b'%' | b'=' | b'>' => {
                i += 2;
                if i < bytes.len() {
                    i += 1;
                }
            }
            // Any other single-char escape: skip ESC + the escaped byte.
            _ => i += 2,
        }
    }
    String::from_utf8(out).expect("stripping ASCII escapes preserves UTF-8")
}

/// Pass 2: resolve `\r` overdraw.
///
/// Per line (split on `\n`), a single trailing `\r` (CRLF) is dropped; any
/// remaining `\r` means the line was overwritten, so only the substring after
/// the LAST `\r` survives.
fn resolve_cr_overdraw(text: &str) -> String {
    text.split('\n')
        .map(|line| {
            let line = line.strip_suffix('\r').unwrap_or(line);
            match line.rfind('\r') {
                Some(idx) => &line[idx + 1..],
                None => line,
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Pass 3: normalize whitespace. Right-trim, tabs→4 spaces, and collapse any
/// run of ≥3 consecutive blank lines down to two.
fn normalize_whitespace(text: &str) -> String {
    let mut out: Vec<String> = Vec::new();
    let mut blank_run = 0usize;
    for line in text.split('\n') {
        let trimmed = line.trim_end();
        if trimmed.is_empty() {
            blank_run += 1;
            if blank_run > 2 {
                continue;
            }
            out.push(String::new());
        } else {
            blank_run = 0;
            // Replace tabs with 4 spaces.
            out.push(trimmed.replace('\t', "    "));
        }
    }
    out.join("\n")
}

/// Minimum furniture glyphs for a line to be treated as TUI decor rather
/// than content (#253).
const FURNITURE_MIN_RUN: usize = 8;

/// Short divider marker a collapsed border line becomes (#253).
const DIVIDER_MARKER: &str = "───";

/// #253: scrub TUI furniture out of pane-tail text (read_tail path).
///
/// The hermes/herdr TUI paints panel borders with box-drawing characters
/// (`─ ═ ╌ ┈ ╭ ╮ ╰ ╯ │ ┼ …`) and progress bars with block glyphs
/// (`█ ▓ ░`). Phone fonts render long `─` runs as continuous dash junk.
/// Run per line AFTER the ANSI/redaction passes, so painted borders still
/// measure as furniture and redaction semantics never change. Rules:
///
/// 1. Border-shaped box-drawing lines (start AND end on a box-drawing
///    char, at least [`FURNITURE_MIN_RUN`] glyphs, >= 60% of the
///    non-whitespace characters) collapse each long box-drawing run to
///    [`DIVIDER_MARKER`] — a pure border becomes exactly the marker.
///    Content lines that merely CONTAIN a `─` run inside real strings
///    (`let sep = "────";`) are not border-shaped and survive untouched —
///    the issue's explicit keep-in-str case.
/// 2. Interior-only vertical lines (`│ … │` with no text and no horizontal
///    run) are dropped.
/// 3. Progress-dominant lines: at least [`FURNITURE_MIN_RUN`] block glyphs
///    at >= 60% of the non-whitespace characters get their block span
///    replaced with a compact ten-cell bar scaled to the run's dark/light
///    ratio.
///
/// Everything else passes through byte-identical. Deterministic and O(n)
/// per line (char scan only — no regex, so long lines cannot backtrack).
pub fn scrub_tui_furniture(line: &str) -> String {
    // Measure on the escape-stripped view: the TUI paints borders with CSI
    // color sequences, and a painted border must count as furniture rather
    // than be diluted by escape bytes.
    let stripped = strip_escapes(line);
    let trimmed = stripped.trim();

    let mut nonws = 0usize;
    let mut box_count = 0usize;
    let mut block_count = 0usize;
    for c in stripped.chars() {
        if !c.is_whitespace() {
            nonws += 1;
            if is_box_drawing(c) {
                box_count += 1;
            } else if is_block_element(c) {
                block_count += 1;
            }
        }
    }
    if nonws == 0 || (box_count == 0 && block_count == 0) {
        return line.to_string();
    }

    // Rule 2: interior-only vertical line -> drop.
    if block_count == 0
        && box_count >= 2
        && box_count == nonws
        && trimmed
            .chars()
            .all(|c| c.is_whitespace() || is_vertical_box(c))
    {
        return String::new();
    }

    // The box-drawing glyphs must wrap the line for it to be a TUI border;
    // a dash run inside real text/strings is content, not furniture.
    let border_shaped = trimmed.chars().next().is_some_and(is_box_drawing)
        && trimmed.chars().last().is_some_and(is_box_drawing);

    // Rule 1: border/rule line -> collapse each maximal box-drawing run
    // to a short divider marker. A pure border collapses to exactly
    // DIVIDER_MARKER; a rail that happens to carry a label keeps the text
    // and loses only the long runs.
    if block_count == 0
        && box_count >= FURNITURE_MIN_RUN
        && border_shaped
        && box_count * 10 >= nonws * 6
    {
        return compact_box_runs(&stripped);
    }

    // Rule 3: progress-dominant line -> compact scaled bar.
    if block_count >= FURNITURE_MIN_RUN && block_count * 10 >= nonws * 6 {
        return compact_progress_span(&stripped);
    }

    line.to_string()
}

/// Replace private-use glyphs emitted by terminal icon fonts with readable
/// ASCII. Ordinary Unicode, including emoji, is preserved.
pub fn scrub_unsupported_glyphs(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut in_run = false;
    for scalar in line.chars() {
        let private_use = ('\u{e000}'..='\u{f8ff}').contains(&scalar)
            || ('\u{f0000}'..='\u{ffffd}').contains(&scalar)
            || ('\u{100000}'..='\u{10fffd}').contains(&scalar);
        if private_use {
            if !in_run {
                out.push_str("[icon]");
                in_run = true;
            }
        } else {
            out.push(scalar);
            in_run = false;
        }
    }
    out
}

fn is_box_drawing(c: char) -> bool {
    ('\u{2500}'..='\u{257F}').contains(&c)
}

fn is_block_element(c: char) -> bool {
    ('\u{2580}'..='\u{259F}').contains(&c)
}

/// The vertical-only box-drawing glyphs (interior rails). A line made of
/// just these (plus whitespace) is an empty pane interior, not content.
fn is_vertical_box(c: char) -> bool {
    matches!(c, '│' | '║' | '┃' | '┆' | '┇' | '┊' | '┋' | '╎' | '╏')
}

/// Replace every maximal run of box-drawing glyphs with the short divider
/// marker, keeping any interior text (rails, titles) intact.
fn compact_box_runs(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut run = 0usize;
    for c in line.chars() {
        if is_box_drawing(c) {
            run += 1;
        } else {
            if run > 0 {
                out.push_str(DIVIDER_MARKER);
                run = 0;
            }
            out.push(c);
        }
    }
    if run > 0 {
        out.push_str(DIVIDER_MARKER);
    }
    out
}

/// Replace the span between the first and last block glyph with a compact
/// 10-cell bar. Light shade `░` counts as empty; every other block glyph
/// counts as filled; the bar rounds the dark/light ratio to tenths.
/// (ponytail: a precise value, when present, rides on the line as `NN%` —
/// the bar is decor, not data.)
fn compact_progress_span(line: &str) -> String {
    let chars: Vec<char> = line.chars().collect();
    let first = chars.iter().position(|c| is_block_element(*c));
    let last = chars.iter().rposition(|c| is_block_element(*c));
    let (Some(first), Some(last)) = (first, last) else {
        return line.to_string();
    };
    let mut total = 0usize;
    let mut dark = 0usize;
    for c in &chars[first..=last] {
        if is_block_element(*c) {
            total += 1;
            if *c != '\u{2591}' {
                dark += 1;
            }
        }
    }
    // `total >= 1` by construction: the span starts at a block glyph.
    let filled = (dark * 10 + total / 2) / total;
    let mut out: String = chars[..first].iter().collect();
    for _ in 0..filled {
        out.push('\u{25B0}'); // ▰
    }
    for _ in filled..10 {
        out.push('\u{25B1}'); // ▱
    }
    out.extend(chars[last + 1..].iter());
    out
}

/// Pass 4: parse a `... +N lines (ctrl+t …)` marker. Returns the N count when
/// the line is such a marker; nothing else.
pub fn parse_truncation_marker(line: &str) -> Option<u32> {
    let t = line.trim();
    let rest = t
        .strip_prefix("...")
        .or_else(|| t.strip_prefix('…'))
        .or_else(|| t.strip_prefix(". . ."))?;
    // Find "+N" somewhere after the ellipsis.
    let plus = rest.find('+')?;
    let after_plus = &rest[plus + 1..];
    let digits: String = after_plus
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect();
    if digits.is_empty() {
        return None;
    }
    let n: u32 = digits.parse().ok()?;
    // The rest must mention "lines" (case-insensitive) to be a transcript
    // hint, not some other "+N".
    let tail = &after_plus[digits.len()..];
    if !tail.to_ascii_lowercase().contains("line") {
        return None;
    }
    Some(n)
}

/// Pass 5 is a NO-OP here: do NOT hard-wrap. A long line passes through
/// byte-for-byte (only the cleaning above runs). The client soft-wraps at its
/// own width.
///
/// Classify one non-blank, non-marker line into a block kind. `role_hint`
/// (when known, from a transcript entry) wins; otherwise the raw pane text is
/// classified by shape. `prev_kind` lets an already-open tool block absorb its
/// own build/test output lines (`Compiling …`, `test result: …`) so a single
/// `$ cargo build` invocation renders as ONE block per the prototype.
fn classify_line(
    line: &str,
    role_hint: Option<TranscriptBlockKind>,
    prev_kind: Option<TranscriptBlockKind>,
) -> TranscriptBlockKind {
    if let Some(kind) = role_hint {
        return kind;
    }
    let t = line.trim_start();
    // Sanity: an empty line is never classified here (callers skip blanks).
    if t.is_empty() {
        return TranscriptBlockKind::System;
    }
    // Diagnostic chrome — AC7 demotion, never deletion.
    if is_system_artifact(t) {
        return TranscriptBlockKind::System;
    }
    // A shell command or command-ish invocation.
    if is_tool_line(t) {
        return TranscriptBlockKind::Tool;
    }
    // A `>`-prefixed line is the operator's typed prompt in the codex/claude
    // TUI (the brief's "a user prompt line" fixture).
    if t.starts_with('>') {
        return TranscriptBlockKind::User;
    }
    // Tool output continuation only when we are ALREADY in a tool block.
    if is_tool_output(line, prev_kind) {
        return TranscriptBlockKind::Tool;
    }
    // Ordinary output defaults to agent.
    TranscriptBlockKind::Agent
}

/// AC7: known diagnostic chrome shapes. These are SYSTEM, never deleted.
fn is_system_artifact(line: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    // Missing environment variable / error banners.
    if lower.contains("missing environment variable")
        || lower.contains("environment variable")
        || lower.starts_with("error:")
        || lower.starts_with("stdout >")
        || lower.starts_with("warning:")
    {
        return true;
    }
    // Codex TUI chrome.
    if lower.contains("ask codex to do anything")
        || lower.contains("ask codex")
        || lower.starts_with('\u{25cf}')
        || lower.starts_with('\u{2022}')
        || line.contains('\u{2500}') // box-drawing horizontal rule
        || line.contains('\u{2501}')
    // heavy box-drawing rule
    {
        return true;
    }
    // A lone quoted token used as an artifact ("Test") — only when it is
    // quote-wrapped and short, so real quoted prose is not demoted.
    if line.len() <= 80 {
        let trimmed = line.trim();
        if trimmed.len() >= 2
            && ((trimmed.starts_with('"') && trimmed.ends_with('"'))
                || (trimmed.starts_with('\'') && trimmed.ends_with('\'')))
            && trimmed[1..trimmed.len() - 1]
                .chars()
                .all(|c| !c.is_whitespace())
        {
            return true;
        }
    }
    false
}

/// Tool-output continuation lines: `Compiling …`, `Finished …`,
/// `Build progress: …`, `test result: …`, and rustc/cargo diagnostics. Only
/// honored when `prev_kind` is already Tool, so a bare `Compiling` line in
/// agent prose is NOT demoted to Tool.
fn is_tool_output(line: &str, prev_kind: Option<TranscriptBlockKind>) -> bool {
    if prev_kind != Some(TranscriptBlockKind::Tool) {
        return false;
    }
    let t = line.trim_start();
    t.starts_with("Compiling ")
        || t.starts_with("Finished ")
        || t.starts_with("Build progress")
        || t.starts_with("test result")
        || t.starts_with("Running ")
        || t.starts_with("Doc-tests")
        || t.starts_with("note:")
        || t.starts_with("error[")
        || t.starts_with("warning[")
        || t.contains("-->")
}

/// A tool-shaped line: a shell prompt, `$`-prefixed command, or a compiler
/// `file:line:col:` diagnostic.
fn is_tool_line(line: &str) -> bool {
    let t = line.trim_start();
    if t.starts_with('$')
        || t.starts_with("cargo ")
        || t.starts_with("npm ")
        || t.starts_with("make ")
        || t.starts_with("git ")
        || t.starts_with("python ")
        || t.starts_with("serverless ")
    {
        return true;
    }
    // compiler diagnostic: `path/file.rs:12:34: message` or `file:line: message`
    if t.len() > 4 {
        let segments: Vec<&str> = t.splitn(3, ':').collect();
        if segments.len() >= 3
            && segments[1].chars().all(|c| c.is_ascii_digit())
            && (segments[2]
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_digit())
                || segments[2].trim_start().starts_with(' '))
        {
            return true;
        }
    }
    false
}

/// Segment a raw string into blocks. `role_hint` (a transcript role) forces
/// every block to that kind for data that already knows its role; pass `None`
/// for raw pane text so the shape heuristics classify it.
pub fn segment(raw: &str, role_hint: Option<&str>, at: Option<u64>) -> Vec<TranscriptBlock> {
    let cleaned = clean(raw);
    segment_cleaned(&cleaned, role_hint, at)
}

/// Segment already-cleaned lines into blocks (used by the read_tail wire layer,
/// where redaction has already run per-line).
pub fn segment_lines(lines: &[String], at: Option<u64>) -> Vec<TranscriptBlock> {
    let joined = lines.join("\n");
    let cleaned = clean(&joined);
    segment_cleaned(&cleaned, None, at)
}

/// Terminal-shaped lines that are STATUS/SESSION CHROME rather than
/// conversation content (#315). These describe the runtime session (model
/// badges, context gauges, state footers, composer hints); they must never
/// enter the conversation as assistant output. Matching is SHAPE-based on
/// the line itself — never keyed to a harness, provider, or model name.
///
/// #315 R2 (F6): real chrome rows are SHORT single terminal rows whose
/// model/context/token words LEAD the row (the row's subject IS the
/// session). Ordinary prose that merely mentions a model or a percentage
/// ("The model achieved 98% accuracy…") is longer and mid-sentence, and
/// must not be demoted.
const SESSION_CHROME_MAX_LEN: usize = 80;

fn is_session_chrome(line: &str) -> bool {
    // Long prose is never chrome, whatever it mentions.
    if line.chars().count() > SESSION_CHROME_MAX_LEN {
        return false;
    }
    let lower = line.to_ascii_lowercase();
    let bare = lower.trim_start();
    // "status: working · esc to interrupt" footers and esc/menu hints
    // (short rows only — the length guard above already ran).
    let is_status_footer =
        lower.starts_with("status:") || bare.starts_with("esc to") || bare.starts_with("(esc");
    // Gauge rows LEAD with the session subject: "model context 42%",
    // "context left 12k", "tokens in flight". Prose like "the model
    // achieved 98% accuracy" mentions a model mid-sentence and stays
    // conversation.
    let leads_with_session_subject =
        bare.starts_with("model") || bare.starts_with("context") || bare.starts_with("tokens");
    let shows_a_gauge = lower.contains('%') || lower.contains("tokens") || lower.contains(" left");
    (leads_with_session_subject && shows_a_gauge) || is_status_footer
}

/// #315: build the CANONICAL semantic block stream for a read window.
///
/// Provenance-first, in order:
/// 1. A line that is a STRUCTURALLY ELIGIBLE echo of a successfully
///    dispatched Corral Prompt for this exact target (matched by content
///    identity — the ledger stores only a hash + length) IS the operator's
///    message: emitted exactly once as a `user` block carrying
///    `prompt_request_id`, regardless of the echo's terminal shape (`›`,
///    `>`, `❯` prefixes, decorations). #315 R2: eligibility requires a
///    typed-input echo — a decoration-prefixed line (`› hello`) or a
///    standalone line equal to the recorded text — so machine output that
///    merely EQUALS an old prompt (`yes`, `ok`) is never promoted. Binding
///    is one-to-one per read over an immutable ledger snapshot, oldest
///    event first, with the window bounded by this read's line count:
///    one event backs at most one echo (exactly-once), repeated identical
///    prompts keep their own request ids, and repeated reads stay stable
///    (the ledger is never consumed).
/// 2. Session chrome (status footers, model/context gauges) is demoted to
///    `system` so runtime metadata never poses as assistant conversation.
/// 3. Everything else keeps the existing mechanical segmentation, except
///    that raw-pane User guesses (`>`-prefix heuristics) are retired: a
///    `>`-prefixed line with NO recorded provenance is terminal-only
///    content of unknown origin (`unknown`), never a guessed human message.
///
/// `lines` is the same redacted, bounded tail the wire already serves;
/// redaction order (D9-before-segmentation) and all bounds (D5) are
/// untouched — this only re-shapes the block view additively.
pub fn canonical_blocks(
    lines: &[String],
    provenance: &crate::core::provenance::PromptProvenance,
    target: &str,
    at: Option<u64>,
) -> Vec<TranscriptBlock> {
    canonical_blocks_with_exchange(
        lines,
        provenance,
        &crate::core::provenance::ExchangeLedger::new(),
        target,
        at,
    )
}

/// #330: [`canonical_blocks`] joined against the structured exchange
/// ledger. A window line whose cleaned, redacted identity equals a recorded
/// agent-side exchange event (the agent's structured blocked question) is
/// emitted as the event's authoritative role — `assistant` → `agent`,
/// `tool` → `tool` — exactly once per read. This is the production seam
/// that keeps a supported live session's Conversation non-empty: the roles
/// come from the STRUCTURED source (Corral-observed events), never from
/// terminal prose, provider, or model names.
pub fn canonical_blocks_with_exchange(
    lines: &[String],
    provenance: &crate::core::provenance::PromptProvenance,
    exchange: &crate::core::provenance::ExchangeLedger,
    target: &str,
    at: Option<u64>,
) -> Vec<TranscriptBlock> {
    let joined = lines.join("\n");
    let cleaned = clean(&joined);
    // Which CLEANED lines are eligible typed-input echoes: the echoed text
    // (decoration-stripped) is what the ledger compares by content
    // identity. #315 R2: eligibility is STRUCTURAL — the line must carry a
    // typed-input decoration (`›`, `>`, `❯`, …) that was actually
    // stripped, i.e. it must LOOK like submitted input. An undecorated
    // line is never a candidate, so machine output that happens to equal a
    // recorded prompt (`yes`, `ok`, `continue`) can never be promoted to
    // `user` under any ledger state. Matching per cleaned line keeps echo
    // routing aligned with the segmentation input. Single-line echoes
    // cover the Corral composers (single-line editors on both clients); a
    // multi-line prompt simply does not dedupe (its echo stays unknown) —
    // never mis-attributed.
    let eligible: Vec<Option<String>> = cleaned
        .split('\n')
        .map(|line| {
            let candidate = line.trim();
            if candidate.is_empty() {
                return None;
            }
            let bare = strip_typed_input_prefix(candidate);
            (bare.len() < candidate.len()).then(|| bare.to_string())
        })
        .collect();
    // The read window is the read itself: at most one binding per eligible
    // echo slot, one-to-one against the ledger's oldest events.
    let echoed: Vec<Option<crate::core::provenance::PromptEvent>> =
        provenance.bind_echoes(target, &eligible, eligible.len());
    // #330: the agent's structured question is plain prose in the terminal
    // — every non-blank line is a candidate for the exchange ledger, bound
    // by the shared cleaned, redacted, trimmed identity.
    let exchange_candidates: Vec<Option<String>> = cleaned
        .split('\n')
        .map(crate::core::provenance::canonical_exchange_text)
        .map(|candidate| (!candidate.is_empty()).then_some(candidate))
        .collect();
    let bound_exchange =
        exchange.bind_events(target, &exchange_candidates, exchange_candidates.len());
    segment_canonical(&cleaned, &echoed, &bound_exchange, at)
}

/// Strip a typed-input decoration prefix (`›`, `>`, `❯`, `$ `, `!`) so a
/// terminal echo compares equal to the recorded prompt text. #330: also
/// strips the supported TUI's composer-echo shape — a session label before
/// the decoration (`orch-session ❯ <text>`), so a recorded prompt binds to
/// the exact echo a live session paints.
fn strip_typed_input_prefix(line: &str) -> &str {
    let mut current = line.trim_start();
    for prefix in ["›", ">", "❯", "❮", "🞈"] {
        if let Some(rest) = current.strip_prefix(prefix) {
            current = rest.trim_start();
        }
    }
    if current.len() < line.trim_start().len() {
        return current;
    }
    // #330 composer-echo shape: `<session-label> <decoration> <text>` —
    // the supported TUI paints submitted input after the session label. A
    // session label is a structured `<token>-session` value; arbitrary output
    // prefixes such as `stdout` must not unlock prompt provenance.
    if let Some((label, rest)) = current.split_once(' ')
        && is_session_label(label)
    {
        let after = rest.trim_start();
        for prefix in ["›", ">", "❯", "❮", "🞈"] {
            if let Some(text) = after.strip_prefix(prefix) {
                return text.trim_start();
            }
        }
    }
    current
}

fn is_session_label(label: &str) -> bool {
    let Some(prefix) = label.strip_suffix("-session") else {
        return false;
    };
    !prefix.is_empty()
        && prefix.chars().all(|character| {
            character.is_ascii_alphanumeric() || character == '-' || character == '_'
        })
}

/// The #315 canonical segmentation: identical mechanical cleaning to
/// [`segment_cleaned`], but line kinds come from recorded Prompt provenance
/// and shape classification that never guesses an unprovenanced human.
/// #330: `exchange_events` carries the structured agent-side event bound to
/// each cleaned line (one slot per line, `None` = unattributed) — a bound
/// event emits its authoritative role (`assistant` → `agent`, `tool` →
/// `tool`).
fn segment_canonical(
    cleaned: &str,
    echoed: &[Option<crate::core::provenance::PromptEvent>],
    exchange_events: &[Option<crate::core::provenance::ExchangeEvent>],
    at: Option<u64>,
) -> Vec<TranscriptBlock> {
    let mut blocks: Vec<TranscriptBlock> = Vec::new();
    let mut pending_truncated: Option<u32> = None;
    // (kind, lines, provenance request_id)
    let mut current: Option<(TranscriptBlockKind, Vec<String>, Option<String>)> = None;

    let flush = |blocks: &mut Vec<TranscriptBlock>,
                 current: &mut Option<(TranscriptBlockKind, Vec<String>, Option<String>)>,
                 at: Option<u64>,
                 truncated: &mut Option<u32>| {
        if let Some((kind, lines, prompt_id)) = current.take() {
            let block = TranscriptBlock {
                kind,
                text: lines.join("\n"),
                at,
                truncated_before: *truncated,
                prompt_request_id: prompt_id,
            };
            *truncated = None;
            if block.text.is_empty() {
                return;
            }
            blocks.push(block);
        }
    };

    for (line_index, line) in cleaned.split('\n').enumerate() {
        let source_echo = echoed.get(line_index).cloned().flatten();
        let source_exchange = exchange_events.get(line_index).cloned().flatten();
        if line.is_empty() {
            flush(&mut blocks, &mut current, at, &mut pending_truncated);
            continue;
        }
        if let Some(n) = parse_truncation_marker(line) {
            flush(&mut blocks, &mut current, at, &mut pending_truncated);
            pending_truncated =
                pending_truncated.map_or(Some(n), |existing| Some(existing.saturating_add(n)));
            continue;
        }

        let kind = if source_echo.is_some() {
            TranscriptBlockKind::User
        } else if let Some(event) = source_exchange.as_ref() {
            // #330: the structured event's authoritative role — the agent's
            // question is attributed by the event, never by the line's
            // prose or the source's identity.
            match event.role {
                crate::core::provenance::ExchangeRole::Assistant => TranscriptBlockKind::Agent,
                crate::core::provenance::ExchangeRole::Tool => TranscriptBlockKind::Tool,
            }
        } else if is_session_chrome(line) {
            TranscriptBlockKind::System
        } else {
            let t = line.trim_start();
            if is_system_artifact(t) {
                TranscriptBlockKind::System
            } else if is_tool_line(t) {
                TranscriptBlockKind::Tool
            } else if t.starts_with('>') {
                // #315: a `>`-prefixed line with NO recorded provenance is
                // terminal-only content of unknown origin — never guessed
                // as the operator's message.
                TranscriptBlockKind::Unknown
            } else if is_tool_output(line, current.as_ref().map(|(k, _, _)| *k)) {
                TranscriptBlockKind::Tool
            } else {
                // #315: ordinary terminal output has no role provenance —
                // it stays unknown rather than being asserted as model
                // output (the issue's "unrecognized lines fall through to
                // Agent" defect).
                TranscriptBlockKind::Unknown
            }
        };
        // A provenance-backed user block carries the DISPATCHED text (the
        // authoritative message), not the terminal's echo decoration, and
        // the signed request id for audit.
        let text = if source_echo.is_some() {
            strip_typed_input_prefix(line.trim()).to_string()
        } else {
            line.to_string()
        };
        let prompt_id = source_echo.map(|event| event.request_id);

        match &mut current {
            Some((cur_kind, _, cur_prompt)) if *cur_kind == kind && *cur_prompt == prompt_id => {
                let (_, lines, _) = current.as_mut().expect("matched arm");
                lines.push(text);
            }
            _ => {
                flush(&mut blocks, &mut current, at, &mut pending_truncated);
                current = Some((kind, vec![text], prompt_id));
            }
        }
    }
    flush(&mut blocks, &mut current, at, &mut pending_truncated);

    if let Some(n) = pending_truncated
        && let Some(last) = blocks.last_mut()
    {
        last.truncated_before = last
            .truncated_before
            .map_or(Some(n), |e| Some(e.saturating_add(n)));
    }
    blocks
}

/// The shared segmenter over a cleaned string.
fn segment_cleaned(
    cleaned: &str,
    role_hint: Option<&str>,
    at: Option<u64>,
) -> Vec<TranscriptBlock> {
    let hint = role_hint.map(TranscriptBlockKind::from_role);
    let mut blocks: Vec<TranscriptBlock> = Vec::new();
    let mut pending_truncated: Option<u32> = None;
    let mut current: Option<(TranscriptBlockKind, Vec<String>)> = None;

    let flush = |blocks: &mut Vec<TranscriptBlock>,
                 current: &mut Option<(TranscriptBlockKind, Vec<String>)>,
                 at: Option<u64>,
                 truncated: &mut Option<u32>| {
        if let Some((kind, lines)) = current.take() {
            let block = TranscriptBlock {
                kind,
                text: lines.join("\n"),
                at,
                truncated_before: *truncated,
                prompt_request_id: None,
            };
            // A marker with no following text should still surface as a
            // divider datum on the block it precedes; if there is nothing
            // before it (a marker at the very start), stamp it onto the first
            // block that follows.
            *truncated = None;
            if block.text.is_empty() {
                return;
            }
            blocks.push(block);
        }
    };

    for line in cleaned.split('\n') {
        // Pass 3 already collapsed blank runs; a single blank separates blocks.
        if line.is_empty() {
            flush(&mut blocks, &mut current, at, &mut pending_truncated);
            continue;
        }
        if let Some(n) = parse_truncation_marker(line) {
            // Close any block already in progress FIRST (it is the most recent
            // content before the elision), then carry the count onto the next
            // block. Flush leaves `pending_truncated` intact when it builds no
            // block, so consecutive markers accumulate.
            flush(&mut blocks, &mut current, at, &mut pending_truncated);
            pending_truncated =
                pending_truncated.map_or(Some(n), |existing| Some(existing.saturating_add(n)));
            continue;
        }
        let prev_kind = current.as_ref().map(|(k, _)| *k);
        let kind = classify_line(line, hint, prev_kind);
        match &mut current {
            Some((cur_kind, lines)) if *cur_kind == kind => {
                lines.push(line.to_string());
            }
            _ => {
                flush(&mut blocks, &mut current, at, &mut pending_truncated);
                current = Some((kind, vec![line.to_string()]));
            }
        }
    }
    flush(&mut blocks, &mut current, at, &mut pending_truncated);

    // A trailing marker with no following content has to land somewhere.
    if let Some(n) = pending_truncated
        && let Some(last) = blocks.last_mut()
    {
        last.truncated_before = last
            .truncated_before
            .map_or(Some(n), |e| Some(e.saturating_add(n)));
    }
    blocks
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(blocks: &[TranscriptBlock]) -> Vec<TranscriptBlockKind> {
        blocks.iter().map(|b| b.kind).collect()
    }

    #[test]
    fn cleans_ansi_csi_sequences() {
        let raw = "\u{1b}[31mred\u{1b}[0m normal";
        assert_eq!(clean(raw), "red normal");
    }

    #[test]
    fn cleans_osc_sequences_bel_and_st() {
        let bel = "\u{1b}]0;title\u{7}text";
        assert_eq!(clean(bel), "text");
        let st = "\u{1b}]0;title\u{1b}\\text";
        assert_eq!(clean(st), "text");
    }

    #[test]
    fn cleans_charset_select_escapes() {
        let raw = "\u{1b}(B\u{1b}%Ghello";
        assert_eq!(clean(raw), "hello");
    }

    #[test]
    fn resolves_cr_overdraw_to_last_segment() {
        // A progress spinner overwrites the line; the final segment wins.
        let raw = "Progress: 10%\rProgress: 50%\rProgress: 100%";
        assert_eq!(clean(raw), "Progress: 100%");
        // CRLF should not leave a trailing \r (a final \n is preserved).
        let crlf = "line1\r\nline2";
        assert_eq!(clean(crlf), "line1\nline2");
    }

    #[test]
    fn normalizes_whitespace() {
        let raw = "  a\tb  \n\n\n\n\nc";
        // tab -> 4 spaces, right-trim, and 5 blanks collapse to 2.
        assert_eq!(clean(raw), "  a    b\n\n\nc");
    }

    #[test]
    fn no_hard_wrap_long_line() {
        let raw = "x".repeat(500);
        assert_eq!(clean(&raw), raw, "long lines pass through intact");
    }

    #[test]
    fn parses_truncation_marker_variants() {
        for line in [
            "... +229 lines (ctrl+t to view transcript)",
            "... +229 lines (ctrl + t to view transcript)",
            "… +40 lines (ctrl+t …)",
            ". . . +7 lines",
        ] {
            let n = parse_truncation_marker(line).unwrap_or_else(|| panic!("marker: {line}"));
            assert!(n >= 7, "marker {line} parsed {n}");
        }
        assert!(parse_truncation_marker("plain output").is_none());
        assert!(parse_truncation_marker("... +99 things (ctrl+t)").is_none());
    }

    #[test]
    fn segment_maps_transcript_roles() {
        let raw = "hello";
        let user = segment(raw, Some("user"), Some(1));
        assert_eq!(user.len(), 1);
        assert_eq!(user[0].kind, TranscriptBlockKind::User);
        assert_eq!(user[0].text, "hello");
        assert_eq!(user[0].at, Some(1));
        let agent = segment(raw, Some("assistant"), None);
        assert_eq!(agent[0].kind, TranscriptBlockKind::Agent);
        let tool = segment(raw, Some("tool"), None);
        assert_eq!(tool[0].kind, TranscriptBlockKind::Tool);
        let sys = segment(raw, Some("developer"), None);
        assert_eq!(sys[0].kind, TranscriptBlockKind::System);
    }

    #[test]
    fn segment_classifies_raw_pane_shape() {
        let raw = "\u{1b}[32m$ cargo build\u{1b}[0m\nCompiling corrald v0.1.0\n> pilot: make it chat\n\u{25cf} Ask Codex to do anything";
        let blocks = segment(raw, None, None);
        assert_eq!(
            kinds(&blocks),
            vec![
                TranscriptBlockKind::Tool,
                TranscriptBlockKind::User,
                TranscriptBlockKind::System,
            ]
        );
    }

    #[test]
    fn segment_lifts_truncation_marker_and_removes_text() {
        let raw = "... +229 lines (ctrl+t to view transcript)\nRight — collapsing Full chat into Recent output.";
        let blocks = segment(raw, None, None);
        assert_eq!(blocks.len(), 1);
        assert_eq!(
            blocks[0].text,
            "Right — collapsing Full chat into Recent output."
        );
        assert_eq!(blocks[0].truncated_before, Some(229));
        assert!(!raw_contains_marker(&blocks, "ctrl+t"));
    }

    fn raw_contains_marker(blocks: &[TranscriptBlock], needle: &str) -> bool {
        blocks.iter().any(|b| b.text.contains(needle))
    }

    #[test]
    fn segment_splits_on_blanks_and_kind_changes() {
        let raw = "outline\n\n$ ls\n\ndetail";
        let blocks = segment(raw, None, None);
        assert_eq!(
            kinds(&blocks),
            vec![
                TranscriptBlockKind::Agent,
                TranscriptBlockKind::Tool,
                TranscriptBlockKind::Agent,
            ]
        );
        assert_eq!(blocks[0].text, "outline");
        assert_eq!(blocks[1].text, "$ ls");
    }

    #[test]
    fn segment_keeps_same_kind_lines_together_until_blank() {
        // A blank line ends the tool block; without it the tool output would
        // absorb the compile line (see `fixture_codex_pane_segments_exactly`).
        let raw = "$ cargo build\n$ cargo test\n\nCompiling done";
        let blocks = segment(raw, None, None);
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0].kind, TranscriptBlockKind::Tool);
        assert_eq!(blocks[0].text, "$ cargo build\n$ cargo test");
        assert_eq!(blocks[1].text, "Compiling done");
    }

    #[test]
    fn segment_does_not_delete_system_artifacts() {
        // AC7: "Missing environment variable…" is diagnostic signal, demoted
        // to System, never dropped.
        let raw = "Missing environment variable: OPENROUTER_API_KEY";
        let blocks = segment(raw, None, None);
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].kind, TranscriptBlockKind::System);
        assert_eq!(
            blocks[0].text,
            "Missing environment variable: OPENROUTER_API_KEY"
        );
    }

    // ---------------------------------------------------------------------
    // #253 scrubber unit tests (fixture inputs: hermes TUI frame box,
    // progress bars, code block with `─` in a string that must survive)
    // ---------------------------------------------------------------------

    #[test]
    fn scrubber_collapses_hermes_tui_frame_borders() {
        // Painted (ANSI-colored) top edge — the real herdr/hermes shape.
        let top = "\u{1b}[38;5;8m╭──────────────────────────────────────────────┐\u{1b}[0m";
        assert_eq!(scrub_tui_furniture(top), DIVIDER_MARKER);
        // Content line inside the frame: only the two `│` rails touch it.
        let content = "│ model: claude-sonnet-4-5  ·  ready                                   │";
        assert_eq!(scrub_tui_furniture(content), content);
        // Heavy/double rules and the bottom edge collapse too.
        let heavy = "╠══════════════════════════════════════════════════╣";
        assert_eq!(scrub_tui_furniture(heavy), DIVIDER_MARKER);
        let bottom = "╰────────────────────────────────────────────────╯";
        assert_eq!(scrub_tui_furniture(bottom), DIVIDER_MARKER);
        // A border that carries a title (ratatui-style) still collapses
        // its runs; the title text stays.
        let titled = format!("╭{} Hermes Agent {}╮", "─".repeat(30), "─".repeat(30));
        assert_eq!(scrub_tui_furniture(&titled), "─── Hermes Agent ───");
        // Interior-only rail line: dropped.
        let interior = "│                                                                    │";
        assert_eq!(scrub_tui_furniture(interior), "");
    }

    #[test]
    fn scrubber_compacts_progress_block_runs() {
        // Painter progress line: label + block run + percent. ▓×16 ░×4 →
        // dark 16/20 → 8 filled cells out of 10.
        let painter = format!(
            "painter {}\u{2591}\u{2591}\u{2591}\u{2591} 80%",
            "\u{2593}".repeat(16)
        );
        assert_eq!(
            scrub_tui_furniture(&painter),
            "painter \u{25B0}\u{25B0}\u{25B0}\u{25B0}\u{25B0}\u{25B0}\u{25B0}\u{25B0}\u{25B1}\u{25B1} 80%"
        );
        // Framed bar line: ▐▓×9░░▌ → dark 11/13 → 8 filled cells.
        let bar = "\u{2590}\u{2593}\u{2593}\u{2593}\u{2593}\u{2593}\u{2593}\u{2593}\u{2593}\u{2593}\u{2591}\u{2591}\u{258C} 12%";
        assert_eq!(
            scrub_tui_furniture(bar),
            "\u{25B0}\u{25B0}\u{25B0}\u{25B0}\u{25B0}\u{25B0}\u{25B0}\u{25B0}\u{25B1}\u{25B1} 12%"
        );
    }

    #[test]
    fn scrubber_keeps_dash_runs_inside_code_strings() {
        // The issue's explicit keep-in-str case: a real string holding a
        // long `─` run must survive (it is not border-shaped).
        let code = "let sep = \"───────────────\";";
        assert_eq!(scrub_tui_furniture(code), code);
        // Even a quoted, fully-dash string literal survives: quotes make it
        // content, not a border.
        let quoted = "\"╭────────────────────╮\"";
        assert_eq!(scrub_tui_furniture(quoted), quoted);
        // A short frame corner is not a long run: keep as-is.
        let corner = "┌──┐";
        assert_eq!(scrub_tui_furniture(corner), corner);
    }

    #[test]
    fn scrubber_keeps_prose_and_short_runs_untouched() {
        let prose = "  → Waiting on your decision…";
        assert_eq!(scrub_tui_furniture(prose), prose);
        let code = "let x = '─';";
        assert_eq!(scrub_tui_furniture(code), code);
        // A rail line with long dash runs and a label: the long runs
        // collapse, the label text survives (not a real border, but the
        // runs are exactly the dash junk the issue targets).
        let mixed = "│──────────────── 40% done ────────────────│";
        assert_eq!(scrub_tui_furniture(mixed), "─── 40% done ───");
    }

    // ---------------------------------------------------------------------
    // AC1/AC2 table-driven fixture tests (captured raw pane text)
    // ---------------------------------------------------------------------

    /// One expected block per line: (kind, text, truncated_before).
    type Expect = (TranscriptBlockKind, &'static str, Option<u32>);

    const CODEX_PANE: &str = include_str!("../../tests/fixtures/transcript_blocks/codex_pane.txt");
    const CLAUDE_PANE: &str =
        include_str!("../../tests/fixtures/transcript_blocks/claude_pane.txt");

    #[test]
    fn fixture_codex_pane_segments_exactly() {
        // AC1: real ANSI color codes + `\r` overdraw + `... +N lines` marker +
        // codex TUI chrome + a user prompt line + tool output. Assert the
        // per-block kind/text/truncated_before, never a fuzzy count.
        let expected: Vec<Expect> = vec![
            // The command + its build/progress output form ONE tool block;
            // the two progress writes collapse to the final segment.
            (
                TranscriptBlockKind::Tool,
                "$ cargo build\nCompiling corrald v0.1.0\nBuild progress: 100%",
                None,
            ),
            // The marker is lifted and REMOVED from text; the block that
            // follows it carries the count.
            (
                TranscriptBlockKind::System,
                "\u{2022} Ask Codex to do anything",
                Some(229),
            ),
            (
                TranscriptBlockKind::User,
                "> pilot: make the transcript read as chat, not a terminal blob",
                None,
            ),
            (
                TranscriptBlockKind::Agent,
                "Right — collapsing Full chat into Recent output, live at the bottom, scroll up for older.",
                None,
            ),
            (
                TranscriptBlockKind::System,
                "Missing environment variable: OPENROUTER_API_KEY",
                None,
            ),
        ];
        let blocks = segment(CODEX_PANE, None, None);
        assert_expected(&blocks, &expected);
    }

    #[test]
    fn fixture_claude_pane_segments_exactly() {
        // AC2: an OSC sequence, a codex TUI box-drawing rule, a tool command,
        // a `... +N lines` marker with spaces, and a stray `"Test"` artifact.
        let expected: Vec<Expect> = vec![
            // The OSC title + `Working` color resolve to plain agent text.
            (TranscriptBlockKind::Agent, "Working on issue #167", None),
            // Box-drawing rule is System chrome.
            (
                TranscriptBlockKind::System,
                "\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}",
                None,
            ),
            // The command + its test output stay one tool block.
            (
                TranscriptBlockKind::Tool,
                "$ cargo test\ntest result: ok. 42 passed",
                None,
            ),
            // The marker carries count 7 onto the following block ("Test").
            (TranscriptBlockKind::System, "\"Test\"", Some(7)),
        ];
        let blocks = segment(CLAUDE_PANE, None, None);
        assert_expected(&blocks, &expected);
    }

    fn assert_expected(blocks: &[TranscriptBlock], expected: &[Expect]) {
        assert_eq!(
            blocks.len(),
            expected.len(),
            "block count mismatch:\n{:#?}",
            blocks
        );
        for (i, (block, (kind, text, truncated))) in blocks.iter().zip(expected).enumerate() {
            assert_eq!(block.kind, *kind, "block {i} kind mismatch: {:?}", block);
            assert_eq!(block.text, *text, "block {i} text mismatch");
            assert_eq!(
                block.truncated_before, *truncated,
                "block {i} truncated_before mismatch"
            );
            // The marker text must never ride in block text (AC1/AC2).
            assert!(
                !block.text.contains("ctrl+t") && !block.text.contains("ctrl + t"),
                "block {i} leaked a ctrl+t marker"
            );
        }
    }

    // ---- #315 R2: session-chrome shape guards (F6) ----

    #[test]
    fn ordinary_prose_with_percent_or_model_words_stays_out_of_system() {
        // Ordinary conversation prose that happens to mention model/context
        // + a percent must NOT be demoted to session chrome.
        for line in [
            "The model achieved 98% accuracy on the benchmark.",
            "In this context 40% of requests were cached.",
            "The model answers with 95% confidence, then retries.",
        ] {
            let blocks = canonical_blocks(
                &[line.to_string()],
                &crate::core::provenance::PromptProvenance::new(),
                "t",
                None,
            );
            assert_ne!(
                kinds(&blocks),
                vec![TranscriptBlockKind::System],
                "ordinary prose demoted to system: {line}"
            );
        }
    }

    #[test]
    fn real_session_gauge_rows_stay_system() {
        // The REAL structured gauge/footer shapes: short rows whose
        // model/context words are the SESSION SUBJECT (leading), not
        // prose about a model.
        for line in [
            "model context 42% · tokens in flight",
            "context left 12k",
            "Model: opus · context 42%",
        ] {
            let blocks = canonical_blocks(
                &[line.to_string()],
                &crate::core::provenance::PromptProvenance::new(),
                "t",
                None,
            );
            assert_eq!(
                kinds(&blocks),
                vec![TranscriptBlockKind::System],
                "real gauge row must stay system: {line}"
            );
        }
    }

    #[test]
    fn esc_footer_hints_stay_system_only_in_short_rows() {
        // Real footer hint (short row) stays system...
        let blocks = canonical_blocks(
            &["status: working · esc to interrupt".to_string()],
            &crate::core::provenance::PromptProvenance::new(),
            "t",
            None,
        );
        assert_eq!(kinds(&blocks), vec![TranscriptBlockKind::System]);
        // ...while long prose that merely mentions esc is NOT demoted.
        let prose = "The wizard said to press esc to cancel the spell, then the \
                     model answered at length about the remaining context left in the buffer.";
        let blocks = canonical_blocks(
            &[prose.to_string()],
            &crate::core::provenance::PromptProvenance::new(),
            "t",
            None,
        );
        assert_ne!(
            kinds(&blocks),
            vec![TranscriptBlockKind::System],
            "long prose mentioning esc demoted to system"
        );
    }

    // ---- #315 R2: typed-input echo eligibility + window (F1/F2/F3) ----

    fn prov_with(events: &[(&str, &str)]) -> crate::core::provenance::PromptProvenance {
        let prov = crate::core::provenance::PromptProvenance::new();
        for (i, (rid, text)) in events.iter().enumerate() {
            prov.record(crate::core::provenance::PromptEvent::new(
                rid, "t", text, i as u64,
            ));
        }
        prov
    }

    #[test]
    fn decorated_echo_binds_once_and_extra_echoes_stay_unknown() {
        // One event, transcript echo + duplicate composer echo: exactly one
        // eligible echo binds (one user block), the extra stays unknown.
        let prov = prov_with(&[("req-9", "ship it")]);
        let blocks = canonical_blocks(
            &[
                "> ship it".into(),
                "".into(),
                "Working on it.".into(),
                "".into(),
                "› ship it".into(),
            ],
            &prov,
            "t",
            None,
        );
        let users: Vec<&TranscriptBlock> = blocks
            .iter()
            .filter(|b| b.kind == TranscriptBlockKind::User)
            .collect();
        assert_eq!(users.len(), 1, "exactly one user block: {blocks:#?}");
        assert_eq!(users[0].prompt_request_id.as_deref(), Some("req-9"));
        assert_eq!(users[0].text, "ship it");
    }

    #[test]
    fn repeated_identical_prompts_bind_in_ledger_order() {
        let prov = prov_with(&[("req-A", "continue"), ("req-B", "continue")]);
        let blocks = canonical_blocks(
            &["> continue".into(), "".into(), "> continue".into()],
            &prov,
            "t",
            None,
        );
        let ids: Vec<Option<&str>> = blocks
            .iter()
            .filter(|b| b.kind == TranscriptBlockKind::User)
            .map(|b| b.prompt_request_id.as_deref())
            .collect();
        assert_eq!(ids, vec![Some("req-A"), Some("req-B")]);
    }

    #[test]
    fn unprefixed_line_never_binds_even_when_text_matches() {
        // No typed-input decoration → not an eligible echo, even though the
        // text equals a recorded prompt (the false-attribution guard).
        let prov = prov_with(&[("req-old", "yes")]);
        let blocks = canonical_blocks(&["yes".into(), "".into(), "done".into()], &prov, "t", None);
        assert!(
            blocks.iter().all(|b| b.kind != TranscriptBlockKind::User),
            "unprefixed match promoted to user: {blocks:#?}"
        );
    }

    #[test]
    fn undecorated_line_is_never_an_echo_candidate() {
        // #315 R2 strict rule: eligibility is STRUCTURAL (a typed-input
        // decoration must be present), so an undecorated line is never a
        // candidate — even when it equals the recorded prompt and nothing
        // else in the pane could claim the binding.
        let prov = prov_with(&[("req-1", "ship the canonical transcript stream")]);
        let blocks = canonical_blocks(
            &["ship the canonical transcript stream".into()],
            &prov,
            "t",
            None,
        );
        assert!(
            blocks.iter().all(|b| b.kind != TranscriptBlockKind::User),
            "undecorated line promoted to user: {blocks:#?}"
        );
    }

    #[test]
    fn machine_output_prefix_does_not_unlock_composer_echo() {
        let prov = prov_with(&[("req-yes", "yes")]);
        let blocks = canonical_blocks(&["stdout > yes".into()], &prov, "t", None);
        assert_eq!(
            kinds(&blocks),
            vec![TranscriptBlockKind::System],
            "ordinary stdout output must stay System: {blocks:#?}"
        );
        assert!(blocks.iter().all(|block| block.prompt_request_id.is_none()));
    }

    #[test]
    fn composer_echo_contract_preserves_build_and_unsupported_shapes() {
        let build = canonical_blocks(
            &["build-session > yes".into()],
            &prov_with(&[("req-build", "yes")]),
            "t",
            None,
        );
        assert_eq!(kinds(&build), vec![TranscriptBlockKind::User]);
        assert_eq!(build[0].prompt_request_id.as_deref(), Some("req-build"));

        let unsupported = canonical_blocks(
            &["corral-wQ-1 ❯ yes".into()],
            &prov_with(&[("req-unsupported", "yes")]),
            "t",
            None,
        );
        assert_eq!(
            kinds(&unsupported),
            vec![TranscriptBlockKind::Unknown],
            "an unsupported session label must not unlock User: {unsupported:#?}"
        );
        assert!(
            unsupported
                .iter()
                .all(|block| block.prompt_request_id.is_none())
        );
    }

    // ---------------------------------------------------------------------
    // #330: the supported live TUI's composer echo shape. The session TUI
    // paints submitted input as "<session-label> ❯ <text>" — a typed-input
    // decoration after the session label. A recorded Corral Prompt whose
    // text equals the echoed text must bind to exactly that one echo, like a
    // leading `>`/`›` decoration (this is what keeps a real live session's
    // Conversation non-empty).
    // ---------------------------------------------------------------------

    #[test]
    fn composer_echo_shape_binds_recorded_prompts_exactly_once() {
        let prov = prov_with(&[("req-echo", "ship the canonical transcript stream")]);
        let blocks = canonical_blocks(
            &[
                "orch-session ❯ ship the canonical transcript stream".into(),
                "".into(),
                "Canonical stream wired end to end.".into(),
            ],
            &prov,
            "t",
            None,
        );
        let users: Vec<&TranscriptBlock> = blocks
            .iter()
            .filter(|b| b.kind == TranscriptBlockKind::User)
            .collect();
        assert_eq!(
            users.len(),
            1,
            "composer echo binds exactly once: {blocks:#?}"
        );
        assert_eq!(users[0].prompt_request_id.as_deref(), Some("req-echo"));
        assert_eq!(users[0].text, "ship the canonical transcript stream");
        let echoed_text = blocks
            .iter()
            .filter(|b| b.text.contains("ship the canonical transcript stream"))
            .count();
        assert_eq!(
            echoed_text, 1,
            "the echo is deduplicated against the recorded prompt"
        );
    }

    // ---------------------------------------------------------------------
    // #330: the authoritative structured-role seam. Corral observes the
    // agent's STRUCTURED blocked-question events (pane.output_matched →
    // waiting_on) and records them with authoritative roles (an
    // approve-tool request is a Tool event, a question/menu is an Assistant
    // event). The canonical stream joins the terminal snapshot against that
    // ledger exactly-once, so a supported live session produces Agent/Tool
    // conversation blocks without any prose inspection.
    // ---------------------------------------------------------------------

    fn exchange_with(
        events: &[(&str, &str, crate::core::provenance::ExchangeRole)],
    ) -> crate::core::provenance::ExchangeLedger {
        let ledger = crate::core::provenance::ExchangeLedger::new();
        for (i, (id, text, role)) in events.iter().enumerate() {
            ledger.record(crate::core::provenance::ExchangeEvent::new(
                id, "t", *role, text, i as u64,
            ));
        }
        ledger
    }

    #[test]
    fn exchange_assistant_event_binds_to_agent_block() {
        let ledger = exchange_with(&[(
            "q-1",
            "Should I proceed with the destructive migration?",
            crate::core::provenance::ExchangeRole::Assistant,
        )]);
        let blocks = crate::core::blocks::canonical_blocks_with_exchange(
            &[
                "Working on the migration.".into(),
                "".into(),
                "Should I proceed with the destructive migration?".into(),
                "".into(),
                "status: working · esc to interrupt".into(),
            ],
            &crate::core::provenance::PromptProvenance::new(),
            &ledger,
            "t",
            None,
        );
        assert_eq!(
            kinds(&blocks),
            vec![
                TranscriptBlockKind::Unknown,
                TranscriptBlockKind::Agent,
                TranscriptBlockKind::System,
            ],
            "the structured assistant question must render as Agent: {blocks:#?}"
        );
        assert_eq!(
            blocks[1].text,
            "Should I proceed with the destructive migration?"
        );
    }

    #[test]
    fn exchange_tool_event_binds_to_tool_block() {
        let ledger = exchange_with(&[(
            "q-2",
            "Approve this change to push 2 commits to demo-catalog-v2?",
            crate::core::provenance::ExchangeRole::Tool,
        )]);
        let blocks = crate::core::blocks::canonical_blocks_with_exchange(
            &["Approve this change to push 2 commits to demo-catalog-v2?".into()],
            &crate::core::provenance::PromptProvenance::new(),
            &ledger,
            "t",
            None,
        );
        assert_eq!(
            kinds(&blocks),
            vec![TranscriptBlockKind::Tool],
            "the structured approve-tool request must render as Tool: {blocks:#?}"
        );
    }

    #[test]
    fn exchange_event_absent_keeps_line_unknown() {
        // #330 AC7 baseline: with NO structured role source the same window
        // stays honest Unknown — nothing is guessed from the prose.
        let blocks = crate::core::blocks::canonical_blocks_with_exchange(
            &[
                "Working on the migration.".into(),
                "".into(),
                "Should I proceed with the destructive migration?".into(),
            ],
            &crate::core::provenance::PromptProvenance::new(),
            &crate::core::provenance::ExchangeLedger::new(),
            "t",
            None,
        );
        assert!(
            blocks
                .iter()
                .all(|b| b.kind == TranscriptBlockKind::Unknown),
            "without the structured role source nothing may be attributed: {blocks:#?}"
        );
    }

    #[test]
    fn exchange_event_renders_exactly_once_per_read() {
        // One structured event, the question echoed twice in the window:
        // exactly one block binds (one-to-one per read), the duplicate stays
        // unknown — no double attribution.
        let ledger = exchange_with(&[(
            "q-1",
            "Should I proceed with the destructive migration?",
            crate::core::provenance::ExchangeRole::Assistant,
        )]);
        let blocks = crate::core::blocks::canonical_blocks_with_exchange(
            &[
                "Should I proceed with the destructive migration?".into(),
                "".into(),
                "Should I proceed with the destructive migration?".into(),
            ],
            &crate::core::provenance::PromptProvenance::new(),
            &ledger,
            "t",
            None,
        );
        let agents: Vec<&TranscriptBlock> = blocks
            .iter()
            .filter(|b| b.kind == TranscriptBlockKind::Agent)
            .collect();
        assert_eq!(
            agents.len(),
            1,
            "the structured event binds exactly once per read: {blocks:#?}"
        );
    }
}
