//! Deterministic statistics used to measure goal outcomes: before/after comparisons of metrics
//! when a recommendation is implemented.

pub mod statistics;
pub mod timeseries;

pub use statistics::{SampleSummary, summarize};
pub use timeseries::{
    ExpectedChange, SeriesPoint, ShiftAssessment, ShiftCriteria, ShiftVerdict, WindowComparison,
    assess_shift, assess_shift_around, compare_around, compare_samples, trend,
};
