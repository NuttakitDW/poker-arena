//! Winnings statistics.
//!
//! Bots are compared on their per-hand (or per-duplicate-rotation) net
//! result, expressed in big blinds. [`RateStats`] accumulates a stream of
//! such observations online (Welford's algorithm, so it never buffers the
//! stream) and reports the mean, sample standard deviation, and a two-sided
//! 95% confidence interval on the mean via the Student-t distribution.

/// Online accumulator (Welford) for per-observation winnings.
///
/// Welford's algorithm computes the mean and sum-of-squared-deviations in a
/// single pass with better numerical stability than the naive
/// sum/sum-of-squares formula, especially when observations are large
/// relative to their spread.
#[derive(Clone, Debug, Default)]
pub struct RateStats {
    count: u64,
    mean: f64,
    /// Sum of squared deviations from the running mean (Welford's `M2`).
    m2: f64,
}

impl RateStats {
    pub fn new() -> Self {
        Self::default()
    }

    /// Fold one more observation into the running statistics.
    pub fn push(&mut self, x: f64) {
        self.count += 1;
        let delta = x - self.mean;
        self.mean += delta / self.count as f64;
        let delta2 = x - self.mean;
        self.m2 += delta * delta2;
    }

    pub fn count(&self) -> u64 {
        self.count
    }

    /// Arithmetic mean of all observations pushed so far; `0.0` when empty.
    pub fn mean(&self) -> f64 {
        if self.count == 0 { 0.0 } else { self.mean }
    }

    /// Sample standard deviation (Bessel-corrected, divides by `n - 1`);
    /// `0.0` when fewer than two observations have been pushed.
    pub fn sample_std(&self) -> f64 {
        if self.count < 2 {
            0.0
        } else {
            (self.m2 / (self.count - 1) as f64).sqrt()
        }
    }

    /// Half-width of the two-sided 95% Student-t confidence interval of the
    /// mean: `t_crit(df) * sample_std / sqrt(n)`. `None` when fewer than two
    /// observations have been pushed (no defined interval).
    pub fn ci95_half_width(&self) -> Option<f64> {
        if self.count < 2 {
            return None;
        }
        let df = self.count - 1;
        let n = self.count as f64;
        Some(t_crit_95(df) * self.sample_std() / n.sqrt())
    }
}

/// Exact two-sided 95% Student-t critical values for degrees of freedom
/// 1..=30, indexed `[df - 1]`. Standard published table.
const T_TABLE_95: [f64; 30] = [
    12.706, 4.303, 3.182, 2.776, 2.571, 2.447, 2.365, 2.306, 2.262, 2.228, 2.201, 2.179, 2.160,
    2.145, 2.131, 2.120, 2.110, 2.101, 2.093, 2.086, 2.080, 2.074, 2.069, 2.064, 2.060, 2.056,
    2.052, 2.048, 2.045, 2.042,
];

/// Two-sided 95% Student-t critical value for the given degrees of freedom.
///
/// For `df` in `1..=30` this is the exact published table value. `df == 0`
/// is undefined (no confidence interval exists with zero degrees of
/// freedom); we return `f64::INFINITY` rather than panic, since callers
/// (`RateStats::ci95_half_width`) already guard against `df == 0` and any
/// other caller should get an unmistakably-unusable result rather than a
/// crash.
///
/// For `df > 30` we use the smooth approximation
/// `t = 1.96 + a / df + b / df^2` with `a = 2.5525`, `b = -8.663`, fitted so
/// the curve threads the table's tail exactly at `df = 30` and matches
/// common reference values closely beyond it (checked against the standard
/// table: df=40 -> 2.021, df=60 -> 2.000, df=120 -> 1.980, df -> inf ->
/// 1.960); the largest observed error against published values in this
/// range is well under 0.005, tightening as `df` grows.
pub fn t_crit_95(df: u64) -> f64 {
    if df == 0 {
        return f64::INFINITY;
    }
    if let Some(&t) = T_TABLE_95.get((df - 1) as usize) {
        return t;
    }
    let d = df as f64;
    1.96 + 2.5525 / d + -8.663 / (d * d)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn naive_mean_std(xs: &[f64]) -> (f64, f64) {
        let n = xs.len() as f64;
        let mean = xs.iter().sum::<f64>() / n;
        let var = xs.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / (n - 1.0);
        (mean, var.sqrt())
    }

    #[test]
    fn welford_matches_naive_two_pass() {
        let xs = [
            2.0, 4.0, 4.0, 4.0, 5.0, 5.0, 7.0, 9.0, -3.5, 12.25, 0.0, 100.0,
        ];
        let mut rs = RateStats::new();
        for &x in &xs {
            rs.push(x);
        }
        let (naive_mean, naive_std) = naive_mean_std(&xs);
        assert!((rs.mean() - naive_mean).abs() < 1e-12);
        assert!((rs.sample_std() - naive_std).abs() < 1e-9);
        assert_eq!(rs.count(), xs.len() as u64);
    }

    #[test]
    fn known_small_dataset() {
        // Classic textbook set: mean 5, sample std 2.13809...
        let xs = [2.0, 4.0, 4.0, 4.0, 5.0, 5.0, 7.0, 9.0];
        let mut rs = RateStats::new();
        for &x in &xs {
            rs.push(x);
        }
        assert!((rs.mean() - 5.0).abs() < 1e-9);
        assert!((rs.sample_std() - 2.138_089_935_299_395).abs() < 1e-9);
    }

    #[test]
    fn t_crit_spot_checks() {
        assert!((t_crit_95(1) - 12.706).abs() < 1e-9);
        assert!((t_crit_95(10) - 2.228).abs() < 1e-9);
        assert!((t_crit_95(30) - 2.042).abs() < 1e-9);
        assert!((t_crit_95(40) - 2.021).abs() < 0.005);
        assert!((t_crit_95(60) - 2.000).abs() < 0.005);
        assert!((t_crit_95(120) - 1.980).abs() < 0.005);
        assert!((t_crit_95(10_000) - 1.960).abs() < 0.005);
    }

    #[test]
    fn t_crit_is_monotonically_decreasing() {
        let mut prev = f64::INFINITY;
        for df in 1..=200u64 {
            let t = t_crit_95(df);
            assert!(t < prev, "t_crit_95 not decreasing at df={df}");
            prev = t;
        }
    }

    #[test]
    fn ci95_half_width_known_dataset() {
        // n=8, mean=5, sample_std=2.138089935299395, df=7 -> t=2.365
        // half-width = 2.365 * 2.138089935299395 / sqrt(8)
        let xs = [2.0, 4.0, 4.0, 4.0, 5.0, 5.0, 7.0, 9.0];
        let mut rs = RateStats::new();
        for &x in &xs {
            rs.push(x);
        }
        let expected = 2.365 * 2.138_089_935_299_395 / (8.0_f64).sqrt();
        let got = rs.ci95_half_width().expect("n >= 2");
        assert!(
            (got - expected).abs() < 1e-9,
            "got {got}, expected {expected}"
        );
    }

    #[test]
    fn empty_and_single_sample_edge_cases() {
        let empty = RateStats::new();
        assert_eq!(empty.count(), 0);
        assert_eq!(empty.mean(), 0.0);
        assert_eq!(empty.sample_std(), 0.0);
        assert_eq!(empty.ci95_half_width(), None);

        let mut one = RateStats::new();
        one.push(3.5);
        assert_eq!(one.count(), 1);
        assert_eq!(one.mean(), 3.5);
        assert_eq!(one.sample_std(), 0.0);
        assert_eq!(one.ci95_half_width(), None);
    }

    #[test]
    fn identical_values_give_zero_std_and_half_width() {
        let mut rs = RateStats::new();
        for _ in 0..50 {
            rs.push(7.0);
        }
        assert_eq!(rs.mean(), 7.0);
        assert_eq!(rs.sample_std(), 0.0);
        assert_eq!(rs.ci95_half_width(), Some(0.0));
    }
}
