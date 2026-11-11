//! osu!backup — backup do osu!lazer para o Google Drive.
//! Fica na bandeja, detecta quando o osu! fecha e envia as mudanças.

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

/// Porta usada apenas como trava de instância única (só existe enquanto o app roda).
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
            tracing::warn!("osu!backup já está em execução");
            return Ok(());
        }
    };

    ui::run(lock)
}
