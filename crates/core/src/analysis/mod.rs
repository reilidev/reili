//! Deterministic statistics used to measure goal outcomes: before/after comparisons of metrics
//! when a recommendation is implemented.

pub(crate) mod statistics;
mod timeseries;

pub use timeseries::{
    ExpectedChange, SeriesPoint, ShiftAssessment, ShiftCriteria, ShiftVerdict, assess_shift_around,
};
