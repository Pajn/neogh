mod github;
mod types;
mod ui;

use crate::github::{detect_pr, fetch_comments, get_gh_token, is_gh_installed, AuthError, PrError};
use crate::types::{Comment, CommentExt};
use crate::ui::{CommentBuffer, Navigator, Sidebar};
use nvim_oxi::api::{self, opts::*, types::*, Buffer, Window};
use nvim_oxi::{Dictionary, Function, Object};
use std::cell::RefCell;
use std::panic::{catch_unwind, AssertUnwindSafe};

thread_local! {
    static STATE: RefCell<Option<PluginState>> = RefCell::new(None);
}

struct PluginState {
    sidebar: Sidebar,
    comments: Vec<Comment>,
    navigator: Navigator,
    buffer: CommentBuffer,
}

impl PluginState {
    fn new(comments: Vec<Comment>) -> Self {
        let buffer = CommentBuffer::new(comments.clone());
        let navigator = Navigator::new(comments.clone());
        Self {
            sidebar: Sidebar::new(),
            comments,
            navigator,
            buffer,
        }
    }
}

fn notify(msg: &str, level: LogLevel) {
    let _ = api::notify(msg, level, &nvim_oxi::Dictionary::new());
}

fn notify_error(msg: &str) {
    notify(msg, LogLevel::Error);
}

fn notify_info(msg: &str) {
    notify(msg, LogLevel::Info);
}

fn check_prerequisites() -> Result<(), String> {
    if !is_gh_installed() {
        return Err("gh CLI not found. Please install: https://cli.github.com".to_string());
    }

    Ok(())
}

fn setup_keymaps(buf: &mut Buffer) -> Result<(), api::Error> {
    let opts = SetKeymapOpts::builder()
        .noremap(true)
        .silent(true)
        .nowait(true)
        .build();

    buf.set_keymap(
        Mode::Normal,
        "j",
        "<Cmd>lua require('neogh').next_comment()<CR>",
        &opts,
    )?;
    buf.set_keymap(
        Mode::Normal,
        "k",
        "<Cmd>lua require('neogh').prev_comment()<CR>",
        &opts,
    )?;
    buf.set_keymap(
        Mode::Normal,
        "q",
        "<Cmd>lua require('neogh').close()<CR>",
        &opts,
    )?;
    buf.set_keymap(
        Mode::Normal,
        "<CR>",
        "<Cmd>lua require('neogh').jump_to_current()<CR>",
        &opts,
    )?;
    buf.set_keymap(
        Mode::Normal,
        "<C-p>",
        "<Cmd>lua require('neogh').focus_sidebar()<CR>",
        &opts,
    )?;

    Ok(())
}

fn setup_autocmds(buf: &Buffer) -> Result<(), api::Error> {
    let opts = CreateAugroupOpts::builder().clear(true).build();
    api::create_augroup("NeoghSidebar", &opts)?;

    let buf_clone = buf.clone();

    let callback = move |_args: AutocmdCallbackArgs| -> bool {
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            STATE
                .with(|state_cell| {
                    let mut state_opt = match state_cell.try_borrow_mut() {
                        Ok(guard) => guard,
                        Err(_) => {
                            return Ok::<(), api::Error>(());
                        }
                    };
                    if let Some(ref mut state) = *state_opt {
                        if state.sidebar.is_open() {
                            let win = state.sidebar.window().cloned();
                            if let Some(sidebar_win) = win {
                                if sidebar_win == Window::current() {
                                    let cursor = sidebar_win.get_cursor()?;
                                    let line = cursor.0;
                                    if let Some(idx) = state.buffer.line_to_comment_index(line) {
                                        state.navigator.set_index(idx);
                                    }
                                }
                            }
                        }
                    }
                    Ok::<(), api::Error>(())
                })
                .is_ok()
        }))
        .unwrap_or(false)
    };

    let opts = CreateAutocmdOpts::builder()
        .buffer(buf_clone)
        .callback(callback)
        .build();

    api::create_autocmd(vec!["CursorMoved"], &opts)?;

    Ok(())
}

fn open() -> Result<(), String> {
    if let Err(e) = check_prerequisites() {
        notify_error(&e);
        return Err(e);
    }


    let token = get_gh_token().map_err(|e| match e {
        AuthError::GhNotFound => "gh CLI not found".to_string(),
        AuthError::NotAuthenticated => "Not authenticated with gh".to_string(),
        AuthError::IoError(msg) => format!("IO error: {}", msg),
    })?;

    notify_info("Fetching PR comments...");

    let pr = detect_pr().map_err(|e| {
        let msg = match e {
            PrError::NotAGitRepo => "Not a git repository".to_string(),
            PrError::GhError(err) => format!("gh error: {}", err),
            PrError::NoAssociatedPr => "No PR associated with current branch".to_string(),
            PrError::IoError(err) => format!("IO error: {}", err),
            PrError::ParseError(err) => format!("Parse error: {}", err),
        };
        notify_error(&msg);
        msg
    })?;

    let comments = fetch_comments(&token, &pr.owner, &pr.repo, pr.number).map_err(|e| {
        let msg = format!("Failed to fetch comments: {}", e);
        notify_error(&msg);
        msg
    })?;

    // compute summary values before moving comments into the PluginState
    let comment_count = comments.len();
    let title = pr.title.clone();
    let number = pr.number;

    // build the state locally and perform all Neovim API calls while NOT holding the
    // global STATE borrow (to avoid re-entrancy / RefCell borrow panics)
    let mut state = PluginState::new(comments);

    let lines = state.buffer.render();

    state.sidebar.open(lines).map_err(|e| {
        let msg = format!("Failed to open sidebar: {}", e);
        notify_error(&msg);
        msg
    })?;

    if let Some(buf) = state.sidebar.buffer_mut() {
        setup_keymaps(buf).map_err(|e| format!("Failed to setup keymaps: {}", e))?;
    }

    if let Some(buf) = state.sidebar.buffer() {
        setup_autocmds(buf).map_err(|e| format!("Failed to setup autocmds: {}", e))?;
    }

    if !state.navigator.is_empty() {
        state
            .navigator
            .set_cursor_to_current(&mut state.sidebar)
            .map_err(|e| format!("Failed to set cursor: {}", e))?;
    }

    state.sidebar.focus().map_err(|e| {
        let msg = format!("Failed to focus sidebar: {}", e);
        notify_error(&msg);
        msg
    })?;

    notify_info(&format!(
        "Loaded {} comments for PR #{}: {}",
        comment_count, number, title
    ));

    // now store the fully-initialized state into the thread-local storage
    STATE.with(|state_cell| {
        let mut state_opt = match state_cell.try_borrow_mut() {
            Ok(guard) => guard,
            Err(_) => return Err("State temporarily unavailable".to_string()),
        };
        *state_opt = Some(state);
        Ok(())
    })?;

    Ok(())
}

fn close() -> Result<(), String> {
    // take the state out of the thread-local storage, then perform Neovim API
    // operations on the owned state outside of the RefCell borrow to avoid
    // potential re-entrancy / borrow conflicts.
    let maybe_state = STATE.with(|state_cell| {
        let mut state_opt = match state_cell.try_borrow_mut() {
            Ok(guard) => guard,
            Err(_) => return Err("State temporarily unavailable".to_string()),
        };
        Ok(state_opt.take())
    })?;

    if let Some(mut state) = maybe_state {
        state.sidebar.close().map_err(|e| {
            let msg = format!("Failed to close sidebar: {}", e);
            notify_error(&msg);
            msg
        })?;
    }

    Ok(())
}

fn toggle() -> Result<(), String> {
    STATE.with(|state_cell| {
        let state_opt = state_cell.borrow();
        if let Some(ref state) = *state_opt {
            if state.sidebar.is_open() {
                drop(state_opt);
                return close();
            }
        }
        drop(state_opt);
        open()
    })
}

fn next_comment() -> Result<(), String> {
    STATE.with(|state_cell| {
        let mut state_opt = match state_cell.try_borrow_mut() {
            Ok(guard) => guard,
            Err(_) => return Err("State temporarily unavailable".to_string()),
        };
        if let Some(ref mut state) = *state_opt {
            if state.navigator.is_empty() {
                notify_info("No comments to navigate");
                return Ok(());
            }

            state.navigator.next();
            state
                .navigator
                .set_cursor_to_current(&mut state.sidebar)
                .map_err(|e| format!("Failed to move cursor: {}", e))?;
        } else {
            notify_error("Sidebar not open. Run :PRComments first");
            return Err("Sidebar not open".to_string());
        }
        Ok(())
    })
}

fn prev_comment() -> Result<(), String> {
    STATE.with(|state_cell| {
        let mut state_opt = match state_cell.try_borrow_mut() {
            Ok(guard) => guard,
            Err(_) => return Err("State temporarily unavailable".to_string()),
        };
        if let Some(ref mut state) = *state_opt {
            if state.navigator.is_empty() {
                notify_info("No comments to navigate");
                return Ok(());
            }

            state.navigator.prev();
            state
                .navigator
                .set_cursor_to_current(&mut state.sidebar)
                .map_err(|e| format!("Failed to move cursor: {}", e))?;
        } else {
            notify_error("Sidebar not open. Run :PRComments first");
            return Err("Sidebar not open".to_string());
        }
        Ok(())
    })
}

fn focus_sidebar() -> Result<(), String> {
    STATE.with(|state_cell| {
        let state_opt = match state_cell.try_borrow() {
            Ok(guard) => guard,
            Err(_) => return Err("State temporarily unavailable".to_string()),
        };
        if let Some(ref state) = *state_opt {
            if state.sidebar.is_open() {
                state.sidebar.focus().map_err(|e| {
                    let msg = format!("Failed to focus sidebar: {}", e);
                    notify_error(&msg);
                    msg
                })?;
            } else {
                notify_error("Sidebar not open. Run :PRComments first");
                return Err("Sidebar not open".to_string());
            }
        } else {
            notify_error("Sidebar not open. Run :PRComments first");
            return Err("Sidebar not open".to_string());
        }
        Ok(())
    })
}

fn jump_to_current() -> Result<(), String> {
    STATE.with(|state_cell| {
        let mut state_opt = match state_cell.try_borrow_mut() {
            Ok(guard) => guard,
            Err(_) => return Err("State temporarily unavailable".to_string()),
        };
        if let Some(ref mut state) = *state_opt {
            if state.navigator.is_empty() {
                notify_info("No comments to jump to");
                return Ok(());
            }

            if let Some(comment) = state.navigator.current() {
                match comment.location() {
                    Some((path, line)) => {
                        state.sidebar.return_focus().map_err(|e| {
                            let msg = format!("Failed to return focus: {}", e);
                            notify_error(&msg);
                            msg
                        })?;

                        let cmd = format!("edit {}", path);
                        api::command(&cmd).map_err(|e| {
                            let msg = format!("Failed to open file {}: {}", path, e);
                            notify_error(&msg);
                            msg
                        })?;

                        let mut win = Window::current();
                        win.set_cursor(line as usize, 0).map_err(|e| {
                            let msg = format!("Failed to set cursor: {}", e);
                            notify_error(&msg);
                            msg
                        })?;

                        api::command("normal! zz").map_err(|e| {
                            let msg = format!("Failed to center view: {}", e);
                            notify_error(&msg);
                            msg
                        })?;
                    }
                    None => {
                        notify_info("This is an issue comment with no file location");
                    }
                }
            }
        } else {
            notify_error("Sidebar not open. Run :PRComments first");
            return Err("Sidebar not open".to_string());
        }
        Ok(())
    })
}

#[nvim_oxi::plugin]
fn neogh() -> nvim_oxi::Result<Dictionary> {
    let prcomments_opts = CreateCommandOpts::builder()
        .desc("Open PR comments sidebar")
        .build();

    let prcomments_cmd = |_args: CommandArgs| {
        let _ = catch_unwind(AssertUnwindSafe(|| open()));
    };

    api::create_user_command("PRComments", prcomments_cmd, &prcomments_opts)?;

    let prcommentsclose_opts = CreateCommandOpts::builder()
        .desc("Close PR comments sidebar")
        .build();

    let prcommentsclose_cmd = |_args: CommandArgs| {
        let _ = catch_unwind(AssertUnwindSafe(|| close()));
    };

    api::create_user_command(
        "PRCommentsClose",
        prcommentsclose_cmd,
        &prcommentsclose_opts,
    )?;

    let open_fn: Function<(), ()> = Function::from_fn(|_| {
        let _ = catch_unwind(AssertUnwindSafe(|| {
            if let Err(e) = open() {
                notify_error(&e);
            }
        }));
    });
    let close_fn: Function<(), ()> = Function::from_fn(|_| {
        let _ = catch_unwind(AssertUnwindSafe(|| {
            if let Err(e) = close() {
                notify_error(&e);
            }
        }));
    });
    let toggle_fn: Function<(), ()> = Function::from_fn(|_| {
        let _ = catch_unwind(AssertUnwindSafe(|| {
            if let Err(e) = toggle() {
                notify_error(&e);
            }
        }));
    });
    let next_fn: Function<(), ()> = Function::from_fn(|_| {
        let _ = catch_unwind(AssertUnwindSafe(|| {
            if let Err(e) = next_comment() {
                notify_error(&e);
            }
        }));
    });
    let prev_fn: Function<(), ()> = Function::from_fn(|_| {
        let _ = catch_unwind(AssertUnwindSafe(|| {
            if let Err(e) = prev_comment() {
                notify_error(&e);
            }
        }));
    });
    let jump_fn: Function<(), ()> = Function::from_fn(|_| {
        let _ = catch_unwind(AssertUnwindSafe(|| {
            if let Err(e) = jump_to_current() {
                notify_error(&e);
            }
        }));
    });
    let focus_fn: Function<(), ()> = Function::from_fn(|_| {
        let _ = catch_unwind(AssertUnwindSafe(|| {
            if let Err(e) = focus_sidebar() {
                notify_error(&e);
            }
        }));
    });

    Ok(Dictionary::from_iter([
        ("open", Object::from(open_fn)),
        ("close", Object::from(close_fn)),
        ("toggle", Object::from(toggle_fn)),
        ("next_comment", Object::from(next_fn)),
        ("prev_comment", Object::from(prev_fn)),
        ("jump_to_current", Object::from(jump_fn)),
        ("focus_sidebar", Object::from(focus_fn)),
    ]))
}
