use crate::github::graphql::fetch_pr_comments;
use crate::types::CommentThread;
use std::fmt;

#[derive(Debug)]
pub enum CommentsError {
    GraphQLError(String),
}

impl fmt::Display for CommentsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CommentsError::GraphQLError(msg) => write!(f, "GitHub GraphQL error: {}", msg),
        }
    }
}

impl std::error::Error for CommentsError {}

pub fn fetch_comments(
    token: &str,
    owner: &str,
    repo: &str,
    pr_number: u64,
) -> Result<Vec<CommentThread>, CommentsError> {
    fetch_pr_comments(token, owner, repo, pr_number)
        .map_err(|e| CommentsError::GraphQLError(e.to_string()))
}
