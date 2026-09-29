//! Report model, grouping, and persistence. Everything the UI shows comes
//! from this serialized model; the frontend never reimplements
//! grouping/scoring (plan §1 architecture rules).

use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::types::{Candidate, ScopeCategory, Tier};

pub use crate::types::ReportEntry;

pub const SCHEMA_VERSION: u32 = 1;

/// A scan run's identity, used for batch-resume persistence.
pub fn new_scan_id() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("dw-{:x}", nanos)
}

/// Themed warning attached to the report when things degraded.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReportWarning {
    pub kind: String, // "murky_spotlight" | "cost_cap" | "llm_failures" | ...
    pub message: String,
}

/// Per-group totals for the report header.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GroupSummary {
    pub category: ScopeCategory,
    pub label: String,
    pub item_count: usize,
    pub total_bytes: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Counters {
    pub files_searched: u64,
    pub bytes_searched: u64,
    pub candidates_found: u64,
    pub recoverable_bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Report {
    pub schema_version: u32,
    pub scan_id: String,
    pub generated_at: DateTime<Utc>,
    pub spotlight_available: bool,
    pub privacy_tier_used: crate::types::PrivacyTier,
    pub model: Option<String>,
    pub llm_cost_usd: f64,
    pub cost_cap_hit: bool,
    /// The user pulled the scan ashore mid-crossing: the report is partial
    /// and every unjudged item is honestly labeled `fallback`. Distinct
    /// from `cost_cap_hit` — the two are different reasons for stopping.
    #[serde(default)]
    pub stopped_early: bool,
    pub counters: Counters,
    pub groups: Vec<GroupSummary>,
    pub entries: Vec<ReportEntry>,
    pub warnings: Vec<ReportWarning>,
}

impl Report {
    /// Group entries by scope category with per-group count + size totals,
    /// sorted by recoverable size descending (plan §3.3.5).
    pub fn groups_sorted(&self) -> Vec<GroupSummary> {
        let mut groups: Vec<GroupSummary> = self.groups.clone();
        groups.sort_by_key(|g| std::cmp::Reverse(g.total_bytes));
        groups
    }

    /// Entries within a group, sorted by score descending.
    pub fn entries_in_group(&self, category: ScopeCategory) -> Vec<&ReportEntry> {
        let mut out: Vec<&ReportEntry> = self
            .entries
            .iter()
            .filter(|e| e.candidate.scope_category == category)
            .collect();
        out.sort_by(|a, b| b.candidate.score.total_cmp(&a.candidate.score));
        out
    }

    /// Persist to `<base>/reports/`. Writes both a timestamped copy and
    /// `last-report.json` (survives relaunch, plan Phase 5).
    pub fn persist(&self) -> crate::Result<Vec<PathBuf>> {
        let dir = crate::paths::reports_dir();
        std::fs::create_dir_all(&dir)?;
        let json = serde_json::to_string_pretty(self)?;
        let stamp = self.generated_at.format("%Y%m%d-%H%M%S");
        let stamped = dir.join(format!("report-{stamp}-{}.json", &self.scan_id[..12.min(self.scan_id.len())]));
        let last = dir.join("last-report.json");
        std::fs::write(&stamped, &json)?;
        std::fs::write(&last, &json)?;
        Ok(vec![stamped, last])
    }

    /// Load the last persisted report.
    pub fn load_last() -> crate::Result<Report> {
        let path = crate::paths::reports_dir().join("last-report.json");
        let text = std::fs::read_to_string(path)?;
        Ok(serde_json::from_str(&text)?)
    }
}

/// Assemble the final report from per-tier assignments (engine calls this
/// in the "Sorting the driftwood" phase).
pub struct AssembleInput {
    pub scan_id: String,
    pub spotlight_available: bool,
    pub privacy_tier: crate::types::PrivacyTier,
    pub model: Option<String>,
    pub llm_cost_usd: f64,
    pub cost_cap_hit: bool,
    pub stopped_early: bool,
    pub counters: Counters,
    pub entries: Vec<ReportEntry>,
    pub warnings: Vec<ReportWarning>,
}

pub fn assemble(input: AssembleInput) -> Report {
    let mut groups: Vec<GroupSummary> = Vec::new();
    for category in [ScopeCategory::Low, ScopeCategory::Medium, ScopeCategory::High] {
        let in_group: Vec<&ReportEntry> = input
            .entries
            .iter()
            .filter(|e| e.candidate.scope_category == category)
            .collect();
        if in_group.is_empty() {
            continue;
        }
        groups.push(GroupSummary {
            category,
            label: category.label().to_string(),
            item_count: in_group.len(),
            total_bytes: in_group.iter().map(|e| e.candidate.size_bytes).sum(),
        });
    }

    Report {
        schema_version: SCHEMA_VERSION,
        scan_id: input.scan_id,
        generated_at: Utc::now(),
        spotlight_available: input.spotlight_available,
        privacy_tier_used: input.privacy_tier,
        model: input.model,
        llm_cost_usd: input.llm_cost_usd,
        cost_cap_hit: input.cost_cap_hit,
        stopped_early: input.stopped_early,
        counters: input.counters,
        groups,
        entries: input.entries,
        warnings: input.warnings,
    }
}

/// Counters tracker shared through the scan.
#[derive(Default)]
pub struct CounterState {
    pub files_searched: std::sync::atomic::AtomicU64,
    pub bytes_searched: std::sync::atomic::AtomicU64,
    pub candidates_found: std::sync::atomic::AtomicU64,
    pub recoverable_bytes: std::sync::atomic::AtomicU64,
}

impl CounterState {
    pub fn snapshot(&self) -> Counters {
        use std::sync::atomic::Ordering::Relaxed;
        Counters {
            files_searched: self.files_searched.load(Relaxed),
            bytes_searched: self.bytes_searched.load(Relaxed),
            candidates_found: self.candidates_found.load(Relaxed),
            recoverable_bytes: self.recoverable_bytes.load(Relaxed),
        }
    }

    /// Emit a counters event with the current totals.
    pub fn emit(&self, sink: &dyn crate::events::EventSink) {
        sink.emit(crate::events::ScanEvent::FilesSearched {
            total: self.files_searched.load(std::sync::atomic::Ordering::Relaxed),
        });
        sink.emit(crate::events::ScanEvent::BytesSearched {
            total: self.bytes_searched.load(std::sync::atomic::Ordering::Relaxed),
        });
        sink.emit(crate::events::ScanEvent::CandidatesFound {
            total: self.candidates_found.load(std::sync::atomic::Ordering::Relaxed),
        });
        sink.emit(crate::events::ScanEvent::RecoverableBytes {
            total: self.recoverable_bytes.load(std::sync::atomic::Ordering::Relaxed),
        });
    }
}

/// Opaque candidate id: short, stable within a scan, no path leakage.
pub fn candidate_id(path: &std::path::Path, nonce: u64) -> String {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    path.hash(&mut hasher);
    nonce.hash(&mut hasher);
    format!("c-{:016x}", hasher.finish())
}

/// The tier for a candidate when no better source exists.
pub fn tier_for_candidate(candidate: &Candidate) -> Tier {
    match candidate.band {
        crate::types::Band::High => Tier::Driftwood,
        crate::types::Band::Low => Tier::Source,
        crate::types::Band::Middle => crate::reason::fallback_tier(candidate.score),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Band, Kind, OrphanStatus, PrivacyTier, ScoreComponents, TierSource};

    fn entry(path: &str, cat: ScopeCategory, bytes: u64, score: f64) -> ReportEntry {
        ReportEntry {
            candidate: Candidate {
                id: candidate_id(std::path::Path::new(path), 1),
                path: path.into(),
                kind: Kind::Folder,
                size_bytes: bytes,
                kind_stats: None,
                last_used_date: None,
                last_used_from_spotlight: false,
                modified_date: None,
                created_date: None,
                orphan_status: OrphanStatus::Unknown,
                scope_category: cat,
                score,
                score_components: ScoreComponents::default(),
                band: Band::Middle,
            },
            tier: Tier::Current,
            tier_source: TierSource::Fallback,
            summary: "s".into(),
            reasoning: String::new(),
            confidence: 1.0,
            llm_model: None,
            privacy_tier_used: PrivacyTier::Standard,
            rule_id: None,
        }
    }

    #[test]
    fn grouping_and_sorting() {
        let report = assemble(AssembleInput {
            scan_id: "t".into(),
            spotlight_available: true,
            privacy_tier: PrivacyTier::Standard,
            model: None,
            llm_cost_usd: 0.0,
            cost_cap_hit: false,
            stopped_early: false,
            counters: Counters::default(),
            entries: vec![
                entry("/a", ScopeCategory::High, 500, 50.0),
                entry("/b", ScopeCategory::Low, 900, 90.0),
                entry("/c", ScopeCategory::Low, 100, 10.0),
            ],
            warnings: vec![],
        });

        let groups = report.groups_sorted();
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].total_bytes, 1000, "bigger group first (900+100)");
        assert_eq!(groups[0].item_count, 2);

        let low = report.entries_in_group(ScopeCategory::Low);
        assert_eq!(low[0].candidate.path, "/b", "sorted by score desc");
        assert_eq!(low[1].candidate.path, "/c");
    }

    #[test]
    fn candidate_ids_are_opaque_and_stable() {
        let a = candidate_id(std::path::Path::new("/x/y"), 7);
        let b = candidate_id(std::path::Path::new("/x/y"), 7);
        let c = candidate_id(std::path::Path::new("/x/z"), 7);
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert!(a.starts_with("c-"));
        assert!(!a.contains('/'));
    }
}
