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

/// The four block kinds a client may render (D7). No chat bubbles, no
/// per-message timestamps — the client renders these with minimal contrast.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TranscriptBlockKind {
    /// The operator's prompt / message. Rendered trailing-aligned + tinted.
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
/// immediately precedes this block (absent = no marker).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TranscriptBlock {
    pub kind: TranscriptBlockKind,
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub truncated_before: Option<u32>,
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
}
