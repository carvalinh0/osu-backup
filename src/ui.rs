//! Tray icon + event loop. A background thread watches osu! and triggers the backup.

use std::net::TcpListener;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use tao::event::{Event, StartCause};
use tao::event_loop::{ControlFlow, EventLoopBuilder, EventLoopProxy};
use tray_icon::menu::{Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};

use crate::config::{self, Config};
use crate::setup;
use crate::sync;
use crate::watcher::{self, Event as WatchEvent, Phase};

const POLL_INTERVAL: Duration = Duration::from_secs(3);
/// Waits for osu! to finish writing the realm before copying it.
const SETTLE_DELAY: Duration = Duration::from_secs(5);

#[derive(Debug)]
pub enum UserEvent {
    Status(String),
    Toast(String),
    OsuStarted,
    OsuClosed,
    Menu(MenuId),
    JobFinished { quit: bool },
}

const ID_BACKUP: &str = "backup";
const ID_PICK: &str = "pick";
const ID_LOGIN: &str = "login";
const ID_QUIT: &str = "quit";

pub fn run(_instance_lock: TcpListener) -> ! {
    #[cfg(target_os = "linux")]
    if let Err(e) = gtk::init() {
        tracing::error!("could not initialize GTK (required for the tray): {e}");
        std::process::exit(1);
    }

    let event_loop = EventLoopBuilder::<UserEvent>::with_user_event().build();
    let proxy = event_loop.create_proxy();

    {
        let p = proxy.clone();
        MenuEvent::set_event_handler(Some(move |e: MenuEvent| {
            let _ = p.send_event(UserEvent::Menu(e.id));
        }));
    }

    spawn_watcher(proxy.clone());

    let status_item = MenuItem::new("Starting…", false, None);
    let menu = Menu::new();
    let _ = menu.append_items(&[
        &status_item,
        &PredefinedMenuItem::separator(),
        &MenuItem::with_id(ID_BACKUP, "Back up now", true, None),
        &MenuItem::with_id(ID_PICK, "Choose osu! folder", true, None),
        &MenuItem::with_id(ID_LOGIN, "Reconnect to Google", true, None),
        &PredefinedMenuItem::separator(),
        &MenuItem::with_id(ID_QUIT, "Quit", true, None),
    ]);

    let busy = Arc::new(AtomicBool::new(false));
    let mut tray: Option<TrayIcon> = None;

    if !config::is_setup_complete(&Config::load().unwrap_or_default()) {
        start_setup(&proxy, &busy, false);
    }

    event_loop.run(move |event, _target, control_flow| {
        *control_flow = ControlFlow::WaitUntil(Instant::now() + Duration::from_millis(200));

        match event {
            Event::NewEvents(StartCause::Init) => match build_tray(&menu) {
                Ok(t) => tray = Some(t),
                Err(e) => {
                    tracing::error!("error creating tray icon: {e:#}");
                    *control_flow = ControlFlow::Exit;
                }
            },

            Event::UserEvent(ue) => {
                if handle_user_event(ue, &status_item, tray.as_ref(), &proxy, &busy) {
                    *control_flow = ControlFlow::Exit;
                }
            }

            Event::MainEventsCleared => {
                // The Linux tray uses GTK, so the GTK loop has to be pumped.
                #[cfg(target_os = "linux")]
                while gtk::events_pending() {
                    gtk::main_iteration_do(false);
                }
            }

            _ => {}
        }
    })
}

fn build_tray(menu: &Menu) -> Result<TrayIcon> {
    TrayIconBuilder::new()
        .with_menu(Box::new(menu.clone()))
        .with_tooltip("osu!backup")
        .with_icon(make_icon()?)
        .build()
        .context("building tray icon")
}

/// Handles one app event. Returns true when the app should exit.
fn handle_user_event(
    event: UserEvent,
    status_item: &MenuItem,
    tray: Option<&TrayIcon>,
    proxy: &EventLoopProxy<UserEvent>,
    busy: &Arc<AtomicBool>,
) -> bool {
    match event {
        UserEvent::Status(text) => {
            status_item.set_text(&text);
            if let Some(t) = tray {
                let _ = t.set_tooltip(Some(format!("osu!backup — {text}")));
            }
        }
        UserEvent::Toast(text) => {
            let _ = notify_rust::Notification::new()
                .appname("osu!backup")
                .summary("osu!backup")
                .body(&text)
                .show();
        }
        UserEvent::OsuStarted => {}
        UserEvent::OsuClosed => start_backup(proxy, busy, true),
        UserEvent::JobFinished { quit } => return quit,
        UserEvent::Menu(id) => return handle_menu(id.as_ref(), proxy, busy),
    }
    false
}

/// Runs the action for a tray menu item. Returns true when the app should exit.
fn handle_menu(id: &str, proxy: &EventLoopProxy<UserEvent>, busy: &Arc<AtomicBool>) -> bool {
    match id {
        ID_BACKUP => start_backup(proxy, busy, false),
        ID_PICK => start_pick_folder(proxy, busy),
        ID_LOGIN => start_setup(proxy, busy, true),
        ID_QUIT => return true,
        _ => {}
    }
    false
}

/// Watches the osu! process and sends events to the main loop.
fn spawn_watcher(proxy: EventLoopProxy<UserEvent>) {
    thread::spawn(move || {
        let Ok(mut sys) = sysinfo::System::new() else {
            tracing::error!("could not initialize process monitoring");
            return;
        };
        let mut phase = Phase::Waiting;
        loop {
            let running = watcher::osu_is_running(&mut sys);
            let (next, event) = watcher::step(phase, running);
            phase = next;
            match event {
                WatchEvent::Started => {
                    let _ = proxy.send_event(UserEvent::OsuStarted);
                    let _ = proxy.send_event(UserEvent::Status("osu! is running".into()));
                }
                WatchEvent::Closed => {
                    let _ = proxy.send_event(UserEvent::OsuClosed);
                }
                WatchEvent::None => {}
            }
            thread::sleep(POLL_INTERVAL);
        }
    });
}

/// Tries to mark the app as busy. Returns false if a job is already running.
fn acquire(busy: &Arc<AtomicBool>, proxy: &EventLoopProxy<UserEvent>) -> bool {
    if busy
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_ok()
    {
        true
    } else {
        let _ = proxy.send_event(UserEvent::Status("Wait for the current task".into()));
        false
    }
}

fn start_backup(proxy: &EventLoopProxy<UserEvent>, busy: &Arc<AtomicBool>, after_close: bool) {
    if !acquire(busy, proxy) {
        return;
    }
    let proxy = proxy.clone();
    let busy = busy.clone();
    thread::spawn(move || {
        let mut quit = false;
        let cfg = Config::load().unwrap_or_default();
        if !config::is_setup_complete(&cfg) {
            let _ = proxy.send_event(UserEvent::Status("Setup pending".into()));
            let _ = proxy.send_event(UserEvent::Toast(
                "Set up the app (credentials, sign-in and folder) before the first backup.".into(),
            ));
        } else {
            if after_close {
                thread::sleep(SETTLE_DELAY);
            }
            let _ = proxy.send_event(UserEvent::Status("Backing up…".into()));
            match sync::run(&cfg) {
                Ok(r) => {
                    let msg = if r.uploaded == 0 {
                        "Backup up to date — nothing new".to_string()
                    } else {
                        format!("Backup complete: {} file(s) uploaded", r.uploaded)
                    };
                    tracing::info!("{msg}");
                    let _ = proxy.send_event(UserEvent::Status(msg.clone()));
                    let _ = proxy.send_event(UserEvent::Toast(msg));
                    quit = after_close;
                }
                Err(e) => {
                    tracing::error!("backup failed: {e:#}");
                    let _ = proxy.send_event(UserEvent::Status("Backup failed".into()));
                    let _ = proxy.send_event(UserEvent::Toast(format!("Backup failed: {e:#}")));
                }
            }
        }
        busy.store(false, Ordering::SeqCst);
        let _ = proxy.send_event(UserEvent::JobFinished { quit });
    });
}

fn start_setup(proxy: &EventLoopProxy<UserEvent>, busy: &Arc<AtomicBool>, force_login: bool) {
    if !acquire(busy, proxy) {
        return;
    }
    let proxy = proxy.clone();
    let busy = busy.clone();
    thread::spawn(move || {
        let notify = setup::status_notifier(proxy.clone());
        match setup::run(force_login, &notify) {
            Ok(()) => {
                let _ = proxy.send_event(UserEvent::Status("Set up — ready".into()));
                let _ = proxy.send_event(UserEvent::Toast("Setup complete.".into()));
            }
            Err(e) => {
                tracing::error!("setup failed: {e:#}");
                let _ = proxy.send_event(UserEvent::Status("Setup incomplete".into()));
                let _ = proxy.send_event(UserEvent::Toast(format!("Setup failed: {e:#}")));
            }
        }
        busy.store(false, Ordering::SeqCst);
        let _ = proxy.send_event(UserEvent::JobFinished { quit: false });
    });
}

fn start_pick_folder(proxy: &EventLoopProxy<UserEvent>, busy: &Arc<AtomicBool>) {
    if !acquire(busy, proxy) {
        return;
    }
    let proxy = proxy.clone();
    let busy = busy.clone();
    thread::spawn(move || {
        let mut cfg = Config::load().unwrap_or_default();
        let start = cfg.osu_dir_or_default();
        let mut dialog = rfd::FileDialog::new().set_title("osu!lazer data folder");
        if let Some(p) = start {
            dialog = dialog.set_directory(p);
        }
        if let Some(picked) = dialog.pick_folder() {
            cfg.osu_dir = Some(picked.clone());
            match cfg.save() {
                Ok(()) => {
                    let _ = proxy
                        .send_event(UserEvent::Status(format!("Folder: {}", picked.display())));
                }
                Err(e) => {
                    let _ =
                        proxy.send_event(UserEvent::Toast(format!("Error saving config: {e:#}")));
                }
            }
        }
        busy.store(false, Ordering::SeqCst);
        let _ = proxy.send_event(UserEvent::JobFinished { quit: false });
    });
}

/// Simple icon generated in code (pink circle), so no external files are needed.
fn make_icon() -> Result<Icon> {
    let size: u32 = 32;
    let mut rgba = Vec::with_capacity((size * size * 4) as usize);
    let c = (size as f32 - 1.0) / 2.0;
    for y in 0..size {
        for x in 0..size {
            let d = ((x as f32 - c).powi(2) + (y as f32 - c).powi(2)).sqrt();
            if d <= c {
                rgba.extend_from_slice(&[255, 102, 170, 255]);
            } else {
                rgba.extend_from_slice(&[0, 0, 0, 0]);
            }
        }
    }
    Icon::from_rgba(rgba, size, size).context("generating tray icon")
}
