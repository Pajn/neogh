use crate::types::{Comment, CommentExt, IssueComment, ReviewComment, User};
use octocrab::Octocrab;
use std::fmt;

#[derive(Debug)]
pub enum CommentsError {
    OctocrabError(String),
}

impl fmt::Display for CommentsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CommentsError::OctocrabError(msg) => write!(f, "GitHub API error: {}", msg),
        }
    }
}

impl std::error::Error for CommentsError {}

pub fn fetch_comments(
    token: &str,
    owner: &str,
    repo: &str,
    pr_number: u64,
) -> Result<Vec<Comment>, CommentsError> {
    let rt = tokio::runtime::Runtime::new()
        .map_err(|e| CommentsError::OctocrabError(e.to_string()))?;
    
    rt.block_on(async {
        let octocrab = Octocrab::builder()
            .personal_token(token.to_string())
            .build()
            .map_err(|e| CommentsError::OctocrabError(e.to_string()))?;

        let mut comments = Vec::new();
        let mut page: u32 = 1;

        // Fetch all review comments with pagination
        loop {
            let result = octocrab
                .pulls(owner, repo)
                .list_comments(Some(pr_number))
                .per_page(100)
                .page(page)
                .send()
                .await
                .map_err(|e| CommentsError::OctocrabError(e.to_string()))?;

            for rc in result.items.iter() {
                // Include ALL review comments, not just those with review_id
                let review_id = rc.pull_request_review_id.map(|id| id.0).unwrap_or(rc.id.0);
                
                comments.push(Comment::Review(ReviewComment {
                    id: review_id,
                    path: rc.path.clone(),
                    line: rc.line.map(|l| l as u32).or(rc.original_line.map(|l| l as u32)),
                    body: rc.body.clone(),
                    user: User {
                        login: rc.user.as_ref()
                            .map(|u| u.login.clone())
                            .unwrap_or_else(|| "unknown".to_string()),
                        html_url: rc.user.as_ref()
                            .map(|u| u.html_url.to_string()),
                    },
                    created_at: rc.created_at,
                    html_url: rc.html_url.to_string(),
                }));
            }

            // Check if there's a next page - result.next contains URL to next page
            if result.next.is_none() || result.items.len() < 100 {
                break;
            }
            page += 1;
        }

        page = 1;

        // Fetch all issue comments with pagination
        loop {
            let result = octocrab
                .issues(owner, repo)
                .list_comments(pr_number)
                .per_page(100)
                .page(page)
                .send()
                .await
                .map_err(|e| CommentsError::OctocrabError(e.to_string()))?;

            for ic in result.items.iter() {
                comments.push(Comment::Issue(IssueComment {
                    id: ic.id.0,
                    body: ic.body.clone().unwrap_or_default(),
                    user: User {
                        login: ic.user.login.clone(),
                        html_url: Some(ic.user.html_url.to_string()),
                    },
                    created_at: ic.created_at,
                    html_url: ic.html_url.to_string(),
                }));
            }

            if result.next.is_none() || result.items.len() < 100 {
                break;
            }
            page += 1;
        }

        comments.sort_by_key(|c| *c.created_at());

        Ok(comments)
    })
}
