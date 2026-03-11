use std::fmt::{Display, Formatter};

use serde::{Deserialize, Serialize};

use crate::types::slack_message::SlackMessage;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InvestigationJobType {
    AlertInvestigation,
}

impl Display for InvestigationJobType {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::AlertInvestigation => formatter.write_str("alert_investigation"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InvestigationJobPayload {
    pub slack_event_id: String,
    pub message: SlackMessage,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AlertInvestigationJob {
    pub job_id: String,
    pub job_type: InvestigationJobType,
    pub received_at: String,
    pub payload: InvestigationJobPayload,
    pub retry_count: u32,
}

pub type InvestigationJob = AlertInvestigationJob;

#[cfg(test)]
mod tests {
    use super::{AlertInvestigationJob, InvestigationJobPayload, InvestigationJobType};
    use crate::types::{SlackMessage, SlackTriggerType};

    #[test]
    fn serializes_and_deserializes_investigation_job() {
        let value = AlertInvestigationJob {
            job_id: "job-1".to_string(),
            job_type: InvestigationJobType::AlertInvestigation,
            received_at: "2026-03-04T00:00:00Z".to_string(),
            payload: InvestigationJobPayload {
                slack_event_id: "evt-1".to_string(),
                message: SlackMessage {
                    slack_event_id: "evt-1".to_string(),
                    team_id: Some("T001".to_string()),
                    trigger: SlackTriggerType::AppMention,
                    channel: "C001".to_string(),
                    user: "U001".to_string(),
                    text: "check alert".to_string(),
                    ts: "123.456".to_string(),
                    thread_ts: Some("123.450".to_string()),
                },
            },
            retry_count: 0,
        };

        let json = serde_json::to_string(&value).expect("serialize investigation job");
        let restored: AlertInvestigationJob =
            serde_json::from_str(&json).expect("deserialize investigation job");

        assert_eq!(restored, value);
    }

    #[test]
    fn formats_job_type_as_snake_case_identifier() {
        assert_eq!(
            InvestigationJobType::AlertInvestigation.to_string(),
            "alert_investigation".to_string()
        );
    }
}
