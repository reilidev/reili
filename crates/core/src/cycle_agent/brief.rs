use chrono::{DateTime, Utc};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BriefSection {
    pub heading: String,
    pub body: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CycleBrief {
    pub run_id: String,
    pub objective_id: String,
    pub sections: Vec<BriefSection>,
    pub now: DateTime<Utc>,
    pub deadline: DateTime<Utc>,
}
