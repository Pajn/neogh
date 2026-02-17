use crate::types::Comment;
use chrono::{DateTime, Utc};

pub struct CommentBuffer {
    comments: Vec<Comment>,
    line_map: Vec<(usize, usize, usize)>,
}

impl CommentBuffer {
    pub fn new(comments: Vec<Comment>) -> Self {
        Self {
            comments,
            line_map: Vec::new(),
        }
    }

    pub fn comments(&self) -> &[Comment] {
        &self.comments
    }

    pub fn is_empty(&self) -> bool {
        self.comments.is_empty()
    }

    pub fn line_to_comment_index(&self, line: usize) -> Option<usize> {
        for &(start, end, idx) in &self.line_map {
            if line >= start && line < end {
                return Some(idx);
            }
        }
        None
    }

    pub fn get_line_for_comment(&self, index: usize) -> Option<usize> {
        self.line_map
            .iter()
            .find(|&&(_, _, idx)| idx == index)
            .map(|&(start, _, _)| start)
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
        } else if diff.num_weeks() < 52 {
            format!(
                "{} week{} ago",
                diff.num_weeks(),
                if diff.num_weeks() == 1 { "" } else { "s" }
            )
        } else {
            format!(
                "{} year{} ago",
                diff.num_weeks() / 52,
                if diff.num_weeks() / 52 == 1 { "" } else { "s" }
            )
        }
    }

    fn render_comment_lines(comment: &Comment) -> Vec<String> {
        let mut lines = Vec::new();

        let separator = "━".repeat(47);
        let sub_separator = "─".repeat(47);

        lines.push(separator.clone());

        let header = match comment {
            Comment::Review(rc) => {
                if let Some(line_num) = rc.line {
                    format!("📝 Review Comment · {}:{}", rc.path, line_num)
                } else {
                    format!("📝 Review Comment · {}", rc.path)
                }
            }
            Comment::Issue(_) => "💬 Issue Comment".to_string(),
        };
        lines.push(header);

        let time_str = Self::format_relative_time(comment.created_at());
        lines.push(format!("@{} • {}", comment.author(), time_str));

        lines.push(sub_separator);

        for body_line in comment.body().lines() {
            lines.push(body_line.to_string());
        }

        lines.push(separator);

        lines
    }

    pub fn render(&mut self) -> Vec<String> {
        self.line_map.clear();

        if self.comments.is_empty() {
            let separator = "━".repeat(47);
            return vec![
                separator.clone(),
                "No comments yet.".to_string(),
                "Comments will appear here when available.".to_string(),
                separator,
            ];
        }

        let mut all_lines = Vec::new();
        let mut current_line = 0;

        for (index, comment) in self.comments.iter().enumerate() {
            let comment_lines = Self::render_comment_lines(comment);
            let start_line = current_line;
            let end_line = current_line + comment_lines.len();

            self.line_map.push((start_line, end_line, index));

            all_lines.extend(comment_lines);
            current_line = end_line;
        }

        all_lines
    }
}
