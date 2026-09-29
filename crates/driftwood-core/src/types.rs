//! Core data model. Defined once here so both wrappers (app, MCP, CLI) and
//! tests consume identical JSON shapes. Field names are the wire contract.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

pub use crate::score::ScoreComponents;

/// Risk tier 1..4 (river names in [`Tier::name`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(into = "u8", try_from = "u8")]
pub enum Tier {
    Driftwood = 1,
    MessageInABottle = 2,
    Current = 3,
    Source = 4,
}

impl Tier {
    pub const ALL: [Tier; 4] = [
        Tier::Driftwood,
        Tier::MessageInABottle,
        Tier::Current,
        Tier::Source,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Tier::Driftwood => "Driftwood",
            Tier::MessageInABottle => "Message in a Bottle",
            Tier::Current => "Current",
            Tier::Source => "Source",
        }
    }
}

impl From<Tier> for u8 {
    fn from(t: Tier) -> u8 {
        t as u8
    }
}

impl TryFrom<u8> for Tier {
    type Error = serde_json::Error;
    fn try_from(v: u8) -> Result<Self, serde_json::Error> {
        use serde::de::Error as _;
        Tier::ALL
            .iter()
            .find(|t| **t as u8 == v)
            .copied()
            .ok_or_else(|| serde_json::Error::custom(format!("invalid tier {v} (1..4)")))
    }
}

impl std::fmt::Display for Tier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} ({})", self.name(), *self as u8)
    }
}

/// How a tier was assigned. Always honest: `fallback` means the LLM failed
/// for this item and the heuristic band label was used instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TierSource {
    /// Top band: auto-labeled Driftwood, LLM skipped.
    AutoHigh,
    /// Bottom band: auto-labeled Source, LLM skipped.
    AutoLow,
    /// Pinned by a local rule from rules.json.
    Rule,
    /// Assigned by the LLM in Stage 2.
    Llm,
    /// The LLM judged a cluster representative; this item shares its
    /// parent directory, orphan status, and cache-likeness bucket, so the
    /// judgment was propagated. Never presented as an independent LLM
    /// judgment — one shared verdict, honestly labeled.
    LlmPropagated,
    /// LLM failed / cost cap hit; heuristic band label used instead.
    Fallback,
    /// Hard never-flag list forced this to tier 4 minimum.
    NeverFlag,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    File,
    Folder,
    App,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OrphanStatus {
    Orphaned,
    Active,
    Unknown,
}

/// Scope category from Section 9 presets. Attached to every candidate; the
/// report groups by it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ScopeCategory {
    Low,
    Medium,
    High,
}

impl ScopeCategory {
    pub fn label(self) -> &'static str {
        match self {
            ScopeCategory::Low => "Low personal risk",
            ScopeCategory::Medium => "Medium personal risk",
            ScopeCategory::High => "High personal risk",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PrivacyTier {
    Minimal,
    Standard,
    Deep,
}

/// Score band (Decision #4). `high` → auto Driftwood, `low` → auto Source,
/// `middle` → LLM.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Band {
    High,
    Middle,
    Low,
}

/// Stage 1 output candidate.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Candidate {
    /// Opaque id, stable within a scan. LLM round-trips this id, never the
    /// path, to prevent path-mangling in responses.
    pub id: String,
    pub path: String,
    pub kind: Kind,
    pub size_bytes: u64,
    /// Aggregate stats for folders: children seen, files seen, sampled
    /// cache-like extension ratio. Absent for files.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind_stats: Option<KindStats>,
    /// Spotlight kMDItemLastUsedDate, when available.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_used_date: Option<DateTime<Utc>>,
    /// Whether `last_used_date` came from Spotlight. Missing data never
    /// blocks candidacy (still-in-the-current is a hard filter only on
    /// positive Spotlight recency).
    pub last_used_from_spotlight: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub modified_date: Option<DateTime<Utc>>,
    /// Creation date — used for age only in the Downloads exception
    /// (download date is meaningful there), never as "last used".
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created_date: Option<DateTime<Utc>>,
    pub orphan_status: OrphanStatus,
    pub scope_category: ScopeCategory,
    pub score: f64,
    pub score_components: ScoreComponents,
    pub band: Band,
}

/// Aggregate stats gathered while sizing folders.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct KindStats {
    pub children: u64,
    pub files: u64,
    /// Fraction (0..1) of sampled children with cache-like extensions.
    pub cache_like_ratio: f64,
    /// Truncated when enumeration hit caps.
    pub truncated: bool,
}

/// Final report entry: candidate + tier decision + human explanation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReportEntry {
    pub candidate: Candidate,
    pub tier: Tier,
    pub tier_source: TierSource,
    /// One sentence: what it is / why it exists.
    pub summary: String,
    /// Full LLM rationale (the "Daydreaming" dropdown content). Empty for
    /// auto/rule tiers.
    #[serde(default)]
    pub reasoning: String,
    /// 0..1 from the LLM; 1.0 for deterministic assignments.
    pub confidence: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub llm_model: Option<String>,
    pub privacy_tier_used: PrivacyTier,
    /// Matching rule id when `tier_source == Rule`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rule_id: Option<String>,
}
