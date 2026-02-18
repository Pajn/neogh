mod actions;
mod github;
mod types;
mod ui;

use crate::actions::{ActionsFetchResult, ActionsNavigator};
use crate::github::{
    detect_chain, detect_pr, fetch_check_runs, fetch_comments, get_gh_token, is_gh_installed,
    resolve_thread, unresolve_thread, AuthError, CheckSuite, PrError,
};
use crate::github::pr::{PrChain, PullRequest};
use crate::types::{Comment, CommentExt, CommentThread, SidebarMode};
use crate::ui::{ActionsBuffer, CommentBuffer, Navigator, Sidebar};
use nvim_oxi::api::{self, opts::*, types::*, Buffer, Window};
use nvim_oxi::libuv::AsyncHandle;
use nvim_oxi::{Dictionary, Function, Object};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::mpsc::{channel, Receiver};
use std::sync::{Arc, Mutex};

enum FetchResult {
    Success {
        threads: Vec<CommentThread>,
        title: String,
        number: u64,
        current_pr: PullRequest,
        pr_chain: Option<PrChain>,
    },
    Error(String),
}

enum PrefetchResult {
    Success { number: u64, threads: Vec<CommentThread> },
    Error { number: u64, msg: String },
}

thread_local! {
    static STATE: RefCell<Option<PluginState>> = RefCell::new(None);
}

struct PluginState {
    sidebar: Sidebar,
    threads: Vec<CommentThread>,
    navigator: Navigator,
    buffer: CommentBuffer,
    collapsed: HashSet<usize>,
    is_loading: bool,
    async_handle: Option<AsyncHandle>,
    current_pr: Option<PullRequest>,
    pr_chain: Option<PrChain>,
    pr_comment_cache: HashMap<u64, Vec<CommentThread>>,
    prefetch_handles: Vec<AsyncHandle>,
    mode: SidebarMode,
    actions_buffer: ActionsBuffer,
    check_suites: Vec<CheckSuite>,
    actions_navigator: ActionsNavigator,
    pr_workflow_cache: HashMap<u64, Vec<CheckSuite>>,
}

impl PluginState {
    fn new(threads: Vec<CommentThread>) -> Self {
        let mut buffer = CommentBuffer::new(threads.clone());
        buffer.initialize_collapsed();
        let navigator = Navigator::new(threads.clone());
        Self {
            sidebar: Sidebar::new(),
            threads,
            navigator,
            buffer,
            collapsed: HashSet::new(),
            is_loading: false,
            async_handle: None,
            current_pr: None,
            pr_chain: None,
            pr_comment_cache: HashMap::new(),
            prefetch_handles: Vec::new(),
            mode: SidebarMode::Comments,
            actions_buffer: ActionsBuffer::new(Vec::new()),
            check_suites: Vec::new(),
            actions_navigator: ActionsNavigator::new(0),
            pr_workflow_cache: HashMap::new(),
        }
    }

    fn loading() -> Self {
        Self {
            sidebar: Sidebar::new(),
            threads: Vec::new(),
            navigator: Navigator::new(Vec::new()),
            buffer: CommentBuffer::new(Vec::new()),
            collapsed: HashSet::new(),
            is_loading: true,
            async_handle: None,
            current_pr: None,
            pr_chain: None,
            pr_comment_cache: HashMap::new(),
            prefetch_handles: Vec::new(),
            mode: SidebarMode::Comments,
            actions_buffer: ActionsBuffer::new(Vec::new()),
            check_suites: Vec::new(),
            actions_navigator: ActionsNavigator::new(0),
            pr_workflow_cache: HashMap::new(),
        }
    }

    fn set_threads(&mut self, threads: Vec<CommentThread>, chain: Option<&PrChain>) {
        self.threads = threads.clone();
        let mut buffer = CommentBuffer::new(threads.clone());
        buffer.set_chain(chain.cloned());
        buffer.initialize_collapsed();
        self.buffer = buffer;
        self.navigator = Navigator::new(threads);
        self.is_loading = false;
    }

    fn set_check_suites(&mut self, suites: Vec<CheckSuite>, chain: Option<&PrChain>) {
        self.check_suites = suites.clone();
        let mut buffer = ActionsBuffer::new(suites.clone());
        buffer.set_chain(chain.cloned());
        self.actions_buffer = buffer;
        self.actions_navigator = ActionsNavigator::new(suites.len());
        self.is_loading = false;
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

    buf.set_keymap(Mode::Normal, "j", "<Cmd>lua require('neogh').next_comment()<CR>", &opts)?;
    buf.set_keymap(Mode::Normal, "k", "<Cmd>lua require('neogh').prev_comment()<CR>", &opts)?;
    buf.set_keymap(Mode::Normal, "q", "<Cmd>lua require('neogh').close()<CR>", &opts)?;
    buf.set_keymap(Mode::Normal, "<CR>", "<Cmd>lua require('neogh').jump_to_current()<CR>", &opts)?;
    buf.set_keymap(Mode::Normal, "<C-p>", "<Cmd>lua require('neogh').focus_sidebar()<CR>", &opts)?;
    buf.set_keymap(Mode::Normal, "za", "<Cmd>lua require('neogh').toggle_collapse()<CR>", &opts)?;
    buf.set_keymap(Mode::Normal, "r", "<Cmd>lua require('neogh').toggle_resolve()<CR>", &opts)?;
    buf.set_keymap(Mode::Normal, "[p", "<Cmd>lua require('neogh').navigate_to_parent_pr()<CR>", &opts)?;
    buf.set_keymap(Mode::Normal, "]p", "<Cmd>lua require('neogh').navigate_to_child_pr()<CR>", &opts)?;
    buf.set_keymap(Mode::Normal, "R", "<Cmd>lua require('neogh').refresh()<CR>", &opts)?;
    buf.set_keymap(Mode::Normal, "<Tab>", "<Cmd>lua require('neogh').switch_mode()<CR>", &opts)?;

    Ok(())
}

fn setup_autocmds(buf: &Buffer) -> Result<(), api::Error> {
    let opts = CreateAugroupOpts::builder().clear(true).build();
    api::create_augroup("NeoghSidebar", &opts)?;

    let buf_clone = buf.clone();

    let callback = move |_args: AutocmdCallbackArgs| -> bool {
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            STATE.with(|state_cell| {
                let mut state_opt = match state_cell.try_borrow_mut() {
                    Ok(guard) => guard,
                    Err(_) => return Ok::<(), api::Error>(()),
                };
                if let Some(ref mut state) = *state_opt {
                    if state.sidebar.is_open() {
                        let win = state.sidebar.window().cloned();
                        if let Some(sidebar_win) = win {
                            if sidebar_win == Window::current() {
                                let cursor = sidebar_win.get_cursor()?;
                                let line = cursor.0;
                                match state.mode {
                                    SidebarMode::Comments => {
                                        if let Some(idx) = state.buffer.line_to_thread_index(line) {
                                            state.navigator.set_index(idx);
                                        }
                                    }
                                    SidebarMode::Actions => {
                                        if let Some(idx) = state.actions_buffer.line_to_suite_index(line) {
                                            state.actions_navigator.set_index(idx);
                                        }
                                    }
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

fn render_loading_lines(mode: SidebarMode) -> Vec<String> {
    let separator = "━".repeat(47);
    let msg = match mode {
        SidebarMode::Comments => "Loading PR comments...",
        SidebarMode::Actions => "Loading workflow status...",
    };
    vec![
        separator.clone(),
        msg.to_string(),
        "Fetching from GitHub...".to_string(),
        separator,
    ]
}

fn open_with_mode(mode: SidebarMode) -> Result<(), String> {
    if let Err(e) = check_prerequisites() {
        notify_error(&e);
        return Err(e);
    }

    let token = get_gh_token().map_err(|e| match e {
        AuthError::GhNotFound => "gh CLI not found".to_string(),
        AuthError::NotAuthenticated => "Not authenticated with gh".to_string(),
        AuthError::IoError(msg) => format!("IO error: {}", msg),
    })?;

    let mut state = PluginState::loading();
    state.mode = mode;

    let loading_lines = render_loading_lines(mode);
    state.sidebar.open(loading_lines).map_err(|e| {
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

    state.sidebar.focus().map_err(|e| {
        let msg = format!("Failed to focus sidebar: {}", e);
        notify_error(&msg);
        msg
    })?;

    match mode {
        SidebarMode::Comments => spawn_comments_fetch(state, token),
        SidebarMode::Actions => spawn_actions_fetch(state, token),
    }
}

fn open() -> Result<(), String> {
    open_with_mode(SidebarMode::Comments)
}

fn open_actions() -> Result<(), String> {
    open_with_mode(SidebarMode::Actions)
}

fn spawn_comments_fetch(mut state: PluginState, token: String) -> Result<(), String> {
    let (sender, receiver) = channel::<FetchResult>();
    let receiver = Arc::new(Mutex::new(receiver));

    let receiver_clone = receiver.clone();
    let handle = AsyncHandle::new(move || {
        let _ = catch_unwind(AssertUnwindSafe(|| {
            let result = receiver_clone.lock().unwrap().try_recv();
            if let Ok(fetch_result) = result {
                STATE.with(|state_cell| {
                    if let Ok(mut state_opt) = state_cell.try_borrow_mut() {
                        if let Some(ref mut state) = *state_opt {
                            match fetch_result {
                                FetchResult::Success { threads, title, number, current_pr, pr_chain } => {
                                    state.pr_comment_cache.insert(number, threads.clone());
                                    state.set_threads(threads, pr_chain.as_ref());
                                    state.current_pr = Some(current_pr.clone());
                                    state.pr_chain = pr_chain.clone();

                                    let lines = state.buffer.render();
                                    if state.sidebar.set_lines(lines).is_ok() {
                                        if !state.navigator.is_empty() {
                                            if let Some(line) = state.buffer.line_for_thread(state.navigator.current_index()) {
                                                let _ = state.sidebar.set_cursor(line + 1, 0);
                                            }
                                        }
                                        notify_info(&format!("Loaded {} thread(s) for PR #{}: {}", state.threads.len(), number, title));
                                    }

                                    // Prefetch other PRs in chain
                                    if let Some(ref chain) = pr_chain {
                                        let other_prs: Vec<_> = chain.chain.iter()
                                            .filter(|pr| pr.number != number)
                                            .cloned()
                                            .collect();
                                        
                                        let token = get_gh_token().ok();
                                        if let Some(token) = token {
                                            for pr_info in other_prs {
                                                let pr_number = pr_info.number;
                                                let owner = current_pr.owner.clone();
                                                let repo = current_pr.repo.clone();
                                                let token_clone = token.clone();

                                                let (sender, receiver) = channel::<PrefetchResult>();
                                                let receiver = Arc::new(Mutex::new(receiver));
                                                let receiver_clone = receiver.clone();

                                                let prefetch_handle = AsyncHandle::new(move || {
                                                    let _ = catch_unwind(AssertUnwindSafe(|| {
                                                        if let Ok(result) = receiver_clone.lock().unwrap().try_recv() {
                                                            STATE.with(|state_cell| {
                                                                if let Ok(mut state_opt) = state_cell.try_borrow_mut() {
                                                                    if let Some(ref mut state) = *state_opt {
                                                                        match result {
                                                                            PrefetchResult::Success { number, threads } => {
                                                                                state.pr_comment_cache.insert(number, threads);
                                                                            }
                                                                            PrefetchResult::Error { number, msg } => {
                                                                                eprintln!("Prefetch failed for PR #{}: {}", number, msg);
                                                                            }
                                                                        }
                                                                    }
                                                                }
                                                            });
                                                        }
                                                    }));
                                                });

                                                if let Ok(handle) = prefetch_handle {
                                                    let handle_clone = handle.clone();
                                                    STATE.with(|state_cell| {
                                                        if let Ok(mut state_opt) = state_cell.try_borrow_mut() {
                                                            if let Some(ref mut state) = *state_opt {
                                                                state.prefetch_handles.push(handle);
                                                            }
                                                        }
                                                    });

                                                    std::thread::spawn(move || {
                                                        let result = match fetch_comments(&token_clone, &owner, &repo, pr_number) {
                                                            Ok(threads) => PrefetchResult::Success { number: pr_number, threads },
                                                            Err(e) => PrefetchResult::Error { number: pr_number, msg: e.to_string() },
                                                        };
                                                        let _ = sender.send(result);
                                                        let _ = handle_clone.send();
                                                    });
                                                }
                                            }
                                        }
                                    }
                                }
                                FetchResult::Error(msg) => {
                                    notify_error(&msg);
                                }
                            }
                        }
                    }
                });
            }
        }));
    }).map_err(|e| format!("Failed to create async handle: {}", e))?;

    let handle_clone = handle.clone();
    state.async_handle = Some(handle);

    STATE.with(|state_cell| {
        let mut state_opt = match state_cell.try_borrow_mut() {
            Ok(guard) => guard,
            Err(_) => return Err("State temporarily unavailable".to_string()),
        };
        *state_opt = Some(state);
        Ok(())
    })?;

    std::thread::spawn(move || {
        let pr_result = detect_pr();

        let fetch_result = match pr_result {
            Ok(pr) => {
                let chain_result = detect_chain(&pr);
                let pr_chain = match chain_result {
                    Ok(chain) => Some(chain),
                    Err(e) => {
                        eprintln!("Failed to detect PR chain: {}", e);
                        None
                    }
                };

                match fetch_comments(&token, &pr.owner, &pr.repo, pr.number) {
                    Ok(threads) => FetchResult::Success {
                        threads,
                        title: pr.title.clone(),
                        number: pr.number,
                        current_pr: pr,
                        pr_chain,
                    },
                    Err(e) => FetchResult::Error(format!("Failed to fetch comments: {}", e)),
                }
            }
            Err(e) => {
                let msg = match e {
                    PrError::NotAGitRepo => "Not a git repository".to_string(),
                    PrError::GhError(err) => format!("gh error: {}", err),
                    PrError::NoAssociatedPr => "No PR associated with current branch".to_string(),
                    PrError::IoError(err) => format!("IO error: {}", err),
                    PrError::ParseError(err) => format!("Parse error: {}", err),
                };
                FetchResult::Error(msg)
            }
        };

        let _ = sender.send(fetch_result);
        let _ = handle_clone.send();
    });

    Ok(())
}

fn spawn_actions_fetch(mut state: PluginState, token: String) -> Result<(), String> {
    let (sender, receiver) = channel::<ActionsFetchResult>();
    let receiver = Arc::new(Mutex::new(receiver));

    let receiver_clone = receiver.clone();
    let handle = AsyncHandle::new(move || {
        let _ = catch_unwind(AssertUnwindSafe(|| {
            let result = receiver_clone.lock().unwrap().try_recv();
            if let Ok(fetch_result) = result {
                STATE.with(|state_cell| {
                    if let Ok(mut state_opt) = state_cell.try_borrow_mut() {
                        if let Some(ref mut state) = *state_opt {
                            match fetch_result {
                                ActionsFetchResult::Success { suites, number, current_pr, pr_chain } => {
                                    state.pr_workflow_cache.insert(number, suites.clone());
                                    state.set_check_suites(suites, pr_chain.as_ref());
                                    state.current_pr = Some(current_pr);
                                    state.pr_chain = pr_chain.clone();

                                    let lines = state.actions_buffer.render();
                                    if state.sidebar.set_lines(lines).is_ok() {
                                        if !state.actions_navigator.is_empty() {
                                            if let Some(line) = state.actions_buffer.line_for_suite(state.actions_navigator.current_index()) {
                                                let _ = state.sidebar.set_cursor(line + 1, 0);
                                            }
                                        }
                                        notify_info(&format!("Loaded {} workflow suite(s) for PR #{}", state.check_suites.len(), number));
                                    }
                                }
                                ActionsFetchResult::Error(msg) => {
                                    notify_error(&msg);
                                }
                            }
                        }
                    }
                });
            }
        }));
    }).map_err(|e| format!("Failed to create async handle: {}", e))?;

    let handle_clone = handle.clone();
    state.async_handle = Some(handle);

    STATE.with(|state_cell| {
        let mut state_opt = match state_cell.try_borrow_mut() {
            Ok(guard) => guard,
            Err(_) => return Err("State temporarily unavailable".to_string()),
        };
        *state_opt = Some(state);
        Ok(())
    })?;

    std::thread::spawn(move || {
        let pr_result = detect_pr();

        let fetch_result = match pr_result {
            Ok(pr) => {
                let chain_result = detect_chain(&pr);
                let pr_chain = match chain_result {
                    Ok(chain) => Some(chain),
                    Err(e) => {
                        eprintln!("Failed to detect PR chain: {}", e);
                        None
                    }
                };

                match fetch_check_runs(&token, &pr.owner, &pr.repo, pr.number) {
                    Ok(suites) => ActionsFetchResult::Success {
                        suites,
                        number: pr.number,
                        current_pr: pr,
                        pr_chain,
                    },
                    Err(e) => ActionsFetchResult::Error(format!("Failed to fetch workflow status: {}", e)),
                }
            }
            Err(e) => {
                let msg = match e {
                    PrError::NotAGitRepo => "Not a git repository".to_string(),
                    PrError::GhError(err) => format!("gh error: {}", err),
                    PrError::NoAssociatedPr => "No PR associated with current branch".to_string(),
                    PrError::IoError(err) => format!("IO error: {}", err),
                    PrError::ParseError(err) => format!("Parse error: {}", err),
                };
                ActionsFetchResult::Error(msg)
            }
        };

        let _ = sender.send(fetch_result);
        let _ = handle_clone.send();
    });

    Ok(())
}

fn close() -> Result<(), String> {
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
            if state.is_loading {
                notify_info("Still loading...");
                return Ok(());
            }
            match state.mode {
                SidebarMode::Comments => {
                    if state.navigator.is_empty() {
                        notify_info("No comments to navigate");
                        return Ok(());
                    }
                    state.navigator.next();
                    if let Some(line) = state.buffer.line_for_thread(state.navigator.current_index()) {
                        state.sidebar.set_cursor(line + 1, 0).map_err(|e| format!("Failed to move cursor: {}", e))?;
                        let _ = api::command("normal! zz");
                    }
                }
                SidebarMode::Actions => {
                    if state.actions_navigator.is_empty() {
                        notify_info("No workflows to navigate");
                        return Ok(());
                    }
                    state.actions_navigator.next();
                    if let Some(line) = state.actions_buffer.line_for_suite(state.actions_navigator.current_index()) {
                        state.sidebar.set_cursor(line + 1, 0).map_err(|e| format!("Failed to move cursor: {}", e))?;
                        let _ = api::command("normal! zz");
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

fn prev_comment() -> Result<(), String> {
    STATE.with(|state_cell| {
        let mut state_opt = match state_cell.try_borrow_mut() {
            Ok(guard) => guard,
            Err(_) => return Err("State temporarily unavailable".to_string()),
        };
        if let Some(ref mut state) = *state_opt {
            if state.is_loading {
                notify_info("Still loading...");
                return Ok(());
            }
            match state.mode {
                SidebarMode::Comments => {
                    if state.navigator.is_empty() {
                        notify_info("No comments to navigate");
                        return Ok(());
                    }
                    state.navigator.prev();
                    if let Some(line) = state.buffer.line_for_thread(state.navigator.current_index()) {
                        state.sidebar.set_cursor(line + 1, 0).map_err(|e| format!("Failed to move cursor: {}", e))?;
                        let _ = api::command("normal! zz");
                    }
                }
                SidebarMode::Actions => {
                    if state.actions_navigator.is_empty() {
                        notify_info("No workflows to navigate");
                        return Ok(());
                    }
                    state.actions_navigator.prev();
                    if let Some(line) = state.actions_buffer.line_for_suite(state.actions_navigator.current_index()) {
                        state.sidebar.set_cursor(line + 1, 0).map_err(|e| format!("Failed to move cursor: {}", e))?;
                        let _ = api::command("normal! zz");
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
            if state.is_loading {
                notify_info("Still loading...");
                return Ok(());
            }
            match state.mode {
                SidebarMode::Comments => {
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
                }
                SidebarMode::Actions => {
                    // Open workflow URL in browser
                    let suite_idx = state.actions_navigator.current_index();
                    if let Some(suite) = state.check_suites.get(suite_idx) {
                        if let Some(run) = suite.check_runs.first() {
                            if let Some(ref url) = run.details_url {
                                let cmd = format!("silent !open '{}'", url);
                                let _ = api::command(&cmd);
                                notify_info("Opened workflow in browser");
                            } else {
                                notify_info("No URL available for this workflow");
                            }
                        } else {
                            notify_info("No check runs in this suite");
                        }
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

fn toggle_collapse() -> Result<(), String> {
    STATE.with(|state_cell| {
        let mut state_opt = match state_cell.try_borrow_mut() {
            Ok(guard) => guard,
            Err(_) => return Err("State temporarily unavailable".to_string()),
        };
        if let Some(ref mut state) = *state_opt {
            if state.is_loading {
                notify_info("Still loading...");
                return Ok(());
            }
            // Collapse only works in Comments mode
            if state.mode != SidebarMode::Comments {
                notify_info("Collapse only works in Comments mode");
                return Ok(());
            }
            let current_idx = state.navigator.current_index();
            state.buffer.toggle_collapse(current_idx);

            let is_now_collapsed = state.buffer.is_collapsed(current_idx);
            notify_info(&format!("Thread {} collapsed: {}", current_idx, is_now_collapsed));

            let lines = state.buffer.render();
            state.sidebar.set_lines(lines).map_err(|e| format!("Failed to update buffer: {}", e))?;

            if let Some(line) = state.buffer.line_for_thread(current_idx) {
                let _ = state.sidebar.set_cursor(line + 1, 0);
            }
        } else {
            notify_error("Sidebar not open. Run :PRComments first");
            return Err("Sidebar not open".to_string());
        }
        Ok(())
    })
}

fn toggle_resolve() -> Result<(), String> {
    STATE.with(|state_cell| {
        let mut state_opt = match state_cell.try_borrow_mut() {
            Ok(guard) => guard,
            Err(_) => return Err("State temporarily unavailable".to_string()),
        };
        if let Some(ref mut state) = *state_opt {
            if state.is_loading {
                notify_info("Still loading...");
                return Ok(());
            }
            // Resolve only works in Comments mode
            if state.mode != SidebarMode::Comments {
                notify_info("Resolve only works in Comments mode");
                return Ok(());
            }
            let current_idx = state.navigator.current_index();

            let thread = match state.threads.get(current_idx) {
                Some(t) => t,
                None => {
                    notify_error("No thread selected");
                    return Ok(());
                }
            };

            let thread_id = match (&thread.root, &thread.thread_id) {
                (Comment::Review(_), Some(id)) => id.clone(),
                _ => {
                    notify_info("Cannot resolve issue comments or threads without thread ID");
                    return Ok(());
                }
            };

            let token = get_gh_token().map_err(|e| format!("Auth error: {:?}", e))?;

            let new_resolved = if thread.is_resolved {
                unresolve_thread(&token, &thread_id).map_err(|e| format!("Failed to unresolve: {:?}", e))?;
                false
            } else {
                resolve_thread(&token, &thread_id).map_err(|e| format!("Failed to resolve: {:?}", e))?;
                true
            };

            state.buffer.set_thread_resolved(current_idx, new_resolved);
            if let Some(t) = state.threads.get_mut(current_idx) {
                t.is_resolved = new_resolved;
            }

            if new_resolved {
                state.buffer.set_collapsed(current_idx, true);
            }

            let lines = state.buffer.render();
            state.sidebar.set_lines(lines).map_err(|e| format!("Failed to update buffer: {}", e))?;

            notify_info(if new_resolved { "Thread resolved" } else { "Thread unresolved" });
        } else {
            notify_error("Sidebar not open. Run :PRComments first");
            return Err("Sidebar not open".to_string());
        }
        Ok(())
    })
}

fn fetch_actions_for_current_pr(current_pr: &PullRequest) {
    let token = match get_gh_token() {
        Ok(t) => t,
        Err(e) => {
            notify_error(&format!("Auth error: {:?}", e));
            return;
        }
    };

    STATE.with(|state_cell| {
        if let Ok(mut state_opt) = state_cell.try_borrow_mut() {
            if let Some(ref mut state) = *state_opt {
                state.is_loading = true;
                let loading_lines = render_loading_lines(SidebarMode::Actions);
                let _ = state.sidebar.set_lines(loading_lines);
            }
        }
    });

    let (sender, receiver) = channel::<ActionsFetchResult>();
    let receiver = Arc::new(Mutex::new(receiver));

    let receiver_clone = receiver.clone();
    let handle_result = AsyncHandle::new(move || {
        let _ = catch_unwind(AssertUnwindSafe(|| {
            let result = receiver_clone.lock().unwrap().try_recv();
            if let Ok(fetch_result) = result {
                STATE.with(|state_cell| {
                    if let Ok(mut state_opt) = state_cell.try_borrow_mut() {
                        if let Some(ref mut state) = *state_opt {
                            match fetch_result {
                                ActionsFetchResult::Success { suites, number, current_pr, pr_chain } => {
                                    state.pr_workflow_cache.insert(number, suites.clone());
                                    state.set_check_suites(suites, pr_chain.as_ref());
                                    state.current_pr = Some(current_pr);
                                    state.pr_chain = pr_chain.clone();

                                    let lines = state.actions_buffer.render();
                                    if state.sidebar.set_lines(lines).is_ok() {
                                        if !state.actions_navigator.is_empty() {
                                            if let Some(line) = state.actions_buffer.line_for_suite(state.actions_navigator.current_index()) {
                                                let _ = state.sidebar.set_cursor(line + 1, 0);
                                            }
                                        }
                                        notify_info(&format!("Loaded {} workflow suite(s) for PR #{}", state.check_suites.len(), number));
                                    }
                                }
                                ActionsFetchResult::Error(msg) => {
                                    state.is_loading = false;
                                    notify_error(&msg);
                                }
                            }
                        }
                    }
                });
            }
        }));
    });

    if let Err(e) = handle_result {
        notify_error(&format!("Failed to create async handle: {}", e));
        return;
    }

    let handle = match handle_result {
        Ok(h) => h,
        Err(e) => {
            notify_error(&format!("Failed to create async handle: {}", e));
            return;
        }
    };

    let handle_clone = handle.clone();
    let pr_clone = current_pr.clone();

    std::thread::spawn(move || {
        let chain_result = detect_chain(&pr_clone);
        let pr_chain = match chain_result {
            Ok(chain) => Some(chain),
            Err(e) => {
                eprintln!("Failed to detect PR chain: {}", e);
                None
            }
        };

        let result = match fetch_check_runs(&token, &pr_clone.owner, &pr_clone.repo, pr_clone.number) {
            Ok(suites) => ActionsFetchResult::Success {
                suites,
                number: pr_clone.number,
                current_pr: pr_clone,
                pr_chain,
            },
            Err(e) => ActionsFetchResult::Error(format!("Failed to fetch workflow status: {}", e)),
        };
        let _ = sender.send(result);
        let _ = handle_clone.send();
    });

    STATE.with(|state_cell| {
        if let Ok(mut state_opt) = state_cell.try_borrow_mut() {
            if let Some(ref mut state) = *state_opt {
                state.async_handle = Some(handle);
            }
        }
    });
}

fn fetch_actions_for_chain(pr_info: &crate::github::pr::PrInfo, owner: &str, repo: &str, new_index: usize) {
    let cached_suites = STATE.with(|state_cell| {
        state_cell.borrow()
            .as_ref()
            .and_then(|state| state.pr_workflow_cache.get(&pr_info.number).cloned())
    });

    if let Some(suites) = cached_suites {
        STATE.with(|state_cell| {
            if let Ok(mut state_opt) = state_cell.try_borrow_mut() {
                if let Some(ref mut state) = *state_opt {
                    state.pr_workflow_cache.insert(pr_info.number, suites.clone());
                    let chain_ref = state.pr_chain.clone();
                    state.set_check_suites(suites, chain_ref.as_ref());
                    if let Some(ref mut chain) = state.pr_chain {
                        chain.current_index = new_index;
                    }
                    let lines = state.actions_buffer.render();
                    let _ = state.sidebar.set_lines(lines);
                    if !state.actions_navigator.is_empty() {
                        if let Some(line) = state.actions_buffer.line_for_suite(state.actions_navigator.current_index()) {
                            let _ = state.sidebar.set_cursor(line + 1, 0);
                        }
                    }
                    notify_info(&format!("Loaded PR #{} workflow (cached): {}", pr_info.number, pr_info.title));
                }
            }
        });
        return;
    }

    let token = match get_gh_token() {
        Ok(t) => t,
        Err(e) => {
            notify_error(&format!("Auth error: {:?}", e));
            return;
        }
    };

    STATE.with(|state_cell| {
        if let Ok(mut state_opt) = state_cell.try_borrow_mut() {
            if let Some(ref mut state) = *state_opt {
                state.is_loading = true;
                let loading_lines = render_loading_lines(SidebarMode::Actions);
                let _ = state.sidebar.set_lines(loading_lines);
            }
        }
    });

    let (sender, receiver) = channel::<actions::ChainActionsFetchResult>();
    let receiver = Arc::new(Mutex::new(receiver));

    let receiver_clone = receiver.clone();
    let handle_result = AsyncHandle::new(move || {
        let _ = catch_unwind(AssertUnwindSafe(|| {
            let result = receiver_clone.lock().unwrap().try_recv();
            if let Ok(fetch_result) = result {
                STATE.with(|state_cell| {
                    if let Ok(mut state_opt) = state_cell.try_borrow_mut() {
                        if let Some(ref mut state) = *state_opt {
                            match fetch_result {
                                actions::ChainActionsFetchResult::Success { suites, number, new_index } => {
                                    state.pr_workflow_cache.insert(number, suites.clone());
                                    let chain_clone = state.pr_chain.clone();
                                    state.set_check_suites(suites, chain_clone.as_ref());
                                    if let Some(ref mut chain) = state.pr_chain {
                                        chain.current_index = new_index;
                                    }
                                    let lines = state.actions_buffer.render();
                                    if state.sidebar.set_lines(lines).is_ok() {
                                        if !state.actions_navigator.is_empty() {
                                            if let Some(line) = state.actions_buffer.line_for_suite(state.actions_navigator.current_index()) {
                                                let _ = state.sidebar.set_cursor(line + 1, 0);
                                            }
                                        }
                                        notify_info(&format!("Loaded PR #{} workflow", number));
                                    }
                                }
                                actions::ChainActionsFetchResult::Error(msg) => {
                                    state.is_loading = false;
                                    notify_error(&msg);
                                }
                            }
                        }
                    }
                });
            }
        }));
    });

    if let Err(e) = handle_result {
        notify_error(&format!("Failed to create async handle: {}", e));
        return;
    }

    let handle = match handle_result {
        Ok(h) => h,
        Err(e) => {
            notify_error(&format!("Failed to create async handle: {}", e));
            return;
        }
    };

    let handle_clone = handle.clone();
    let owner = owner.to_string();
    let repo = repo.to_string();
    let pr_number = pr_info.number;

    std::thread::spawn(move || {
        let result = match fetch_check_runs(&token, &owner, &repo, pr_number) {
            Ok(suites) => actions::ChainActionsFetchResult::Success {
                suites,
                number: pr_number,
                new_index,
            },
            Err(e) => actions::ChainActionsFetchResult::Error(format!("Failed to fetch workflow status: {}", e)),
        };
        let _ = sender.send(result);
        let _ = handle_clone.send();
    });

    STATE.with(|state_cell| {
        if let Ok(mut state_opt) = state_cell.try_borrow_mut() {
            if let Some(ref mut state) = *state_opt {
                state.async_handle = Some(handle);
            }
        }
    });
}

fn switch_mode() -> Result<(), String> {
    STATE.with(|state_cell| {
        let mut state_opt = match state_cell.try_borrow_mut() {
            Ok(guard) => guard,
            Err(_) => return Err("State temporarily unavailable".to_string()),
        };
        if let Some(ref mut state) = *state_opt {
            let new_mode = state.mode.toggle();
            state.mode = new_mode;

            // If switching to Actions mode and we have no data, fetch it
            if new_mode == SidebarMode::Actions && state.check_suites.is_empty() {
                if let Some(ref pr) = state.current_pr.clone() {
                    drop(state_opt);
                    fetch_actions_for_current_pr(&pr);
                    return Ok(());
                }
            }

            // Render with appropriate buffer
            let lines = match state.mode {
                SidebarMode::Comments => state.buffer.render(),
                SidebarMode::Actions => state.actions_buffer.render(),
            };
            state.sidebar.set_lines(lines).map_err(|e| format!("Failed to update buffer: {}", e))?;

            // Reset cursor position
            let line = match state.mode {
                SidebarMode::Comments => state.buffer.line_for_thread(state.navigator.current_index()),
                SidebarMode::Actions => state.actions_buffer.line_for_suite(state.actions_navigator.current_index()),
            };
            if let Some(l) = line {
                let _ = state.sidebar.set_cursor(l + 1, 0);
                let _ = api::command("normal! zz");
            }

            notify_info(&format!("Switched to {} mode", state.mode.to_display()));
        } else {
            notify_error("Sidebar not open. Run :PRComments first");
            return Err("Sidebar not open".to_string());
        }
        Ok(())
    })
}

enum ChainFetchResult {
    Success {
        threads: Vec<CommentThread>,
        title: String,
        number: u64,
        new_index: usize,
    },
    Error(String),
}

fn fetch_pr_comments_for_chain(pr_info: &crate::github::pr::PrInfo, owner: &str, repo: &str, new_index: usize) {
    let cached_threads = STATE.with(|state_cell| {
        state_cell.borrow()
            .as_ref()
            .and_then(|state| state.pr_comment_cache.get(&pr_info.number).cloned())
    });

    if let Some(threads) = cached_threads {
        STATE.with(|state_cell| {
            if let Ok(mut state_opt) = state_cell.try_borrow_mut() {
                if let Some(ref mut state) = *state_opt {
                    state.pr_comment_cache.insert(pr_info.number, threads.clone());
                    let chain_ref = state.pr_chain.clone();
                    state.set_threads(threads, chain_ref.as_ref());
                    if let Some(ref mut chain) = state.pr_chain {
                        chain.current_index = new_index;
                    }
                    let lines = state.buffer.render();
                    let _ = state.sidebar.set_lines(lines);
                    if !state.navigator.is_empty() {
                        let _ = state.navigator.set_cursor_to_current(&mut state.sidebar);
                    }
                    notify_info(&format!("Loaded PR #{} (cached): {}", pr_info.number, pr_info.title));
                }
            }
        });
        return;
    }

    let token = match get_gh_token() {
        Ok(t) => t,
        Err(e) => {
            notify_error(&format!("Auth error: {:?}", e));
            return;
        }
    };

    STATE.with(|state_cell| {
        if let Ok(mut state_opt) = state_cell.try_borrow_mut() {
            if let Some(ref mut state) = *state_opt {
                state.is_loading = true;
                let loading_lines = render_loading_lines(SidebarMode::Comments);
                let _ = state.sidebar.set_lines(loading_lines);
            }
        }
    });

    let (sender, receiver) = channel::<ChainFetchResult>();
    let receiver = Arc::new(Mutex::new(receiver));

    let receiver_clone = receiver.clone();
    let handle_result = AsyncHandle::new(move || {
        let _ = catch_unwind(AssertUnwindSafe(|| {
            let result = receiver_clone.lock().unwrap().try_recv();
            if let Ok(fetch_result) = result {
                STATE.with(|state_cell| {
                    if let Ok(mut state_opt) = state_cell.try_borrow_mut() {
                        if let Some(ref mut state) = *state_opt {
                            match fetch_result {
                                ChainFetchResult::Success { threads, title: _, number, new_index } => {
                                    state.pr_comment_cache.insert(number, threads.clone());
                                    let chain_clone = state.pr_chain.clone();
                                    state.set_threads(threads, chain_clone.as_ref());
                                    if let Some(ref mut chain) = state.pr_chain {
                                        chain.current_index = new_index;
                                    }
                                    let lines = state.buffer.render();
                                    if state.sidebar.set_lines(lines).is_ok() {
                                        if !state.navigator.is_empty() {
                                            if let Some(line) = state.buffer.line_for_thread(state.navigator.current_index()) {
                                                let _ = state.sidebar.set_cursor(line + 1, 0);
                                            }
                                        }
                                        notify_info(&format!("Loaded PR #{}", number));
                                    }
                                }
                                ChainFetchResult::Error(msg) => {
                                    state.is_loading = false;
                                    notify_error(&msg);
                                }
                            }
                        }
                    }
                });
            }
        }));
    });

    if let Err(e) = handle_result {
        notify_error(&format!("Failed to create async handle: {}", e));
        return;
    }

    let handle = match handle_result {
        Ok(h) => h,
        Err(e) => {
            notify_error(&format!("Failed to create async handle: {}", e));
            return;
        }
    };

    let handle_clone = handle.clone();
    let owner = owner.to_string();
    let repo = repo.to_string();
    let pr_number = pr_info.number;

    std::thread::spawn(move || {
        let result = match fetch_comments(&token, &owner, &repo, pr_number) {
            Ok(threads) => ChainFetchResult::Success {
                threads,
                title: String::new(),
                number: pr_number,
                new_index,
            },
            Err(e) => ChainFetchResult::Error(format!("Failed to fetch comments: {}", e)),
        };
        let _ = sender.send(result);
        let _ = handle_clone.send();
    });

    STATE.with(|state_cell| {
        if let Ok(mut state_opt) = state_cell.try_borrow_mut() {
            if let Some(ref mut state) = *state_opt {
                state.async_handle = Some(handle);
            }
        }
    });
}

fn navigate_to_parent_pr() -> Result<(), String> {
    STATE.with(|state_cell| {
        let mut state_opt = match state_cell.try_borrow_mut() {
            Ok(guard) => guard,
            Err(_) => return Err("State temporarily unavailable".to_string()),
        };
        if let Some(ref mut state) = *state_opt {
            if state.is_loading {
                notify_info("Still loading...");
                return Ok(());
            }
            if let Some(ref chain) = state.pr_chain {
                if chain.is_root() {
                    notify_info("Already at the root PR (no parent)");
                    return Ok(());
                }
                if let Some(parent_info) = chain.parent() {
                    let parent_index = chain.current_index.saturating_sub(1);
                    let parent_clone = parent_info.clone();
                    let mode = state.mode;
                    drop(state_opt);
                    if let Some(ref current_pr) = STATE.with(|sc| {
                        sc.borrow().as_ref().map(|s| s.current_pr.clone())
                    }).flatten() {
                        match mode {
                            SidebarMode::Comments => fetch_pr_comments_for_chain(&parent_clone, &current_pr.owner, &current_pr.repo, parent_index),
                            SidebarMode::Actions => fetch_actions_for_chain(&parent_clone, &current_pr.owner, &current_pr.repo, parent_index),
                        }
                    }
                } else {
                    notify_info("No parent PR found");
                }
            } else {
                notify_info("No PR chain detected");
            }
        } else {
            notify_error("Sidebar not open. Run :PRComments first");
            return Err("Sidebar not open".to_string());
        }
        Ok(())
    })
}

fn navigate_to_child_pr() -> Result<(), String> {
    STATE.with(|state_cell| {
        let mut state_opt = match state_cell.try_borrow_mut() {
            Ok(guard) => guard,
            Err(_) => return Err("State temporarily unavailable".to_string()),
        };
        if let Some(ref mut state) = *state_opt {
            if state.is_loading {
                notify_info("Still loading...");
                return Ok(());
            }
            if let Some(ref chain) = state.pr_chain {
                if chain.is_tip() {
                    notify_info("Already at the tip PR (no child)");
                    return Ok(());
                }
                if let Some(child_info) = chain.child() {
                    let child_index = chain.current_index + 1;
                    let child_clone = child_info.clone();
                    let mode = state.mode;
                    drop(state_opt);
                    if let Some(ref current_pr) = STATE.with(|sc| {
                        sc.borrow().as_ref().map(|s| s.current_pr.clone())
                    }).flatten() {
                        match mode {
                            SidebarMode::Comments => fetch_pr_comments_for_chain(&child_clone, &current_pr.owner, &current_pr.repo, child_index),
                            SidebarMode::Actions => fetch_actions_for_chain(&child_clone, &current_pr.owner, &current_pr.repo, child_index),
                        }
                    }
                } else {
                    notify_info("No child PR found");
                }
            } else {
                notify_info("No PR chain detected");
            }
        } else {
            notify_error("Sidebar not open. Run :PRComments first");
            return Err("Sidebar not open".to_string());
        }
        Ok(())
    })
}

enum RefreshFetchResult {
    Success {
        threads: Vec<CommentThread>,
        title: String,
        number: u64,
        current_pr: PullRequest,
        pr_chain: Option<PrChain>,
    },
    Error(String),
}

fn fetch_fresh_comments(current_pr: &PullRequest) {
    let token = match get_gh_token() {
        Ok(t) => t,
        Err(e) => {
            notify_error(&format!("Auth error: {:?}", e));
            return;
        }
    };

    STATE.with(|state_cell| {
        if let Ok(mut state_opt) = state_cell.try_borrow_mut() {
            if let Some(ref mut state) = *state_opt {
                state.is_loading = true;
                let loading_lines = render_loading_lines(SidebarMode::Comments);
                let _ = state.sidebar.set_lines(loading_lines);
            }
        }
    });

    let (sender, receiver) = channel::<RefreshFetchResult>();
    let receiver = Arc::new(Mutex::new(receiver));

    let receiver_clone = receiver.clone();
    let handle_result = AsyncHandle::new(move || {
        let _ = catch_unwind(AssertUnwindSafe(|| {
            let result = receiver_clone.lock().unwrap().try_recv();
            if let Ok(fetch_result) = result {
                STATE.with(|state_cell| {
                    if let Ok(mut state_opt) = state_cell.try_borrow_mut() {
                        if let Some(ref mut state) = *state_opt {
                            match fetch_result {
                                RefreshFetchResult::Success { threads, title, number, current_pr, pr_chain } => {
                                    state.pr_comment_cache.insert(number, threads.clone());
                                    state.set_threads(threads, pr_chain.as_ref());
                                    state.current_pr = Some(current_pr);
                                    state.pr_chain = pr_chain.clone();
                                    let lines = state.buffer.render();
                                    if state.sidebar.set_lines(lines).is_ok() {
                                        if !state.navigator.is_empty() {
                                            if let Some(line) = state.buffer.line_for_thread(state.navigator.current_index()) {
                                                let _ = state.sidebar.set_cursor(line + 1, 0);
                                            }
                                        }
                                        notify_info(&format!("Refreshed {} thread(s) for PR #{}: {}", state.threads.len(), number, title));
                                    }
                                }
                                RefreshFetchResult::Error(msg) => {
                                    state.is_loading = false;
                                    notify_error(&msg);
                                }
                            }
                        }
                    }
                });
            }
        }));
    });

    if let Err(e) = handle_result {
        notify_error(&format!("Failed to create async handle: {}", e));
        return;
    }

    let handle = match handle_result {
        Ok(h) => h,
        Err(e) => {
            notify_error(&format!("Failed to create async handle: {}", e));
            return;
        }
    };

    let handle_clone = handle.clone();
    let pr_clone = current_pr.clone();

    std::thread::spawn(move || {
        let chain_result = detect_chain(&pr_clone);
        let pr_chain = match chain_result {
            Ok(chain) => Some(chain),
            Err(e) => {
                eprintln!("Failed to detect PR chain: {}", e);
                None
            }
        };

        let result = match fetch_comments(&token, &pr_clone.owner, &pr_clone.repo, pr_clone.number) {
            Ok(threads) => RefreshFetchResult::Success {
                threads,
                title: pr_clone.title.clone(),
                number: pr_clone.number,
                current_pr: pr_clone,
                pr_chain,
            },
            Err(e) => RefreshFetchResult::Error(format!("Failed to fetch comments: {}", e)),
        };
        let _ = sender.send(result);
        let _ = handle_clone.send();
    });

    STATE.with(|state_cell| {
        if let Ok(mut state_opt) = state_cell.try_borrow_mut() {
            if let Some(ref mut state) = *state_opt {
                state.async_handle = Some(handle);
            }
        }
    });
}

fn refresh() -> Result<(), String> {
    STATE.with(|state_cell| {
        let mut state_opt = match state_cell.try_borrow_mut() {
            Ok(guard) => guard,
            Err(_) => return Err("State temporarily unavailable".to_string()),
        };
        if let Some(ref mut state) = *state_opt {
            if state.is_loading {
                notify_info("Still loading...");
                return Ok(());
            }

            let current_pr = match &state.current_pr {
                Some(pr) => pr.clone(),
                None => {
                    notify_error("No PR loaded");
                    return Ok(());
                }
            };

            state.pr_comment_cache.remove(&current_pr.number);

            drop(state_opt);

            fetch_fresh_comments(&current_pr);
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

    let practions_opts = CreateCommandOpts::builder()
        .desc("Open PR workflow status sidebar")
        .build();

    let practions_cmd = |_args: CommandArgs| {
        let _ = catch_unwind(AssertUnwindSafe(|| open_actions()));
    };

    api::create_user_command("PRActions", practions_cmd, &practions_opts)?;

    let prcommentsclose_opts = CreateCommandOpts::builder()
        .desc("Close PR comments sidebar")
        .build();

    let prcommentsclose_cmd = |_args: CommandArgs| {
        let _ = catch_unwind(AssertUnwindSafe(|| close()));
    };

    api::create_user_command("PRCommentsClose", prcommentsclose_cmd, &prcommentsclose_opts)?;

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
    let toggle_collapse_fn: Function<(), ()> = Function::from_fn(|_| {
        let _ = catch_unwind(AssertUnwindSafe(|| {
            if let Err(e) = toggle_collapse() {
                notify_error(&e);
            }
        }));
    });
    let toggle_resolve_fn: Function<(), ()> = Function::from_fn(|_| {
        let _ = catch_unwind(AssertUnwindSafe(|| {
            if let Err(e) = toggle_resolve() {
                notify_error(&e);
            }
        }));
    });
    let navigate_to_parent_fn: Function<(), ()> = Function::from_fn(|_| {
        let _ = catch_unwind(AssertUnwindSafe(|| {
            if let Err(e) = navigate_to_parent_pr() {
                notify_error(&e);
            }
        }));
    });
    let navigate_to_child_fn: Function<(), ()> = Function::from_fn(|_| {
        let _ = catch_unwind(AssertUnwindSafe(|| {
            if let Err(e) = navigate_to_child_pr() {
                notify_error(&e);
            }
        }));
    });
    let refresh_fn: Function<(), ()> = Function::from_fn(|_| {
        let _ = catch_unwind(AssertUnwindSafe(|| {
            if let Err(e) = refresh() {
                notify_error(&e);
            }
        }));
    });
    let switch_mode_fn: Function<(), ()> = Function::from_fn(|_| {
        let _ = catch_unwind(AssertUnwindSafe(|| {
            if let Err(e) = switch_mode() {
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
        ("toggle_collapse", Object::from(toggle_collapse_fn)),
        ("toggle_resolve", Object::from(toggle_resolve_fn)),
        ("navigate_to_parent_pr", Object::from(navigate_to_parent_fn)),
        ("navigate_to_child_pr", Object::from(navigate_to_child_fn)),
        ("refresh", Object::from(refresh_fn)),
        ("switch_mode", Object::from(switch_mode_fn)),
    ]))
}
