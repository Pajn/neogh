use std::io;
use std::process::Command;

#[derive(Debug)]
pub enum AuthError {
    GhNotFound,
    NotAuthenticated,
    IoError(String),
}

impl std::fmt::Display for AuthError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AuthError::GhNotFound => write!(
                f,
                "gh CLI not found. Please install: https://cli.github.com"
            ),
            AuthError::NotAuthenticated => {
                write!(f, "Not authenticated with gh. Run: gh auth login")
            }
            AuthError::IoError(msg) => write!(f, "IO error: {}", msg),
        }
    }
}

impl std::error::Error for AuthError {}

pub fn is_gh_installed() -> bool {
    Command::new("gh")
        .arg("--version")
        .output()
        .map(|_| true)
        .unwrap_or(false)
}

pub fn get_gh_token() -> Result<String, AuthError> {
    if !is_gh_installed() {
        return Err(AuthError::GhNotFound);
    }

    let output = Command::new("gh")
        .arg("auth")
        .arg("token")
        .output()
        .map_err(|e| AuthError::IoError(e.to_string()))?;

    if !output.status.success() {
        return Err(AuthError::NotAuthenticated);
    }

    let token = String::from_utf8_lossy(&output.stdout).trim().to_string();

    if token.is_empty() {
        return Err(AuthError::NotAuthenticated);
    }

    Ok(token)
}
