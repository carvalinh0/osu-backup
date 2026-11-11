/// Nomes de processo do osu!lazer (Windows e Linux).
const OSU_PROCESS_NAMES: &[&str] = &["osu!.exe", "osu!", "osu!lazer", "osu!lazer.exe"];

/// Fase do acompanhamento do jogo.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// Esperando o osu! abrir.
    Waiting,
    /// osu! está aberto.
    Playing,
}

/// Evento produzido por um poll.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    None,
    /// osu! foi detectado pela primeira vez.
    Started,
    /// osu! fechou: hora de fazer o backup.
    Closed,
}

pub fn is_osu_process(name: &str) -> bool {
    OSU_PROCESS_NAMES
        .iter()
        .any(|n| name.eq_ignore_ascii_case(n))
}

/// Função pura: dado o estado atual e se o osu! está rodando, devolve o novo estado e o evento.
pub fn step(phase: Phase, osu_running: bool) -> (Phase, Event) {
    match (phase, osu_running) {
        (Phase::Waiting, true) => (Phase::Playing, Event::Started),
        (Phase::Playing, false) => (Phase::Waiting, Event::Closed),
        (p, _) => (p, Event::None),
    }
}

/// Verifica no sistema se algum processo do osu! está ativo.
pub fn osu_is_running(sys: &mut sysinfo::System) -> bool {
    sys.refresh_processes_specifics(
        sysinfo::ProcessesToUpdate::All,
        true,
        sysinfo::ProcessRefreshKind::nothing(),
    );
    sys.processes()
        .values()
        .any(|p| is_osu_process(&p.name().to_string_lossy()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_osu_process_names() {
        assert!(is_osu_process("osu!.exe"));
        assert!(is_osu_process("osu!"));
        assert!(is_osu_process("OSU!.EXE"));
        assert!(!is_osu_process("firefox"));
        assert!(!is_osu_process("osu-backup"));
    }

    #[test]
    fn state_machine_fires_backup_on_close() {
        let (p, e) = step(Phase::Waiting, false);
        assert_eq!((p, e), (Phase::Waiting, Event::None));

        let (p, e) = step(p, true);
        assert_eq!((p, e), (Phase::Playing, Event::Started));

        let (p, e) = step(p, true);
        assert_eq!((p, e), (Phase::Playing, Event::None));

        let (p, e) = step(p, false);
        assert_eq!((p, e), (Phase::Waiting, Event::Closed));
    }
}
