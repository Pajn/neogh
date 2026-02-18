//! Cursor tracking and file jumping

use crate::types::{Comment, CommentExt, CommentThread};
use crate::ui::Sidebar;
use nvim_oxi::api::{self, opts::EchoOpts, Window};

/// Tracks cursor position in comments and handles navigation.
/// Uses 0-based line numbers to match buffer rendering.
pub struct Navigator {
    threads: Vec<CommentThread>,
    current_index: usize,
    line_map: Vec<usize>,
}

impl Navigator {
    pub fn new(threads: Vec<CommentThread>) -> Self {
        let line_map = Self::build_line_map(&threads);
        Self {
            threads,
            current_index: 0,
            line_map,
        }
    }

    fn build_line_map(threads: &[CommentThread]) -> Vec<usize> {
        let mut map = Vec::new();
        let mut line = 0;
        for thread in threads {
            map.push(line);
            line += thread.height();
        }
        map
    }

    pub fn current_thread(&self) -> Option<&CommentThread> {
        self.threads.get(self.current_index)
    }

    pub fn current(&self) -> Option<&Comment> {
        self.threads.get(self.current_index).map(|t| &t.root)
    }

    pub fn is_empty(&self) -> bool {
        self.threads.is_empty()
    }

    pub fn next(&mut self) -> Option<&Comment> {
        if self.threads.is_empty() {
            return None;
        }
        if self.current_index + 1 < self.threads.len() {
            self.current_index += 1;
        }
        self.current()
    }

    pub fn prev(&mut self) -> Option<&Comment> {
        if self.threads.is_empty() {
            return None;
        }
        if self.current_index > 0 {
            self.current_index -= 1;
        }
        self.current()
    }

    pub fn set_index(&mut self, index: usize) -> Option<&Comment> {
        if index < self.threads.len() {
            self.current_index = index;
        }
        self.current()
    }

    pub fn current_index(&self) -> usize {
        self.current_index
    }

    pub fn line_for_index(&self, index: usize) -> Option<usize> {
        self.line_map.get(index).copied()
    }

    pub fn index_for_line(&self, line: usize) -> Option<usize> {
        for (idx, &start_line) in self.line_map.iter().enumerate() {
            let thread = match self.threads.get(idx) {
                Some(t) => t,
                None => continue,
            };
            let height = thread.height();
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

    pub fn set_cursor_to_thread(
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
        self.set_cursor_to_thread(sidebar, self.current_index)
    }
}
