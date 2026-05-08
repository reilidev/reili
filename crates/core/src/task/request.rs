use crate::messaging::slack::{SlackMessage, SlackThreadMessage};

use super::TaskMemoryItem;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskRequest {
    pub trigger_message: SlackMessage,
    pub thread_messages: Vec<SlackThreadMessage>,
    pub memory_items: Vec<TaskMemoryItem>,
}
