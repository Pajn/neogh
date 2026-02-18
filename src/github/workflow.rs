use chrono::{DateTime, Utc};

/// Represents a check suite (GitHub Actions workflow run)
#[derive(Debug, Clone)]
pub struct CheckSuite {
    pub app_name: String,
    pub status: CheckStatus,
    pub conclusion: Option<CheckConclusion>,
    pub check_runs: Vec<CheckRun>,
}

/// Individual check run within a suite
#[derive(Debug, Clone)]
pub struct CheckRun {
    pub name: String,
    pub status: CheckStatus,
    pub conclusion: Option<CheckConclusion>,
    pub started_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
    pub details_url: Option<String>,
}

/// Status of a check
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckStatus {
    Queued,
    InProgress,
    Completed,
    Waiting,
    Requested,
    Pending,
    Unknown,
}

/// Conclusion of a completed check
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckConclusion {
    Success,
    Failure,
    Neutral,
    Cancelled,
    Skipped,
    TimedOut,
    ActionRequired,
    Unknown,
}

impl CheckSuite {
    /// Returns true if all checks in this suite have passed
    pub fn is_success(&self) -> bool {
        self.conclusion == Some(CheckConclusion::Success)
    }

    /// Returns true if any check in this suite has failed
    pub fn is_failure(&self) -> bool {
        self.conclusion == Some(CheckConclusion::Failure)
    }

    /// Returns true if the suite is still running
    pub fn is_in_progress(&self) -> bool {
        self.status == CheckStatus::InProgress || self.status == CheckStatus::Queued
    }
}

impl CheckStatus {
    pub fn from_str(s: &str) -> Self {
        match s {
            "QUEUED" => CheckStatus::Queued,
            "IN_PROGRESS" => CheckStatus::InProgress,
            "COMPLETED" => CheckStatus::Completed,
            "WAITING" => CheckStatus::Waiting,
            "REQUESTED" => CheckStatus::Requested,
            "PENDING" => CheckStatus::Pending,
            _ => CheckStatus::Unknown,
        }
    }

    pub fn to_display(&self) -> &'static str {
        match self {
            CheckStatus::Queued => "⏳ Queued",
            CheckStatus::InProgress => "🔄 Running",
            CheckStatus::Completed => "✓ Done",
            CheckStatus::Waiting => "⏳ Waiting",
            CheckStatus::Requested => "📋 Requested",
            CheckStatus::Pending => "⏳ Pending",
            CheckStatus::Unknown => "? Unknown",
        }
    }
}

impl CheckConclusion {
    pub fn from_str(s: &str) -> Self {
        match s {
            "SUCCESS" => CheckConclusion::Success,
            "FAILURE" => CheckConclusion::Failure,
            "NEUTRAL" => CheckConclusion::Neutral,
            "CANCELLED" => CheckConclusion::Cancelled,
            "SKIPPED" => CheckConclusion::Skipped,
            "TIMED_OUT" => CheckConclusion::TimedOut,
            "ACTION_REQUIRED" => CheckConclusion::ActionRequired,
            _ => CheckConclusion::Unknown,
        }
    }

    pub fn to_display(&self) -> &'static str {
        match self {
            CheckConclusion::Success => "✅ Success",
            CheckConclusion::Failure => "❌ Failure",
            CheckConclusion::Neutral => "⚪ Neutral",
            CheckConclusion::Cancelled => "🚫 Cancelled",
            CheckConclusion::Skipped => "⏭️ Skipped",
            CheckConclusion::TimedOut => "⏰ Timed Out",
            CheckConclusion::ActionRequired => "⚠️ Action Required",
            CheckConclusion::Unknown => "? Unknown",
        }
    }
}
