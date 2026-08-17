//! Per-model USD pricing (G34) — the only place token counts become dollars.
//!
//! Claude Code and Codex carry token counts but no native cost field
//! (opencode's session store already has `cost` computed by the provider,
//! so it needs no pricing table). Prices are USD per token (MTok price /
//! 1e6) so the hot path is a multiply, never a divide.
//!
//! **These are snapshots, not a live feed — verify before trusting a
//! dollar figure for billing.** Claude prices are the first-party API
//! rates documented in Anthropic's own model catalog (as of 2026-08-17;
//! see `docs/corral/DECISIONS.md` D34 for sourcing). Codex prices were
//! fetched from `developers.openai.com/api/docs/pricing` the same day.
//! Cache-write pricing for Claude follows the documented 1.25x (5-minute
//! TTL) / 2x (1-hour TTL) multipliers over the input rate; cache-read is
//! 0.1x. A model not in either table returns `None` — its tokens are
//! real but contribute $0 to any cost sum (a floor, never a fabricated
//! number).

/// USD per token for each billed component. `cache_write_5m`/`cache_write_1h`
/// only apply to Claude (OpenAI's cache pricing has no separate write cost —
/// caching is automatic and only the cached-read rate differs from input).
#[derive(Debug, Clone, Copy)]
pub struct ModelRate {
    pub input: f64,
    pub output: f64,
    pub cache_write_5m: f64,
    pub cache_write_1h: f64,
    pub cache_read: f64,
}

const fn per_mtok(usd: f64) -> f64 {
    usd / 1_000_000.0
}

/// Build a Claude rate from its published $/MTok input+output; cache
/// tiers are derived via the documented multipliers.
const fn claude_rate(input_per_mtok: f64, output_per_mtok: f64) -> ModelRate {
    ModelRate {
        input: per_mtok(input_per_mtok),
        output: per_mtok(output_per_mtok),
        cache_write_5m: per_mtok(input_per_mtok * 1.25),
        cache_write_1h: per_mtok(input_per_mtok * 2.0),
        cache_read: per_mtok(input_per_mtok * 0.1),
    }
}

/// name, rate — matched by exact id or a recognized alias prefix (see
/// [`claude_model_rate`]). Current-generation models only; older/deprecated
/// aliases (`claude-opus-4-5`, `claude-sonnet-4-5`, `claude-*-3-*`, …) are
/// intentionally absent rather than guessed — they return `None`.
const CLAUDE_RATES: &[(&str, ModelRate)] = &[
    ("claude-fable-5", claude_rate(10.00, 50.00)),
    ("claude-mythos-5", claude_rate(10.00, 50.00)),
    ("claude-opus-5", claude_rate(5.00, 25.00)),
    ("claude-opus-4-8", claude_rate(5.00, 25.00)),
    ("claude-opus-4-7", claude_rate(5.00, 25.00)),
    ("claude-opus-4-6", claude_rate(5.00, 25.00)),
    ("claude-sonnet-5", claude_rate(3.00, 15.00)),
    ("claude-sonnet-4-6", claude_rate(3.00, 15.00)),
    ("claude-haiku-4-5", claude_rate(1.00, 5.00)),
];

/// codex/OpenAI models observed in rollout `turn_context.model`. `cached`
/// stands in for [`ModelRate::cache_read`]; there is no write-tier cost.
const CODEX_RATES: &[(&str, ModelRate)] = &[
    (
        "gpt-5.6-sol",
        ModelRate {
            input: per_mtok(5.00),
            output: per_mtok(30.00),
            cache_write_5m: 0.0,
            cache_write_1h: 0.0,
            cache_read: per_mtok(0.50),
        },
    ),
    (
        "gpt-5.6-terra",
        ModelRate {
            input: per_mtok(2.00),
            output: per_mtok(12.00),
            cache_write_5m: 0.0,
            cache_write_1h: 0.0,
            cache_read: per_mtok(0.20),
        },
    ),
    (
        "gpt-5.6-luna",
        ModelRate {
            input: per_mtok(0.20),
            output: per_mtok(1.20),
            cache_write_5m: 0.0,
            cache_write_1h: 0.0,
            cache_read: per_mtok(0.02),
        },
    ),
    (
        "gpt-5.5",
        ModelRate {
            input: per_mtok(5.00),
            output: per_mtok(30.00),
            cache_write_5m: 0.0,
            cache_write_1h: 0.0,
            cache_read: per_mtok(0.50),
        },
    ),
    (
        "gpt-5.4",
        ModelRate {
            input: per_mtok(2.50),
            output: per_mtok(15.00),
            cache_write_5m: 0.0,
            cache_write_1h: 0.0,
            cache_read: per_mtok(0.25),
        },
    ),
    (
        "gpt-5.3-codex",
        ModelRate {
            input: per_mtok(1.75),
            output: per_mtok(14.00),
            cache_write_5m: 0.0,
            cache_write_1h: 0.0,
            cache_read: per_mtok(0.175),
        },
    ),
    (
        "gpt-5-mini",
        ModelRate {
            input: per_mtok(0.25),
            output: per_mtok(2.00),
            cache_write_5m: 0.0,
            cache_write_1h: 0.0,
            cache_read: per_mtok(0.025),
        },
    ),
    (
        "gpt-5",
        ModelRate {
            input: per_mtok(1.25),
            output: per_mtok(10.00),
            cache_write_5m: 0.0,
            cache_write_1h: 0.0,
            cache_read: per_mtok(0.125),
        },
    ),
];

/// Look up a Claude model's rate. Matches by exact id first, then by
/// longest-recognized-prefix (dated snapshots like
/// `claude-sonnet-4-5-20250929` and the `-fast` deployment suffix both
/// carry the base id as a prefix).
pub fn claude_model_rate(model: &str) -> Option<ModelRate> {
    lookup_rate(CLAUDE_RATES, model)
}

/// Look up a codex/OpenAI model's rate by the same prefix-matching rule.
pub fn codex_model_rate(model: &str) -> Option<ModelRate> {
    lookup_rate(CODEX_RATES, model)
}

fn lookup_rate(table: &[(&str, ModelRate)], model: &str) -> Option<ModelRate> {
    if let Some((_, rate)) = table.iter().find(|(id, _)| *id == model) {
        return Some(*rate);
    }
    table
        .iter()
        .filter(|(id, _)| model.starts_with(id))
        .max_by_key(|(id, _)| id.len())
        .map(|(_, rate)| *rate)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_claude_ids_resolve() {
        let r = claude_model_rate("claude-opus-5").expect("known model");
        assert!((r.input - per_mtok(5.00)).abs() < 1e-12);
        assert!((r.output - per_mtok(25.00)).abs() < 1e-12);
    }

    #[test]
    fn dated_snapshot_prefix_resolves_to_base_model() {
        let r = claude_model_rate("claude-sonnet-4-6-20260101").expect("prefix match");
        assert!((r.input - per_mtok(3.00)).abs() < 1e-12);
    }

    #[test]
    fn unknown_model_is_unpriced_not_zero_by_accident() {
        assert!(claude_model_rate("claude-3-opus-20240229").is_none());
        assert!(codex_model_rate("some-future-model").is_none());
    }

    #[test]
    fn cache_tiers_follow_the_documented_multipliers() {
        let r = claude_model_rate("claude-opus-5").unwrap();
        assert!((r.cache_write_5m - r.input * 1.25).abs() < 1e-15);
        assert!((r.cache_write_1h - r.input * 2.0).abs() < 1e-15);
        assert!((r.cache_read - r.input * 0.1).abs() < 1e-15);
    }

    #[test]
    fn codex_models_resolve() {
        let r = codex_model_rate("gpt-5.6-sol").expect("known model");
        assert!((r.output - per_mtok(30.00)).abs() < 1e-12);
    }
}
