use std::sync::Arc;

use reili_core::error::PortError;
use reili_core::messaging::slack::{
    AppendSlackCanvasMemoryInput, SlackCanvasMemoryPort, SlackCanvasMemoryVisibility,
};
use rig::completion::ToolDefinition;
use rig::tool::Tool;
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::support::json::to_json_string;
use super::support::slack_soft_error::to_slack_tool_soft_error;

pub struct SaveMemoryToolInput {
    pub canvas_memory_port: Arc<dyn SlackCanvasMemoryPort>,
    pub channel_id: String,
    pub channel_name: Option<String>,
    /// Message timestamp of the current task thread, used as the memory's Source provenance.
    pub source_message_ts: String,
}

/// Shared state and append logic for the memory-saving tools. `save_memory` and
/// `save_shared_memory` differ only in the visibility they write with.
#[derive(Clone)]
struct SaveMemoryToolCore {
    canvas_memory_port: Arc<dyn SlackCanvasMemoryPort>,
    channel_id: String,
    channel_name: Option<String>,
    source_message_ts: String,
}

impl SaveMemoryToolCore {
    fn from_input(input: SaveMemoryToolInput) -> Self {
        Self {
            canvas_memory_port: input.canvas_memory_port,
            channel_id: input.channel_id,
            channel_name: input.channel_name,
            source_message_ts: input.source_message_ts,
        }
    }

    async fn append(
        &self,
        visibility: SlackCanvasMemoryVisibility,
        args: SaveMemoryArgs,
    ) -> Result<String, PortError> {
        let append_result = self
            .canvas_memory_port
            .append_memory(AppendSlackCanvasMemoryInput {
                visibility,
                channel_id: self.channel_id.clone(),
                channel_name: self.channel_name.clone(),
                source_message_ts: self.source_message_ts.clone(),
                fact: args.fact,
                evidence: args.evidence,
                scope: args.scope,
            })
            .await;

        match append_result {
            Ok(()) => to_json_string(&SaveMemoryResult { ok: true }),
            Err(error) => {
                tracing::warn!(
                    channel_id = self.channel_id,
                    visibility = ?visibility,
                    error = error.message,
                    "Failed to save memory to Slack Canvas",
                );
                to_json_string(&to_slack_tool_soft_error(&error))
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveMemoryArgs {
    pub fact: String,
    pub evidence: String,
    pub scope: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct SaveMemoryResult {
    ok: bool,
}

fn memory_tool_parameters() -> serde_json::Value {
    json!({
        "type": "object",
        "properties": {
            "fact": {
                "type": "string",
                "description": "A concise, affirmative statement of the verified fact, phrased as \"X is Y.\" Record only what is true."
            },
            "evidence": {
                "type": "string",
                "description": "Where this was confirmed: file path, document, ticket, runbook, dashboard, log source, or user instruction."
            },
            "scope": {
                "type": "string",
                "description": "The project, repository, service, environment, workflow, or operational area where this applies."
            }
        },
        "required": ["fact", "evidence", "scope"]
    })
}

/// Saves a channel-scoped memory (recalled only in the current channel).
#[derive(Clone)]
pub struct SaveMemoryTool {
    core: SaveMemoryToolCore,
}

impl SaveMemoryTool {
    pub fn new(input: SaveMemoryToolInput) -> Self {
        Self {
            core: SaveMemoryToolCore::from_input(input),
        }
    }
}

impl Tool for SaveMemoryTool {
    const NAME: &'static str = "save_memory";

    type Error = PortError;
    type Args = SaveMemoryArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description: "Persist one durable, reusable fact to THIS channel's memory for future \
investigations in this channel. Provide fact, evidence, and scope. Channel and source are attached \
automatically. Use this for facts specific to this channel's systems; for facts true across all \
channels use save_shared_memory instead."
                .to_string(),
            parameters: memory_tool_parameters(),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        self.core
            .append(SlackCanvasMemoryVisibility::Channel, args)
            .await
    }
}

/// Saves a memory shared across every channel this assistant serves (recalled in all of them).
#[derive(Clone)]
pub struct SaveSharedMemoryTool {
    core: SaveMemoryToolCore,
}

impl SaveSharedMemoryTool {
    pub fn new(input: SaveMemoryToolInput) -> Self {
        Self {
            core: SaveMemoryToolCore::from_input(input),
        }
    }
}

impl Tool for SaveSharedMemoryTool {
    const NAME: &'static str = "save_shared_memory";

    type Error = PortError;
    type Args = SaveMemoryArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description:
                "Persist one durable, reusable fact that applies across ALL channels this \
assistant serves, not just the current one. Use only for facts that hold regardless of channel \
(for example organization-wide conventions, shared tooling, or cross-team policies). For a fact \
specific to this channel's systems, use save_memory instead. Provide fact, evidence, and scope; \
source is attached automatically."
                    .to_string(),
            parameters: memory_tool_parameters(),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        self.core
            .append(SlackCanvasMemoryVisibility::Shared, args)
            .await
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use reili_core::error::PortError;
    use reili_core::messaging::slack::{
        AppendSlackCanvasMemoryInput, MockSlackCanvasMemoryPort, SlackCanvasMemoryPort,
        SlackCanvasMemoryVisibility,
    };
    use rig::tool::Tool;

    use super::{SaveMemoryArgs, SaveMemoryTool, SaveMemoryToolInput, SaveSharedMemoryTool};

    fn capturing_port() -> (
        MockSlackCanvasMemoryPort,
        Arc<Mutex<Vec<AppendSlackCanvasMemoryInput>>>,
    ) {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let append_calls = Arc::clone(&calls);
        let mut port = MockSlackCanvasMemoryPort::new();
        port.expect_append_memory().times(1).returning(
            move |input: AppendSlackCanvasMemoryInput| {
                append_calls.lock().expect("lock calls").push(input);
                Ok(())
            },
        );
        (port, calls)
    }

    fn tool_input(port: Arc<dyn SlackCanvasMemoryPort>) -> SaveMemoryToolInput {
        SaveMemoryToolInput {
            canvas_memory_port: port,
            channel_id: "C001".to_string(),
            channel_name: Some("alerts".to_string()),
            source_message_ts: "1760000000.000001".to_string(),
        }
    }

    fn args() -> SaveMemoryArgs {
        SaveMemoryArgs {
            fact: "checkout-api owns /checkout".to_string(),
            evidence: "services/checkout README".to_string(),
            scope: "checkout production".to_string(),
        }
    }

    #[tokio::test]
    async fn save_memory_appends_with_channel_visibility() {
        let (port, calls) = capturing_port();
        let tool = SaveMemoryTool::new(tool_input(Arc::new(port)));

        let output = tool.call(args()).await.expect("call save_memory");

        assert_eq!(output, "{\"ok\":true}");
        let captured = calls.lock().expect("lock calls").clone();
        assert_eq!(captured.len(), 1);
        assert_eq!(captured[0].visibility, SlackCanvasMemoryVisibility::Channel);
        assert_eq!(captured[0].channel_id, "C001");
        assert_eq!(captured[0].fact, "checkout-api owns /checkout");
    }

    #[tokio::test]
    async fn save_shared_memory_appends_with_shared_visibility() {
        let (port, calls) = capturing_port();
        let tool = SaveSharedMemoryTool::new(tool_input(Arc::new(port)));

        let output = tool.call(args()).await.expect("call save_shared_memory");

        assert_eq!(output, "{\"ok\":true}");
        let captured = calls.lock().expect("lock calls").clone();
        assert_eq!(captured[0].visibility, SlackCanvasMemoryVisibility::Shared);
    }

    #[tokio::test]
    async fn soft_fails_when_append_returns_error() {
        let mut port = MockSlackCanvasMemoryPort::new();
        port.expect_append_memory()
            .times(1)
            .returning(|_| Err(PortError::service_error("canvas_editing_locked", "locked")));

        let tool = SaveMemoryTool::new(tool_input(Arc::new(port)));
        let output = tool
            .call(args())
            .await
            .expect("call save_memory should not hard-fail");

        assert!(output.contains("\"ok\":false"));
    }
}
