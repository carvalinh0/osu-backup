/// osu!lazer process names (Windows and Linux).
const OSU_PROCESS_NAMES: &[&str] = &["osu!.exe", "osu!", "osu!lazer", "osu!lazer.exe"];

/// Game tracking phase.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// Waiting for osu! to open.
    Waiting,
    /// osu! is open.
    Playing,
}

/// Event produced by a poll.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    None,
    /// osu! was detected for the first time.
    Started,
    /// osu! closed: time to back up.
    Closed,
}

pub fn is_osu_process(name: &str) -> bool {
    OSU_PROCESS_NAMES
        .iter()
        .any(|n| name.eq_ignore_ascii_case(n))
}

/// Pure function: given the current phase and whether osu! is running, returns the new phase and the event.
pub fn step(phase: Phase, osu_running: bool) -> (Phase, Event) {
    match (phase, osu_running) {
        (Phase::Waiting, true) => (Phase::Playing, Event::Started),
        (Phase::Playing, false) => (Phase::Waiting, Event::Closed),
        (p, _) => (p, Event::None),
    }
}

/// Checks whether any osu! process is running.
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
