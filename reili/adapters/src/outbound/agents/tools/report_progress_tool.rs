use std::sync::Arc;

use reili_shared::error::PortError;
use reili_shared::investigation::{
    InvestigationProgressEvent, InvestigationProgressEventInput, InvestigationProgressEventPort,
};
use rig::completion::ToolDefinition;
use rig::tool::Tool;
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::tool_json::to_json_string;

pub struct ReportProgressToolInput {
    pub on_progress_event: Arc<dyn InvestigationProgressEventPort>,
    pub owner_id: String,
}

#[derive(Clone)]
pub struct ReportProgressTool {
    on_progress_event: Arc<dyn InvestigationProgressEventPort>,
    owner_id: String,
}

impl ReportProgressTool {
    pub fn new(input: ReportProgressToolInput) -> Self {
        Self {
            on_progress_event: input.on_progress_event,
            owner_id: input.owner_id,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReportProgressArgs {
    pub title: String,
    pub summary: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct ReportProgressResult {
    ok: bool,
}

impl Tool for ReportProgressTool {
    const NAME: &'static str = "report_progress";

    type Error = PortError;
    type Args = ReportProgressArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description:
                "Report a short progress summary before starting a new investigation step."
                    .to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "title": {
                        "type": "string",
                        "description": "Short title of the next investigation step."
                    },
                    "summary": {
                        "type": "string",
                        "description": "Short details for the step."
                    }
                },
                "required": ["title", "summary"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let publish_result = self
            .on_progress_event
            .publish(InvestigationProgressEventInput {
                owner_id: self.owner_id.clone(),
                event: InvestigationProgressEvent::ReasoningSummaryCreated {
                    title: args.title,
                    summary: args.summary,
                },
            })
            .await;
        if let Err(error) = publish_result {
            tracing::error!(
                owner_id = self.owner_id,
                error = error.message,
                "Failed to publish progress summary",
            );
        }

        to_json_string(&ReportProgressResult { ok: true })
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use async_trait::async_trait;
    use reili_shared::error::PortError;
    use reili_shared::investigation::{
        InvestigationProgressEvent, InvestigationProgressEventInput, InvestigationProgressEventPort,
    };
    use rig::tool::Tool;

    use super::{ReportProgressArgs, ReportProgressTool, ReportProgressToolInput};

    struct MockProgressEventPort {
        calls: Arc<Mutex<Vec<InvestigationProgressEventInput>>>,
        should_fail: bool,
    }

    impl MockProgressEventPort {
        fn successful(calls: Arc<Mutex<Vec<InvestigationProgressEventInput>>>) -> Self {
            Self {
                calls,
                should_fail: false,
            }
        }

        fn failing(calls: Arc<Mutex<Vec<InvestigationProgressEventInput>>>) -> Self {
            Self {
                calls,
                should_fail: true,
            }
        }
    }

    #[async_trait]
    impl InvestigationProgressEventPort for MockProgressEventPort {
        async fn publish(&self, input: InvestigationProgressEventInput) -> Result<(), PortError> {
            if self.should_fail {
                return Err(PortError::new("progress publish failed"));
            }

            self.calls.lock().expect("lock calls").push(input);
            Ok(())
        }
    }

    #[tokio::test]
    async fn publishes_reasoning_summary_event_with_owner_id() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let tool = ReportProgressTool::new(ReportProgressToolInput {
            on_progress_event: Arc::new(MockProgressEventPort::successful(Arc::clone(&calls))),
            owner_id: "coordinator".to_string(),
        });

        let output = tool
            .call(ReportProgressArgs {
                title: "Collect logs".to_string(),
                summary: "Investigate recent errors".to_string(),
            })
            .await
            .expect("call report_progress");

        assert_eq!(output, "{\"ok\":true}");
        assert_eq!(
            calls.lock().expect("lock calls").as_slice(),
            &[InvestigationProgressEventInput {
                owner_id: "coordinator".to_string(),
                event: InvestigationProgressEvent::ReasoningSummaryCreated {
                    title: "Collect logs".to_string(),
                    summary: "Investigate recent errors".to_string(),
                },
            }]
        );
    }

    #[tokio::test]
    async fn soft_fails_when_progress_publish_returns_error() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let tool = ReportProgressTool::new(ReportProgressToolInput {
            on_progress_event: Arc::new(MockProgressEventPort::failing(Arc::clone(&calls))),
            owner_id: "coordinator".to_string(),
        });

        let output = tool
            .call(ReportProgressArgs {
                title: "Collect logs".to_string(),
                summary: "Inspect the latest failures".to_string(),
            })
            .await
            .expect("call report_progress");

        assert_eq!(output, "{\"ok\":true}");
        assert!(calls.lock().expect("lock calls").is_empty());
    }
}
