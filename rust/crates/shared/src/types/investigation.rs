use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InvestigationSource {
    DatadogLogs,
    DatadogMetrics,
    DatadogEvents,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InvestigationTask {
    pub task_id: String,
    pub source: InvestigationSource,
    pub priority: u32,
    pub deadline_ms: u64,
    pub payload: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Evidence {
    pub source: InvestigationSource,
    pub summary: String,
    pub raw: Value,
    pub observed_at: String,
    pub confidence: f64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InvestigationFailure {
    pub source: InvestigationSource,
    pub reason: String,
}

pub type InvestigationResult = String;

#[cfg(test)]
mod tests {
    use super::{Evidence, InvestigationFailure, InvestigationSource, InvestigationTask};
    use serde_json::json;

    #[test]
    fn serializes_and_deserializes_investigation_task() {
        let value = InvestigationTask {
            task_id: "task-1".to_string(),
            source: InvestigationSource::DatadogLogs,
            priority: 1,
            deadline_ms: 30_000,
            payload: json!({"query": "service:api"}),
        };

        let json = serde_json::to_string(&value).expect("serialize investigation task");
        let restored: InvestigationTask =
            serde_json::from_str(&json).expect("deserialize investigation task");

        assert_eq!(restored, value);
    }

    #[test]
    fn serializes_and_deserializes_evidence_and_failure() {
        let evidence = Evidence {
            source: InvestigationSource::DatadogEvents,
            summary: "deployment started".to_string(),
            raw: json!({"id": "evt-1"}),
            observed_at: "2026-03-04T00:00:00Z".to_string(),
            confidence: 0.8,
        };
        let failure = InvestigationFailure {
            source: InvestigationSource::DatadogMetrics,
            reason: "query timeout".to_string(),
        };

        let evidence_json = serde_json::to_string(&evidence).expect("serialize evidence");
        let failure_json = serde_json::to_string(&failure).expect("serialize failure");

        let restored_evidence: Evidence =
            serde_json::from_str(&evidence_json).expect("deserialize evidence");
        let restored_failure: InvestigationFailure =
            serde_json::from_str(&failure_json).expect("deserialize failure");

        assert_eq!(restored_evidence, evidence);
        assert_eq!(restored_failure, failure);
    }
}
