//! Human collaboration on Work Items: comments from trusted repository members are ingested as
//! feedback for the next investigation. Comments never change permissions, budgets, or scope.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HumanComment {
    /// Backend comment id; opaque to Reili and only passed back as a cursor.
    pub id: String,
    pub author: String,
    /// True when the author is a trusted member of the Work State project (for GitHub: an owner,
    /// member, or collaborator of the management repository).
    pub author_trusted: bool,
    pub body: String,
    pub created_at: DateTime<Utc>,
    pub url: String,
}
