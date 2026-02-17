use chrono::{DateTime, Utc};
use serde::Deserialize;

/// A review comment on a specific line of code
#[derive(Debug, Clone, Deserialize)]
pub struct ReviewComment {
    pub id: u64,
    pub path: String,
    pub line: Option<u32>,
    pub body: String,
    pub user: User,
    pub created_at: DateTime<Utc>,
    pub html_url: String,
}

/// A general issue comment on the PR
#[derive(Debug, Clone, Deserialize)]
pub struct IssueComment {
    pub id: u64,
    pub body: String,
    pub user: User,
    pub created_at: DateTime<Utc>,
    pub html_url: String,
}

/// User information
#[derive(Debug, Clone, Deserialize)]
pub struct User {
    pub login: String,
    #[serde(default)]
    pub html_url: Option<String>,
}

/// Unified comment type
#[derive(Debug, Clone)]
pub enum Comment {
    Review(ReviewComment),
    Issue(IssueComment),
}

impl Comment {
    pub fn author(&self) -> &str {
        match self {
            Comment::Review(c) => &c.user.login,
            Comment::Issue(c) => &c.user.login,
        }
    }

    pub fn body(&self) -> &str {
        match self {
            Comment::Review(c) => &c.body,
            Comment::Issue(c) => &c.body,
        }
    }

    pub fn created_at(&self) -> &DateTime<Utc> {
        match self {
            Comment::Review(c) => &c.created_at,
            Comment::Issue(c) => &c.created_at,
        }
    }

    /// Returns (path, line) for review comments, None for issue comments
    pub fn location(&self) -> Option<(&str, u32)> {
        match self {
            Comment::Review(c) => c.line.map(|l| (c.path.as_str(), l)),
            Comment::Issue(_) => None,
        }
    }
}
