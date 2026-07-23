use std::sync::Arc;

use reili_core::error::PortError;
use rig::completion::ToolDefinition;
use rig::tool::Tool;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::outbound::esa::{EsaPostGetInput, EsaPostGetPort};

use super::support::esa_soft_error::to_esa_tool_soft_error;
use super::support::json::to_json_string;

#[derive(Clone)]
pub struct GetPostTool {
    esa_post_get_port: Arc<dyn EsaPostGetPort>,
}

impl GetPostTool {
    pub fn new(esa_post_get_port: Arc<dyn EsaPostGetPort>) -> Self {
        Self { esa_post_get_port }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GetPostArgs {
    pub number: u64,
}

impl Tool for GetPostTool {
    const NAME: &'static str = "get_post";

    type Error = PortError;
    type Args = GetPostArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description: "Fetch a single esa post by its post number (the numeric ID shown in \
                the esa URL, e.g. the 123 in https://docs.esa.io/posts/123). Use this when you \
                already know which post you want, such as one referenced by search_posts results \
                or by a link shared in Slack or GitHub."
                .to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "number": {
                        "type": "integer",
                        "minimum": 1,
                        "description": "The esa post number to fetch, e.g. 123 for https://docs.esa.io/posts/123."
                    }
                },
                "required": ["number"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        match self
            .esa_post_get_port
            .get_post(EsaPostGetInput {
                number: args.number,
            })
            .await
        {
            Ok(result) => to_json_string(&result),
            Err(error) => to_json_string(&to_esa_tool_soft_error(&error)),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use crate::outbound::esa::{EsaPost, EsaPostGetInput, EsaPostGetPort};
    use async_trait::async_trait;
    use reili_core::error::PortError;
    use rig::tool::Tool;

    use super::{GetPostArgs, GetPostTool};

    struct MockEsaPostGetPort {
        calls: Arc<Mutex<Vec<EsaPostGetInput>>>,
        result: Result<EsaPost, PortError>,
    }

    #[async_trait]
    impl EsaPostGetPort for MockEsaPostGetPort {
        async fn get_post(&self, input: EsaPostGetInput) -> Result<EsaPost, PortError> {
            self.calls.lock().expect("lock calls").push(input);
            self.result.clone()
        }
    }

    fn build_tool(
        calls: Arc<Mutex<Vec<EsaPostGetInput>>>,
        result: Result<EsaPost, PortError>,
    ) -> GetPostTool {
        GetPostTool::new(Arc::new(MockEsaPostGetPort { calls, result }))
    }

    #[tokio::test]
    async fn converts_args_to_get_input_and_returns_json() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let tool = build_tool(
            Arc::clone(&calls),
            Ok(EsaPost {
                number: 102,
                name: "Runbook".to_string(),
                wip: false,
                body_md: "# Runbook".to_string(),
                url: Some("https://docs.esa.io/posts/102".to_string()),
                category: Some("SRE".to_string()),
                tags: vec!["alert".to_string()],
                created_at: None,
                updated_at: None,
                created_by: None,
                updated_by: None,
                comments_count: Some(1),
                watchers_count: None,
            }),
        );

        let output = tool
            .call(GetPostArgs { number: 102 })
            .await
            .expect("call get_post");

        assert!(output.contains("Runbook"));
        assert!(output.contains("https://docs.esa.io/posts/102"));

        let captured = calls.lock().expect("lock calls");
        assert_eq!(captured.len(), 1);
        assert_eq!(captured[0].number, 102);
    }

    #[tokio::test]
    async fn returns_soft_error_json_when_post_not_found() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let tool = build_tool(
            Arc::clone(&calls),
            Err(PortError::http_status(
                404,
                "esa API request failed: endpoint=get_post status=404 error=not_found",
            )),
        );

        let output = tool
            .call(GetPostArgs { number: 999 })
            .await
            .expect("call get_post");

        assert!(output.contains("not_found"));
        assert!(output.contains("\"retryable\":false"));
        assert!(output.contains("\"statusCode\":404"));
        assert_eq!(calls.lock().expect("lock calls").len(), 1);
    }

    #[tokio::test]
    async fn tool_schema_has_required_number() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let tool = build_tool(
            calls,
            Ok(EsaPost {
                number: 1,
                name: String::new(),
                wip: false,
                body_md: String::new(),
                url: None,
                category: None,
                tags: Vec::new(),
                created_at: None,
                updated_at: None,
                created_by: None,
                updated_by: None,
                comments_count: None,
                watchers_count: None,
            }),
        );

        let definition = tool.definition("test".to_string()).await;
        assert_eq!(definition.name, "get_post");
        let required = definition.parameters["required"]
            .as_array()
            .expect("required array");
        assert!(required.contains(&serde_json::Value::String("number".to_string())));
        assert_eq!(
            definition.parameters["properties"]["number"]["type"],
            serde_json::Value::String("integer".to_string())
        );
    }
}
