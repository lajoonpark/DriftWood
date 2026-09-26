//! Drift score: weighted 0–100 sum of the seven heuristics (notes §8),
//! with starting weights from plan §3.1.5, all tunable via `DriftTuning`.

pub mod banding;

use serde::{Deserialize, Serialize};

use crate::config::ScoreWeights;
use crate::types::{Kind, OrphanStatus, ScopeCategory};

/// Per-component contributions (all in their own weight's units, summing
/// to the total score). Serialized into every candidate so the CLI can
/// `--dump-components` and dogfooding can see exactly why.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct ScoreComponents {
    pub size: f64,
    pub age: f64,
    pub cache_location: f64,
    pub orphan: f64,
    pub depth: f64,
    pub file_type: f64,
    pub child_count: f64,
}

impl ScoreComponents {
    pub fn total(&self) -> f64 {
        (self.size
            + self.age
            + self.cache_location
            + self.orphan
            + self.depth
            + self.file_type
            + self.child_count)
            .clamp(0.0, 100.0)
    }
}

/// Inputs gathered by Stage 1 for one candidate unit.
#[derive(Debug, Clone)]
pub struct ScoreInput {
    pub kind: Kind,
    pub size_bytes: u64,
    /// Age reference in days (Spotlight last-used preferred; see
    /// `spotlight::age_reference`). `None` → no age signal.
    pub age_days: Option<f64>,
    pub scope_category: ScopeCategory,
    pub root_cache_like: bool,
    pub orphan_status: OrphanStatus,
    /// Depth of the unit relative to the scan root (top-level units = 1).
    pub depth: usize,
    /// Cache-like extension ratio (files sampled); for plain files, 1.0
    /// when the file's own extension is cache-like.
    pub cache_like_ratio: f64,
    pub children: u64,
}

/// Size: 25 max, log₂-scaled. 1 MiB → 0, 64 GiB → max.
pub fn size_component(size_bytes: u64, weight: f64) -> f64 {
    if size_bytes == 0 {
        return 0.0;
    }
    let log = size_bytes.ilog2() as f64;
    let lo = 20.0; // 1 MiB
    let hi = 36.0; // 64 GiB
    weight * ((log - lo) / (hi - lo)).clamp(0.0, 1.0)
}

/// Age: 25 max, days/365 capped.
pub fn age_component(age_days: Option<f64>, weight: f64) -> f64 {
    match age_days {
        Some(d) if d > 0.0 => weight * (d / 365.0).min(1.0),
        _ => 0.0,
    }
}

/// Cache-location bool: 15 when the unit lives in a cache-like root.
pub fn cache_location_component(cache_like_root: bool, category: ScopeCategory, weight: f64) -> f64 {
    if cache_like_root && category == ScopeCategory::Low {
        weight
    } else {
        0.0
    }
}

/// Orphan status: orphaned = full weight, active = 0, unknown = 0
/// (unknown must not be penalized — Spotlight gaps shouldn't inflate).
pub fn orphan_component(status: OrphanStatus, weight: f64) -> f64 {
    match status {
        OrphanStatus::Orphaned => weight,
        _ => 0.0,
    }
}

/// Depth: 5 max, deeper = slightly higher (less likely active). Top-level
/// units sit at depth 1 → 0.
pub fn depth_component(depth: usize, weight: f64) -> f64 {
    weight * ((depth.saturating_sub(1) as f64) / 8.0).clamp(0.0, 1.0)
}

/// File-type histogram: 10 max, scaled by cache-like extension ratio.
pub fn file_type_component(cache_like_ratio: f64, weight: f64) -> f64 {
    weight * cache_like_ratio.clamp(0.0, 1.0)
}

/// Child count (folders only): 5 max, log10-scaled, 10k+ children → max.
pub fn child_count_component(kind: Kind, children: u64, weight: f64) -> f64 {
    if kind == Kind::File || children == 0 {
        return 0.0;
    }
    let log = (children as f64).log10();
    weight * (log / 4.0).clamp(0.0, 1.0)
}

pub fn compute(input: &ScoreInput, weights: &ScoreWeights) -> ScoreComponents {
    ScoreComponents {
        size: size_component(input.size_bytes, weights.size),
        age: age_component(input.age_days, weights.age),
        cache_location: cache_location_component(input.root_cache_like, input.scope_category, weights.cache_location),
        orphan: orphan_component(input.orphan_status, weights.orphan),
        depth: depth_component(input.depth, weights.depth),
        file_type: file_type_component(input.cache_like_ratio, weights.file_type),
        child_count: child_count_component(input.kind, input.children, weights.child_count),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::ScopeCategory::*;

    fn input() -> ScoreInput {
        ScoreInput {
            kind: Kind::Folder,
            size_bytes: 0,
            age_days: None,
            scope_category: Low,
            root_cache_like: true,
            orphan_status: OrphanStatus::Unknown,
            depth: 1,
            cache_like_ratio: 0.0,
            children: 0,
        }
    }

    fn w() -> ScoreWeights {
        ScoreWeights::default()
    }

    #[test]
    fn size_is_log2_scaled_and_bounded() {
        assert_eq!(size_component(0, 25.0), 0.0);
        assert_eq!(size_component(1024 * 1024, 25.0), 0.0, "1 MiB floor");
        let max = size_component(64 * 1024 * 1024 * 1024 * 1024u64, 25.0);
        assert!((max - 25.0).abs() < 1e-9, "64 GiB → max");
        let mid = size_component(1024 * 1024 * 1024, 25.0); // 1 GiB → (30-20)/16 * 25
        assert!((mid - 15.625).abs() < 1e-9);
        assert!(size_component(1024 * 1024 * 1024, 25.0) > size_component(64 * 1024 * 1024, 25.0));
    }

    #[test]
    fn age_capped_at_one_year() {
        assert_eq!(age_component(None, 25.0), 0.0);
        assert!((age_component(Some(365.0), 25.0) - 25.0).abs() < 1e-9);
        assert!((age_component(Some(3650.0), 25.0) - 25.0).abs() < 1e-9, "capped");
        assert!((age_component(Some(182.5), 25.0) - 12.5).abs() < 1e-9);
    }

    #[test]
    fn cache_location_needs_low_category() {
        assert_eq!(cache_location_component(true, Low, 15.0), 15.0);
        assert_eq!(cache_location_component(true, High, 15.0), 0.0);
        assert_eq!(cache_location_component(false, Low, 15.0), 0.0);
    }

    #[test]
    fn orphan_scoring() {
        assert_eq!(orphan_component(OrphanStatus::Orphaned, 15.0), 15.0);
        assert_eq!(orphan_component(OrphanStatus::Active, 15.0), 0.0);
        assert_eq!(orphan_component(OrphanStatus::Unknown, 15.0), 0.0);
    }

    #[test]
    fn depth_scales_from_top_level() {
        assert_eq!(depth_component(1, 5.0), 0.0);
        assert_eq!(depth_component(9, 5.0), 5.0);
        assert_eq!(depth_component(20, 5.0), 5.0);
    }

    #[test]
    fn child_count_folder_only() {
        assert_eq!(child_count_component(Kind::File, 1000, 5.0), 0.0);
        assert_eq!(child_count_component(Kind::Folder, 0, 5.0), 0.0);
        assert!(child_count_component(Kind::Folder, 10_000, 5.0) > 4.5);
        assert!(child_count_component(Kind::Folder, 10, 5.0) < 1.5);
    }

    #[test]
    fn total_is_bounded_sum() {
        let mut inp = input();
        inp.size_bytes = 100 * 1024 * 1024 * 1024u64;
        inp.age_days = Some(1000.0);
        inp.orphan_status = OrphanStatus::Orphaned;
        inp.cache_like_ratio = 1.0;
        inp.children = 100_000;
        inp.depth = 9; // max depth contribution
        let c = compute(&inp, &w());
        assert!((c.total() - 100.0).abs() < 1e-6, "maxed inputs hit 100");

        let c0 = compute(&input(), &w());
        assert_eq!(c0.total(), 15.0, "only cache-location contributes");
    }
}
