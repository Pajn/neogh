use crate::github::pr::PrChain;
use crate::github::{CheckConclusion, CheckRun, CheckStatus, CheckSuite};
use chrono::{DateTime, Utc};

/// Renders GitHub Actions workflow status to lines.
/// Uses 0-based line numbers to match get_cursor() behavior.
pub struct ActionsBuffer {
    suites: Vec<CheckSuite>,
    line_map: Vec<(usize, usize, usize)>, // (start_line, end_line, suite_index)
    pr_chain: Option<PrChain>,
}

impl ActionsBuffer {
    pub fn new(suites: Vec<CheckSuite>) -> Self {
        Self {
            suites,
            line_map: Vec::new(),
            pr_chain: None,
        }
    }

    pub fn set_chain(&mut self, chain: Option<PrChain>) {
        self.pr_chain = chain;
    }

    pub fn suites(&self) -> &[CheckSuite] {
        &self.suites
    }

    pub fn is_empty(&self) -> bool {
        self.suites.is_empty()
    }

    pub fn line_to_suite_index(&self, line: usize) -> Option<usize> {
        for &(start, end, idx) in &self.line_map {
            if line >= start && line < end {
                return Some(idx);
            }
        }
        None
    }

    pub fn line_for_suite(&self, index: usize) -> Option<usize> {
        for &(start, _end, idx) in &self.line_map {
            if idx == index {
                return Some(start);
            }
        }
        None
    }

    /// Returns the (start_line, end_line) for a suite at the given index
    pub fn suite_line_range(&self, index: usize) -> Option<(usize, usize)> {
        for &(start, end, idx) in &self.line_map {
            if idx == index {
                return Some((start, end));
            }
        }
        None
    }

    fn format_relative_time(dt: &DateTime<Utc>) -> String {
        let now = Utc::now();
        let diff = now.signed_duration_since(*dt);

        if diff.num_minutes() < 1 {
            "just now".to_string()
        } else if diff.num_minutes() < 60 {
            format!(
                "{} minute{} ago",
                diff.num_minutes(),
                if diff.num_minutes() == 1 { "" } else { "s" }
            )
        } else if diff.num_hours() < 24 {
            format!(
                "{} hour{} ago",
                diff.num_hours(),
                if diff.num_hours() == 1 { "" } else { "s" }
            )
        } else if diff.num_days() < 30 {
            format!(
                "{} day{} ago",
                diff.num_days(),
                if diff.num_days() == 1 { "" } else { "s" }
            )
        } else {
            format!(
                "{} week{} ago",
                diff.num_weeks(),
                if diff.num_weeks() == 1 { "" } else { "s" }
            )
        }
    }

    fn render_chain_header(&self) -> Vec<String> {
        let separator = "━".repeat(47);
        let mut lines = Vec::new();

        if let Some(ref chain) = self.pr_chain {
            if chain.chain.len() > 1 {
                lines.push(separator.clone());

                let mut chain_parts: Vec<String> = Vec::new();

                if let Some(first) = chain.chain.first() {
                    chain_parts.push(first.base_ref.clone());
                }

                for (i, pr) in chain.chain.iter().enumerate() {
                    if i == chain.current_index {
                        chain_parts.push(format!("#{} (viewing)", pr.number));
                    } else {
                        chain_parts.push(format!("#{}", pr.number));
                    }
                }

                lines.push(format!("PR Chain: {}", chain_parts.join(" ← ")));
                lines.push(separator.clone());
            }
        }

        lines
    }

    fn render_suite(&self, suite: &CheckSuite) -> Vec<String> {
        let mut lines = Vec::new();
        let separator = "━".repeat(47);
        let sub_separator = "─".repeat(47);

        let status_icon = if suite.is_failure() {
            "❌"
        } else if suite.is_success() {
            "✅"
        } else if suite.is_in_progress() {
            "🔄"
        } else {
            "⚠️"
        };

        let conclusion_str = match &suite.conclusion {
            Some(c) => format!(" ({})", c.to_display()),
            None => format!(" ({})", suite.status.to_display()),
        };

        lines.push(separator.clone());
        lines.push(format!(
            "{} {}{}",
            status_icon, suite.app_name, conclusion_str
        ));
        lines.push(sub_separator.clone());

        for run in &suite.check_runs {
            let run_icon = match &run.conclusion {
                Some(CheckConclusion::Success) => "✅",
                Some(CheckConclusion::Failure) => "❌",
                Some(CheckConclusion::Skipped) => "⏭️",
                Some(CheckConclusion::Cancelled) => "🚫",
                Some(CheckConclusion::TimedOut) => "⏰",
                Some(CheckConclusion::ActionRequired) => "⚠️",
                Some(CheckConclusion::Neutral) => "⚪",
                Some(CheckConclusion::Unknown) => "❓",
                None => match run.status {
                    CheckStatus::InProgress => "🔄",
                    CheckStatus::Queued => "⏳",
                    CheckStatus::Waiting => "⏳",
                    CheckStatus::Pending => "⏳",
                    CheckStatus::Requested => "📋",
                    CheckStatus::Completed => "✓",
                    CheckStatus::Unknown => "❓",
                },
            };

            let time_str = if let Some(completed) = &run.completed_at {
                format!("completed {}", Self::format_relative_time(completed))
            } else if let Some(started) = &run.started_at {
                format!("started {}", Self::format_relative_time(started))
            } else {
                "pending".to_string()
            };

            lines.push(format!("  {} {} ({})", run_icon, run.name, time_str));
        }

        if suite.check_runs.is_empty() {
            lines.push("  No check runs".to_string());
        }

        lines.push(separator);

        lines
    }

    pub fn render(&mut self) -> Vec<String> {
        self.line_map.clear();

        if self.suites.is_empty() {
            let separator = "━".repeat(47);
            let chain_header = self.render_chain_header();
            let mut result = chain_header;
            result.extend(vec![
                separator.clone(),
                "No workflow runs found.".to_string(),
                "CI/CD runs will appear here.".to_string(),
                separator,
            ]);
            return result;
        }

        let mut all_lines = Vec::new();
        let mut current_line: usize = 0;

        let chain_header = self.render_chain_header();
        if !chain_header.is_empty() {
            current_line = chain_header.len();
            all_lines.extend(chain_header);
        }

        for (index, suite) in self.suites.iter().enumerate() {
            let suite_lines = self.render_suite(suite);
            let start_line = current_line;
            let end_line = start_line + suite_lines.len();

            self.line_map.push((start_line, end_line, index));

            all_lines.extend(suite_lines);
            current_line = end_line;
        }

        all_lines
    }
}
