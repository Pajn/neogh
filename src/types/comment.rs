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

/// Trait for common comment operations
pub trait CommentExt {
    fn author(&self) -> &str;
    fn body(&self) -> &str;
    fn created_at(&self) -> &DateTime<Utc>;
    fn location(&self) -> Option<(&str, u32)>;
    
    /// Number of lines this comment takes when rendered.
    /// Matches CommentBuffer::render_comment_lines:
    /// - separator (1)
    /// - header (1)
    /// - author/time (1)
    /// - sub_separator (1)
    /// - body lines (N, minimum 1)
    /// - separator (1)
    /// Total: 5 + body_lines
    fn height(&self) -> usize {
        let body_lines = self.body().lines().count().max(1);
        5 + body_lines
    }
}

impl CommentExt for Comment {
    fn author(&self) -> &str {
        match self {
            Comment::Review(c) => &c.user.login,
            Comment::Issue(c) => &c.user.login,
        }
    }

    fn body(&self) -> &str {
        match self {
            Comment::Review(c) => &c.body,
            Comment::Issue(c) => &c.body,
        }
    }

    fn created_at(&self) -> &DateTime<Utc> {
        match self {
            Comment::Review(c) => &c.created_at,
            Comment::Issue(c) => &c.created_at,
        }
    }

    fn location(&self) -> Option<(&str, u32)> {
        match self {
            Comment::Review(c) => c.line.map(|l| (c.path.as_str(), l)),
            Comment::Issue(_) => None,
        }
    }
}

impl Comment {
    pub fn height(&self) -> usize {
        CommentExt::height(self)
    }
}
