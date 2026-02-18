use chrono::{DateTime, Utc};
use serde::Deserialize;

/// A review comment on a specific line of code
#[derive(Debug, Clone, Deserialize)]
pub struct ReviewComment {
    pub id: u64,
    pub path: String,
    pub line: Option<u32>,
    pub original_line: Option<u32>,
    pub body: String,
    pub user: User,
    pub created_at: DateTime<Utc>,
    pub html_url: String,
    pub commit_id: String,
    pub original_commit_id: String,
    pub diff_hunk: String,
    pub in_reply_to_id: Option<u64>,
    pub pull_request_review_id: Option<u64>,
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

impl ReviewComment {
    pub fn navigation_line(&self) -> Option<u32> {
        self.line.or(self.original_line)
    }

    pub fn is_on_older_commit(&self, head_sha: &str) -> bool {
        self.commit_id != head_sha
    }

    pub fn is_line_deleted(&self) -> bool {
        self.line.is_none() && self.original_line.is_some()
    }
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

/// A thread of review comments (root + replies) or a single comment
#[derive(Debug, Clone)]
pub struct CommentThread {
    pub thread_id: Option<String>,
    pub is_resolved: bool,
    pub root: Comment,
    pub replies: Vec<Comment>,
}

impl CommentThread {
    pub fn single(comment: Comment) -> Self {
        Self {
            thread_id: None,
            is_resolved: false,
            root: comment,
            replies: Vec::new(),
        }
    }

    pub fn all_comments(&self) -> Vec<&Comment> {
        std::iter::once(&self.root)
            .chain(self.replies.iter())
            .collect()
    }

    pub fn created_at(&self) -> &DateTime<Utc> {
        self.root.created_at()
    }

    pub fn height(&self) -> usize {
        let root_height = self.root.height();
        let replies_height: usize = self.replies.iter().map(|c| c.height()).sum();
        root_height + replies_height
    }
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
            Comment::Review(c) => c.navigation_line().map(|l| (c.path.as_str(), l)),
            Comment::Issue(_) => None,
        }
    }
}

impl Comment {
    pub fn height(&self) -> usize {
        CommentExt::height(self)
    }
}
