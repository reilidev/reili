use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use rig::agent::{HookAction, PromptHook};
use rig::completion::{CompletionModel, Message};

#[derive(Clone, Default)]
pub struct RequestCountHook {
    requests: Arc<AtomicU32>,
}

impl RequestCountHook {
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn request_count(&self) -> u32 {
        self.requests.load(Ordering::Relaxed)
    }
}

impl<M> PromptHook<M> for RequestCountHook
where
    M: CompletionModel,
{
    fn on_completion_call(
        &self,
        _prompt: &Message,
        _history: &[Message],
    ) -> impl std::future::Future<Output = HookAction> + Send {
        self.requests.fetch_add(1, Ordering::Relaxed);
        async { HookAction::cont() }
    }
}
