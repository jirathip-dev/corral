//! Secret redaction (D9/D13): ONE shared pass that strips secret-shaped text
//! before any bytes leave the machine.
//!
//! ## Where it is applied
//!
//! At the ADAPTER boundary, on ingest: the herdr adapter redacts pane text
//! (`waiting_on` prompts/choices, reasons, titles, display names) before it
//! becomes a canonical record, so every downstream serialization — `GET
//! /snapshot`, SSE deltas, drive responses, audit entries — is redacted by
//! construction. The stored canonical record keeps what it needs (identity,
//! state, prompt text minus the secret spans); only secret-shaped text is
//! replaced with [`REDACTED`].
//!
//! ## Rules (conservative by construction)
//!
//! False positives (redacting harmless text) cost nothing; false negatives
//! leak secrets. When in doubt, redact. The five rules:
//!
//! 1. `sk-ant-*` — Anthropic API keys (the full token, including the
//!    `sk-ant-` prefix). A bare `sk-ant-` with nothing after it is not
//!    redacted.
//! 2. `ghp_*` — GitHub personal access tokens (the full token).
//! 3. `AKIA*` — AWS access key IDs (`AKIA` + at least one `[A-Z0-9]`;
//!    production IDs are `AKIA` + 16).
//! 4. High-entropy runs — an alphanumeric run of ≥ 24 chars containing a
//!    lowercase letter AND an uppercase letter AND a digit. This is the
//!    conservative reading of "mixed case/digits": a 64-char lowercase hex
//!    digest (commit/sha-like) or an all-uppercase base32 string is NOT a
//!    secret by this rule and passes through untouched.
//! 5. `.env`-shaped assignments — `NAME=VALUE` where NAME (underscore-
//!    separated segments, trailing digits ignored) contains a secret-ish
//!    segment: TOKEN, SECRET, PASSWORD, PASSWD, KEY, CREDENTIAL, API, AUTH.
//!    The VALUE (rest of the line) is redacted; the NAME stays visible so
//!    the display says which setting was scrubbed. Empty values are kept.
//!
//! Prefix rules (1-3) only fire at a word boundary — never mid-token — so
//! prose like "mysk-ant-thing" is untouched.
//!
//! Not covered (by design): paths/identifiers (`worktree_path`, pane ids),
//! which are identity, not display text; `github_pat_*` tokens are only
//! caught when their body is long enough for rule 4.

use std::borrow::Cow;

/// Marker every redacted span is replaced with.
pub const REDACTED: &str = "[REDACTED]";

const ANTHROPIC_PREFIX: &str = "sk-ant-";
const GH_PAT_PREFIX: &str = "ghp_";
const AWS_KEY_PREFIX: &str = "AKIA";

/// Conservative high-entropy threshold: runs shorter than this are kept even
/// when mixed-case (short IDs are common in prose).
const HIGH_ENTROPY_MIN_LEN: usize = 24;

/// Secret-ish `.env` name segments (compared case-insensitively, after
/// trimming trailing digits and splitting on `_`).
const SECRET_NAME_SEGMENTS: [&str; 8] = [
    "api",
    "auth",
    "credential",
    "key",
    "password",
    "passwd",
    "secret",
    "token",
];

/// Redact every secret-shaped span in `input`. Returns `Cow::Borrowed` when
/// nothing matched, so ordinary prose never allocates.
///
/// Idempotent: `redact(redact(s)) == redact(s)` — [`REDACTED`] matches no
/// rule.
pub fn redact<'a>(input: &'a str) -> Cow<'a, str> {
    let bytes = input.as_bytes();
    let mut spans: Vec<(usize, usize)> = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        // A token abutting a previous match starts at a real word boundary
        // even though the char before it is a token char (e.g. an AKIA id
        // directly followed by a ghp_ token).
        let at_span_edge = spans.last().is_some_and(|(_, end)| *end == i);
        if let Some((start, end)) = prefix_token_at(input, i, at_span_edge) {
            spans.push((start, end));
            i = end;
            continue;
        }
        if let Some((start, end)) = env_value_at(input, i) {
            spans.push((start, end));
            i = end;
            continue;
        }
        if let Some((start, end)) = high_entropy_at(input, i) {
            spans.push((start, end));
            i = end;
            continue;
        }
        // ASCII-only patterns: one-byte steps are safe through UTF-8.
        i += 1;
    }
    if spans.is_empty() {
        return Cow::Borrowed(input);
    }
    // Merge touching spans so adjacent tokens collapse into one marker.
    let mut merged: Vec<(usize, usize)> = Vec::with_capacity(spans.len());
    for (start, end) in spans {
        if let Some(last) = merged.last_mut()
            && last.1 == start
        {
            last.1 = end;
        } else {
            merged.push((start, end));
        }
    }
    let mut out = String::with_capacity(input.len());
    let mut cursor = 0;
    for (start, end) in merged {
        out.push_str(&input[cursor..start]);
        out.push_str(REDACTED);
        cursor = end;
    }
    out.push_str(&input[cursor..]);
    Cow::Owned(out)
}

/// Rules 1-3: known token prefixes at a word boundary, followed by at least
/// one continuation character. Returns the full token span. `at_span_edge`
/// marks a position where a previous rule already matched (the char before
/// is the tail of that match, not a mid-word position).
fn prefix_token_at(input: &str, i: usize, at_span_edge: bool) -> Option<(usize, usize)> {
    let bytes = input.as_bytes();
    if !at_span_edge && i > 0 && is_token_char(bytes[i - 1]) {
        return None; // mid-token: not a standalone secret
    }
    let rest = &bytes[i..];
    let (prefix, kind) = if rest.starts_with(ANTHROPIC_PREFIX.as_bytes()) {
        (ANTHROPIC_PREFIX, PrefixKind::Token)
    } else if rest.starts_with(GH_PAT_PREFIX.as_bytes()) {
        (GH_PAT_PREFIX, PrefixKind::Alnum)
    } else if rest.starts_with(AWS_KEY_PREFIX.as_bytes()) {
        (AWS_KEY_PREFIX, PrefixKind::UpperDigit)
    } else {
        return None;
    };
    let mut j = i + prefix.len();
    match kind {
        PrefixKind::Token => {
            while j < bytes.len() && is_token_char(bytes[j]) {
                j += 1;
            }
        }
        PrefixKind::Alnum => {
            while j < bytes.len() && bytes[j].is_ascii_alphanumeric() {
                j += 1;
            }
        }
        PrefixKind::UpperDigit => {
            while j < bytes.len() && (bytes[j].is_ascii_uppercase() || bytes[j].is_ascii_digit()) {
                j += 1;
            }
        }
    }
    (j > i + prefix.len()).then_some((i, j))
}

#[derive(Clone, Copy)]
enum PrefixKind {
    /// `sk-ant-`: token chars include `_` and `-` (Anthropic keys are
    /// `sk-ant-api03-` + base64-ish payload).
    Token,
    /// `ghp_`: alphanumerics only.
    Alnum,
    /// `AKIA`: uppercase + digits (AWS access key IDs).
    UpperDigit,
}

/// Rule 5: a secret-named `NAME=VALUE` assignment — redact the VALUE (rest
/// of the line after `=`), keep `NAME=` visible so the display says which
/// setting was scrubbed.
fn env_value_at(input: &str, i: usize) -> Option<(usize, usize)> {
    let bytes = input.as_bytes();
    if !is_ident_start(bytes[i]) || (i > 0 && is_ident_char(bytes[i - 1])) {
        return None;
    }
    let mut j = i + 1;
    while j < bytes.len() && is_ident_char(bytes[j]) {
        j += 1;
    }
    if j >= bytes.len() || bytes[j] != b'=' || !name_is_secret(&input[i..j]) {
        return None;
    }
    let mut k = j + 1;
    while k < bytes.len() && bytes[k] != b'\n' {
        k += 1;
    }
    (k > j + 1).then_some((j + 1, k))
}

/// Rule 5's name test: any underscore-separated segment (trailing digits
/// trimmed) is a secret-ish name segment.
fn name_is_secret(name: &str) -> bool {
    name.to_ascii_lowercase().split('_').any(|segment| {
        let segment = segment.trim_end_matches(|c: char| c.is_ascii_digit());
        SECRET_NAME_SEGMENTS.contains(&segment)
    })
}

/// Rule 4: a maximal alphanumeric run of ≥ 24 chars containing lowercase AND
/// uppercase AND a digit.
fn high_entropy_at(input: &str, i: usize) -> Option<(usize, usize)> {
    let bytes = input.as_bytes();
    if !bytes[i].is_ascii_alphanumeric() {
        return None;
    }
    let mut j = i;
    let (mut lower, mut upper, mut digit) = (false, false, false);
    while j < bytes.len() && bytes[j].is_ascii_alphanumeric() {
        let b = bytes[j];
        lower |= b.is_ascii_lowercase();
        upper |= b.is_ascii_uppercase();
        digit |= b.is_ascii_digit();
        j += 1;
    }
    (j - i >= HIGH_ENTROPY_MIN_LEN && lower && upper && digit).then_some((i, j))
}

fn is_ident_start(b: u8) -> bool {
    b.is_ascii_alphabetic() || b == b'_'
}

fn is_ident_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

fn is_token_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'-'
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One documented Anthropic key shape: `sk-ant-api03-` + 24 mixed chars.
    const SK_ANT: &str = "sk-ant-api03-AB12cdEF34ghIJ56klMN78op";
    /// One documented GitHub PAT shape: `ghp_` + 36 mixed chars.
    const GHP: &str = "ghp_AbCdEfGhIjKlMnOpQrStUvWxYz1234567890";
    /// One documented AWS access key ID shape: `AKIA` + 16 uppercase/digits.
    const AKIA: &str = "AKIA1234567890ABCDEF";

    fn redacted(s: &str) -> String {
        redact(s).into_owned()
    }

    #[test]
    fn anthropic_keys_are_redacted() {
        assert_eq!(redacted(SK_ANT), "[REDACTED]");
        assert_eq!(redacted("sk-ant-xxx"), "[REDACTED]", "short form still a key");
        assert_eq!(
            redacted(&format!("use {SK_ANT} in the call")),
            "use [REDACTED] in the call"
        );
        assert_eq!(redacted("sk-ant-"), "sk-ant-", "bare prefix is not a secret");
        assert_eq!(
            redacted("mysk-ant-abc"),
            "mysk-ant-abc",
            "mid-word occurrence is not a standalone token"
        );
    }

    #[test]
    fn github_pats_are_redacted() {
        assert_eq!(redacted(GHP), "[REDACTED]");
        assert_eq!(redacted("ghp_yyy"), "[REDACTED]", "short form still a PAT");
        assert_eq!(
            redacted(&format!("token is {GHP} end")),
            "token is [REDACTED] end"
        );
        assert_eq!(redacted("ghp_"), "ghp_", "bare prefix is not a secret");
    }

    #[test]
    fn aws_access_key_ids_are_redacted() {
        assert_eq!(redacted(AKIA), "[REDACTED]");
        assert_eq!(redacted("AKIAZZZZ"), "[REDACTED]", "short form still a key id");
        assert_eq!(
            redacted(&format!("access key {AKIA} for the account")),
            "access key [REDACTED] for the account"
        );
        assert_eq!(redacted("AKIA"), "AKIA", "bare prefix is not a secret");
    }

    #[test]
    fn the_live_test_shape_redacts_all_three_prefixes() {
        assert_eq!(redacted("sk-ant-xxx ghp_yyy AKIAZZZZ"), "[REDACTED] [REDACTED] [REDACTED]");
    }

    #[test]
    fn multiple_and_adjacent_tokens_collapse() {
        let text = format!("{GHP} {AKIA} {SK_ANT}");
        assert_eq!(redacted(&text), "[REDACTED] [REDACTED] [REDACTED]");
        // Adjacent tokens merge into one marker.
        assert_eq!(redacted(&format!("{AKIA}{GHP}")), "[REDACTED]");
    }

    #[test]
    fn high_entropy_runs_are_redacted_conservatively() {
        let mixed = "AbCdEf123456AbCdEf123456"; // exactly 24 chars, all three classes
        assert_eq!(redacted(mixed), "[REDACTED]");
        assert_eq!(
            redacted("AbCdEf123456AbCdEf12345"),
            "AbCdEf123456AbCdEf12345",
            "23 chars: too short"
        );
        assert_eq!(
            redacted("abcdef0123456789abcdef0123456789abcdef"),
            "abcdef0123456789abcdef0123456789abcdef",
            "hex digest: lowercase+digits only, not a secret by rule 4"
        );
        assert_eq!(
            redacted("ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789"),
            "ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789",
            "base32-ish: uppercase+digits only, no lowercase"
        );
        assert_eq!(
            redacted(&format!("key {mixed} in env")),
            "key [REDACTED] in env"
        );
    }

    #[test]
    fn env_shaped_lines_redact_values_only() {
        assert_eq!(redacted("API_KEY=abc123"), "API_KEY=[REDACTED]");
        assert_eq!(redacted("GITHUB_TOKEN=xyz"), "GITHUB_TOKEN=[REDACTED]");
        assert_eq!(
            redacted("AWS_SECRET_ACCESS_KEY=supersecret"),
            "AWS_SECRET_ACCESS_KEY=[REDACTED]"
        );
        assert_eq!(redacted("DB_PASSWORD=postgres"), "DB_PASSWORD=[REDACTED]");
        assert_eq!(redacted("KEY1=abc"), "KEY1=[REDACTED]", "trailing digits trimmed");
        assert_eq!(redacted("run with API_KEY=abc123 now"), "run with API_KEY=[REDACTED]");
        assert_eq!(redacted("PASSWORD="), "PASSWORD=", "empty value has nothing to redact");
    }

    #[test]
    fn env_shaped_non_secrets_pass_through() {
        assert_eq!(redacted("LOG_LEVEL=debug"), "LOG_LEVEL=debug");
        assert_eq!(redacted("DATABASE_URL=postgres://u:p@h/db"), "DATABASE_URL=postgres://u:p@h/db");
        assert_eq!(redacted("MONKEY=banana"), "MONKEY=banana", "'key' inside a word is not a marker");
        assert_eq!(redacted("NODE_ENV=production"), "NODE_ENV=production");
    }

    #[test]
    fn multiline_text_is_processed_line_by_line() {
        let text = "API_KEY=abc\nLOG_LEVEL=debug\nGITHUB_TOKEN=xyz";
        assert_eq!(redacted(text), "API_KEY=[REDACTED]\nLOG_LEVEL=debug\nGITHUB_TOKEN=[REDACTED]");
        let prose = "first line\nsecond: ghp_qrstuv\nthird";
        assert_eq!(redacted(prose), "first line\nsecond: [REDACTED]\nthird");
    }

    #[test]
    fn ordinary_prose_passes_through_untouched() {
        let prose = "The quick brown fox jumps over the lazy dog. \
                     Please approve this change before continuing.";
        assert_eq!(redacted(prose), prose);
        assert!(
            matches!(redact(prose), Cow::Borrowed(_)),
            "no secret matched: must not allocate"
        );
        assert_eq!(redacted(""), "");
        assert_eq!(redacted("192.168.1.1 /Users/jirathip/Projects/herdr-board"), "192.168.1.1 /Users/jirathip/Projects/herdr-board");
    }

    #[test]
    fn redaction_is_idempotent() {
        let texts = [
            SK_ANT,
            GHP,
            AKIA,
            "AbCdEf123456AbCdEf12345678",
            "API_KEY=abc123",
            "the key ghp_xyz is in API_KEY=env",
        ];
        for t in texts {
            let once = redacted(t);
            assert_eq!(redacted(&once), once, "second pass must be a no-op: {t}");
        }
    }
}
