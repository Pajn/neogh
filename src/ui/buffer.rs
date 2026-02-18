use crate::github::pr::PrChain;
use crate::types::{Comment, CommentExt, CommentThread};
use chrono::{DateTime, Utc};
use regex::Regex;
use std::collections::HashSet;

fn clean_body(body: &str) -> String {
    let re = Regex::new(r"<!--.*?-->").unwrap();
    let without_comments = re.replace_all(body, "");

    let re_tags = Regex::new(r"<[^>]+>").unwrap();
    let without_tags = re_tags.replace_all(&without_comments, "");

    without_tags
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .to_string()
}

/// Renders comments to lines and tracks line->comment mapping.
/// Uses 0-based line numbers to match get_cursor() behavior.
pub struct CommentBuffer {
    threads: Vec<CommentThread>,
    collapsed: HashSet<usize>,
    line_map: Vec<(usize, usize, usize, bool)>,
    pr_chain: Option<PrChain>,
}

impl CommentBuffer {
    pub fn new(threads: Vec<CommentThread>) -> Self {
        Self {
            threads,
            collapsed: HashSet::new(),
            line_map: Vec::new(),
            pr_chain: None,
        }
    }

    pub fn set_chain(&mut self, chain: Option<PrChain>) {
        self.pr_chain = chain;
    }

    pub fn threads(&self) -> &[CommentThread] {
        &self.threads
    }

    pub fn is_collapsed(&self, index: usize) -> bool {
        self.collapsed.contains(&index)
    }

    pub fn toggle_collapse(&mut self, index: usize) {
        if self.collapsed.contains(&index) {
            self.collapsed.remove(&index);
        } else {
            self.collapsed.insert(index);
        }
    }

    pub fn initialize_collapsed(&mut self) {
        for (index, thread) in self.threads.iter().enumerate() {
            if thread.is_resolved {
                self.collapsed.insert(index);
            }
        }
    }

    pub fn set_collapsed(&mut self, index: usize, collapsed: bool) {
        if collapsed {
            self.collapsed.insert(index);
        } else {
            self.collapsed.remove(&index);
        }
    }

    pub fn set_thread_resolved(&mut self, index: usize, is_resolved: bool) {
        if let Some(thread) = self.threads.get_mut(index) {
            thread.is_resolved = is_resolved;
        }
    }

    pub fn line_to_thread_index(&self, line: usize) -> Option<usize> {
        for &(start, end, idx, _) in &self.line_map {
            if line >= start && line < end {
                return Some(idx);
            }
        }
        None
    }

    pub fn line_for_thread(&self, index: usize) -> Option<usize> {
        for &(start, _end, idx, is_reply) in &self.line_map {
            if idx == index && !is_reply {
                return Some(start);
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

    fn render_comment_lines(comment: &Comment, is_resolved: bool) -> Vec<String> {
        Self::render_comment_lines_with_prefix(comment, "", is_resolved)
    }

    fn render_comment_lines_with_prefix(
        comment: &Comment,
        prefix: &str,
        is_resolved: bool,
    ) -> Vec<String> {
        let mut lines = Vec::new();

        let separator = "━".repeat(47);
        let sub_separator = "─".repeat(47);

        lines.push(format!("{}{}", prefix, separator));

        let header = match comment {
            Comment::Review(rc) => {
                let status = if is_resolved {
                    " ✓ [resolved]"
                } else if rc.is_line_deleted() {
                    " [outdated]"
                } else {
                    ""
                };
                if let Some(line_num) = rc.navigation_line() {
                    format!("📝 Review Comment · {}:{}{}", rc.path, line_num, status)
                } else {
                    format!("📝 Review Comment · {}{}", rc.path, status)
                }
            }
            Comment::Issue(_) => "💬 Issue Comment".to_string(),
        };
        lines.push(format!("{}{}", prefix, header));

        let time_str = Self::format_relative_time(comment.created_at());
        lines.push(format!("{}@{} • {}", prefix, comment.author(), time_str));

        lines.push(format!("{}{}", prefix, sub_separator));

        let cleaned_body = clean_body(comment.body());
        let body_lines: Vec<&str> = cleaned_body.lines().collect();
        if body_lines.is_empty() {
            lines.push(format!("{} ", prefix));
        } else {
            for body_line in body_lines {
                lines.push(format!("{}{}", prefix, body_line));
            }
        }

        lines.push(format!("{}{}", prefix, separator));

        lines
    }

    fn render_reply_lines(comment: &Comment) -> Vec<String> {
        Self::render_comment_lines_with_prefix(comment, "└─", false)
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

    pub fn render(&mut self) -> Vec<String> {
        self.line_map.clear();

        if self.threads.is_empty() {
            let separator = "━".repeat(47);
            let chain_header = self.render_chain_header();
            let mut result = chain_header;
            result.extend(vec![
                separator.clone(),
                "No comments yet.".to_string(),
                "Comments will appear here when available.".to_string(),
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

        for (index, thread) in self.threads.iter().enumerate() {
            let is_collapsed = self.collapsed.contains(&index);

            let root_lines = Self::render_comment_lines(&thread.root, thread.is_resolved);
            let start_line = current_line;
            let end_line = start_line + root_lines.len();

            self.line_map.push((start_line, end_line, index, false));

            all_lines.extend(root_lines);
            current_line = end_line;

            if !is_collapsed {
                for _reply in &thread.replies {
                    let reply_lines = Self::render_reply_lines(_reply);
                    let reply_start = current_line;
                    let reply_end = reply_start + reply_lines.len();

                    self.line_map.push((reply_start, reply_end, index, true));

                    all_lines.extend(reply_lines);
                    current_line = reply_end;
                }
            }
        }

        all_lines
    }
}
