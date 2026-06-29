#![allow(clippy::arc_with_non_send_sync)]
pub mod application;
pub mod daemon;
pub mod domain;
pub mod infrastructure;
pub mod interfaces;

use crate::infrastructure::config::AppConfig;

fn main() -> anyhow::Result<()> {
    // Load configuration, then run the daemon on a single-threaded runtime +
    // LocalSet (the SQLite-backed services are !Sync; the loops are !Send and
    // run cooperatively, never touching a connection in parallel).
    let config = AppConfig::load()?;

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let local = tokio::task::LocalSet::new();
    local.block_on(&rt, crate::daemon::run(config))
}
