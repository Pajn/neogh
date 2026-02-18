use std::process::Command;

#[derive(Debug)]
pub enum PrError {
    NotAGitRepo,
    GhError(String),
    NoAssociatedPr,
    IoError(String),
    ParseError(String),
}

impl std::fmt::Display for PrError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PrError::NotAGitRepo => write!(f, "Not a git repository"),
            PrError::GhError(err) => write!(f, "gh error: {}", err),
            PrError::NoAssociatedPr => write!(f, "No PR associated with current branch"),
            PrError::IoError(err) => write!(f, "IO error: {}", err),
            PrError::ParseError(err) => write!(f, "Parse error: {}", err),
        }
    }
}

#[derive(Debug, Clone)]
pub struct PullRequest {
    pub number: u64,
    pub title: String,
    pub html_url: String,
    pub head_ref: String,
    pub base_ref: String,
    pub owner: String,
    pub repo: String,
}

pub fn detect_pr() -> Result<PullRequest, PrError> {
    let output = Command::new("gh")
        .args([
            "pr",
            "view",
            "--json",
            "number,title,headRefName,baseRefName,url",
        ])
        .output()
        .map_err(|e| PrError::IoError(e.to_string()))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        if stderr.contains("not a git repository")
            || stderr.contains("could not find a local checkout")
        {
            return Err(PrError::NotAGitRepo);
        }
        if stderr.contains("no pull requests found") || stderr.contains("could not find") {
            return Err(PrError::NoAssociatedPr);
        }
        return Err(PrError::GhError(stderr.to_string()));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let response: serde_json::Value = serde_json::from_str(&stdout)
        .map_err(|e| PrError::ParseError(e.to_string()))?;

    let url = response["url"].as_str().unwrap_or("");
    
    // Parse owner/repo from URL: https://github.com/owner/repo/pull/123
    let parts: Vec<&str> = url.trim_end_matches('/').split('/').collect();
    let (owner, repo) = if parts.len() >= 5 && parts[2] == "github.com" {
        (parts[3].to_string(), parts[4].to_string())
    } else {
        (String::new(), String::new())
    };

    Ok(PullRequest {
        number: response["number"].as_u64().unwrap_or(0),
        title: response["title"].as_str().unwrap_or("").to_string(),
        html_url: url.to_string(),
        head_ref: response["headRefName"].as_str().unwrap_or("").to_string(),
        base_ref: response["baseRefName"].as_str().unwrap_or("").to_string(),
        owner,
        repo,
    })
}
