//! Actions mode specific logic

use crate::github::pr::{PrChain, PullRequest};
use crate::github::{detect_chain, fetch_check_runs, get_gh_token, CheckSuite};
use crate::types::SidebarMode;
use crate::ui::ActionsBuffer;
use nvim_oxi::api::{self, opts::*, types::*};
use nvim_oxi::libuv::AsyncHandle;
use std::collections::HashMap;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::mpsc::channel;
use std::sync::{Arc, Mutex};

/// Simple navigator for actions mode
pub struct ActionsNavigator {
    current_index: usize,
    count: usize,
}

impl ActionsNavigator {
    pub fn new(count: usize) -> Self {
        Self {
            current_index: 0,
            count,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    pub fn current_index(&self) -> usize {
        self.current_index
    }

    pub fn set_index(&mut self, index: usize) {
        if index < self.count {
            self.current_index = index;
        }
    }

    pub fn next(&mut self) {
        if self.current_index + 1 < self.count {
            self.current_index += 1;
        }
    }

    pub fn prev(&mut self) {
        if self.current_index > 0 {
            self.current_index -= 1;
        }
    }
}

pub enum ActionsFetchResult {
    Success {
        suites: Vec<CheckSuite>,
        number: u64,
        current_pr: PullRequest,
        pr_chain: Option<PrChain>,
    },
    Error(String),
}

pub enum ChainActionsFetchResult {
    Success {
        suites: Vec<CheckSuite>,
        number: u64,
        new_index: usize,
    },
    Error(String),
}

pub enum WorkflowPrefetchResult {
    Success {
        number: u64,
        suites: Vec<CheckSuite>,
    },
    Error {
        number: u64,
        msg: String,
    },
}
