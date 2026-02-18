//! Cursor tracking and file jumping

use crate::types::{Comment, CommentExt};
use crate::ui::Sidebar;
use nvim_oxi::api::{self, opts::EchoOpts, Window};

/// Tracks cursor position in comments and handles navigation.
/// Uses 0-based line numbers to match buffer rendering.
pub struct Navigator {
    comments: Vec<Comment>,
    current_index: usize,
    /// Start line (0-based) for each comment index
    line_map: Vec<usize>,
}

impl Navigator {
    /// Create navigator from the same comments array used by CommentBuffer
    pub fn new(comments: Vec<Comment>) -> Self {
        let line_map = Self::build_line_map(&comments);
        Self {
            comments,
            current_index: 0,
            line_map,
        }
    }

    fn build_line_map(comments: &[Comment]) -> Vec<usize> {
        let mut map = Vec::new();
        let mut line = 0;
        for comment in comments {
            map.push(line);
            line += comment.height();
        }
        map
    }

    pub fn current(&self) -> Option<&Comment> {
        self.comments.get(self.current_index)
    }

    pub fn is_empty(&self) -> bool {
        self.comments.is_empty()
    }

    pub fn next(&mut self) -> Option<&Comment> {
        if self.comments.is_empty() {
            return None;
        }
        if self.current_index + 1 < self.comments.len() {
            self.current_index += 1;
        }
        self.current()
    }

    pub fn prev(&mut self) -> Option<&Comment> {
        if self.comments.is_empty() {
            return None;
        }
        if self.current_index > 0 {
            self.current_index -= 1;
        }
        self.current()
    }

    pub fn set_index(&mut self, index: usize) -> Option<&Comment> {
        if index < self.comments.len() {
            self.current_index = index;
        }
        self.current()
    }

    /// Get the start line (0-based) for a comment index
    pub fn line_for_index(&self, index: usize) -> Option<usize> {
        self.line_map.get(index).copied()
    }

    /// Convert cursor line (0-based) to comment index
    pub fn index_for_line(&self, line: usize) -> Option<usize> {
        for (idx, &start_line) in self.line_map.iter().enumerate() {
            let comment = match self.comments.get(idx) {
                Some(c) => c,
                None => continue,
            };
            let height = comment.height();
            if line >= start_line && line < start_line + height {
                return Some(idx);
            }
        }
        None
    }

    pub fn jump_to_comment(&self, comment: &Comment) -> Result<(), api::Error> {
        match comment.location() {
            Some((path, line)) => {
                let cmd = format!("edit {}", path);
                api::command(&cmd)?;
                let mut win = Window::current();
                win.set_cursor(line as usize, 0)?;
                api::command("normal! zz")?;
                Ok(())
            }
            None => {
                let msg = match comment {
                    Comment::Review(_) => "Review comment has no line number",
                    Comment::Issue(_) => "Issue comments have no file location",
                };
                api::echo(
                    vec![(msg.to_string(), None::<String>)],
                    true,
                    &EchoOpts::default(),
                )?;
                Ok(())
            }
        }
    }

    pub fn set_cursor_to_comment(
        &self,
        sidebar: &mut Sidebar,
        index: usize,
    ) -> Result<(), api::Error> {
        if let Some(line) = self.line_for_index(index) {
            // set_cursor expects 1-based line number
            sidebar.set_cursor(line + 1, 0)?;
        }
        Ok(())
    }

    pub fn set_cursor_to_current(&self, sidebar: &mut Sidebar) -> Result<(), api::Error> {
        self.set_cursor_to_comment(sidebar, self.current_index)
    }
}
