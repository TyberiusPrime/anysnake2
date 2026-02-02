pub mod util;

#[allow(unused_imports)]
use log::{debug, error, info, trace, warn};
use std::sync::{atomic::AtomicBool, atomic::Ordering, Arc, OnceLock};

use anyhow::Result;
use lazy_static::lazy_static;

lazy_static! {
    /// whether ctrl-c can terminate us right now.
    static ref CTRL_C_ALLOWED: Arc<AtomicBool> = Arc::new(AtomicBool::new(true));
}

lazy_static! {
    ///Set after reading the config so we can call nix shell .. from anywhere
    static ref OUTSIDE_NIXPKGS_URL: OnceLock<String> = OnceLock::new();
}

pub fn install_ctrl_c_handler() -> Result<()> {
    let c = CTRL_C_ALLOWED.clone();
    Ok(ctrlc::set_handler(move || {
        if c.load(Ordering::Relaxed) {
            error!("anysnake aborted");
            std::process::exit(1);
        }
    })?)
}

pub fn define_outside_nipkgs_url(url: String) {
    OUTSIDE_NIXPKGS_URL
        .set(url)
        .expect("Trying to set outside nixpkgs url twice");
}

pub fn get_outside_nixpkgs_url() -> Option<&'static str> {
    OUTSIDE_NIXPKGS_URL.get().map(|s| s.as_str())
}

pub fn run_without_ctrl_c<T>(func: impl Fn() -> Result<T>) -> Result<T> {
    CTRL_C_ALLOWED.store(false, Ordering::SeqCst);
    let res = func();
    CTRL_C_ALLOWED.store(true, Ordering::SeqCst);
    res
}

pub fn safe_python_package_name(input: &str) -> String {
    input.replace('_', "-")
}

#[derive(Debug)]
pub struct ErrorWithExitCode {
    pub msg: String,
    pub exit_code: i32,
}

impl ErrorWithExitCode {
    pub fn new(exit_code: i32, msg: String) -> Self {
        ErrorWithExitCode { msg, exit_code }
    }
}

impl std::fmt::Display for ErrorWithExitCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.msg)
    }
}
