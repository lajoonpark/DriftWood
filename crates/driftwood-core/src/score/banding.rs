//! Banding (Decision #4): score quantiles over the candidate list, with
//! absolute fallbacks when the list is small. Ties at a band boundary fall
//! INTO the middle band (conservative: ambiguous items get LLM attention
//! rather than auto-labels).

use crate::config::Banding;
use crate::types::Band;

/// Assign bands to scores (descending). Returns one band per input score,
/// in input order.
pub fn assign_bands(scores: &mut [(usize, f64)], cfg: &Banding) -> Vec<Band> {
    // scores: (original_index, score). Sort descending by score.
    scores.sort_by(|a, b| b.1.total_cmp(&a.1));
    let n = scores.len();

    let mut bands = vec![Band::Middle; n];
    if n == 0 {
        return bands;
    }

    if n < cfg.small_list_threshold {
        // Absolute cutoffs for small lists.
        for (band, s) in bands.iter_mut().zip(scores.iter()) {
            *band = if s.1 >= cfg.high_absolute {
                Band::High
            } else if s.1 <= cfg.low_absolute {
                Band::Low
            } else {
                Band::Middle
            };
        }
        return unsort(bands, scores);
    }

    let high_count = ((n as f64) * cfg.high_quantile).floor().max(1.0) as usize;
    let low_count = ((n as f64) * cfg.low_quantile).floor().max(1.0) as usize;

    // Boundary scores: ties at the boundary stay in the middle band.
    let high_cutoff = scores[high_count.min(n) - 1].1;
    let low_cutoff = scores[n - low_count.min(n)].1;

    for (i, (band, s)) in bands.iter_mut().zip(scores.iter()).enumerate() {
        *band = if s.1 > high_cutoff && i < high_count {
            Band::High
        } else if s.1 < low_cutoff && i >= n - low_count {
            Band::Low
        } else {
            Band::Middle
        };
    }
    unsort(bands, scores)
}

fn unsort(bands: Vec<Band>, scores: &[(usize, f64)]) -> Vec<Band> {
    let mut out = vec![Band::Middle; bands.len()];
    for (band, (idx, _)) in bands.into_iter().zip(scores.iter()) {
        out[*idx] = band;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Banding;

    fn bands_for(scores: &[f64], cfg: &Banding) -> Vec<Band> {
        let mut indexed: Vec<(usize, f64)> = scores.iter().copied().enumerate().collect();
        assign_bands(&mut indexed, cfg)
    }

    #[test]
    fn quantile_banding() {
        // 100 candidates, 25% / 50% / 25% (ties at the boundary stay middle).
        let scores: Vec<f64> = (0..100).map(|i| i as f64).collect();
        let bands = bands_for(&scores, &Banding::default());
        assert_eq!(bands.iter().filter(|b| **b == Band::High).count(), 24);
        assert_eq!(bands.iter().filter(|b| **b == Band::Low).count(), 24);
        assert_eq!(bands.iter().filter(|b| **b == Band::Middle).count(), 52);
        // Highest score is high, lowest is low.
        assert_eq!(bands[99], Band::High);
        assert_eq!(bands[0], Band::Low);
    }

    #[test]
    fn small_list_uses_absolute_cutoffs() {
        let scores: Vec<f64> = vec![10.0, 30.0, 50.0, 80.0, 90.0];
        let bands = bands_for(&scores, &Banding::default());
        assert_eq!(bands, vec![Band::Low, Band::Middle, Band::Middle, Band::High, Band::High]);
    }

    #[test]
    fn ties_at_boundary_fall_to_middle() {
        // 20 candidates all equal score → cutoff == that score → all middle.
        let scores: Vec<f64> = vec![50.0; 20];
        let bands = bands_for(&scores, &Banding::default());
        assert!(bands.iter().all(|b| *b == Band::Middle));
    }

    #[test]
    fn ties_split_correctly() {
        // 20 items: 18 at 50, one 100, one 0.
        let mut scores = vec![50.0; 18];
        scores.push(100.0);
        scores.push(0.0);
        let bands = bands_for(&scores, &Banding::default());
        // high_count = 3; boundary score = 50 → the single 100 is high,
        // the 50s (ties) stay middle.
        assert_eq!(bands.iter().filter(|b| **b == Band::High).count(), 1);
        assert_eq!(bands.iter().filter(|b| **b == Band::Low).count(), 1);
        assert_eq!(bands.iter().filter(|b| **b == Band::Middle).count(), 18);
    }

    #[test]
    fn order_is_preserved() {
        let scores: Vec<f64> = vec![5.0, 95.0, 50.0];
        let bands = bands_for(&scores, &Banding::default());
        assert_eq!(bands, vec![Band::Low, Band::High, Band::Middle]);
    }
}
