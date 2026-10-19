use std::sync::{Arc, Mutex};

use crate::work::CycleReport;

pub const SUBMIT_CYCLE_REPORT_TOOL_NAME: &str = "submit_cycle_report";

/// Captures the single structured report the agent submits.
#[derive(Debug, Clone, Default)]
pub struct ReportSlot {
    report: Arc<Mutex<Option<CycleReport>>>,
}

impl ReportSlot {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn submit(&self, report: CycleReport) {
        let mut slot = match self.report.lock() {
            Ok(slot) => slot,
            Err(poisoned) => poisoned.into_inner(),
        };
        *slot = Some(report);
    }

    #[must_use]
    pub fn take(&self) -> Option<CycleReport> {
        match self.report.lock() {
            Ok(mut slot) => slot.take(),
            Err(poisoned) => poisoned.into_inner().take(),
        }
    }
}
