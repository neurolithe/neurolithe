#![allow(clippy::arc_with_non_send_sync)]
pub mod application;
pub mod daemon;
pub mod domain;
pub mod infrastructure;
pub mod interfaces;

use crate::infrastructure::config::AppConfig;

fn main() -> anyhow::Result<()> {
    let config = AppConfig::load()?;

    // `neurolithe mcp` serves only the MCP tools over stdio (for a client
    // session that shares the live stores); no args runs the full daemon.
    let mcp_only = std::env::args().nth(1).as_deref() == Some("mcp");

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let local = tokio::task::LocalSet::new();
    if mcp_only {
        local.block_on(&rt, crate::daemon::run_mcp(config))
    } else {
        // Full daemon on a single-threaded runtime + LocalSet (the SQLite-backed
        // services are !Sync; the loops are !Send and run cooperatively).
        local.block_on(&rt, crate::daemon::run(config))
    }
}
