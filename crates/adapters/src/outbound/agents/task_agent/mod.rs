mod factory;
mod instructions;
mod objective_instructions;
mod prompt;
mod spawn;
mod sub_agent;

pub(crate) use factory::OBJECTIVE_TOOL_CONCURRENCY;
pub use factory::{
    AgentInstructionsConfig, BuildObjectiveAgentInput, BuildTaskAgentInput,
    CreateTaskAgentFactoryInput, TaskAgentConfig, TaskAgentExecutionContext, TaskAgentFactory,
    TaskAgentRunContext,
};
pub use objective_instructions::{BuildObjectivePromptInput, build_objective_prompt};
pub use prompt::{BuildTaskPromptInput, build_task_prompt};
