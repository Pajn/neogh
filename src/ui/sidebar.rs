//! Sidebar window management

use nvim_oxi::api::{self, opts::OptionOpts, Buffer, Window};

pub struct Sidebar {
    win: Option<Window>,
    buf: Option<Buffer>,
    previous_win: Option<Window>,
}

impl Sidebar {
    pub fn new() -> Self {
        Self {
            win: None,
            buf: None,
            previous_win: None,
        }
    }

    pub fn is_open(&self) -> bool {
        self.win.as_ref().map(|w| w.is_valid()).unwrap_or(false)
    }

    pub fn buffer(&self) -> Option<&Buffer> {
        self.buf.as_ref()
    }

    pub fn buffer_mut(&mut self) -> Option<&mut Buffer> {
        self.buf.as_mut()
    }

    pub fn window(&self) -> Option<&Window> {
        self.win.as_ref()
    }

    pub fn previous_window(&self) -> Option<&Window> {
        self.previous_win.as_ref()
    }

    pub fn open(&mut self, lines: Vec<String>) -> Result<(), api::Error> {
        if self.is_open() {
            self.set_lines(lines)?;
            return Ok(());
        }

        self.previous_win = Some(Window::current());

        // Create a new buffer
        let mut buf = api::create_buf(true, false)?;

        buf.set_lines(
            ..,
            false,
            lines.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
        )?;

        api::set_option_value(
            "buftype",
            "nofile",
            &OptionOpts::builder().buffer(buf.clone()).build(),
        )?;
        api::set_option_value(
            "bufhidden",
            "hide",
            &OptionOpts::builder().buffer(buf.clone()).build(),
        )?;
        api::set_option_value(
            "swapfile",
            false,
            &OptionOpts::builder().buffer(buf.clone()).build(),
        )?;
        api::set_option_value(
            "filetype",
            "neogh",
            &OptionOpts::builder().buffer(buf.clone()).build(),
        )?;
        api::set_option_value(
            "modifiable",
            false,
            &OptionOpts::builder().buffer(buf.clone()).build(),
        )?;

        // Open vertical split on the right side
        api::command("botright vsplit")?;

        // The new window becomes current
        let win = Window::current();

        // Set the buffer in the new window
        api::set_current_buf(&buf)?;

        // Set window options
        api::set_option_value(
            "number",
            false,
            &OptionOpts::builder().win(win.clone()).build(),
        )?;
        api::set_option_value(
            "relativenumber",
            false,
            &OptionOpts::builder().win(win.clone()).build(),
        )?;
        api::set_option_value(
            "signcolumn",
            "no",
            &OptionOpts::builder().win(win.clone()).build(),
        )?;
        api::set_option_value(
            "cursorline",
            true,
            &OptionOpts::builder().win(win.clone()).build(),
        )?;

        // Set window width
        api::command("vertical resize 50")?;

        self.buf = Some(buf);
        self.win = Some(win);
        Ok(())
    }

    pub fn set_lines(&mut self, lines: Vec<String>) -> Result<(), api::Error> {
        if let Some(ref mut buf) = self.buf {
            api::set_option_value(
                "modifiable",
                true,
                &OptionOpts::builder().buffer(buf.clone()).build(),
            )?;

            buf.set_lines(
                ..,
                false,
                lines.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
            )?;

            api::set_option_value(
                "modifiable",
                false,
                &OptionOpts::builder().buffer(buf.clone()).build(),
            )?;
        }
        Ok(())
    }

    pub fn close(&mut self) -> Result<(), api::Error> {
        if let Some(win) = self.win.take() {
            if win.is_valid() {
                // Close the window
                api::set_current_win(&win)?;
                api::command("close")?;
            }
        }
        self.buf = None;
        self.win = None;
        Ok(())
    }

    pub fn focus(&self) -> Result<(), api::Error> {
        if let Some(ref win) = self.win {
            if win.is_valid() {
                api::set_current_win(win)?;
            }
        }
        Ok(())
    }

    pub fn return_focus(&self) -> Result<(), api::Error> {
        if let Some(ref win) = self.previous_win {
            if win.is_valid() {
                api::set_current_win(win)?;
            }
        }
        Ok(())
    }

    pub fn set_cursor(&mut self, line: usize, col: usize) -> Result<(), api::Error> {
        if let Some(ref mut win) = self.win {
            if win.is_valid() {
                win.set_cursor(line, col)?;
            }
        }
        Ok(())
    }
}

impl Default for Sidebar {
    fn default() -> Self {
        Self::new()
    }
}
