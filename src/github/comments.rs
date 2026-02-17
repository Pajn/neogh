use std::fmt;
use std::process::Command;

use chrono::{DateTime, Utc};

use crate::types::{Comment, IssueComment, ReviewComment, User};

#[derive(Debug)]
pub enum CommentsError {
    GhError(String),
    ParseError(String),
    NoPr,
}

impl fmt::Display for CommentsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CommentsError::GhError(msg) => write!(f, "gh command failed: {}", msg),
            CommentsError::ParseError(msg) => write!(f, "JSON parsing failed: {}", msg),
            CommentsError::NoPr => write!(f, "no PR number provided"),
        }
    }
}

impl std::error::Error for CommentsError {}

#[derive(Debug, Clone, serde::Deserialize)]
struct GhUser {
    login: String,
}

#[derive(Debug, Clone, serde::Deserialize)]
struct GhReviewComment {
    pull_request_review_id: u64,
    path: String,
    line: Option<u32>,
    body: String,
    user: GhUser,
    created_at: String,
    html_url: String,
}

#[derive(Debug, Clone, serde::Deserialize)]
struct GhIssueComment {
    id: u64,
    body: String,
    user: GhUser,
    created_at: String,
    html_url: String,
}

fn parse_datetime(s: &str) -> Result<DateTime<Utc>, CommentsError> {
    DateTime::parse_from_rfc3339(s)
        .map(|dt| dt.with_timezone(&Utc))
        .map_err(|e| CommentsError::ParseError(format!("failed to parse timestamp '{}': {}", s, e)))
}

fn convert_review_comment(gh: GhReviewComment) -> Result<ReviewComment, CommentsError> {
    let created_at = parse_datetime(&gh.created_at)?;
    Ok(ReviewComment {
        id: gh.pull_request_review_id,
        path: gh.path,
        line: gh.line,
        body: gh.body,
        user: User {
            login: gh.user.login,
            html_url: None,
        },
        created_at,
        html_url: gh.html_url,
    })
}

fn convert_issue_comment(gh: GhIssueComment) -> Result<IssueComment, CommentsError> {
    let created_at = parse_datetime(&gh.created_at)?;
    Ok(IssueComment {
        id: gh.id,
        body: gh.body,
        user: User {
            login: gh.user.login,
            html_url: None,
        },
        created_at,
        html_url: gh.html_url,
    })
}

fn run_gh_api(endpoint: &str) -> Result<String, CommentsError> {
    let output = Command::new("gh")
        .args(["api", endpoint])
        .output()
        .map_err(|e| CommentsError::GhError(format!("failed to execute gh: {}", e)))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(CommentsError::GhError(stderr.to_string()));
    }

    String::from_utf8(output.stdout)
        .map_err(|e| CommentsError::ParseError(format!("invalid UTF-8 in response: {}", e)))
}

pub fn fetch_comments(
    owner: &str,
    repo: &str,
    pr_number: u64,
) -> Result<Vec<Comment>, CommentsError> {
    let mut comments = Vec::new();

    let review_endpoint = format!("repos/{}/{}/pulls/{}/comments", owner, repo, pr_number);
    match run_gh_api(&review_endpoint) {
        Ok(json) => {
            let gh_comments: Vec<GhReviewComment> = serde_json::from_str(&json).map_err(|e| {
                CommentsError::ParseError(format!("failed to parse review comments: {}", e))
            })?;
            for gh in gh_comments {
                match convert_review_comment(gh) {
                    Ok(review) => comments.push(Comment::Review(review)),
                    Err(e) => eprintln!("Warning: {}", e),
                }
            }
        }
        Err(CommentsError::GhError(ref msg)) if msg.contains("404") => {}
        Err(e) => return Err(e),
    }

    let issue_endpoint = format!("repos/{}/{}/issues/{}/comments", owner, repo, pr_number);
    match run_gh_api(&issue_endpoint) {
        Ok(json) => {
            let gh_comments: Vec<GhIssueComment> = serde_json::from_str(&json).map_err(|e| {
                CommentsError::ParseError(format!("failed to parse issue comments: {}", e))
            })?;
            for gh in gh_comments {
                match convert_issue_comment(gh) {
                    Ok(issue) => comments.push(Comment::Issue(issue)),
                    Err(e) => eprintln!("Warning: {}", e),
                }
            }
        }
        Err(CommentsError::GhError(ref msg)) if msg.contains("404") => {}
        Err(e) => return Err(e),
    }

    comments.sort_by_key(|c| *c.created_at());

    Ok(comments)
}
