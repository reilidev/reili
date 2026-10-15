//! Deterministic descriptive and inferential statistics used to verify claims independently of
//! the LLM. Every function is pure and returns `None` (or an error) instead of inventing a result
//! when the input does not carry enough information.

use serde::{Deserialize, Serialize};

/// Smallest sample size per group for which inferential tests are attempted.
pub const MIN_SAMPLES_PER_GROUP: usize = 3;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SampleSummary {
    pub count: usize,
    pub mean: f64,
    pub std_dev: f64,
    pub min: f64,
    pub max: f64,
    pub median: f64,
    pub p95: f64,
}

#[must_use]
pub fn finite_values(values: &[f64]) -> Vec<f64> {
    values
        .iter()
        .copied()
        .filter(|value| value.is_finite())
        .collect()
}

#[must_use]
pub fn summarize(values: &[f64]) -> Option<SampleSummary> {
    let values = finite_values(values);
    if values.is_empty() {
        return None;
    }
    let mut sorted = values.clone();
    sorted.sort_by(f64::total_cmp);

    Some(SampleSummary {
        count: values.len(),
        mean: mean(&values)?,
        std_dev: sample_variance(&values).map_or(0.0, f64::sqrt),
        min: sorted[0],
        max: sorted[sorted.len() - 1],
        median: quantile_sorted(&sorted, 0.5)?,
        p95: quantile_sorted(&sorted, 0.95)?,
    })
}

#[must_use]
pub fn mean(values: &[f64]) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    #[allow(clippy::cast_precision_loss)]
    Some(values.iter().sum::<f64>() / values.len() as f64)
}

/// Unbiased sample variance (n - 1 denominator).
#[must_use]
pub fn sample_variance(values: &[f64]) -> Option<f64> {
    if values.len() < 2 {
        return None;
    }
    let mean = mean(values)?;
    let sum_sq = values
        .iter()
        .map(|value| (value - mean).powi(2))
        .sum::<f64>();
    #[allow(clippy::cast_precision_loss)]
    Some(sum_sq / (values.len() - 1) as f64)
}

/// Linear-interpolated quantile of an already sorted slice, `q` in `[0, 1]`.
#[must_use]
pub fn quantile_sorted(sorted: &[f64], q: f64) -> Option<f64> {
    if sorted.is_empty() || !(0.0..=1.0).contains(&q) {
        return None;
    }
    #[allow(clippy::cast_precision_loss)]
    let position = q * (sorted.len() - 1) as f64;
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let lower = position.floor() as usize;
    let upper = (lower + 1).min(sorted.len() - 1);
    #[allow(clippy::cast_precision_loss)]
    let fraction = position - lower as f64;
    Some(sorted[lower] + (sorted[upper] - sorted[lower]) * fraction)
}

#[must_use]
pub fn median(values: &[f64]) -> Option<f64> {
    let mut sorted = finite_values(values);
    sorted.sort_by(f64::total_cmp);
    quantile_sorted(&sorted, 0.5)
}

/// Median absolute deviation (unscaled).
#[must_use]
pub fn median_absolute_deviation(values: &[f64]) -> Option<f64> {
    let center = median(values)?;
    let deviations: Vec<f64> = values.iter().map(|value| (value - center).abs()).collect();
    median(&deviations)
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WelchTestResult {
    pub mean_before: f64,
    pub mean_after: f64,
    pub absolute_change: f64,
    /// `None` when the baseline mean is zero.
    pub relative_change: Option<f64>,
    pub t_statistic: f64,
    pub degrees_of_freedom: f64,
    pub p_value_two_sided: f64,
    /// Cohen's d using the pooled standard deviation; `None` when both groups are constant.
    pub effect_size: Option<f64>,
}

/// Welch's unequal-variance t-test comparing `after` against `before`.
#[must_use]
pub fn welch_t_test(before: &[f64], after: &[f64]) -> Option<WelchTestResult> {
    let before = finite_values(before);
    let after = finite_values(after);
    if before.len() < MIN_SAMPLES_PER_GROUP || after.len() < MIN_SAMPLES_PER_GROUP {
        return None;
    }
    let mean_before = mean(&before)?;
    let mean_after = mean(&after)?;
    let var_before = sample_variance(&before)?;
    let var_after = sample_variance(&after)?;
    #[allow(clippy::cast_precision_loss)]
    let (n_before, n_after) = (before.len() as f64, after.len() as f64);
    let se_sq = var_before / n_before + var_after / n_after;
    let absolute_change = mean_after - mean_before;
    let relative_change = (mean_before != 0.0).then(|| absolute_change / mean_before.abs());
    let pooled_sd = (((n_before - 1.0) * var_before + (n_after - 1.0) * var_after)
        / (n_before + n_after - 2.0))
        .sqrt();
    let effect_size = (pooled_sd > 0.0).then(|| absolute_change / pooled_sd);

    if se_sq == 0.0 {
        // Both groups are constant: any difference is exact, no difference is exact too.
        let p_value = if absolute_change == 0.0 { 1.0 } else { 0.0 };
        return Some(WelchTestResult {
            mean_before,
            mean_after,
            absolute_change,
            relative_change,
            t_statistic: if absolute_change == 0.0 {
                0.0
            } else {
                f64::INFINITY.copysign(absolute_change)
            },
            degrees_of_freedom: n_before + n_after - 2.0,
            p_value_two_sided: p_value,
            effect_size,
        });
    }

    let t_statistic = absolute_change / se_sq.sqrt();
    let df_denominator = (var_before / n_before).powi(2) / (n_before - 1.0)
        + (var_after / n_after).powi(2) / (n_after - 1.0);
    let degrees_of_freedom = if df_denominator > 0.0 {
        se_sq.powi(2) / df_denominator
    } else {
        n_before + n_after - 2.0
    };
    let p_value_two_sided = student_t_two_sided_p(t_statistic, degrees_of_freedom)?;

    Some(WelchTestResult {
        mean_before,
        mean_after,
        absolute_change,
        relative_change,
        t_statistic,
        degrees_of_freedom,
        p_value_two_sided,
        effect_size,
    })
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MannWhitneyResult {
    pub u_statistic: f64,
    pub z_score: f64,
    pub p_value_two_sided: f64,
}

/// Mann-Whitney U test with normal approximation and tie correction. Robust to outliers and
/// non-normal latency distributions, so it complements [`welch_t_test`].
#[must_use]
pub fn mann_whitney_u(before: &[f64], after: &[f64]) -> Option<MannWhitneyResult> {
    let before = finite_values(before);
    let after = finite_values(after);
    if before.len() < MIN_SAMPLES_PER_GROUP || after.len() < MIN_SAMPLES_PER_GROUP {
        return None;
    }
    let combined: Vec<f64> = before.iter().chain(after.iter()).copied().collect();
    let ranks = average_ranks(&combined);
    let rank_sum_after: f64 = ranks[before.len()..].iter().sum();
    #[allow(clippy::cast_precision_loss)]
    let (n1, n2) = (before.len() as f64, after.len() as f64);
    let u_after = rank_sum_after - n2 * (n2 + 1.0) / 2.0;
    let mean_u = n1 * n2 / 2.0;
    let n = n1 + n2;
    let tie_term = tie_correction_term(&combined);
    let variance = n1 * n2 / 12.0 * ((n + 1.0) - tie_term / (n * (n - 1.0)));
    if variance <= 0.0 {
        return Some(MannWhitneyResult {
            u_statistic: u_after,
            z_score: 0.0,
            p_value_two_sided: 1.0,
        });
    }
    let continuity = if u_after > mean_u {
        -0.5
    } else if u_after < mean_u {
        0.5
    } else {
        0.0
    };
    let z_score = (u_after - mean_u + continuity) / variance.sqrt();
    Some(MannWhitneyResult {
        u_statistic: u_after,
        z_score,
        p_value_two_sided: (2.0 * (1.0 - normal_cdf(z_score.abs()))).clamp(0.0, 1.0),
    })
}

fn tie_correction_term(values: &[f64]) -> f64 {
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    let mut total = 0.0;
    let mut index = 0;
    while index < sorted.len() {
        let mut end = index + 1;
        while end < sorted.len() && sorted[end] == sorted[index] {
            end += 1;
        }
        #[allow(clippy::cast_precision_loss)]
        let ties = (end - index) as f64;
        total += ties.powi(3) - ties;
        index = end;
    }
    total
}

/// 1-based ranks; tied values share the average of their ranks.
#[must_use]
pub fn average_ranks(values: &[f64]) -> Vec<f64> {
    let mut order: Vec<usize> = (0..values.len()).collect();
    order.sort_by(|left, right| values[*left].total_cmp(&values[*right]));
    let mut ranks = vec![0.0; values.len()];
    let mut index = 0;
    while index < order.len() {
        let mut end = index + 1;
        while end < order.len() && values[order[end]] == values[order[index]] {
            end += 1;
        }
        #[allow(clippy::cast_precision_loss)]
        let average_rank = (index + end + 1) as f64 / 2.0;
        for position in &order[index..end] {
            ranks[*position] = average_rank;
        }
        index = end;
    }
    ranks
}

#[must_use]
pub fn pearson_correlation(xs: &[f64], ys: &[f64]) -> Option<f64> {
    if xs.len() != ys.len() || xs.len() < MIN_SAMPLES_PER_GROUP {
        return None;
    }
    let mean_x = mean(xs)?;
    let mean_y = mean(ys)?;
    let mut covariance = 0.0;
    let mut var_x = 0.0;
    let mut var_y = 0.0;
    for (x, y) in xs.iter().zip(ys) {
        covariance += (x - mean_x) * (y - mean_y);
        var_x += (x - mean_x).powi(2);
        var_y += (y - mean_y).powi(2);
    }
    if var_x == 0.0 || var_y == 0.0 {
        return None;
    }
    Some(covariance / (var_x.sqrt() * var_y.sqrt()))
}

#[must_use]
pub fn spearman_correlation(xs: &[f64], ys: &[f64]) -> Option<f64> {
    if xs.len() != ys.len() {
        return None;
    }
    pearson_correlation(&average_ranks(xs), &average_ranks(ys))
}

/// Two-sided p-value for a Pearson correlation coefficient under H0: rho = 0.
#[must_use]
pub fn correlation_p_value(r: f64, n: usize) -> Option<f64> {
    if n < 4 || !r.is_finite() {
        return None;
    }
    if r.abs() >= 1.0 {
        return Some(0.0);
    }
    #[allow(clippy::cast_precision_loss)]
    let df = (n - 2) as f64;
    let t = r * (df / (1.0 - r * r)).sqrt();
    student_t_two_sided_p(t, df)
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LinearTrend {
    pub slope: f64,
    pub intercept: f64,
    pub r_squared: f64,
}

#[must_use]
pub fn linear_regression(xs: &[f64], ys: &[f64]) -> Option<LinearTrend> {
    if xs.len() != ys.len() || xs.len() < 2 {
        return None;
    }
    let mean_x = mean(xs)?;
    let mean_y = mean(ys)?;
    let mut sxx = 0.0;
    let mut sxy = 0.0;
    let mut syy = 0.0;
    for (x, y) in xs.iter().zip(ys) {
        sxx += (x - mean_x).powi(2);
        sxy += (x - mean_x) * (y - mean_y);
        syy += (y - mean_y).powi(2);
    }
    if sxx == 0.0 {
        return None;
    }
    let slope = sxy / sxx;
    let r_squared = if syy == 0.0 {
        1.0
    } else {
        (sxy * sxy) / (sxx * syy)
    };
    Some(LinearTrend {
        slope,
        intercept: mean_y - slope * mean_x,
        r_squared,
    })
}

/// Standard normal CDF (Abramowitz & Stegun 7.1.26, |error| < 1.5e-7).
#[must_use]
pub fn normal_cdf(x: f64) -> f64 {
    0.5 * (1.0 + erf(x / std::f64::consts::SQRT_2))
}

fn erf(x: f64) -> f64 {
    let sign = if x < 0.0 { -1.0 } else { 1.0 };
    let x = x.abs();
    let t = 1.0 / (1.0 + 0.327_591_1 * x);
    let y = 1.0
        - (((((1.061_405_429 * t - 1.453_152_027) * t) + 1.421_413_741) * t - 0.284_496_736) * t
            + 0.254_829_592)
            * t
            * (-x * x).exp();
    sign * y
}

/// Two-sided p-value of Student's t distribution with `df` degrees of freedom.
#[must_use]
pub fn student_t_two_sided_p(t: f64, df: f64) -> Option<f64> {
    if !df.is_finite() || df <= 0.0 || t.is_nan() {
        return None;
    }
    if t.is_infinite() {
        return Some(0.0);
    }
    let x = df / (df + t * t);
    regularized_incomplete_beta(df / 2.0, 0.5, x).map(|value| value.clamp(0.0, 1.0))
}

fn ln_gamma(x: f64) -> f64 {
    const COEFFICIENTS: [f64; 6] = [
        76.180_091_729_471_46,
        -86.505_320_329_416_77,
        24.014_098_240_830_91,
        -1.231_739_572_450_155,
        0.001_208_650_973_866_179,
        -0.000_005_395_239_384_953,
    ];
    let mut y = x;
    let tmp = x + 5.5;
    let tmp = tmp - (x + 0.5) * tmp.ln();
    let mut series = 1.000_000_000_190_015;
    for coefficient in COEFFICIENTS {
        y += 1.0;
        series += coefficient / y;
    }
    -tmp + (2.506_628_274_631_000_5 * series / x).ln()
}

/// Regularized incomplete beta function I_x(a, b) (Numerical Recipes `betai`).
fn regularized_incomplete_beta(a: f64, b: f64, x: f64) -> Option<f64> {
    if !(0.0..=1.0).contains(&x) {
        return None;
    }
    if x == 0.0 || x == 1.0 {
        return Some(x);
    }
    let front =
        (ln_gamma(a + b) - ln_gamma(a) - ln_gamma(b) + a * x.ln() + b * (1.0 - x).ln()).exp();
    if x < (a + 1.0) / (a + b + 2.0) {
        Some(front * beta_continued_fraction(a, b, x)? / a)
    } else {
        Some(1.0 - front * beta_continued_fraction(b, a, 1.0 - x)? / b)
    }
}

fn beta_continued_fraction(a: f64, b: f64, x: f64) -> Option<f64> {
    const MAX_ITERATIONS: u32 = 300;
    const EPSILON: f64 = 3.0e-14;
    const FPMIN: f64 = 1.0e-300;

    let qab = a + b;
    let qap = a + 1.0;
    let qam = a - 1.0;
    let mut c = 1.0;
    let mut d = 1.0 - qab * x / qap;
    if d.abs() < FPMIN {
        d = FPMIN;
    }
    d = 1.0 / d;
    let mut h = d;
    for iteration in 1..=MAX_ITERATIONS {
        let m = f64::from(iteration);
        let m2 = 2.0 * m;
        let aa = m * (b - m) * x / ((qam + m2) * (a + m2));
        d = 1.0 + aa * d;
        if d.abs() < FPMIN {
            d = FPMIN;
        }
        c = 1.0 + aa / c;
        if c.abs() < FPMIN {
            c = FPMIN;
        }
        d = 1.0 / d;
        h *= d * c;
        let aa = -(a + m) * (qab + m) * x / ((a + m2) * (qap + m2));
        d = 1.0 + aa * d;
        if d.abs() < FPMIN {
            d = FPMIN;
        }
        c = 1.0 + aa / c;
        if c.abs() < FPMIN {
            c = FPMIN;
        }
        d = 1.0 / d;
        let delta = d * c;
        h *= delta;
        if (delta - 1.0).abs() < EPSILON {
            return Some(h);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::{
        average_ranks, correlation_p_value, linear_regression, mann_whitney_u, median,
        median_absolute_deviation, normal_cdf, pearson_correlation, spearman_correlation,
        student_t_two_sided_p, summarize, welch_t_test,
    };

    fn assert_close(actual: f64, expected: f64, tolerance: f64) {
        assert!(
            (actual - expected).abs() <= tolerance,
            "expected {expected}, got {actual}"
        );
    }

    #[test]
    fn summarizes_values_and_ignores_non_finite() {
        let summary = summarize(&[1.0, 2.0, 3.0, 4.0, f64::NAN]).expect("summary");

        assert_eq!(summary.count, 4);
        assert_close(summary.mean, 2.5, 1e-12);
        assert_close(summary.median, 2.5, 1e-12);
        assert_close(summary.std_dev, 1.290_994_448_7, 1e-9);
        assert_close(summary.p95, 3.85, 1e-12);
    }

    #[test]
    fn student_t_matches_reference_values() {
        // Reference: t=2.0, df=10 → two-sided p ≈ 0.07339
        assert_close(
            student_t_two_sided_p(2.0, 10.0).expect("p"),
            0.073_388,
            1e-4,
        );
        // t=0 → p=1
        assert_close(student_t_two_sided_p(0.0, 5.0).expect("p"), 1.0, 1e-9);
        assert_close(normal_cdf(1.959_964), 0.975, 1e-6);
    }

    #[test]
    fn welch_detects_clear_shift() {
        let before = [100.0, 102.0, 98.0, 101.0, 99.0, 100.0, 103.0, 97.0];
        let after = [130.0, 128.0, 133.0, 131.0, 129.0, 132.0, 127.0, 130.0];

        let result = welch_t_test(&before, &after).expect("result");

        assert!(result.p_value_two_sided < 1e-6);
        assert_close(result.absolute_change, 30.0, 1e-9);
        assert_close(result.relative_change.expect("relative"), 0.3, 1e-9);
        assert!(result.effect_size.expect("effect") > 5.0);
    }

    #[test]
    fn welch_reports_no_shift_for_same_distribution() {
        let before = [10.0, 11.0, 9.0, 10.5, 9.5, 10.0];
        let after = [10.2, 9.8, 10.1, 10.4, 9.6, 9.9];

        let result = welch_t_test(&before, &after).expect("result");

        assert!(result.p_value_two_sided > 0.5);
    }

    #[test]
    fn welch_requires_minimum_samples() {
        assert!(welch_t_test(&[1.0, 2.0], &[3.0, 4.0, 5.0]).is_none());
    }

    #[test]
    fn mann_whitney_detects_shift_with_outliers() {
        let before = [10.0, 11.0, 10.0, 12.0, 11.0, 10.0, 500.0];
        let after = [20.0, 21.0, 22.0, 20.0, 23.0, 21.0, 22.0];

        let result = mann_whitney_u(&before, &after).expect("result");

        // Normal approximation with tie correction: z ≈ 2.19 → p ≈ 0.028.
        assert!(result.p_value_two_sided < 0.05);
        assert!(result.p_value_two_sided > 0.01);
    }

    #[test]
    fn ranks_share_average_for_ties() {
        assert_eq!(
            average_ranks(&[10.0, 20.0, 10.0, 30.0]),
            vec![1.5, 3.0, 1.5, 4.0]
        );
    }

    #[test]
    fn correlations_detect_monotonic_relationships() {
        let xs = [1.0, 2.0, 3.0, 4.0, 5.0, 6.0];
        let ys = [1.0, 4.0, 9.0, 16.0, 25.0, 36.0];

        assert_close(spearman_correlation(&xs, &ys).expect("rho"), 1.0, 1e-12);
        assert!(pearson_correlation(&xs, &ys).expect("r") > 0.95);
        assert!(correlation_p_value(0.98, 6).expect("p") < 0.01);
        assert!(pearson_correlation(&xs, &[1.0; 6]).is_none());
    }

    #[test]
    fn regression_finds_slope() {
        let trend = linear_regression(&[0.0, 1.0, 2.0, 3.0], &[1.0, 3.0, 5.0, 7.0]).expect("trend");

        assert_close(trend.slope, 2.0, 1e-12);
        assert_close(trend.intercept, 1.0, 1e-12);
        assert_close(trend.r_squared, 1.0, 1e-12);
    }

    #[test]
    fn robust_center_and_spread() {
        assert_eq!(median(&[3.0, 1.0, 2.0]), Some(2.0));
        assert_eq!(
            median_absolute_deviation(&[1.0, 2.0, 3.0, 100.0]),
            Some(1.0)
        );
    }
}
