//! Human collaboration on Work Items: comments from trusted Work State project members are ingested as
//! feedback for the next cycle. Comments never change permissions, budgets, or scope.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::record::CommentId;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HumanComment {
    pub id: CommentId,
    pub author: String,
    /// True when the author is a trusted member of the Work State project (for GitHub: an owner,
    /// member, or collaborator of the management repository).
    pub author_trusted: bool,
    pub body: String,
    pub created_at: DateTime<Utc>,
    pub url: String,
}
