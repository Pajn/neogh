//! Cursor tracking and file jumping

use crate::types::Comment;
use crate::ui::{CommentBuffer, Sidebar};
use nvim_oxi::api::{self, opts::EchoOpts, Window};

pub struct Navigator {
    comments: Vec<Comment>,
    current_index: usize,
    line_map: Vec<usize>,
}

impl Navigator {
    pub fn new(comments: Vec<Comment>) -> Self {
        let line_map = Self::build_line_map(&comments);
        Self {
            comments,
            current_index: 0,
            line_map,
        }
    }

    pub fn from_buffer(buffer: &CommentBuffer) -> Self {
        Self::new(buffer.comments().to_vec())
    }

    fn build_line_map(comments: &[Comment]) -> Vec<usize> {
        let mut map = Vec::new();
        let mut line = 0;
        for comment in comments {
            map.push(line);
            line += Self::comment_height(comment);
        }
        map
    }

    fn comment_height(comment: &Comment) -> usize {
        let body_lines = comment.body().lines().count();
        2 + body_lines.max(1)
    }

    pub fn current_index(&self) -> usize {
        self.current_index
    }

    pub fn current(&self) -> Option<&Comment> {
        self.comments.get(self.current_index)
    }

    pub fn comments(&self) -> &[Comment] {
        &self.comments
    }

    pub fn count(&self) -> usize {
        self.comments.len()
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

    pub fn reset(&mut self) {
        self.current_index = 0;
    }

    pub fn update_comments(&mut self, comments: Vec<Comment>) {
        self.line_map = Self::build_line_map(&comments);
        self.comments = comments;
        if self.current_index >= self.comments.len() && !self.comments.is_empty() {
            self.current_index = self.comments.len().saturating_sub(1);
        }
    }

    pub fn line_for_index(&self, index: usize) -> Option<usize> {
        self.line_map.get(index).copied()
    }

    pub fn index_for_line(&self, line: usize) -> Option<usize> {
        for (idx, &start_line) in self.line_map.iter().enumerate() {
            let height = Self::comment_height(&self.comments[idx]);
            if line >= start_line && line < start_line + height {
                return Some(idx);
            }
        }
        None
    }

    pub fn jump_to_comment(&self, comment: &Comment) -> Result<(), api::Error> {
        match comment {
            Comment::Review(review) => {
                if let Some(line) = review.line {
                    let path = &review.path;
                    let cmd = format!("edit {}", path);
                    api::command(&cmd)?;
                    let mut win = Window::current();
                    win.set_cursor(line as usize, 0)?;
                    api::command("normal! zz")?;
                }
                Ok(())
            }
            Comment::Issue(_) => {
                api::echo(
                    vec![(
                        "Issue comments have no file location".to_string(),
                        None::<String>,
                    )],
                    true,
                    &EchoOpts::default(),
                )?;
                Ok(())
            }
        }
    }

    pub fn jump_to_current(&self) -> Result<(), api::Error> {
        if let Some(comment) = self.current() {
            self.jump_to_comment(comment)
        } else {
            Ok(())
        }
    }

    pub fn set_cursor_to_comment(
        &self,
        sidebar: &mut Sidebar,
        index: usize,
    ) -> Result<(), api::Error> {
        if let Some(line) = self.line_for_index(index) {
            sidebar.set_cursor(line + 1, 0)?;
        }
        Ok(())
    }

    pub fn set_cursor_to_current(&self, sidebar: &mut Sidebar) -> Result<(), api::Error> {
        self.set_cursor_to_comment(sidebar, self.current_index)
    }

    pub fn first(&mut self) -> Option<&Comment> {
        if self.comments.is_empty() {
            return None;
        }
        self.current_index = 0;
        self.current()
    }

    pub fn last(&mut self) -> Option<&Comment> {
        if self.comments.is_empty() {
            return None;
        }
        self.current_index = self.comments.len().saturating_sub(1);
        self.current()
    }
}
