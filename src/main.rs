//! osu!backup — backs up osu!lazer to Google Drive.
//! Lives in the tray, detects when osu! closes and uploads the changes.

#![cfg_attr(
    all(target_os = "windows", not(debug_assertions)),
    windows_subsystem = "windows"
)]

mod auth;
mod backup;
mod config;
mod drive;
mod setup;
mod sync;
mod ui;
mod watcher;

use std::net::TcpListener;

use anyhow::Result;

/// Port used only as a single-instance lock (held only while the app runs).
const INSTANCE_LOCK_PORT: u16 = 47319;

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "osubackup=info".into()),
        )
        .init();

    let lock = match TcpListener::bind(("127.0.0.1", INSTANCE_LOCK_PORT)) {
        Ok(l) => l,
        Err(_) => {
            tracing::warn!("osu!backup is already running");
            return Ok(());
        }
    };

    ui::run(lock)
}
