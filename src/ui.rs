//! Ícone na bandeja + loop de eventos. Thread de fundo observa o osu! e dispara o backup.

use std::net::TcpListener;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use tao::event::{Event, StartCause};
use tao::event_loop::{ControlFlow, EventLoopBuilder, EventLoopProxy};
use tray_icon::menu::{Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};

use crate::config::{self, Config};
use crate::setup;
use crate::sync;
use crate::watcher::{self, Event as WatchEvent, Phase};

const POLL_INTERVAL: Duration = Duration::from_secs(3);
/// Espera o osu! terminar de gravar o realm antes de copiar.
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
        tracing::error!("não consegui iniciar o GTK (necessário para a bandeja): {e}");
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

    let status_item = MenuItem::new("Iniciando…", false, None);
    let menu = Menu::new();
    let _ = menu.append_items(&[
        &status_item,
        &PredefinedMenuItem::separator(),
        &MenuItem::with_id(ID_BACKUP, "Fazer backup agora", true, None),
        &MenuItem::with_id(ID_PICK, "Escolher pasta do osu!", true, None),
        &MenuItem::with_id(ID_LOGIN, "Reconectar ao Google", true, None),
        &PredefinedMenuItem::separator(),
        &MenuItem::with_id(ID_QUIT, "Sair", true, None),
    ]);

    let busy = Arc::new(AtomicBool::new(false));
    let mut tray: Option<TrayIcon> = None;

    if !config::is_setup_complete(&Config::load().unwrap_or_default()) {
        start_setup(&proxy, &busy, false);
    }

    event_loop.run(move |event, _target, control_flow| {
        *control_flow = ControlFlow::WaitUntil(Instant::now() + Duration::from_millis(200));

        match event {
            Event::NewEvents(StartCause::Init) => {
                match TrayIconBuilder::new()
                    .with_menu(Box::new(menu.clone()))
                    .with_tooltip("osu!backup")
                    .with_icon(make_icon())
                    .build()
                {
                    Ok(t) => tray = Some(t),
                    Err(e) => {
                        tracing::error!("erro criando ícone da bandeja: {e}");
                        *control_flow = ControlFlow::Exit;
                    }
                }
            }

            Event::UserEvent(ue) => match ue {
                UserEvent::Status(text) => {
                    status_item.set_text(&text);
                    if let Some(t) = &tray {
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
                UserEvent::OsuClosed => {
                    start_backup(&proxy, &busy, true);
                }
                UserEvent::JobFinished { quit } => {
                    if quit {
                        *control_flow = ControlFlow::Exit;
                    }
                }
                UserEvent::Menu(id) => match id.as_ref() {
                    ID_BACKUP => start_backup(&proxy, &busy, false),
                    ID_PICK => start_pick_folder(&proxy, &busy),
                    ID_LOGIN => start_setup(&proxy, &busy, true),
                    ID_QUIT => *control_flow = ControlFlow::Exit,
                    _ => {}
                },
            },

            Event::MainEventsCleared => {
                // O tray no Linux usa GTK: é preciso bombear o loop do GTK.
                #[cfg(target_os = "linux")]
                while gtk::events_pending() {
                    gtk::main_iteration_do(false);
                }
            }

            _ => {}
        }
    })
}

/// Observa o processo do osu! e envia eventos ao loop principal.
fn spawn_watcher(proxy: EventLoopProxy<UserEvent>) {
    thread::spawn(move || {
        let Ok(mut sys) = sysinfo::System::new() else {
            tracing::error!("não consegui inicializar a leitura de processos");
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
                    let _ = proxy.send_event(UserEvent::Status("osu! aberto".into()));
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

/// Tenta marcar a aplicação como ocupada. Retorna false se já houver um job rodando.
fn acquire(busy: &Arc<AtomicBool>, proxy: &EventLoopProxy<UserEvent>) -> bool {
    if busy
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_ok()
    {
        true
    } else {
        let _ = proxy.send_event(UserEvent::Status("Aguarde a tarefa atual".into()));
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
            let _ = proxy.send_event(UserEvent::Status("Configuração pendente".into()));
            let _ = proxy.send_event(UserEvent::Toast(
                "Configure o app (credenciais, login e pasta) antes do primeiro backup.".into(),
            ));
        } else {
            if after_close {
                thread::sleep(SETTLE_DELAY);
            }
            let _ = proxy.send_event(UserEvent::Status("Fazendo backup…".into()));
            match sync::run(&cfg) {
                Ok(r) => {
                    let msg = if r.uploaded == 0 {
                        "Backup em dia — nada novo".to_string()
                    } else {
                        format!("Backup concluído: {} arquivo(s) enviado(s)", r.uploaded)
                    };
                    tracing::info!("{msg}");
                    let _ = proxy.send_event(UserEvent::Status(msg.clone()));
                    let _ = proxy.send_event(UserEvent::Toast(msg));
                    quit = after_close;
                }
                Err(e) => {
                    tracing::error!("falha no backup: {e:#}");
                    let _ = proxy.send_event(UserEvent::Status("Falha no backup".into()));
                    let _ = proxy.send_event(UserEvent::Toast(format!("Falha no backup: {e:#}")));
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
                let _ = proxy.send_event(UserEvent::Status("Configurado — pronto".into()));
                let _ = proxy.send_event(UserEvent::Toast("Configuração concluída.".into()));
            }
            Err(e) => {
                tracing::error!("configuração falhou: {e:#}");
                let _ = proxy.send_event(UserEvent::Status("Configuração incompleta".into()));
                let _ = proxy.send_event(UserEvent::Toast(format!("Configuração falhou: {e:#}")));
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
        let mut dialog = rfd::FileDialog::new().set_title("Pasta de dados do osu!lazer");
        if let Some(p) = start {
            dialog = dialog.set_directory(p);
        }
        if let Some(picked) = dialog.pick_folder() {
            cfg.osu_dir = Some(picked.clone());
            match cfg.save() {
                Ok(()) => {
                    let _ =
                        proxy.send_event(UserEvent::Status(format!("Pasta: {}", picked.display())));
                }
                Err(e) => {
                    let _ =
                        proxy.send_event(UserEvent::Toast(format!("Erro salvando config: {e:#}")));
                }
            }
        }
        busy.store(false, Ordering::SeqCst);
        let _ = proxy.send_event(UserEvent::JobFinished { quit: false });
    });
}

/// Ícone simples gerado em código (círculo rosa), para não depender de arquivos externos.
fn make_icon() -> Icon {
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
    Icon::from_rgba(rgba, size, size).expect("ícone inválido")
}
