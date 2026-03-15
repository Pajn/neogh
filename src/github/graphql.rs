use crate::github::workflow::{CheckConclusion, CheckRun, CheckStatus, CheckSuite};
use crate::types::{Comment, CommentThread, IssueComment, ReviewComment, User};
use chrono::{DateTime, Utc};
use octocrab::Octocrab;
use serde::Deserialize;

#[derive(Debug)]
pub enum GraphQLError {
    RequestFailed(String),
}

impl std::fmt::Display for GraphQLError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GraphQLError::RequestFailed(msg) => write!(f, "Request failed: {}", msg),
        }
    }
}

impl std::error::Error for GraphQLError {}

/// Resolve a review thread
pub fn resolve_thread(token: &str, thread_id: &str) -> Result<bool, GraphQLError> {
    tokio::task::block_in_place(|| {
        let rt = tokio::runtime::Runtime::new()
            .map_err(|e| GraphQLError::RequestFailed(e.to_string()))?;
        
        rt.block_on(async {
        let octocrab = Octocrab::builder()
            .personal_token(token.to_string())
            .build()
            .map_err(|e| GraphQLError::RequestFailed(e.to_string()))?;
        
        let query = r#"
            mutation($threadId: ID!) {
                resolveReviewThread(input: {threadId: $threadId}) {
                    thread {
                        id
                        isResolved
                    }
                }
            }
        "#;
        
        let variables = serde_json::json!({
            "query": query,
            "variables": {
                "threadId": thread_id
            }
        });
        
        let response: serde_json::Value = octocrab
            .graphql(&variables)
            .await
            .map_err(|e| GraphQLError::RequestFailed(e.to_string()))?;
        
        let is_resolved = response
            .get("data")
            .and_then(|d| d.get("resolveReviewThread"))
            .and_then(|r| r.get("thread"))
            .and_then(|t| t.get("isResolved"))
            .and_then(|b| b.as_bool())
            .unwrap_or(false);
        
        Ok(is_resolved)
        })
    })
}

/// Unresolve a review thread
pub fn unresolve_thread(token: &str, thread_id: &str) -> Result<bool, GraphQLError> {
    tokio::task::block_in_place(|| {
        let rt = tokio::runtime::Runtime::new()
            .map_err(|e| GraphQLError::RequestFailed(e.to_string()))?;
        
        rt.block_on(async {
        let octocrab = Octocrab::builder()
            .personal_token(token.to_string())
            .build()
            .map_err(|e| GraphQLError::RequestFailed(e.to_string()))?;
        
        let query = r#"
            mutation($threadId: ID!) {
                unresolveReviewThread(input: {threadId: $threadId}) {
                    thread {
                        id
                        isResolved
                    }
                }
            }
        "#;
        
        let variables = serde_json::json!({
            "query": query,
            "variables": {
                "threadId": thread_id
            }
        });
        
        let response: serde_json::Value = octocrab
            .graphql(&variables)
            .await
            .map_err(|e| GraphQLError::RequestFailed(e.to_string()))?;
        
        let is_resolved = response
            .get("data")
            .and_then(|d| d.get("unresolveReviewThread"))
            .and_then(|r| r.get("thread"))
            .and_then(|t| t.get("isResolved"))
            .and_then(|b| b.as_bool())
            .unwrap_or(true);
        
        Ok(is_resolved)
        })
    })
}

#[derive(Debug, Clone, Deserialize)]
pub struct PrInfo {
    pub number: u64,
    pub title: String,
    #[serde(rename = "headRefName")]
    pub head_ref_name: String,
    #[serde(rename = "baseRefName")]
    pub base_ref_name: String,
}

#[derive(Debug, Clone)]
pub struct RelatedPrs {
    pub parent: Option<PrInfo>,
    pub children: Vec<PrInfo>,
}

#[derive(Debug, Deserialize)]
struct RelatedPrsResponse {
    data: RelatedPrsData,
}

#[derive(Debug, Deserialize)]
struct RelatedPrsData {
    repository: RelatedPrsRepository,
}

#[derive(Debug, Deserialize)]
struct RelatedPrsRepository {
    #[serde(rename = "prsByHead")]
    prs_by_head: PrConnection,
    #[serde(rename = "prsByBase")]
    prs_by_base: PrConnection,
}

#[derive(Debug, Deserialize)]
struct PrConnection {
    nodes: Vec<PrInfo>,
}

pub fn find_related_prs(
    token: &str,
    owner: &str,
    repo: &str,
    head_ref: &str,
    base_ref: &str,
) -> Result<RelatedPrs, GraphQLError> {
    tokio::task::block_in_place(|| {
        let rt = tokio::runtime::Runtime::new()
            .map_err(|e| GraphQLError::RequestFailed(e.to_string()))?;

        rt.block_on(async {
            let octocrab = Octocrab::builder()
                .personal_token(token.to_string())
                .build()
                .map_err(|e| GraphQLError::RequestFailed(e.to_string()))?;

            let query = r#"
                query($owner: String!, $repo: String!, $headRef: String, $baseRef: String) {
                    repository(owner: $owner, name: $repo) {
                        prsByHead: pullRequests(headRefName: $headRef, first: 10, states: [OPEN]) {
                            nodes {
                                number
                                title
                                headRefName
                                baseRefName
                            }
                        }
                        prsByBase: pullRequests(baseRefName: $baseRef, first: 10, states: [OPEN]) {
                            nodes {
                                number
                                title
                                headRefName
                                baseRefName
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
                    "headRef": base_ref,
                    "baseRef": head_ref
                }
            });

            let response: RelatedPrsResponse = octocrab
                .graphql(&variables)
                .await
                .map_err(|e| GraphQLError::RequestFailed(e.to_string()))?;

            let parent = response
                .data
                .repository
                .prs_by_head
                .nodes
                .into_iter()
                .next();

            let children = response.data.repository.prs_by_base.nodes;

            Ok(RelatedPrs { parent, children })
        })
    })
}

#[derive(Debug, Deserialize)]
struct PrCommentsResponse {
    data: PrCommentsData,
}

#[derive(Debug, Deserialize)]
struct PrCommentsData {
    repository: PrCommentsRepository,
}

#[derive(Debug, Deserialize)]
struct PrCommentsRepository {
    #[serde(rename = "pullRequest")]
    pull_request: PrCommentsPullRequest,
}

#[derive(Debug, Deserialize)]
struct PrCommentsPullRequest {
    #[serde(rename = "reviewThreads")]
    review_threads: ReviewThreadsConnection,
    comments: IssueCommentsConnection,
}

#[derive(Debug, Deserialize)]
struct ReviewThreadsConnection {
    nodes: Vec<ReviewThreadNode>,
}

#[derive(Debug, Deserialize)]
struct ReviewThreadNode {
    id: String,
    #[serde(rename = "isResolved")]
    is_resolved: bool,
    path: Option<String>,
    line: Option<i32>,
    comments: ThreadCommentsConnection,
}

#[derive(Debug, Deserialize)]
struct ThreadCommentsConnection {
    nodes: Vec<ThreadCommentNode>,
}

#[derive(Debug, Deserialize)]
struct ThreadCommentNode {
    id: String,
    #[serde(rename = "databaseId")]
    database_id: i64,
    body: String,
    author: Option<Author>,
    #[serde(rename = "createdAt")]
    created_at: String,
    path: Option<String>,
    line: Option<i32>,
    #[serde(rename = "originalLine")]
    original_line: Option<i32>,
}

#[derive(Debug, Deserialize)]
struct IssueCommentsConnection {
    nodes: Vec<IssueCommentNode>,
}

#[derive(Debug, Deserialize)]
struct IssueCommentNode {
    id: String,
    #[serde(rename = "databaseId")]
    database_id: i64,
    body: String,
    author: Option<Author>,
    #[serde(rename = "createdAt")]
    created_at: String,
}

#[derive(Debug, Deserialize)]
struct Author {
    login: String,
}

pub fn fetch_pr_comments(
    token: &str,
    owner: &str,
    repo: &str,
    pr_number: u64,
) -> Result<Vec<CommentThread>, GraphQLError> {
    tokio::task::block_in_place(|| {
        let rt = tokio::runtime::Runtime::new()
            .map_err(|e| GraphQLError::RequestFailed(e.to_string()))?;

        rt.block_on(async {
            let octocrab = Octocrab::builder()
                .personal_token(token.to_string())
                .build()
                .map_err(|e| GraphQLError::RequestFailed(e.to_string()))?;

            let query = r#"
                query($owner: String!, $repo: String!, $number: Int!) {
                    repository(owner: $owner, name: $repo) {
                        pullRequest(number: $number) {
                            reviewThreads(first: 100) {
                                nodes {
                                    id
                                    isResolved
                                    path
                                    line
                                    comments(first: 100) {
                                        nodes {
                                            databaseId
                                            id
                                            body
                                            author { login }
                                            createdAt
                                            path
                                            line
                                            originalLine
                                        }
                                    }
                                }
                            }
                            comments(first: 100) {
                                nodes {
                                    databaseId
                                    id
                                    body
                                    author { login }
                                    createdAt
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

            let response: PrCommentsResponse = octocrab
                .graphql(&variables)
                .await
                .map_err(|e| GraphQLError::RequestFailed(e.to_string()))?;

            let mut threads = Vec::new();

            for thread_node in response.data.repository.pull_request.review_threads.nodes {
                let thread_id = thread_node.id;
                let is_resolved = thread_node.is_resolved;

                let comment_nodes = thread_node.comments.nodes;
                if comment_nodes.is_empty() {
                    continue;
                }

                let mut comments: Vec<ReviewComment> = comment_nodes
                    .into_iter()
                    .map(|c| {
                        let created_at: DateTime<Utc> = c.created_at
                            .parse()
                            .unwrap_or_else(|_| Utc::now());
                        ReviewComment {
                            id: c.database_id as u64,
                            node_id: Some(c.id),
                            path: c.path.or(thread_node.path.clone()).unwrap_or_default(),
                            line: c.line.map(|l| l as u32),
                            original_line: c.original_line.map(|l| l as u32),
                            body: c.body,
                            user: User {
                                login: c.author.map(|a| a.login).unwrap_or_else(|| "unknown".to_string()),
                                html_url: None,
                            },
                            created_at,
                            html_url: String::new(),
                            commit_id: String::new(),
                            original_commit_id: String::new(),
                            diff_hunk: String::new(),
                            in_reply_to_id: None,
                            pull_request_review_id: None,
                        }
                    })
                    .collect();

                comments.sort_by_key(|c| c.created_at);

                let root = comments.remove(0);
                let replies: Vec<Comment> = comments.into_iter().map(Comment::Review).collect();

                threads.push(CommentThread {
                    thread_id: Some(thread_id),
                    is_resolved,
                    root: Comment::Review(root),
                    replies,
                });
            }

            for issue_comment in response.data.repository.pull_request.comments.nodes {
                let created_at: DateTime<Utc> = issue_comment.created_at
                    .parse()
                    .unwrap_or_else(|_| Utc::now());

                threads.push(CommentThread::single(Comment::Issue(IssueComment {
                    id: issue_comment.database_id as u64,
                    node_id: Some(issue_comment.id),
                    body: issue_comment.body,
                    user: User {
                        login: issue_comment.author.map(|a| a.login).unwrap_or_else(|| "unknown".to_string()),
                        html_url: None,
                    },
                    created_at,
                    html_url: String::new(),
                })));
            }

            threads.sort_by_key(|t| *t.created_at());

            Ok(threads)
        })
    })
}

// === Check Runs / Workflow Status ===

#[derive(Debug, Deserialize)]
struct CheckRunsResponse {
    data: CheckRunsData,
}

#[derive(Debug, Deserialize)]
struct CheckRunsData {
    repository: CheckRunsRepository,
}

#[derive(Debug, Deserialize)]
struct CheckRunsRepository {
    #[serde(rename = "pullRequest")]
    pull_request: CheckRunsPullRequest,
}

#[derive(Debug, Deserialize)]
struct CheckRunsPullRequest {
    commits: CommitConnection,
}

#[derive(Debug, Deserialize)]
struct CommitConnection {
    nodes: Vec<CommitNode>,
}

#[derive(Debug, Deserialize)]
struct CommitNode {
    commit: CommitInfo,
}

#[derive(Debug, Deserialize)]
struct CommitInfo {
    #[serde(rename = "checkSuites")]
    check_suites: Option<CheckSuiteConnection>,
}

#[derive(Debug, Deserialize)]
struct CheckSuiteConnection {
    nodes: Vec<CheckSuiteNode>,
}

#[derive(Debug, Deserialize)]
struct CheckSuiteNode {
    app: Option<AppInfo>,
    status: String,
    conclusion: Option<String>,
    #[serde(rename = "checkRuns")]
    check_runs: Option<CheckRunConnection>,
}

#[derive(Debug, Deserialize)]
struct AppInfo {
    name: String,
}

#[derive(Debug, Deserialize)]
struct CheckRunConnection {
    nodes: Vec<CheckRunNode>,
}

#[derive(Debug, Deserialize)]
struct CheckRunNode {
    name: String,
    status: String,
    conclusion: Option<String>,
    #[serde(rename = "startedAt")]
    started_at: Option<String>,
    #[serde(rename = "completedAt")]
    completed_at: Option<String>,
    #[serde(rename = "detailsUrl")]
    details_url: Option<String>,
}

/// Fetch check suites and check runs for a PR's latest commit
pub fn fetch_check_runs(
    token: &str,
    owner: &str,
    repo: &str,
    pr_number: u64,
) -> Result<Vec<CheckSuite>, GraphQLError> {
    tokio::task::block_in_place(|| {
        let rt = tokio::runtime::Runtime::new()
            .map_err(|e| GraphQLError::RequestFailed(e.to_string()))?;

        rt.block_on(async {
            let octocrab = Octocrab::builder()
                .personal_token(token.to_string())
                .build()
                .map_err(|e| GraphQLError::RequestFailed(e.to_string()))?;

            let query = r#"
                query($owner: String!, $repo: String!, $number: Int!) {
                    repository(owner: $owner, name: $repo) {
                        pullRequest(number: $number) {
                            commits(last: 1) {
                                nodes {
                                    commit {
                                        checkSuites(first: 20) {
                                            nodes {
                                                app { name }
                                                status
                                                conclusion
                                                checkRuns(first: 50) {
                                                    nodes {
                                                        name
                                                        status
                                                        conclusion
                                                        startedAt
                                                        completedAt
                                                        detailsUrl
                                                    }
                                                }
                                            }
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

            // First get raw response to debug errors
            let raw_response: serde_json::Value = octocrab
                .graphql(&variables)
                .await
                .map_err(|e| GraphQLError::RequestFailed(e.to_string()))?;
            
            // Check for GraphQL errors
            if let Some(errors) = raw_response.get("errors") {
                let error_msg = errors.to_string();
                eprintln!("GraphQL errors: {}", error_msg);
                return Err(GraphQLError::RequestFailed(format!("GraphQL error: {}", error_msg)));
            }

            let response: CheckRunsResponse = serde_json::from_value(raw_response)
                .map_err(|e| GraphQLError::RequestFailed(format!("Parse error: {}", e)))?;

            let mut suites = Vec::new();

            for commit_node in response.data.repository.pull_request.commits.nodes {
                if let Some(check_suites) = commit_node.commit.check_suites {
                    for suite_node in check_suites.nodes {
                        let app_name = suite_node.app.map(|a| a.name).unwrap_or_else(|| "Unknown".to_string());
                        let status = CheckStatus::from_str(&suite_node.status);
                        let conclusion = suite_node.conclusion.map(|c| CheckConclusion::from_str(&c));

                        let mut check_runs = Vec::new();
                        if let Some(cr_conn) = suite_node.check_runs {
                            for cr_node in cr_conn.nodes {
                                let started_at = cr_node.started_at.and_then(|s| s.parse().ok());
                                let completed_at = cr_node.completed_at.and_then(|s| s.parse().ok());

                                check_runs.push(CheckRun {
                                    name: cr_node.name,
                                    status: CheckStatus::from_str(&cr_node.status),
                                    conclusion: cr_node.conclusion.map(|c| CheckConclusion::from_str(&c)),
                                    started_at,
                                    completed_at,
                                    details_url: cr_node.details_url,
                                });
                            }
                        }

                        suites.push(CheckSuite {
                            app_name,
                            status,
                            conclusion,
                            check_runs,
                        });
                    }
                }
            }

            Ok(suites)
        })
    })
}
