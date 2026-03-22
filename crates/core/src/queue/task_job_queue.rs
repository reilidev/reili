use super::JobQueuePort;
use crate::task::TaskJob;

pub type TaskJobQueuePort = dyn JobQueuePort<TaskJob>;
