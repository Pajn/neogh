use serde::Deserialize;
use std::process::Command;

#[derive(Debug)]
pub enum PrError {
    NotAGitRepo,
    GhError(String),
    NoAssociatedPr,
    IoError(String),
    ParseError(String),
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PullRequest {
    pub number: u64,
    pub title: String,
    pub html_url: String,
    pub head_ref: String,
    pub base_ref: String,
    pub owner: String,
    pub repo: String,
}

#[derive(Debug, Deserialize)]
struct GhPrResponse {
    number: u64,
    title: String,
    head_ref_name: String,
    base_ref_name: String,
    url: String,
    head_repository: HeadRepository,
}

#[derive(Debug, Deserialize)]
struct HeadRepository {
    owner: Owner,
    name: String,
}

#[derive(Debug, Deserialize)]
struct Owner {
    login: String,
}

pub fn detect_pr() -> Result<PullRequest, PrError> {
    let output = Command::new("gh")
        .args([
            "pr",
            "view",
            "--json",
            "number,title,headRefName,baseRefName,url,headRepository",
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
    let response: GhPrResponse =
        serde_json::from_str(&stdout).map_err(|e| PrError::ParseError(e.to_string()))?;

    Ok(PullRequest {
        number: response.number,
        title: response.title,
        html_url: response.url,
        head_ref: response.head_ref_name,
        base_ref: response.base_ref_name,
        owner: response.head_repository.owner.login,
        repo: response.head_repository.name,
    })
}
