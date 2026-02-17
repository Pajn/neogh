//! Sidebar window management

use nvim_oxi::api::{self, opts::OptionOpts, types::*, Buffer, Window};

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

        let width: u32 = 45;
        let editor_width: u32 = api::get_option_value("columns", &OptionOpts::default())?;
        let height: u32 = api::get_option_value("lines", &OptionOpts::default())?;

        let config = WindowConfig::builder()
            .relative(WindowRelativeTo::Editor)
            .row(0f64)
            .col(editor_width as f64)
            .width(width)
            .height(height.saturating_sub(1))
            .anchor(WindowAnchor::NorthEast)
            .style(WindowStyle::Minimal)
            .build();

        let win = api::open_win(&buf, false, &config)?;

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

        self.buf = Some(buf);
        self.win = Some(win);
        Ok(())
    }

    pub fn set_lines(&mut self, lines: Vec<String>) -> Result<(), api::Error> {
        if let Some(ref mut buf) = self.buf {
            buf.set_lines(
                ..,
                false,
                lines.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
            )?;
        }
        Ok(())
    }

    pub fn close(&mut self) -> Result<(), api::Error> {
        if let Some(win) = self.win.take() {
            if win.is_valid() {
                win.close(true)?;
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
