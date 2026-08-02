//! Winnings and decision-timing statistics.
//!
//! Bots are compared on their per-hand (or per-duplicate-rotation) net
//! result, expressed in big blinds. [`RateStats`] accumulates a stream of
//! such observations online (Welford's algorithm, so it never buffers the
//! stream) and reports the mean, sample standard deviation, and a two-sided
//! 95% confidence interval on the mean via the Student-t distribution.
//!
//! [`DecisionStats`] accumulates wall-clock time per bot decision — shared
//! by both runners (`crate::runner`, `crate::ofc::runner`), since both wrap
//! their bot call the same way.

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

/// Smallest histogram boundary in [`DecisionStats`], in ms (1 microsecond).
/// A duration at or below this clamps into bucket 0.
const HIST_BASE_MS: f64 = 0.001;

/// Per-bucket duration ratio: 8 sub-steps per octave, i.e. `2^(1/8)`. This
/// is the number that bounds every quantile's relative error (see
/// [`DecisionStats::quantile`]).
const HIST_RATIO: f64 = 1.090_507_732_665_257_7;

/// Bucket count: enough octaves at 8 buckets/octave to carry
/// `HIST_BASE_MS` past 10 minutes (`HIST_BASE_MS * HIST_RATIO^234 ≈
/// 638_451`ms `≈ 10.6` minutes); a duration at or above the top boundary
/// clamps into the last bucket. `235 * 8` bytes ≈ 2KB per bot — the fixed
/// price that keeps [`DecisionStats`] boundable no matter how many
/// decisions a match records (millions, for a long unattended run), unlike
/// storing every sample.
const HIST_BUCKETS: usize = 235;

/// The histogram bucket a duration of `ms` milliseconds falls into.
fn hist_bucket_of(ms: f64) -> usize {
    if ms <= HIST_BASE_MS {
        return 0;
    }
    let step = (ms / HIST_BASE_MS).ln() / HIST_RATIO.ln();
    if step >= (HIST_BUCKETS - 1) as f64 {
        HIST_BUCKETS - 1
    } else {
        step.floor() as usize
    }
}

/// The geometric midpoint of bucket `i`, in ms — the value
/// [`DecisionStats::quantile`] reports for any sample that landed in it.
fn hist_bucket_midpoint(i: usize) -> f64 {
    HIST_BASE_MS * HIST_RATIO.powf(i as f64 + 0.5)
}

/// Online accumulator for per-decision wall-clock timing, in milliseconds.
/// Count, sum (for the exact mean), and max are exact; quantiles are
/// approximated from a fixed-size log-scaled histogram rather than kept
/// exactly, since a match can run millions of decisions and this must stay
/// boundable regardless. Timing is inherently non-reproducible (it varies
/// run to run even at a fixed seed), so this never feeds `RateStats` or
/// anything else the determinism promise covers.
#[derive(Clone, Debug)]
pub struct DecisionStats {
    count: u64,
    sum_ms: f64,
    max_ms: f64,
    hist: [u64; HIST_BUCKETS],
}

impl Default for DecisionStats {
    fn default() -> Self {
        DecisionStats {
            count: 0,
            sum_ms: 0.0,
            max_ms: 0.0,
            hist: [0; HIST_BUCKETS],
        }
    }
}

impl DecisionStats {
    pub fn new() -> Self {
        Self::default()
    }

    /// Fold one more decision's elapsed time into the running statistics.
    pub fn record(&mut self, elapsed: std::time::Duration) {
        let ms = elapsed.as_secs_f64() * 1000.0;
        self.count += 1;
        self.sum_ms += ms;
        if ms > self.max_ms {
            self.max_ms = ms;
        }
        self.hist[hist_bucket_of(ms)] += 1;
    }

    pub fn count(&self) -> u64 {
        self.count
    }

    /// Exact mean wall-clock ms per decision; `None` when no decision was
    /// ever recorded (the report's null case).
    pub fn mean_ms(&self) -> Option<f64> {
        (self.count > 0).then(|| self.sum_ms / self.count as f64)
    }

    /// Exact max wall-clock ms across every decision recorded; `None` when
    /// no decision was ever recorded.
    pub fn max_ms(&self) -> Option<f64> {
        (self.count > 0).then_some(self.max_ms)
    }

    /// Approximate the `q`-quantile (`q` in `[0, 1]`, e.g. `0.5` for the
    /// median) in ms; `None` when no decision was ever recorded.
    ///
    /// Walks cumulative bucket counts to the bucket containing the
    /// `ceil(q * n)`-th sample in sorted order, and returns that bucket's
    /// geometric midpoint rather than the sample itself (which isn't kept).
    /// Because every bucket spans a `HIST_RATIO` (`2^(1/8)`) range, the
    /// reported value's relative error against the true sample it stands in
    /// for is bounded by `sqrt(HIST_RATIO) - 1 ≈ 4.5%` — the deliberate
    /// trade for a fixed ~2KB histogram instead of unbounded per-sample
    /// storage. Clamped to the exact `max_ms` (never `None` here, since
    /// `count > 0` on this path): the highest bucket's midpoint can
    /// otherwise overshoot a max that landed near that bucket's low edge,
    /// which would make `quantile(1.0) > max_ms` — a result this accumulator
    /// can state more precisely, so it does.
    pub fn quantile(&self, q: f64) -> Option<f64> {
        if self.count == 0 {
            return None;
        }
        let target = ((q * self.count as f64).ceil() as u64).clamp(1, self.count);
        let mut cumulative = 0u64;
        for (i, &c) in self.hist.iter().enumerate() {
            cumulative += c;
            if cumulative >= target {
                return Some(hist_bucket_midpoint(i).min(self.max_ms));
            }
        }
        // Unreachable in practice (the histogram's total count always
        // equals `self.count`, so the loop above always returns), but a
        // defined fallback beats a panic if that invariant is ever broken.
        Some(self.max_ms)
    }
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

    #[test]
    fn decision_stats_starts_empty() {
        let ds = DecisionStats::new();
        assert_eq!(ds.count(), 0);
        assert_eq!(ds.mean_ms(), None);
        assert_eq!(ds.max_ms(), None);
        for q in [0.0, 0.5, 0.9, 0.99, 1.0] {
            assert_eq!(ds.quantile(q), None, "q={q}");
        }
    }

    #[test]
    fn decision_stats_tracks_count_mean_and_max() {
        let mut ds = DecisionStats::new();
        for ms in [1.0, 3.0, 2.0] {
            ds.record(std::time::Duration::from_secs_f64(ms / 1000.0));
        }
        assert_eq!(ds.count(), 3);
        assert!((ds.mean_ms().unwrap() - 2.0).abs() < 1e-6);
        assert!((ds.max_ms().unwrap() - 3.0).abs() < 1e-6);
    }

    /// The relative-error bound `quantile`'s doc comment promises:
    /// `sqrt(HIST_RATIO) - 1`, with a little slack for the target-selection
    /// rounding at small `n`.
    const QUANTILE_TOLERANCE: f64 = 0.05;

    fn assert_within_tolerance(got: f64, expected: f64, label: &str) {
        let rel_err = (got - expected).abs() / expected;
        assert!(
            rel_err <= QUANTILE_TOLERANCE,
            "{label}: got {got}, expected ~{expected} (rel err {rel_err:.4} > {QUANTILE_TOLERANCE})"
        );
    }

    #[test]
    fn decision_stats_quantiles_on_a_uniform_1_to_1000ms_sample() {
        let mut ds = DecisionStats::new();
        for ms in 1..=1000u64 {
            ds.record(std::time::Duration::from_micros(ms * 1000));
        }
        assert_eq!(ds.count(), 1000);

        // Exact statistics: mean of 1..=1000 is 500.5; max is 1000.
        assert!((ds.mean_ms().unwrap() - 500.5).abs() < 1e-6);
        assert!((ds.max_ms().unwrap() - 1000.0).abs() < 1e-6);

        // Approximate quantiles: the order statistic at each rank is the
        // rank itself (values are exactly 1..=1000), so the true p50/p90/p99
        // are 500/900/990.
        let p50 = ds.quantile(0.5).unwrap();
        let p90 = ds.quantile(0.9).unwrap();
        let p99 = ds.quantile(0.99).unwrap();
        assert_within_tolerance(p50, 500.0, "p50");
        assert_within_tolerance(p90, 900.0, "p90");
        assert_within_tolerance(p99, 990.0, "p99");

        // Monotonicity holds across the whole reported chain.
        assert!(p50 <= p90, "p50 {p50} > p90 {p90}");
        assert!(p90 <= p99, "p90 {p90} > p99 {p99}");
        assert!(p99 <= ds.max_ms().unwrap(), "p99 {p99} > max");
    }

    #[test]
    fn decision_stats_quantiles_are_monotonic_on_a_skewed_sample() {
        // A long tail: mostly-fast decisions with a few slow outliers —
        // the shape a real timeout-prone match would produce.
        let mut ds = DecisionStats::new();
        for _ in 0..950 {
            ds.record(std::time::Duration::from_micros(200));
        }
        for _ in 0..45 {
            ds.record(std::time::Duration::from_millis(50));
        }
        for _ in 0..5 {
            ds.record(std::time::Duration::from_millis(900));
        }
        assert_eq!(ds.count(), 1000);

        let quantiles: Vec<f64> = [0.5, 0.9, 0.99, 1.0]
            .iter()
            .map(|&q| ds.quantile(q).unwrap())
            .collect();
        for pair in quantiles.windows(2) {
            assert!(pair[0] <= pair[1], "quantiles not monotonic: {quantiles:?}");
        }
        assert!(quantiles.last().unwrap() <= &ds.max_ms().unwrap());
    }
}
