//! Deterministic analysis used as an "outside judge" for LLM conclusions: numbers are recomputed,
//! shifts are tested statistically, and verdicts never come from the model itself.

pub mod expression;
pub mod slo;
pub mod statistics;
pub mod timeseries;

pub use expression::{ExpressionError, evaluate_expression};
pub use slo::{ErrorBudgetReport, error_budget};
pub use statistics::{SampleSummary, summarize};
pub use timeseries::{
    Anomaly, ChangePoint, ExpectedChange, LaggedCorrelation, SeriesPoint, ShiftAssessment,
    ShiftCriteria, ShiftVerdict, WindowComparison, assess_shift, assess_shift_around,
    compare_around, compare_samples, detect_anomalies, detect_change_points,
    strongest_lagged_correlation, trend,
};
