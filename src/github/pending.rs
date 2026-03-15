use crate::types::{Comment, CommentThread, ReviewComment, User};
use chrono::{DateTime, Utc};
use octocrab::Octocrab;
use serde::Deserialize;

#[derive(Debug)]
pub enum PendingCommentsError {
    RequestFailed(String),
}

impl std::fmt::Display for PendingCommentsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PendingCommentsError::RequestFailed(msg) => write!(f, "Request failed: {}", msg),
        }
    }
}

impl std::error::Error for PendingCommentsError {}

#[derive(Debug, Deserialize)]
struct PendingCommentsResponse {
    data: PendingCommentsData,
}

#[derive(Debug, Deserialize)]
struct PendingCommentsData {
    repository: PendingRepository,
}

#[derive(Debug, Deserialize)]
struct PendingRepository {
    #[serde(rename = "pullRequest")]
    pull_request: PendingPullRequest,
}

#[derive(Debug, Deserialize)]
struct PendingPullRequest {
    reviews: PendingReviewsConnection,
    #[serde(rename = "headRefOid")]
    head_ref_oid: Option<String>,
}

#[derive(Debug, Deserialize)]
struct PendingReviewsConnection {
    nodes: Vec<PendingReviewNode>,
}

#[derive(Debug, Deserialize)]
struct PendingReviewNode {
    state: String,
    author: Option<PendingAuthor>,
    comments: PendingReviewCommentsConnection,
}

#[derive(Debug, Deserialize)]
struct PendingAuthor {
    login: String,
}

#[derive(Debug, Deserialize)]
struct PendingReviewCommentsConnection {
    nodes: Vec<PendingReviewCommentNode>,
}

#[derive(Debug, Deserialize)]
struct PendingReviewCommentNode {
    id: String,
    #[serde(rename = "databaseId")]
    database_id: i64,
    body: String,
    path: Option<String>,
    line: Option<i32>,
    #[serde(rename = "originalLine")]
    original_line: Option<i32>,
    #[serde(rename = "createdAt")]
    created_at: String,
    #[serde(rename = "url")]
    html_url: Option<String>,
    #[serde(rename = "diffHunk")]
    diff_hunk: Option<String>,
    commit: Option<PendingCommitNode>,
    #[serde(rename = "originalCommit")]
    original_commit: Option<PendingCommitNode>,
}

#[derive(Debug, Deserialize)]
struct PendingCommitNode {
    oid: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ViewerResponse {
    data: ViewerData,
}

#[derive(Debug, Deserialize)]
struct ViewerData {
    viewer: ViewerNode,
}

#[derive(Debug, Deserialize)]
struct ViewerNode {
    login: String,
}

pub fn fetch_pending_review_comments(
    token: &str,
    owner: &str,
    repo: &str,
    pr_number: u64,
) -> Result<Vec<CommentThread>, PendingCommentsError> {
    tokio::task::block_in_place(|| {
        let rt = tokio::runtime::Runtime::new()
            .map_err(|e| PendingCommentsError::RequestFailed(e.to_string()))?;

        rt.block_on(async {
            let octocrab = Octocrab::builder()
                .personal_token(token.to_string())
                .build()
                .map_err(|e| PendingCommentsError::RequestFailed(e.to_string()))?;

            let viewer_query = serde_json::json!({
                "query": "query { viewer { login } }"
            });

            let viewer_response: ViewerResponse = octocrab
                .graphql(&viewer_query)
                .await
                .map_err(|e| PendingCommentsError::RequestFailed(e.to_string()))?;

            let viewer_login = viewer_response.data.viewer.login;

            let query = r#"
                query($owner: String!, $repo: String!, $number: Int!) {
                    repository(owner: $owner, name: $repo) {
                        pullRequest(number: $number) {
                            headRefOid
                            reviews(last: 50) {
                                nodes {
                                    state
                                    author { login }
                                    comments(last: 100) {
                                        nodes {
                                            databaseId
                                            id
                                            body
                                            path
                                            line
                                            originalLine
                                            createdAt
                                            url
                                            diffHunk
                                            commit { oid }
                                            originalCommit { oid }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            "#;

            let variables = serde_json::json!({
                "query": query,
                "variables": {
                    "owner": owner,
                    "repo": repo,
                    "number": pr_number as i32
                }
            });

            let response: PendingCommentsResponse = octocrab
                .graphql(&variables)
                .await
                .map_err(|e| PendingCommentsError::RequestFailed(e.to_string()))?;

            let head_oid = response
                .data
                .repository
                .pull_request
                .head_ref_oid
                .unwrap_or_default();

            let mut threads = Vec::new();

            for review in response.data.repository.pull_request.reviews.nodes {
                let author_login = review
                    .author
                    .map(|a| a.login)
                    .unwrap_or_else(|| "unknown".to_string());

                if review.state != "PENDING" || author_login != viewer_login {
                    continue;
                }

                for c in review.comments.nodes {
                    let created_at: DateTime<Utc> = c.created_at.parse().unwrap_or_else(|_| Utc::now());
                    let commit_id = c
                        .commit
                        .and_then(|n| n.oid)
                        .unwrap_or_else(|| head_oid.clone());
                    let original_commit_id = c
                        .original_commit
                        .and_then(|n| n.oid)
                        .unwrap_or_else(|| commit_id.clone());

                    let comment = ReviewComment {
                        id: c.database_id.max(0) as u64,
                        node_id: Some(c.id),
                        path: c.path.unwrap_or_default(),
                        line: c.line.and_then(|l| if l > 0 { Some(l as u32) } else { None }),
                        original_line: c
                            .original_line
                            .and_then(|l| if l > 0 { Some(l as u32) } else { None }),
                        body: c.body,
                        user: User {
                            login: author_login.clone(),
                            html_url: None,
                        },
                        created_at,
                        html_url: c.html_url.unwrap_or_default(),
                        commit_id,
                        original_commit_id,
                        diff_hunk: c.diff_hunk.unwrap_or_default(),
                        in_reply_to_id: None,
                        pull_request_review_id: None,
                    };

                    threads.push(CommentThread::single(Comment::Review(comment)));
                }
            }

            threads.sort_by_key(|t| *t.created_at());
            Ok(threads)
        })
    })
}

pub fn delete_pending_review_comment(
    token: &str,
    comment_node_id: &str,
) -> Result<(), PendingCommentsError> {
    tokio::task::block_in_place(|| {
        let rt = tokio::runtime::Runtime::new()
            .map_err(|e| PendingCommentsError::RequestFailed(e.to_string()))?;

        rt.block_on(async {
            let octocrab = Octocrab::builder()
                .personal_token(token.to_string())
                .build()
                .map_err(|e| PendingCommentsError::RequestFailed(e.to_string()))?;

            let mutation = serde_json::json!({
                "query": r#"
                    mutation($id: ID!) {
                        deletePullRequestReviewComment(input: { pullRequestReviewCommentId: $id }) {
                            clientMutationId
                        }
                    }
                "#,
                "variables": { "id": comment_node_id }
            });

            let _: serde_json::Value = octocrab
                .graphql(&mutation)
                .await
                .map_err(|e| PendingCommentsError::RequestFailed(e.to_string()))?;
            Ok(())
        })
    })
}

pub fn edit_pending_review_comment(
    token: &str,
    comment_node_id: &str,
    body: &str,
) -> Result<(), PendingCommentsError> {
    tokio::task::block_in_place(|| {
        let rt = tokio::runtime::Runtime::new()
            .map_err(|e| PendingCommentsError::RequestFailed(e.to_string()))?;

        rt.block_on(async {
            let octocrab = Octocrab::builder()
                .personal_token(token.to_string())
                .build()
                .map_err(|e| PendingCommentsError::RequestFailed(e.to_string()))?;

            let mutation = serde_json::json!({
                "query": r#"
                    mutation($id: ID!, $body: String!) {
                        updatePullRequestReviewComment(
                            input: {
                                pullRequestReviewCommentId: $id,
                                body: $body
                            }
                        ) {
                            pullRequestReviewComment { id }
                        }
                    }
                "#,
                "variables": {
                    "id": comment_node_id,
                    "body": body
                }
            });

            let _: serde_json::Value = octocrab
                .graphql(&mutation)
                .await
                .map_err(|e| PendingCommentsError::RequestFailed(e.to_string()))?;
            Ok(())
        })
    })
}

pub fn delete_issue_comment(
    token: &str,
    comment_node_id: &str,
) -> Result<(), PendingCommentsError> {
    tokio::task::block_in_place(|| {
        let rt = tokio::runtime::Runtime::new()
            .map_err(|e| PendingCommentsError::RequestFailed(e.to_string()))?;

        rt.block_on(async {
            let octocrab = Octocrab::builder()
                .personal_token(token.to_string())
                .build()
                .map_err(|e| PendingCommentsError::RequestFailed(e.to_string()))?;

            let mutation = serde_json::json!({
                "query": r#"
                    mutation($id: ID!) {
                        deleteIssueComment(input: { id: $id }) {
                            clientMutationId
                        }
                    }
                "#,
                "variables": { "id": comment_node_id }
            });

            let _: serde_json::Value = octocrab
                .graphql(&mutation)
                .await
                .map_err(|e| PendingCommentsError::RequestFailed(e.to_string()))?;
            Ok(())
        })
    })
}

pub fn edit_issue_comment(
    token: &str,
    comment_node_id: &str,
    body: &str,
) -> Result<(), PendingCommentsError> {
    tokio::task::block_in_place(|| {
        let rt = tokio::runtime::Runtime::new()
            .map_err(|e| PendingCommentsError::RequestFailed(e.to_string()))?;

        rt.block_on(async {
            let octocrab = Octocrab::builder()
                .personal_token(token.to_string())
                .build()
                .map_err(|e| PendingCommentsError::RequestFailed(e.to_string()))?;

            let mutation = serde_json::json!({
                "query": r#"
                    mutation($id: ID!, $body: String!) {
                        updateIssueComment(input: { id: $id, body: $body }) {
                            issueComment { id }
                        }
                    }
                "#,
                "variables": {
                    "id": comment_node_id,
                    "body": body
                }
            });

            let _: serde_json::Value = octocrab
                .graphql(&mutation)
                .await
                .map_err(|e| PendingCommentsError::RequestFailed(e.to_string()))?;
            Ok(())
        })
    })
}
