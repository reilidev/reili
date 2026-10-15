//! Service level objective arithmetic: availability, error budget consumption, and burn rate.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ErrorBudgetReport {
    pub target: f64,
    pub total_events: f64,
    pub bad_events: f64,
    pub achieved: f64,
    pub budget_events: f64,
    /// Fraction of the window's budget consumed (1.0 = exhausted, can exceed 1.0).
    pub budget_consumed: f64,
    /// Ratio of the observed error rate to the allowed error rate.
    pub burn_rate: f64,
    pub objective_met: bool,
}

/// Computes error-budget figures from event counts. `target` is a ratio in `(0, 1)`, e.g. 0.999.
#[must_use]
pub fn error_budget(target: f64, good_events: f64, total_events: f64) -> Option<ErrorBudgetReport> {
    if !(target > 0.0 && target < 1.0)
        || !good_events.is_finite()
        || !total_events.is_finite()
        || total_events <= 0.0
        || good_events < 0.0
        || good_events > total_events
    {
        return None;
    }
    let bad_events = total_events - good_events;
    let achieved = good_events / total_events;
    let allowed_error_rate = 1.0 - target;
    let budget_events = allowed_error_rate * total_events;

    Some(ErrorBudgetReport {
        target,
        total_events,
        bad_events,
        achieved,
        budget_events,
        budget_consumed: bad_events / budget_events,
        burn_rate: (bad_events / total_events) / allowed_error_rate,
        objective_met: achieved >= target,
    })
}

#[cfg(test)]
mod tests {
    use super::error_budget;

    #[test]
    fn computes_budget_consumption_and_burn_rate() {
        let report = error_budget(0.999, 99_950.0, 100_000.0).expect("report");

        assert!((report.budget_events - 100.0).abs() < 1e-6);
        assert!((report.budget_consumed - 0.5).abs() < 1e-6);
        assert!((report.burn_rate - 0.5).abs() < 1e-6);
        assert!(report.objective_met);
    }

    #[test]
    fn flags_exhausted_budget() {
        let report = error_budget(0.99, 9_700.0, 10_000.0).expect("report");

        assert!(!report.objective_met);
        assert!((report.burn_rate - 3.0).abs() < 1e-6);
    }

    #[test]
    fn rejects_invalid_inputs() {
        assert!(error_budget(1.0, 1.0, 1.0).is_none());
        assert!(error_budget(0.99, 11.0, 10.0).is_none());
        assert!(error_budget(0.99, 0.0, 0.0).is_none());
    }
}
