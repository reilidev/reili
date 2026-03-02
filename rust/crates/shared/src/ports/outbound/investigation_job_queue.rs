use super::JobQueuePort;
use crate::types::InvestigationJob;

pub type InvestigationJobQueuePort = dyn JobQueuePort<InvestigationJob>;
