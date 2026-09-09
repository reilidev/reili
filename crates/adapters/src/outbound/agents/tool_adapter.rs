//! Adapts a statically typed [`rig::tool::Tool`] into a [`rig::tool::DynamicTool`] so it can sit
//! alongside runtime-discovered MCP tools in one heterogeneous collection (a connector's
//! sub-agent tool set, or a lead-resolved spawn selection).

use rig::tool::{DynamicTool, IntoToolOutput, ToolExecutionError, tool_definition};

/// Erases `tool` behind a [`DynamicTool`], forwarding calls through [`rig::tool::Tool::call`] and
/// converting its typed output/error into the canonical [`ToolOutput`]/[`ToolExecutionError`] pair.
pub(super) fn into_dynamic_tool<T>(tool: T) -> DynamicTool
where
    T: rig::tool::Tool + Clone + 'static,
{
    let definition = tool_definition(&tool);

    DynamicTool::new(
        definition.name,
        definition.description,
        definition.parameters,
        move |context, arguments| {
            let tool = tool.clone();
            Box::pin(async move {
                let args: T::Args = serde_json::from_value(arguments).map_err(|error| {
                    ToolExecutionError::invalid_args(format!(
                        "{} arguments were invalid: {error}",
                        T::NAME
                    ))
                })?;

                match tool.call(context, args).await {
                    Ok(output) => output.into_tool_output(),
                    Err(error) => Err(tool.map_error(error)),
                }
            })
        },
    )
}

#[cfg(test)]
mod tests {
    use rig::tool::{Tool, ToolContext, ToolSet};
    use serde::{Deserialize, Serialize};

    use super::into_dynamic_tool;

    #[derive(Clone)]
    struct EchoTool;

    #[derive(Deserialize)]
    struct EchoArgs {
        message: String,
    }

    #[derive(Serialize)]
    struct EchoOutput {
        echoed: String,
    }

    impl Tool for EchoTool {
        const NAME: &'static str = "echo";

        type Error = std::convert::Infallible;
        type Args = EchoArgs;
        type Output = EchoOutput;

        fn description(&self) -> String {
            "Echoes the given message".to_string()
        }

        fn parameters(&self) -> serde_json::Value {
            serde_json::json!({
                "type": "object",
                "properties": { "message": { "type": "string" } },
                "required": ["message"]
            })
        }

        async fn call(
            &self,
            _context: &mut ToolContext,
            args: Self::Args,
        ) -> Result<Self::Output, Self::Error> {
            Ok(EchoOutput {
                echoed: args.message,
            })
        }
    }

    #[tokio::test]
    async fn forwards_name_definition_and_call_result() {
        let dynamic = into_dynamic_tool(EchoTool);
        assert_eq!(dynamic.name(), "echo");
        assert_eq!(dynamic.definition().description, "Echoes the given message");

        let tool_set = ToolSet::from_dynamic_tools(vec![dynamic]);
        let mut context = ToolContext::new();
        let result = tool_set
            .execute(
                "echo",
                serde_json::json!({ "message": "hi" }).to_string(),
                &mut context,
            )
            .await;

        assert!(result.is_success());
        assert_eq!(
            result.output().as_json(),
            Some(&serde_json::json!({"echoed": "hi"}))
        );
    }
}
