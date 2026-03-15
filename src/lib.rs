mod actions;
mod github;
mod types;
mod ui;

use crate::actions::{ActionsFetchResult, ActionsNavigator, WorkflowPrefetchResult};
use crate::github::{
    delete_issue_comment, delete_pending_review_comment, detect_chain, detect_pr, edit_issue_comment,
    edit_pending_review_comment, fetch_check_runs, fetch_comments, fetch_pending_review_comments,
    get_gh_token, is_gh_installed, resolve_thread, unresolve_thread, AuthError, CheckSuite, PrError,
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
use std::path::{Path, PathBuf};
use std::process::Command;
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
    static REVIEW_SIGNS: RefCell<Option<ReviewSignsState>> = RefCell::new(None);
}

struct ReviewSignsState {
    cwd: PathBuf,
    lines_by_path: HashMap<String, HashSet<usize>>,
}

enum ReviewSignsFetchResult {
    Success(ReviewSignsState),
    Error(String),
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
    pending_edit: Option<PendingEditSession>,
}

struct PendingEditSession {
    bufnr: Buffer,
    win: Window,
    comment_node_id: String,
    kind: EditableCommentKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EditableCommentKind {
    Review,
    Issue,
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
            pending_edit: None,
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
            pending_edit: None,
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

fn review_signs_namespace() -> u32 {
    api::create_namespace("neogh_review_comment_signs")
}

fn normalize_relative_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn relative_to_cwd(path: &Path, cwd: &Path) -> Option<String> {
    path.strip_prefix(cwd).ok().map(normalize_relative_path)
}

fn collect_review_comment_lines(threads: &[CommentThread]) -> HashMap<String, HashSet<usize>> {
    let mut lines_by_path: HashMap<String, HashSet<usize>> = HashMap::new();
    for thread in threads {
        let mut add_comment = |comment: &Comment| {
            if let Comment::Review(rc) = comment {
                let path = rc.path.trim();
                let line = rc.navigation_line().unwrap_or(0) as usize;
                if !path.is_empty() && line > 0 {
                    lines_by_path
                        .entry(path.to_string())
                        .or_default()
                        .insert(line);
                }
            }
        };
        add_comment(&thread.root);
        for reply in &thread.replies {
            add_comment(reply);
        }
    }
    lines_by_path
}

fn apply_review_comment_signs_to_buffer(buf: &mut Buffer) -> Result<(), String> {
    let state = REVIEW_SIGNS.with(|cell| cell.borrow().as_ref().map(|s| ReviewSignsState {
        cwd: s.cwd.clone(),
        lines_by_path: s.lines_by_path.clone(),
    }));
    let Some(state) = state else {
        return Ok(());
    };

    let path = buf
        .get_name()
        .map_err(|e| format!("Failed to get buffer name: {}", e))?;
    if path.as_os_str().is_empty() {
        return Ok(());
    }

    let relative = relative_to_cwd(&path, &state.cwd).unwrap_or_else(|| normalize_relative_path(&path));
    let lines = state.lines_by_path.get(&relative);

    let ns = review_signs_namespace();
    buf.clear_namespace(ns, ..)
        .map_err(|e| format!("Failed to clear review signs: {}", e))?;

    if let Some(lines) = lines {
        for line in lines {
            let opts = SetExtmarkOpts::builder()
                .sign_text("RC")
                .sign_hl_group("DiagnosticSignHint")
                .priority(50)
                .build();
            let _ = buf.set_extmark(ns, line.saturating_sub(1), 0, &opts);
        }
    }

    Ok(())
}

fn apply_review_comment_signs_current_buffer() -> Result<(), String> {
    let mut buf = api::get_current_buf();
    apply_review_comment_signs_to_buffer(&mut buf)
}

fn setup_review_signs_autocmd() -> Result<(), String> {
    let opts = CreateAugroupOpts::builder().clear(true).build();
    api::create_augroup("NeoghReviewSigns", &opts)
        .map_err(|e| format!("Failed to create review signs augroup: {}", e))?;

    let callback = move |_args: AutocmdCallbackArgs| -> bool {
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _ = apply_review_comment_signs_current_buffer();
        }))
        .is_ok()
    };

    let opts = CreateAutocmdOpts::builder().callback(callback).build();
    api::create_autocmd(vec!["BufEnter", "BufWinEnter"], &opts)
        .map_err(|e| format!("Failed to create review signs autocmd: {}", e))?;
    Ok(())
}

fn check_prerequisites() -> Result<(), String> {
    if !is_gh_installed() {
        return Err("gh CLI not found. Please install: https://cli.github.com".to_string());
    }
    Ok(())
}

fn get_authenticated_login() -> Result<String, String> {
    let output = Command::new("gh")
        .args(["api", "user", "--jq", ".login"])
        .output()
        .map_err(|e| format!("Failed to run gh api user: {}", e))?;

    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
    }

    let login = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if login.is_empty() {
        return Err("Failed to determine authenticated GitHub login".to_string());
    }
    Ok(login)
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
    buf.set_keymap(Mode::Normal, "c", "<Cmd>lua require('neogh').reply_to_thread()<CR>", &opts)?;
    buf.set_keymap(Mode::Normal, "e", "<Cmd>lua require('neogh').edit_pending_comment()<CR>", &opts)?;
    buf.set_keymap(Mode::Normal, "d", "<Cmd>lua require('neogh').delete_pending_comment()<CR>", &opts)?;
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
                                    SidebarMode::PendingReview => {
                                        if let Some(idx) = state.buffer.line_to_thread_index(line) {
                                            state.navigator.set_index(idx);
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
        SidebarMode::PendingReview => "Loading pending review comments...",
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
        SidebarMode::PendingReview => spawn_pending_review_fetch(state, token),
    }
}

fn open() -> Result<(), String> {
    open_with_mode(SidebarMode::Comments)
}

fn open_actions() -> Result<(), String> {
    open_with_mode(SidebarMode::Actions)
}

fn open_pr_review_picker() -> Result<(), String> {
    if let Err(e) = check_prerequisites() {
        notify_error(&e);
        return Err(e);
    }

    get_gh_token().map_err(|e| {
        let msg = match e {
            AuthError::GhNotFound => "gh CLI not found".to_string(),
            AuthError::NotAuthenticated => "Not authenticated with gh".to_string(),
            AuthError::IoError(msg) => format!("IO error: {}", msg),
        };
        notify_error(&msg);
        msg
    })?;

    api::command("lua require('neogh.review').open()").map_err(|e| {
        let msg = format!("Failed to open PR review picker: {}", e);
        notify_error(&msg);
        msg
    })?;

    Ok(())
}

fn load_review_comment_signs() -> Result<(), String> {
    if let Err(e) = check_prerequisites() {
        notify_error(&e);
        return Err(e);
    }

    let token = get_gh_token().map_err(|e| match e {
        AuthError::GhNotFound => "gh CLI not found".to_string(),
        AuthError::NotAuthenticated => "Not authenticated with gh".to_string(),
        AuthError::IoError(msg) => format!("IO error: {}", msg),
    })?;

    setup_review_signs_autocmd()?;

    let cwd = std::env::current_dir().map_err(|e| format!("Failed to get cwd: {}", e))?;
    let (sender, receiver) = channel::<ReviewSignsFetchResult>();
    let receiver = Arc::new(Mutex::new(receiver));
    let receiver_clone = receiver.clone();

    let handle = AsyncHandle::new(move || {
        let _ = catch_unwind(AssertUnwindSafe(|| {
            if let Ok(result) = receiver_clone.lock().unwrap().try_recv() {
                match result {
                    ReviewSignsFetchResult::Success(state) => {
                        REVIEW_SIGNS.with(|cell| {
                            *cell.borrow_mut() = Some(state);
                        });
                        let _ = apply_review_comment_signs_current_buffer();
                    }
                    ReviewSignsFetchResult::Error(msg) => {
                        notify_error(&msg);
                    }
                }
            }
        }));
    })
    .map_err(|e| format!("Failed to create async handle: {}", e))?;

    let handle_clone = handle.clone();
    std::thread::spawn(move || {
        let result = match detect_pr() {
            Ok(pr) => match fetch_comments(&token, &pr.owner, &pr.repo, pr.number) {
                Ok(threads) => {
                    let state = ReviewSignsState {
                        cwd,
                        lines_by_path: collect_review_comment_lines(&threads),
                    };
                    ReviewSignsFetchResult::Success(state)
                }
                Err(e) => ReviewSignsFetchResult::Error(format!(
                    "Failed to fetch review comments for signs: {}",
                    e
                )),
            },
            Err(e) => {
                let msg = match e {
                    PrError::NotAGitRepo => "Not a git repository".to_string(),
                    PrError::GhError(err) => format!("gh error: {}", err),
                    PrError::NoAssociatedPr => "No PR associated with current branch".to_string(),
                    PrError::IoError(err) => format!("IO error: {}", err),
                    PrError::ParseError(err) => format!("Parse error: {}", err),
                };
                ReviewSignsFetchResult::Error(msg)
            }
        };

        let _ = sender.send(result);
        let _ = handle_clone.send();
    });

    Ok(())
}

fn apply_review_comment_signs() -> Result<(), String> {
    apply_review_comment_signs_current_buffer()
}

fn open_pr_review_comment() -> Result<(), String> {
    if let Err(e) = check_prerequisites() {
        notify_error(&e);
        return Err(e);
    }

    get_gh_token().map_err(|e| {
        let msg = match e {
            AuthError::GhNotFound => "gh CLI not found".to_string(),
            AuthError::NotAuthenticated => "Not authenticated with gh".to_string(),
            AuthError::IoError(msg) => format!("IO error: {}", msg),
        };
        notify_error(&msg);
        msg
    })?;

    api::command("lua require('neogh.review_comment').open()").map_err(|e| {
        let msg = format!("Failed to open PR review comment editor: {}", e);
        notify_error(&msg);
        msg
    })?;

    Ok(())
}

fn open_pr_review_submit(event: &str) -> Result<(), String> {
    if let Err(e) = check_prerequisites() {
        notify_error(&e);
        return Err(e);
    }

    get_gh_token().map_err(|e| {
        let msg = match e {
            AuthError::GhNotFound => "gh CLI not found".to_string(),
            AuthError::NotAuthenticated => "Not authenticated with gh".to_string(),
            AuthError::IoError(msg) => format!("IO error: {}", msg),
        };
        notify_error(&msg);
        msg
    })?;

    let cmd = format!("lua require('neogh.review_submit').open('{}')", event);
    api::command(&cmd).map_err(|e| {
        let msg = format!("Failed to open PR review submit editor: {}", e);
        notify_error(&msg);
        msg
    })?;

    Ok(())
}

fn open_thread_reply_editor(thread_id: &str) -> Result<(), String> {
    if let Err(e) = check_prerequisites() {
        notify_error(&e);
        return Err(e);
    }

    get_gh_token().map_err(|e| {
        let msg = match e {
            AuthError::GhNotFound => "gh CLI not found".to_string(),
            AuthError::NotAuthenticated => "Not authenticated with gh".to_string(),
            AuthError::IoError(msg) => format!("IO error: {}", msg),
        };
        notify_error(&msg);
        msg
    })?;

    let escaped = thread_id.replace("\\", "\\\\").replace("'", "\\'");
    let cmd = format!("lua require('neogh.thread_reply').open('{}')", escaped);
    api::command(&cmd).map_err(|e| {
        let msg = format!("Failed to open thread reply editor: {}", e);
        notify_error(&msg);
        msg
    })?;

    Ok(())
}

fn open_pending_review_sidebar() -> Result<(), String> {
    if let Err(e) = check_prerequisites() {
        notify_error(&e);
        return Err(e);
    }

    get_gh_token().map_err(|e| {
        let msg = match e {
            AuthError::GhNotFound => "gh CLI not found".to_string(),
            AuthError::NotAuthenticated => "Not authenticated with gh".to_string(),
            AuthError::IoError(msg) => format!("IO error: {}", msg),
        };
        notify_error(&msg);
        msg
    })?;

    open_with_mode(SidebarMode::PendingReview)
}

fn close_pending_edit_session(state: &mut PluginState) {
    if let Some(session) = state.pending_edit.take() {
        if session.win.is_valid() {
            let _ = api::set_current_win(&session.win);
            let _ = api::command("close");
        }
    }
}

fn edit_pending_comment() -> Result<(), String> {
    STATE.with(|state_cell| {
        let mut state_opt = match state_cell.try_borrow_mut() {
            Ok(guard) => guard,
            Err(_) => return Err("State temporarily unavailable".to_string()),
        };

        let state = match state_opt.as_mut() {
            Some(s) => s,
            None => {
                notify_error("Sidebar not open. Run :PRPendingComments first");
                return Err("Sidebar not open".to_string());
            }
        };

        if state.is_loading {
            notify_info("Still loading...");
            return Ok(());
        }
        if state.mode != SidebarMode::PendingReview && state.mode != SidebarMode::Comments {
            notify_info("Edit comment works in Comments and Pending Review sidebars");
            return Ok(());
        }

        let current_idx = state.navigator.current_index();
        let thread = match state.threads.get(current_idx) {
            Some(t) => t,
            None => {
                notify_error("No comment selected");
                return Ok(());
            }
        };

        let viewer_login = if state.mode == SidebarMode::Comments {
            Some(get_authenticated_login()?)
        } else {
            None
        };

        let (comment_node_id, body, kind) = match &thread.root {
            Comment::Review(rc) => {
                if let Some(ref login) = viewer_login {
                    if rc.user.login != *login {
                        notify_info("You can only edit your own comments");
                        return Ok(());
                    }
                }
                (
                    rc.node_id.clone().ok_or_else(|| "Review comment node ID is missing".to_string())?,
                    rc.body.clone(),
                    EditableCommentKind::Review,
                )
            }
            Comment::Issue(ic) => {
                if state.mode != SidebarMode::Comments {
                    notify_info("Selected item is not editable in this mode");
                    return Ok(());
                }
                if let Some(ref login) = viewer_login {
                    if ic.user.login != *login {
                        notify_info("You can only edit your own comments");
                        return Ok(());
                    }
                }
                (
                    ic.node_id.clone().ok_or_else(|| "Issue comment node ID is missing".to_string())?,
                    ic.body.clone(),
                    EditableCommentKind::Issue,
                )
            }
        };

        close_pending_edit_session(state);

        api::command("botright 8split").map_err(|e| format!("Failed to open editor split: {}", e))?;
        let win = Window::current();
        let mut buf = api::create_buf(false, true).map_err(|e| format!("Failed to create edit buffer: {}", e))?;
        api::set_current_buf(&buf).map_err(|e| format!("Failed to set edit buffer: {}", e))?;

        api::set_option_value(
            "buftype",
            "nofile",
            &OptionOpts::builder().buffer(buf.clone()).build(),
        )
        .map_err(|e| format!("Failed to set buftype: {}", e))?;
        api::set_option_value(
            "bufhidden",
            "wipe",
            &OptionOpts::builder().buffer(buf.clone()).build(),
        )
        .map_err(|e| format!("Failed to set bufhidden: {}", e))?;
        api::set_option_value(
            "swapfile",
            false,
            &OptionOpts::builder().buffer(buf.clone()).build(),
        )
        .map_err(|e| format!("Failed to set swapfile: {}", e))?;
        api::set_option_value(
            "filetype",
            "markdown",
            &OptionOpts::builder().buffer(buf.clone()).build(),
        )
        .map_err(|e| format!("Failed to set filetype: {}", e))?;
        api::set_option_value(
            "modifiable",
            true,
            &OptionOpts::builder().buffer(buf.clone()).build(),
        )
        .map_err(|e| format!("Failed to make buffer modifiable: {}", e))?;

        let lines: Vec<&str> = if body.is_empty() {
            vec![""]
        } else {
            body.lines().collect()
        };
        buf.set_lines(.., false, lines)
            .map_err(|e| format!("Failed to set edit lines: {}", e))?;

        let opts = SetKeymapOpts::builder()
            .noremap(true)
            .silent(true)
            .nowait(true)
            .build();
        buf.set_keymap(
            Mode::Normal,
            "q",
            "<Cmd>lua require('neogh').cancel_pending_comment_edit()<CR>",
            &opts,
        )
        .map_err(|e| format!("Failed to set keymap: {}", e))?;
        buf.set_keymap(
            Mode::Normal,
            "<C-s>",
            "<Cmd>lua require('neogh').submit_pending_comment_edit()<CR>",
            &opts,
        )
        .map_err(|e| format!("Failed to set keymap: {}", e))?;
        buf.set_keymap(
            Mode::Insert,
            "<C-s>",
            "<Cmd>lua require('neogh').submit_pending_comment_edit()<CR>",
            &opts,
        )
        .map_err(|e| format!("Failed to set keymap: {}", e))?;

        state.pending_edit = Some(PendingEditSession {
            bufnr: buf,
            win,
            comment_node_id,
            kind,
        });

        notify_info("Edit comment and press <C-s> to save (q to cancel)");
        Ok(())
    })
}

fn cancel_pending_comment_edit() -> Result<(), String> {
    STATE.with(|state_cell| {
        let mut state_opt = match state_cell.try_borrow_mut() {
            Ok(guard) => guard,
            Err(_) => return Err("State temporarily unavailable".to_string()),
        };
        if let Some(ref mut state) = *state_opt {
            close_pending_edit_session(state);
            Ok(())
        } else {
            Err("Sidebar not open".to_string())
        }
    })
}

fn submit_pending_comment_edit() -> Result<(), String> {
    let payload: Result<(String, EditableCommentKind, String, Option<PullRequest>, SidebarMode), String> =
        STATE.with(|state_cell| {
        let mut state_opt = match state_cell.try_borrow_mut() {
            Ok(guard) => guard,
            Err(_) => return Err("State temporarily unavailable".to_string()),
        };
        let state = state_opt
            .as_mut()
            .ok_or_else(|| "Sidebar not open".to_string())?;

        let session = state
            .pending_edit
            .as_ref()
            .ok_or_else(|| "No active pending comment edit session".to_string())?;

        let lines: Vec<String> = session
            .bufnr
            .get_lines(.., false)
            .map_err(|e| format!("Failed to read edit buffer: {}", e))?
            .map(|line| line.to_string())
            .collect();
        let body = lines.join("\n");
        if body.trim().is_empty() {
            return Err("Comment body cannot be empty".to_string());
        }

        let current_pr = state.current_pr.clone();
        let mode = state.mode;

        Ok((
            session.comment_node_id.clone(),
            session.kind,
            body,
            current_pr,
            mode,
        ))
    });
    let (comment_node_id, kind, body, current_pr, mode) = payload?;

    let token = get_gh_token().map_err(|e| format!("Auth error: {:?}", e))?;

    match kind {
        EditableCommentKind::Review => edit_pending_review_comment(&token, &comment_node_id, &body)
            .map_err(|e| format!("Failed to edit review comment: {}", e))?,
        EditableCommentKind::Issue => edit_issue_comment(&token, &comment_node_id, &body)
            .map_err(|e| format!("Failed to edit issue comment: {}", e))?,
    }

    STATE.with(|state_cell| {
        if let Ok(mut state_opt) = state_cell.try_borrow_mut() {
            if let Some(ref mut state) = *state_opt {
                close_pending_edit_session(state);
            }
        }
    });

    notify_info("Comment updated");
    if let Some(pr) = current_pr {
        if mode == SidebarMode::PendingReview {
            fetch_fresh_pending_comments(&pr);
        } else {
            fetch_fresh_comments(&pr);
        }
    }
    Ok(())
}

fn delete_pending_comment() -> Result<(), String> {
    let maybe_payload = STATE.with(|state_cell| {
        let mut state_opt = match state_cell.try_borrow_mut() {
            Ok(guard) => guard,
            Err(_) => return Err("State temporarily unavailable".to_string()),
        };

        let state = match state_opt.as_mut() {
            Some(s) => s,
            None => {
                notify_error("Sidebar not open. Run :PRPendingComments first");
                return Err("Sidebar not open".to_string());
            }
        };

        if state.is_loading {
            notify_info("Still loading...");
            return Ok(None);
        }
        if state.mode != SidebarMode::PendingReview && state.mode != SidebarMode::Comments {
            notify_info("Delete comment works in Comments and Pending Review sidebars");
            return Ok(None);
        }

        let current_idx = state.navigator.current_index();
        let thread = state
            .threads
            .get(current_idx)
            .ok_or_else(|| "No comment selected".to_string())?;

        let viewer_login = if state.mode == SidebarMode::Comments {
            Some(get_authenticated_login()?)
        } else {
            None
        };

        let (comment_id, kind) = match &thread.root {
            Comment::Review(rc) => {
                if let Some(ref login) = viewer_login {
                    if rc.user.login != *login {
                        notify_info("You can only delete your own comments");
                        return Ok(None);
                    }
                }
                (
                    rc.node_id
                        .clone()
                        .ok_or_else(|| "Review comment node ID is missing".to_string())?,
                    EditableCommentKind::Review,
                )
            }
            Comment::Issue(ic) => {
                if state.mode != SidebarMode::Comments {
                    notify_info("Selected item is not deletable in this mode");
                    return Ok(None);
                }
                if let Some(ref login) = viewer_login {
                    if ic.user.login != *login {
                        notify_info("You can only delete your own comments");
                        return Ok(None);
                    }
                }
                (
                    ic.node_id
                        .clone()
                        .ok_or_else(|| "Issue comment node ID is missing".to_string())?,
                    EditableCommentKind::Issue,
                )
            }
        };

        let current_pr = state
            .current_pr
            .clone()
            .ok_or_else(|| "No PR loaded".to_string())?;

        Ok(Some((
            comment_id,
            kind,
            state.mode,
            current_pr,
        )))
    })?;

    let Some((comment_id, kind, mode, current_pr)) = maybe_payload else {
        return Ok(());
    };

    let token = get_gh_token().map_err(|e| format!("Auth error: {:?}", e))?;

    match kind {
        EditableCommentKind::Review => delete_pending_review_comment(&token, &comment_id)
            .map_err(|e| format!("Failed to delete review comment: {}", e))?,
        EditableCommentKind::Issue => delete_issue_comment(&token, &comment_id)
            .map_err(|e| format!("Failed to delete issue comment: {}", e))?,
    }
    notify_info("Comment deleted");
    if mode == SidebarMode::PendingReview {
        fetch_fresh_pending_comments(&current_pr);
    } else {
        fetch_fresh_comments(&current_pr);
    }
    Ok(())
}

fn reply_to_thread() -> Result<(), String> {
    STATE.with(|state_cell| {
        let mut state_opt = match state_cell.try_borrow_mut() {
            Ok(guard) => guard,
            Err(_) => return Err("State temporarily unavailable".to_string()),
        };

        let state = match state_opt.as_mut() {
            Some(s) => s,
            None => {
                notify_error("Sidebar not open. Run :PRComments first");
                return Err("Sidebar not open".to_string());
            }
        };

        if state.is_loading {
            notify_info("Still loading...");
            return Ok(());
        }

        if state.mode != SidebarMode::Comments {
            notify_info("Thread reply only works in Comments mode");
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
                notify_info("Cannot reply to issue comments or threads without thread ID");
                return Ok(());
            }
        };

        open_thread_reply_editor(&thread_id)
    })
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
                                    state.current_pr = Some(current_pr.clone());
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

                                                let (sender, receiver) = channel::<WorkflowPrefetchResult>();
                                                let receiver = Arc::new(Mutex::new(receiver));
                                                let receiver_clone = receiver.clone();

                                                let prefetch_handle = AsyncHandle::new(move || {
                                                    let _ = catch_unwind(AssertUnwindSafe(|| {
                                                        if let Ok(result) = receiver_clone.lock().unwrap().try_recv() {
                                                            STATE.with(|state_cell| {
                                                                if let Ok(mut state_opt) = state_cell.try_borrow_mut() {
                                                                    if let Some(ref mut state) = *state_opt {
                                                                        match result {
                                                                            WorkflowPrefetchResult::Success { number, suites } => {
                                                                                state.pr_workflow_cache.insert(number, suites);
                                                                            }
                                                                            WorkflowPrefetchResult::Error { number, msg } => {
                                                                                eprintln!("Workflow prefetch failed for PR #{}: {}", number, msg);
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
                                                        let result = match fetch_check_runs(&token_clone, &owner, &repo, pr_number) {
                                                            Ok(suites) => WorkflowPrefetchResult::Success { number: pr_number, suites },
                                                            Err(e) => WorkflowPrefetchResult::Error { number: pr_number, msg: e.to_string() },
                                                        };
                                                        let _ = sender.send(result);
                                                        let _ = handle_clone.send();
                                                    });
                                                }
                                            }
                                        }
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

enum PendingFetchResult {
    Success {
        threads: Vec<CommentThread>,
        number: u64,
        current_pr: PullRequest,
    },
    Error(String),
}

fn spawn_pending_review_fetch(mut state: PluginState, token: String) -> Result<(), String> {
    let (sender, receiver) = channel::<PendingFetchResult>();
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
                                PendingFetchResult::Success {
                                    threads,
                                    number,
                                    current_pr,
                                } => {
                                    state.set_threads(threads, None);
                                    state.current_pr = Some(current_pr);
                                    state.pr_chain = None;

                                    let lines = state.buffer.render();
                                    if state.sidebar.set_lines(lines).is_ok() {
                                        if !state.navigator.is_empty() {
                                            if let Some(line) =
                                                state.buffer.line_for_thread(state.navigator.current_index())
                                            {
                                                let _ = state.sidebar.set_cursor(line + 1, 0);
                                            }
                                        }
                                        notify_info(&format!(
                                            "Loaded {} pending review comment(s) for PR #{}",
                                            state.threads.len(),
                                            number
                                        ));
                                    }
                                }
                                PendingFetchResult::Error(msg) => {
                                    state.is_loading = false;
                                    notify_error(&msg);
                                }
                            }
                        }
                    }
                });
            }
        }));
    })
    .map_err(|e| format!("Failed to create async handle: {}", e))?;

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
            Ok(pr) => match fetch_pending_review_comments(&token, &pr.owner, &pr.repo, pr.number) {
                Ok(threads) => PendingFetchResult::Success {
                    threads,
                    number: pr.number,
                    current_pr: pr,
                },
                Err(e) => {
                    PendingFetchResult::Error(format!("Failed to fetch pending review comments: {}", e))
                }
            },
            Err(e) => {
                let msg = match e {
                    PrError::NotAGitRepo => "Not a git repository".to_string(),
                    PrError::GhError(err) => format!("gh error: {}", err),
                    PrError::NoAssociatedPr => "No PR associated with current branch".to_string(),
                    PrError::IoError(err) => format!("IO error: {}", err),
                    PrError::ParseError(err) => format!("Parse error: {}", err),
                };
                PendingFetchResult::Error(msg)
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
                SidebarMode::PendingReview => {
                    if state.navigator.is_empty() {
                        notify_info("No pending comments to navigate");
                        return Ok(());
                    }
                    state.navigator.next();
                    if let Some(line) = state.buffer.line_for_thread(state.navigator.current_index()) {
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
                SidebarMode::PendingReview => {
                    if state.navigator.is_empty() {
                        notify_info("No pending comments to navigate");
                        return Ok(());
                    }
                    state.navigator.prev();
                    if let Some(line) = state.buffer.line_for_thread(state.navigator.current_index()) {
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
                SidebarMode::PendingReview => {
                    if state.navigator.is_empty() {
                        notify_info("No pending comments to jump to");
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
                                notify_info("Pending comment has no file location");
                            }
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
            if state.mode == SidebarMode::PendingReview {
                notify_info("Mode switching is disabled in pending review sidebar");
                return Ok(());
            }
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
                SidebarMode::PendingReview => state.buffer.render(),
            };
            state.sidebar.set_lines(lines).map_err(|e| format!("Failed to update buffer: {}", e))?;

            // Reset cursor position
            let line = match state.mode {
                SidebarMode::Comments => state.buffer.line_for_thread(state.navigator.current_index()),
                SidebarMode::Actions => state.actions_buffer.line_for_suite(state.actions_navigator.current_index()),
                SidebarMode::PendingReview => state.buffer.line_for_thread(state.navigator.current_index()),
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
                            SidebarMode::PendingReview => notify_info("Chain navigation is disabled in pending review sidebar"),
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
                            SidebarMode::PendingReview => notify_info("Chain navigation is disabled in pending review sidebar"),
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

fn fetch_fresh_pending_comments(current_pr: &PullRequest) {
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
                let loading_lines = render_loading_lines(SidebarMode::PendingReview);
                let _ = state.sidebar.set_lines(loading_lines);
            }
        }
    });

    let (sender, receiver) = channel::<PendingFetchResult>();
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
                                PendingFetchResult::Success {
                                    threads,
                                    number,
                                    current_pr,
                                } => {
                                    state.set_threads(threads, None);
                                    state.current_pr = Some(current_pr);
                                    state.pr_chain = None;
                                    let lines = state.buffer.render();
                                    if state.sidebar.set_lines(lines).is_ok() {
                                        if !state.navigator.is_empty() {
                                            if let Some(line) =
                                                state.buffer.line_for_thread(state.navigator.current_index())
                                            {
                                                let _ = state.sidebar.set_cursor(line + 1, 0);
                                            }
                                        }
                                        notify_info(&format!(
                                            "Refreshed {} pending review comment(s) for PR #{}",
                                            state.threads.len(),
                                            number
                                        ));
                                    }
                                }
                                PendingFetchResult::Error(msg) => {
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
        let result =
            match fetch_pending_review_comments(&token, &pr_clone.owner, &pr_clone.repo, pr_clone.number) {
                Ok(threads) => PendingFetchResult::Success {
                    threads,
                    number: pr_clone.number,
                    current_pr: pr_clone,
                },
                Err(e) => {
                    PendingFetchResult::Error(format!("Failed to fetch pending review comments: {}", e))
                }
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
            let mode = state.mode;

            drop(state_opt);

            match mode {
                SidebarMode::Comments => fetch_fresh_comments(&current_pr),
                SidebarMode::Actions => fetch_actions_for_current_pr(&current_pr),
                SidebarMode::PendingReview => fetch_fresh_pending_comments(&current_pr),
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

    let prreview_opts = CreateCommandOpts::builder()
        .desc("Pick an open PR for review")
        .build();

    let prreview_cmd = |_args: CommandArgs| {
        let _ = catch_unwind(AssertUnwindSafe(|| open_pr_review_picker()));
    };

    api::create_user_command("PRReview", prreview_cmd, &prreview_opts)?;

    let prreviewsigns_opts = CreateCommandOpts::builder()
        .desc("Load review comment signs for Diffview buffers")
        .build();
    let prreviewsigns_cmd = |_args: CommandArgs| {
        let _ = catch_unwind(AssertUnwindSafe(|| {
            if let Err(e) = load_review_comment_signs() {
                notify_error(&e);
            }
        }));
    };
    api::create_user_command("PRReviewSignsLoad", prreviewsigns_cmd, &prreviewsigns_opts)?;

    let prreview_comment_opts = CreateCommandOpts::builder()
        .desc("Add a PR review comment for selected Diffview lines")
        .range(CommandRange::CurrentLine)
        .build();

    let prreview_comment_cmd = |_args: CommandArgs| {
        let _ = catch_unwind(AssertUnwindSafe(|| open_pr_review_comment()));
    };

    api::create_user_command("PRReviewComment", prreview_comment_cmd, &prreview_comment_opts)?;

    let prreview_approve_opts = CreateCommandOpts::builder()
        .desc("Finish review with approve")
        .build();
    let prreview_approve_cmd = |_args: CommandArgs| {
        let _ = catch_unwind(AssertUnwindSafe(|| open_pr_review_submit("APPROVE")));
    };
    api::create_user_command("PRReviewApprove", prreview_approve_cmd, &prreview_approve_opts)?;

    let prreview_request_changes_opts = CreateCommandOpts::builder()
        .desc("Finish review with request changes")
        .build();
    let prreview_request_changes_cmd = |_args: CommandArgs| {
        let _ = catch_unwind(AssertUnwindSafe(|| open_pr_review_submit("REQUEST_CHANGES")));
    };
    api::create_user_command(
        "PRReviewRequestChanges",
        prreview_request_changes_cmd,
        &prreview_request_changes_opts,
    )?;

    let prreview_finish_comment_opts = CreateCommandOpts::builder()
        .desc("Finish review with comment")
        .build();
    let prreview_finish_comment_cmd = |_args: CommandArgs| {
        let _ = catch_unwind(AssertUnwindSafe(|| open_pr_review_submit("COMMENT")));
    };
    api::create_user_command(
        "PRReviewFinishComment",
        prreview_finish_comment_cmd,
        &prreview_finish_comment_opts,
    )?;

    let prreplythread_opts = CreateCommandOpts::builder()
        .desc("Reply to selected review thread in comments sidebar")
        .build();
    let prreplythread_cmd = |_args: CommandArgs| {
        let _ = catch_unwind(AssertUnwindSafe(|| reply_to_thread()));
    };
    api::create_user_command("PRReplyThread", prreplythread_cmd, &prreplythread_opts)?;

    let prpendingcomments_opts = CreateCommandOpts::builder()
        .desc("Open sidebar with pending review comments")
        .build();
    let prpendingcomments_cmd = |_args: CommandArgs| {
        let _ = catch_unwind(AssertUnwindSafe(|| open_pending_review_sidebar()));
    };
    api::create_user_command(
        "PRPendingComments",
        prpendingcomments_cmd,
        &prpendingcomments_opts,
    )?;

    let prpendingcommentedit_opts = CreateCommandOpts::builder()
        .desc("Edit selected comment (pending review or comments sidebar)")
        .build();
    let prpendingcommentedit_cmd = |_args: CommandArgs| {
        let _ = catch_unwind(AssertUnwindSafe(|| {
            if let Err(e) = edit_pending_comment() {
                notify_error(&e);
            }
        }));
    };
    api::create_user_command(
        "PRPendingCommentEdit",
        prpendingcommentedit_cmd,
        &prpendingcommentedit_opts,
    )?;
    let prcommentedit_cmd = |_args: CommandArgs| {
        let _ = catch_unwind(AssertUnwindSafe(|| {
            if let Err(e) = edit_pending_comment() {
                notify_error(&e);
            }
        }));
    };
    api::create_user_command(
        "PRCommentEdit",
        prcommentedit_cmd,
        &prpendingcommentedit_opts,
    )?;

    let prpendingcommentdelete_opts = CreateCommandOpts::builder()
        .desc("Delete selected comment (pending review or comments sidebar)")
        .build();
    let prpendingcommentdelete_cmd = |_args: CommandArgs| {
        let _ = catch_unwind(AssertUnwindSafe(|| {
            if let Err(e) = delete_pending_comment() {
                notify_error(&e);
            }
        }));
    };
    api::create_user_command(
        "PRPendingCommentDelete",
        prpendingcommentdelete_cmd,
        &prpendingcommentdelete_opts,
    )?;
    let prcommentdelete_cmd = |_args: CommandArgs| {
        let _ = catch_unwind(AssertUnwindSafe(|| {
            if let Err(e) = delete_pending_comment() {
                notify_error(&e);
            }
        }));
    };
    api::create_user_command(
        "PRCommentDelete",
        prcommentdelete_cmd,
        &prpendingcommentdelete_opts,
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
    let review_pr_fn: Function<(), ()> = Function::from_fn(|_| {
        let _ = catch_unwind(AssertUnwindSafe(|| {
            if let Err(e) = open_pr_review_picker() {
                notify_error(&e);
            }
        }));
    });
    let load_review_signs_fn: Function<(), ()> = Function::from_fn(|_| {
        let _ = catch_unwind(AssertUnwindSafe(|| {
            if let Err(e) = load_review_comment_signs() {
                notify_error(&e);
            }
        }));
    });
    let apply_review_signs_fn: Function<(), ()> = Function::from_fn(|_| {
        let _ = catch_unwind(AssertUnwindSafe(|| {
            if let Err(e) = apply_review_comment_signs() {
                notify_error(&e);
            }
        }));
    });
    let review_comment_fn: Function<(), ()> = Function::from_fn(|_| {
        let _ = catch_unwind(AssertUnwindSafe(|| {
            if let Err(e) = open_pr_review_comment() {
                notify_error(&e);
            }
        }));
    });
    let review_approve_fn: Function<(), ()> = Function::from_fn(|_| {
        let _ = catch_unwind(AssertUnwindSafe(|| {
            if let Err(e) = open_pr_review_submit("APPROVE") {
                notify_error(&e);
            }
        }));
    });
    let review_request_changes_fn: Function<(), ()> = Function::from_fn(|_| {
        let _ = catch_unwind(AssertUnwindSafe(|| {
            if let Err(e) = open_pr_review_submit("REQUEST_CHANGES") {
                notify_error(&e);
            }
        }));
    });
    let review_finish_comment_fn: Function<(), ()> = Function::from_fn(|_| {
        let _ = catch_unwind(AssertUnwindSafe(|| {
            if let Err(e) = open_pr_review_submit("COMMENT") {
                notify_error(&e);
            }
        }));
    });
    let reply_to_thread_fn: Function<(), ()> = Function::from_fn(|_| {
        let _ = catch_unwind(AssertUnwindSafe(|| {
            if let Err(e) = reply_to_thread() {
                notify_error(&e);
            }
        }));
    });
    let open_pending_comments_fn: Function<(), ()> = Function::from_fn(|_| {
        let _ = catch_unwind(AssertUnwindSafe(|| {
            if let Err(e) = open_pending_review_sidebar() {
                notify_error(&e);
            }
        }));
    });
    let edit_pending_comment_fn: Function<(), ()> = Function::from_fn(|_| {
        let _ = catch_unwind(AssertUnwindSafe(|| {
            if let Err(e) = edit_pending_comment() {
                notify_error(&e);
            }
        }));
    });
    let delete_pending_comment_fn: Function<(), ()> = Function::from_fn(|_| {
        let _ = catch_unwind(AssertUnwindSafe(|| {
            if let Err(e) = delete_pending_comment() {
                notify_error(&e);
            }
        }));
    });
    let submit_pending_comment_edit_fn: Function<(), ()> = Function::from_fn(|_| {
        let _ = catch_unwind(AssertUnwindSafe(|| {
            if let Err(e) = submit_pending_comment_edit() {
                notify_error(&e);
            }
        }));
    });
    let cancel_pending_comment_edit_fn: Function<(), ()> = Function::from_fn(|_| {
        let _ = catch_unwind(AssertUnwindSafe(|| {
            if let Err(e) = cancel_pending_comment_edit() {
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
        ("review_pr", Object::from(review_pr_fn)),
        ("load_review_signs", Object::from(load_review_signs_fn)),
        ("apply_review_signs", Object::from(apply_review_signs_fn)),
        ("review_comment", Object::from(review_comment_fn)),
        ("review_approve", Object::from(review_approve_fn)),
        ("review_request_changes", Object::from(review_request_changes_fn)),
        ("review_finish_comment", Object::from(review_finish_comment_fn)),
        ("reply_to_thread", Object::from(reply_to_thread_fn)),
        ("open_pending_comments", Object::from(open_pending_comments_fn)),
        ("edit_pending_comment", Object::from(edit_pending_comment_fn)),
        ("delete_pending_comment", Object::from(delete_pending_comment_fn)),
        ("submit_pending_comment_edit", Object::from(submit_pending_comment_edit_fn)),
        ("cancel_pending_comment_edit", Object::from(cancel_pending_comment_edit_fn)),
    ]))
}
