use super::auth::get_gh_token;
use super::graphql::{self, GraphQLError};
use super::pr::{PrChain, PrInfo, PullRequest};

#[derive(Debug)]
pub enum ChainError {
    AuthError(String),
    GraphQLError(String),
    NoToken,
}

impl std::fmt::Display for ChainError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ChainError::AuthError(msg) => write!(f, "Auth error: {}", msg),
            ChainError::GraphQLError(msg) => write!(f, "GraphQL error: {}", msg),
            ChainError::NoToken => write!(f, "No GitHub token available"),
        }
    }
}

impl std::error::Error for ChainError {}

impl From<GraphQLError> for ChainError {
    fn from(err: GraphQLError) -> Self {
        match err {
            GraphQLError::RequestFailed(msg) => ChainError::GraphQLError(msg),
        }
    }
}

fn convert_pr_info(gql_info: graphql::PrInfo) -> PrInfo {
    PrInfo {
        number: gql_info.number,
        title: gql_info.title,
        head_ref: gql_info.head_ref_name,
        base_ref: gql_info.base_ref_name,
    }
}

fn is_default_branch(branch: &str) -> bool {
    matches!(branch, "main" | "master" | "develop" | "development")
}

pub fn detect_chain(pr: &PullRequest) -> Result<PrChain, ChainError> {
    let token = get_gh_token().map_err(|e| ChainError::AuthError(e.to_string()))?;

    let mut chain = Vec::new();
    let mut ancestors = Vec::new();
    let mut current_base = pr.base_ref.clone();
    let owner = pr.owner.clone();
    let repo = pr.repo.clone();

    while !is_default_branch(&current_base) {
        let related = find_related_prs(&token, &owner, &repo, "", &current_base)?;
        if let Some(parent) = related.parent {
            current_base = parent.base_ref_name.clone();
            ancestors.push(convert_pr_info(parent));
        } else {
            break;
        }
    }

    for ancestor in ancestors.into_iter().rev() {
        chain.push(ancestor);
    }

    let current_info = PrInfo::from(pr.clone());
    chain.push(current_info);

    let mut children = Vec::new();
    let current_head = pr.head_ref.clone();

    let related = find_related_prs(&token, &owner, &repo, &current_head, "")?;
    for child in related.children {
        children.push(convert_pr_info(child));
    }

    let children_count = children.len();
    chain.extend(children);

    let current_index = chain.len() - children_count - 1;

    Ok(PrChain {
        chain,
        current_index,
    })
}

fn find_related_prs(
    token: &str,
    owner: &str,
    repo: &str,
    head_ref: &str,
    base_ref: &str,
) -> Result<graphql::RelatedPrs, ChainError> {
    graphql::find_related_prs(token, owner, repo, head_ref, base_ref).map_err(ChainError::from)
}

pub fn detect_chain_with_token(pr: &PullRequest, token: &str) -> Result<PrChain, ChainError> {
    let mut chain = Vec::new();
    let mut ancestors = Vec::new();
    let mut current_base = pr.base_ref.clone();
    let owner = pr.owner.clone();
    let repo = pr.repo.clone();

    while !is_default_branch(&current_base) {
        let related = graphql::find_related_prs(token, &owner, &repo, "", &current_base)
            .map_err(ChainError::from)?;
        if let Some(parent) = related.parent {
            current_base = parent.base_ref_name.clone();
            ancestors.push(convert_pr_info(parent));
        } else {
            break;
        }
    }

    for ancestor in ancestors.into_iter().rev() {
        chain.push(ancestor);
    }

    let current_info = PrInfo::from(pr.clone());
    chain.push(current_info);

    let mut children = Vec::new();
    let current_head = pr.head_ref.clone();

    let related = graphql::find_related_prs(token, &owner, &repo, &current_head, "")
        .map_err(ChainError::from)?;
    for child in related.children {
        children.push(convert_pr_info(child));
    }

    let children_count = children.len();
    chain.extend(children);

    let current_index = chain.len() - children_count - 1;

    Ok(PrChain {
        chain,
        current_index,
    })
}
