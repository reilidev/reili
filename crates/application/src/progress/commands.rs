#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordToolCallStarted {
    pub owner_id: String,
    pub task_id: String,
    pub title: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordToolCallCompleted {
    pub owner_id: String,
    pub task_id: String,
    pub title: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordProgressSummary {
    pub owner_id: String,
    pub title: String,
    pub summary: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordMessageOutputCreated {
    pub owner_id: String,
}
