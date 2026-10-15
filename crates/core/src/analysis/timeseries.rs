//! Time-series analyses: window comparison around a split time (e.g. when a recommendation was
//! implemented) and a deterministic verdict for "metric X shifted in direction D".

use serde::{Deserialize, Serialize};

use super::statistics::{
    LinearTrend, MannWhitneyResult, SampleSummary, WelchTestResult, linear_regression,
    mann_whitney_u, summarize, welch_t_test,
};

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SeriesPoint {
    pub timestamp_ms: i64,
    pub value: f64,
}

#[must_use]
pub fn values_of(points: &[SeriesPoint]) -> Vec<f64> {
    points.iter().map(|point| point.value).collect()
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WindowComparison {
    pub before: Option<SampleSummary>,
    pub after: Option<SampleSummary>,
    pub welch: Option<WelchTestResult>,
    pub mann_whitney: Option<MannWhitneyResult>,
}

/// Compares points strictly before `split_ms` with points at or after it, optionally excluding a
/// settling gap of `exclude_ms` on each side of the split (deploy rollout noise).
#[must_use]
pub fn compare_around(points: &[SeriesPoint], split_ms: i64, exclude_ms: i64) -> WindowComparison {
    let before: Vec<f64> = points
        .iter()
        .filter(|point| point.timestamp_ms < split_ms - exclude_ms)
        .map(|point| point.value)
        .collect();
    let after: Vec<f64> = points
        .iter()
        .filter(|point| point.timestamp_ms >= split_ms + exclude_ms)
        .map(|point| point.value)
        .collect();
    compare_samples(&before, &after)
}

#[must_use]
pub fn compare_samples(before: &[f64], after: &[f64]) -> WindowComparison {
    WindowComparison {
        before: summarize(before),
        after: summarize(after),
        welch: welch_t_test(before, after),
        mann_whitney: mann_whitney_u(before, after),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExpectedChange {
    Increase,
    Decrease,
    AnyChange,
    NoChange,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ShiftCriteria {
    /// Significance level each test must reach.
    pub alpha: f64,
    /// Minimum |relative change| considered operationally material (0.1 = 10%).
    pub min_relative_change: f64,
}

impl Default for ShiftCriteria {
    fn default() -> Self {
        Self {
            alpha: 0.01,
            min_relative_change: 0.1,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShiftVerdict {
    Supported,
    Contradicted,
    Inconclusive,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ShiftAssessment {
    pub verdict: ShiftVerdict,
    pub reason: String,
}

/// Turns a window comparison into a verdict for an expected change. Both Welch's t-test and the
/// Mann-Whitney U test must agree on significance before a change is considered real, and the
/// change must also be operationally material.
#[must_use]
pub fn assess_shift(
    comparison: &WindowComparison,
    expected: ExpectedChange,
    criteria: ShiftCriteria,
) -> ShiftAssessment {
    let (Some(welch), Some(mann_whitney)) = (&comparison.welch, &comparison.mann_whitney) else {
        return ShiftAssessment {
            verdict: ShiftVerdict::Inconclusive,
            reason: "insufficient samples on one side of the split (need at least 3 each)"
                .to_string(),
        };
    };

    let significant =
        welch.p_value_two_sided < criteria.alpha && mann_whitney.p_value_two_sided < criteria.alpha;
    let clearly_not_significant =
        welch.p_value_two_sided > 0.2 && mann_whitney.p_value_two_sided > 0.2;
    let relative = welch
        .relative_change
        .unwrap_or(if welch.absolute_change == 0.0 {
            0.0
        } else {
            f64::INFINITY
        });
    let material = relative.abs() >= criteria.min_relative_change;
    let observed = if welch.absolute_change > 0.0 {
        "increase"
    } else if welch.absolute_change < 0.0 {
        "decrease"
    } else {
        "no change"
    };
    let detail = format!(
        "mean {:.4} -> {:.4} ({observed}, relative {}), welch p={:.2e}, mann-whitney p={:.2e}",
        welch.mean_before,
        welch.mean_after,
        format_relative(welch.relative_change),
        welch.p_value_two_sided,
        mann_whitney.p_value_two_sided,
    );

    let direction_matches = match expected {
        ExpectedChange::Increase => welch.absolute_change > 0.0,
        ExpectedChange::Decrease => welch.absolute_change < 0.0,
        ExpectedChange::AnyChange => welch.absolute_change != 0.0,
        ExpectedChange::NoChange => false,
    };

    let verdict = match expected {
        ExpectedChange::NoChange => {
            if significant && material {
                ShiftVerdict::Contradicted
            } else if !significant && !material {
                ShiftVerdict::Supported
            } else {
                ShiftVerdict::Inconclusive
            }
        }
        _ => {
            if significant && material && direction_matches {
                ShiftVerdict::Supported
            } else if (significant && material && !direction_matches)
                || (clearly_not_significant && !material)
            {
                ShiftVerdict::Contradicted
            } else {
                ShiftVerdict::Inconclusive
            }
        }
    };

    ShiftAssessment {
        verdict,
        reason: detail,
    }
}

/// Share of the observed shift a pre-existing trend must explain before the shift is no longer
/// attributed to the split point.
const TREND_EXPLAINS_SHIFT_RATIO: f64 = 0.5;
const TREND_MIN_R_SQUARED: f64 = 0.5;

/// Window comparison around `split_ms` that also guards against trend confounding: when the
/// metric was already moving in the same direction before the split and that trend, extrapolated,
/// explains most of the "after" level, a supported shift is downgraded to inconclusive.
#[must_use]
pub fn assess_shift_around(
    points: &[SeriesPoint],
    split_ms: i64,
    exclude_ms: i64,
    expected: ExpectedChange,
    criteria: ShiftCriteria,
) -> ShiftAssessment {
    let comparison = compare_around(points, split_ms, exclude_ms);
    let assessment = assess_shift(&comparison, expected, criteria);
    if assessment.verdict != ShiftVerdict::Supported {
        return assessment;
    }
    let before: Vec<SeriesPoint> = points
        .iter()
        .copied()
        .filter(|point| point.timestamp_ms < split_ms - exclude_ms)
        .collect();
    let after: Vec<SeriesPoint> = points
        .iter()
        .copied()
        .filter(|point| point.timestamp_ms >= split_ms + exclude_ms)
        .collect();
    let (Some(before_trend), Some(welch)) = (trend(&before), comparison.welch.as_ref()) else {
        return assessment;
    };
    let Some(first) = before.first() else {
        return assessment;
    };
    #[allow(clippy::cast_precision_loss)]
    let mean_after_hours = after
        .iter()
        .map(|point| (point.timestamp_ms - first.timestamp_ms) as f64 / 3_600_000.0)
        .sum::<f64>()
        / after.len().max(1) as f64;
    let predicted_after = before_trend.intercept + before_trend.slope * mean_after_hours;
    let shift = welch.mean_after - welch.mean_before;
    let same_direction = before_trend.slope * shift > 0.0;
    let unexplained = (welch.mean_after - predicted_after).abs();
    if same_direction
        && before_trend.r_squared >= TREND_MIN_R_SQUARED
        && shift != 0.0
        && unexplained < shift.abs() * TREND_EXPLAINS_SHIFT_RATIO
    {
        return ShiftAssessment {
            verdict: ShiftVerdict::Inconclusive,
            reason: format!(
                "{}; however a pre-existing trend (slope {:.4}/h, r²={:.2}) already predicts an after-split mean of {:.4}, so the change cannot be attributed to the split time",
                assessment.reason, before_trend.slope, before_trend.r_squared, predicted_after
            ),
        };
    }
    assessment
}

fn format_relative(relative: Option<f64>) -> String {
    relative.map_or_else(
        || "n/a".to_string(),
        |value| format!("{:+.1}%", value * 100.0),
    )
}

#[must_use]
pub fn trend(points: &[SeriesPoint]) -> Option<LinearTrend> {
    let first = points.first()?.timestamp_ms;
    #[allow(clippy::cast_precision_loss)]
    let hours: Vec<f64> = points
        .iter()
        .map(|point| (point.timestamp_ms - first) as f64 / 3_600_000.0)
        .collect();
    linear_regression(&hours, &values_of(points))
}

#[cfg(test)]
mod tests {
    use super::{
        ExpectedChange, SeriesPoint, ShiftCriteria, ShiftVerdict, assess_shift,
        assess_shift_around, compare_around, trend,
    };

    fn series(values: &[f64]) -> Vec<SeriesPoint> {
        values
            .iter()
            .enumerate()
            .map(|(index, value)| SeriesPoint {
                timestamp_ms: i64::try_from(index).expect("index") * 60_000,
                value: *value,
            })
            .collect()
    }

    fn noisy_step(before: f64, after: f64, len: usize) -> Vec<f64> {
        (0..len * 2)
            .map(|index| {
                let base = if index < len { before } else { after };
                let wobble = [0.0, 1.0, -1.0, 0.5, -0.5][index % 5];
                base + wobble
            })
            .collect()
    }

    #[test]
    fn supports_increase_claim_on_real_step() {
        let points = series(&noisy_step(100.0, 140.0, 20));
        let comparison = compare_around(&points, 20 * 60_000, 0);

        let assessment = assess_shift(
            &comparison,
            ExpectedChange::Increase,
            ShiftCriteria::default(),
        );

        assert_eq!(assessment.verdict, ShiftVerdict::Supported);
    }

    #[test]
    fn contradicts_increase_claim_when_metric_dropped() {
        let points = series(&noisy_step(140.0, 100.0, 20));
        let comparison = compare_around(&points, 20 * 60_000, 0);

        let assessment = assess_shift(
            &comparison,
            ExpectedChange::Increase,
            ShiftCriteria::default(),
        );

        assert_eq!(assessment.verdict, ShiftVerdict::Contradicted);
    }

    #[test]
    fn contradicts_change_claim_on_flat_noise() {
        let points = series(&noisy_step(100.0, 100.0, 20));
        let comparison = compare_around(&points, 20 * 60_000, 0);

        let assessment = assess_shift(
            &comparison,
            ExpectedChange::AnyChange,
            ShiftCriteria::default(),
        );

        assert_eq!(assessment.verdict, ShiftVerdict::Contradicted);
        let no_change = assess_shift(
            &comparison,
            ExpectedChange::NoChange,
            ShiftCriteria::default(),
        );
        assert_eq!(no_change.verdict, ShiftVerdict::Supported);
    }

    #[test]
    fn is_inconclusive_with_too_few_points() {
        let points = series(&[1.0, 2.0, 30.0, 40.0]);
        let comparison = compare_around(&points, 2 * 60_000, 0);

        let assessment = assess_shift(
            &comparison,
            ExpectedChange::Increase,
            ShiftCriteria::default(),
        );

        assert_eq!(assessment.verdict, ShiftVerdict::Inconclusive);
    }

    #[test]
    fn excludes_rollout_gap_around_split() {
        let mut values = noisy_step(100.0, 140.0, 20);
        values[19] = 1_000.0;
        values[20] = 1_000.0;
        let points = series(&values);

        let comparison = compare_around(&points, 20 * 60_000, 60_000);

        assert_eq!(comparison.before.expect("before").count, 19);
        assert_eq!(comparison.after.expect("after").count, 19);
    }

    #[test]
    fn computes_hourly_trend() {
        let points = series(&[0.0, 1.0, 2.0, 3.0]);

        let slope = trend(&points).expect("trend").slope;

        assert!((slope - 60.0).abs() < 1e-9);
    }

    #[test]
    fn steady_trend_is_not_attributed_to_the_split() {
        let values: Vec<f64> = (0..40)
            .map(|index| 100.0 + f64::from(index) * 2.0)
            .collect();
        let points = series(&values);

        let naive = assess_shift(
            &compare_around(&points, 20 * 60_000, 0),
            ExpectedChange::Increase,
            ShiftCriteria::default(),
        );
        let guarded = assess_shift_around(
            &points,
            20 * 60_000,
            0,
            ExpectedChange::Increase,
            ShiftCriteria::default(),
        );

        assert_eq!(naive.verdict, ShiftVerdict::Supported);
        assert_eq!(guarded.verdict, ShiftVerdict::Inconclusive);
        assert!(guarded.reason.contains("pre-existing trend"));
    }

    #[test]
    fn real_step_survives_trend_guard() {
        let points = series(&noisy_step(100.0, 140.0, 20));

        let guarded = assess_shift_around(
            &points,
            20 * 60_000,
            0,
            ExpectedChange::Increase,
            ShiftCriteria::default(),
        );

        assert_eq!(guarded.verdict, ShiftVerdict::Supported);
    }
}
