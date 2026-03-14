use super::JobQueuePort;
use crate::investigation::InvestigationJob;

pub type InvestigationJobQueuePort = dyn JobQueuePort<InvestigationJob>;
