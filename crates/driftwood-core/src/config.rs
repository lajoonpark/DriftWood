//! Configuration: every tunable lives here so Phase 2 dogfooding can
//! override weights/thresholds without code edits, and so the same config
//! shape flows into production.

use serde::{Deserialize, Serialize};

use crate::rules::Rule;
use crate::types::{PrivacyTier, ScopeCategory};

/// The seven drift-score weights (max contributions, plan §3.1.5).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ScoreWeights {
    pub size: f64,
    pub age: f64,
    pub cache_location: f64,
    pub orphan: f64,
    pub depth: f64,
    pub file_type: f64,
    pub child_count: f64,
}

impl Default for ScoreWeights {
    fn default() -> Self {
        Self {
            size: 25.0,
            age: 25.0,
            cache_location: 15.0,
            orphan: 15.0,
            depth: 5.0,
            file_type: 10.0,
            child_count: 5.0,
        }
    }
}

/// Banding per Decision #4: score quantiles over the candidate list, with
/// absolute fallbacks for small lists.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Banding {
    /// Fraction of top scores auto-labeled Driftwood.
    pub high_quantile: f64,
    /// Fraction of bottom scores auto-labeled Source.
    pub low_quantile: f64,
    /// Below this candidate count, use absolute cutoffs instead.
    pub small_list_threshold: usize,
    /// Absolute cutoffs used when the list is small.
    pub high_absolute: f64,
    pub low_absolute: f64,
}

impl Default for Banding {
    fn default() -> Self {
        Self {
            high_quantile: 0.25,
            low_quantile: 0.25,
            small_list_threshold: 20,
            high_absolute: 75.0,
            low_absolute: 25.0,
        }
    }
}

/// Stage 2 batching + cost guardrail (Decision #3).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Reasoning {
    /// Candidates per score-proximity batch (plan: 20–30).
    pub batch_size: usize,
    /// Hard per-scan USD cap; 0 disables the cap.
    pub cost_cap_usd: f64,
    /// HTTP timeout per LLM call, seconds.
    pub timeout_secs: u64,
    /// Retries with backoff per batch.
    pub retries: u32,
    /// Optional few-shot injection of past corrections (privacy-gated).
    pub few_shot: bool,
    /// Max few-shot examples injected per batch.
    pub few_shot_count: usize,
}

impl Default for Reasoning {
    fn default() -> Self {
        Self {
            batch_size: 25,
            cost_cap_usd: 0.50,
            timeout_secs: 120,
            retries: 2,
            few_shot: false,
            few_shot_count: 5,
        }
    }
}

/// Walk/enumeration caps (edge cases §4.3, §4.4, symlink depth).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct WalkCaps {
    /// Max entries enumerated per scope root (top-level units).
    pub max_entries_per_root: usize,
    /// Max children inspected when sizing a folder.
    pub max_children_when_sizing: usize,
    /// Max entries listed for the Deep privacy tier payload.
    pub deep_listing_cap: usize,
    /// Hard walk depth cap (symlink loop / runaway nesting guard).
    pub max_walk_depth: usize,
    /// Entries fetched per mdls invocation.
    pub mdls_batch_size: usize,
    /// Still-in-the-current window in days (notes §5: 3 weeks).
    pub recency_days: i64,
}

impl Default for WalkCaps {
    fn default() -> Self {
        Self {
            max_entries_per_root: 500,
            max_children_when_sizing: 20_000,
            deep_listing_cap: 100,
            max_walk_depth: 12,
            mdls_batch_size: 64,
            recency_days: 21,
        }
    }
}

/// All tunables in one place. Loadable from a TOML file next to the CLI.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct DriftTuning {
    pub weights: ScoreWeights,
    pub banding: Banding,
    pub reasoning: Reasoning,
    pub walk: WalkCaps,
}

impl DriftTuning {
    pub fn from_toml_str(s: &str) -> crate::Result<Self> {
        toml::from_str(s).map_err(|e| crate::DriftError::Config(e.to_string()))
    }

    pub fn from_toml_file(path: &std::path::Path) -> crate::Result<Self> {
        let s = std::fs::read_to_string(path)
            .map_err(|e| crate::DriftError::Config(format!("reading {}: {e}", path.display())))?;
        Self::from_toml_str(&s)
    }

    pub fn to_toml_string(&self) -> String {
        toml::to_string_pretty(self).unwrap_or_default()
    }
}

/// Per-scan input for [`crate::engine::run_scan`].
#[derive(Debug, Clone)]
pub struct ScanConfig {
    /// Selected scope categories.
    pub scopes: Vec<ScopeCategory>,
    pub privacy_tier: PrivacyTier,
    /// Run Stage 2 (LLM). When false, tiers come from bands/rules only.
    pub stage2: bool,
    /// OpenRouter model id. A setting, never a constant (Decision #2).
    pub model: String,
    /// OpenRouter API key. Read from the environment by the wrappers, never
    /// hardcoded or persisted by core.
    pub api_key: Option<String>,
    /// When false (default), every Stage-2 call is routed only to
    /// zero-data-retention providers (`zdr: true` + `data_collection: deny`).
    /// Opting in via the settings "Danger zone" drops that restriction so
    /// non-ZDR models (most free ones) can serve the river — an explicit
    /// user decision, never a default.
    pub allow_non_zdr: bool,
    /// Rules snapshot (from the memory folder) applied before any LLM call.
    pub rules: Vec<Rule>,
    pub tuning: DriftTuning,
    /// When true (default), the engine takes the single-flight lock,
    /// persists batch judgments and the final report under the base dir.
    /// Tests and `--diff`-style tooling set this false.
    pub persist: bool,
}

impl Default for ScanConfig {
    fn default() -> Self {
        Self {
            scopes: vec![ScopeCategory::Low],
            privacy_tier: PrivacyTier::Standard,
            stage2: false,
            model: default_model().to_string(),
            api_key: None,
            allow_non_zdr: false,
            rules: Vec::new(),
            tuning: DriftTuning::default(),
            persist: true,
        }
    }
}

/// Cheap-fast default; final choice is a dogfooding concern (Decision #2).
pub fn default_model() -> &'static str {
    "openai/gpt-4o-mini"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toml_roundtrip_defaults() {
        let t = DriftTuning::default();
        let s = t.to_toml_string();
        let t2 = DriftTuning::from_toml_str(&s).unwrap();
        assert_eq!(t2.weights.size, 25.0);
        assert_eq!(t2.banding.high_quantile, 0.25);
    }

    #[test]
    fn toml_partial_overrides() {
        let t = DriftTuning::from_toml_str("[weights]\nsize = 30.0\n").unwrap();
        assert_eq!(t.weights.size, 30.0);
        assert_eq!(t.weights.age, 25.0, "unspecified fields keep defaults");
    }
}
