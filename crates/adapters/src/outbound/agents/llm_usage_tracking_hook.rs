use rig::agent::{HookAction, PromptHook};
use rig::completion::CompletionModel;

use super::llm_usage_collector::LlmUsageCollector;

#[derive(Clone)]
pub struct LlmUsageTrackingHook {
    usage_collector: LlmUsageCollector,
}

impl LlmUsageTrackingHook {
    pub fn new(usage_collector: LlmUsageCollector) -> Self {
        Self { usage_collector }
    }
}

impl<M> PromptHook<M> for LlmUsageTrackingHook
where
    M: CompletionModel,
{
    fn on_completion_call(
        &self,
        _prompt: &rig::message::Message,
        _history: &[rig::message::Message],
    ) -> impl std::future::Future<Output = HookAction> + Send {
        let hook = self.clone();

        async move {
            hook.usage_collector.record_request();
            HookAction::cont()
        }
    }

    fn on_completion_response(
        &self,
        _prompt: &rig::message::Message,
        response: &rig::completion::CompletionResponse<M::Response>,
    ) -> impl std::future::Future<Output = HookAction> + Send {
        let hook = self.clone();
        let usage = response.usage;

        async move {
            hook.usage_collector.record_usage(&usage);
            HookAction::cont()
        }
    }
}
