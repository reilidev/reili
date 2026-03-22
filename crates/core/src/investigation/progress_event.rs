use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::error::PortError;

pub const INVESTIGATION_LEAD_PROGRESS_OWNER_ID: &str = "investigation_lead";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum InvestigationProgressEvent {
    ProgressSummaryCreated { title: String, summary: String },
    ToolCallStarted { task_id: String, title: String },
    ToolCallCompleted { task_id: String, title: String },
    MessageOutputCreated,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InvestigationProgressEventInput {
    pub owner_id: String,
    pub event: InvestigationProgressEvent,
}

#[cfg_attr(test, mockall::automock)]
#[async_trait]
pub trait InvestigationProgressEventPort: Send + Sync {
    async fn publish(&self, input: InvestigationProgressEventInput) -> Result<(), PortError>;
}

#[cfg(test)]
mod tests {
    use super::{InvestigationProgressEvent, InvestigationProgressEventInput};

    #[test]
    fn serializes_and_deserializes_progress_event() {
        let value = InvestigationProgressEventInput {
            owner_id: "investigation_lead".to_string(),
            event: InvestigationProgressEvent::ToolCallStarted {
                task_id: "task-1".to_string(),
                title: "Query logs".to_string(),
            },
        };

        let json = serde_json::to_string(&value).expect("serialize progress event");
        let restored: InvestigationProgressEventInput =
            serde_json::from_str(&json).expect("deserialize progress event");

        assert_eq!(restored, value);
    }
}
