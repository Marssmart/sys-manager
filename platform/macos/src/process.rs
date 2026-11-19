//! Process listing and termination for macOS.

use std::io;
use std::path::Path;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread;
use std::time::{Duration, UNIX_EPOCH};

use sys_manager_common::process::{check_kill_allowed, KillError, ProcessInfo};
use sysinfo::{Process, ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind, Users};

const REFRESH_INTERVAL: Duration = Duration::from_secs(2);

/// Lists every process. Keep the `System` alive between calls so CPU usage has a previous sample to diff against.
pub fn list_processes(system: &mut System, users: Option<&Users>) -> Vec<ProcessInfo> {
    system.refresh_processes_specifics(
        ProcessesToUpdate::All,
        true,
        ProcessRefreshKind::nothing()
            .with_cpu()
            .with_memory()
            .with_exe(UpdateKind::OnlyIfNotSet)
            .with_user(UpdateKind::OnlyIfNotSet),
    );
    system
        .processes()
        .values()
        .map(|process| process_info(process, users))
        .collect()
}

fn process_info(process: &Process, users: Option<&Users>) -> ProcessInfo {
    let exe_path = process.exe().map(Path::to_path_buf);
    let name = match exe_path.as_deref().and_then(Path::file_name) {
        Some(file_name) => file_name.to_string_lossy().into_owned(),
        None => process.name().to_string_lossy().into_owned(),
    };
    let user = process.user_id().and_then(|uid| {
        users?
            .get_user_by_id(uid)
            .map(|user| user.name().to_string())
    });
    let started_at = match process.start_time() {
        0 => None,
        seconds => Some(UNIX_EPOCH + Duration::from_secs(seconds)),
    };
    ProcessInfo {
        pid: process.pid().as_u32(),
        parent_pid: process.parent().map(|pid| pid.as_u32()),
        name,
        exe_path,
        user,
        cpu_percent: process.cpu_usage(),
        memory_bytes: process.memory(),
        started_at,
    }
}

/// Terminates `pid` with SIGKILL, after the guard that refuses PIDs 0 and 1 and this process.
pub fn kill_process(pid: u32) -> Result<(), KillError> {
    check_kill_allowed(pid, std::process::id())?;
    let raw_pid = libc::pid_t::try_from(pid).map_err(|_| KillError::NotFound)?;
    // SAFETY: kill(2) takes plain integers and has no memory-safety preconditions.
    let status = unsafe { libc::kill(raw_pid, libc::SIGKILL) };
    if status == 0 {
        Ok(())
    } else {
        Err(kill_error(io::Error::last_os_error()))
    }
}

fn kill_error(error: io::Error) -> KillError {
    match error.raw_os_error() {
        Some(libc::EPERM | libc::EACCES) => KillError::PermissionDenied,
        Some(libc::ESRCH) => KillError::NotFound,
        _ => KillError::Failed(error.to_string()),
    }
}

pub enum ProcessEvent {
    Listed(Vec<ProcessInfo>),
    KillFinished {
        pid: u32,
        name: String,
        result: Result<(), KillError>,
    },
}

enum Command {
    Kill { pid: u32, name: String },
}

/// Lists processes every few seconds and performs kills on a background thread.
pub struct ProcessWorker {
    commands: Sender<Command>,
    events: Receiver<ProcessEvent>,
}

impl ProcessWorker {
    pub fn start() -> Self {
        let (command_tx, command_rx) = mpsc::channel();
        let (event_tx, event_rx) = mpsc::channel();
        thread::Builder::new()
            .name("process-worker".to_string())
            .spawn(move || run_worker(command_rx, event_tx))
            .expect("failed to spawn the process worker thread");
        Self {
            commands: command_tx,
            events: event_rx,
        }
    }

    pub fn kill(&self, pid: u32, name: String) {
        let _ = self.commands.send(Command::Kill { pid, name });
    }

    /// Returns the events produced since the last call, without blocking.
    pub fn drain_events(&self) -> Vec<ProcessEvent> {
        self.events.try_iter().collect()
    }
}

fn run_worker(commands: Receiver<Command>, events: Sender<ProcessEvent>) {
    let mut system = System::new().expect("failed to initialize process information");
    let users = Users::new_with_refreshed_list().ok();
    loop {
        let listed = list_processes(&mut system, users.as_ref());
        if events.send(ProcessEvent::Listed(listed)).is_err() {
            return;
        }
        match commands.recv_timeout(REFRESH_INTERVAL) {
            Ok(Command::Kill { pid, name }) => {
                let result = kill_process(pid);
                if events
                    .send(ProcessEvent::KillFinished { pid, name, result })
                    .is_err()
                {
                    return;
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::process::ExitStatusExt;
    use std::process::{Child, Command as StdCommand};
    use sys_manager_common::process::KillRefusal;

    /// Spawns a disposable `sleep` process. Only this test's own child is ever signalled.
    struct SleepChild(Child);

    impl SleepChild {
        fn spawn() -> Self {
            Self(StdCommand::new("sleep").arg("120").spawn().unwrap())
        }
    }

    impl Drop for SleepChild {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    #[test]
    fn refuses_init_and_this_process_without_signalling_anything() {
        assert_eq!(
            kill_process(1),
            Err(KillError::Refused(KillRefusal::SystemProcess))
        );
        assert_eq!(
            kill_process(std::process::id()),
            Err(KillError::Refused(KillRefusal::OwnProcess))
        );
    }

    #[test]
    fn kills_a_spawned_child() {
        let mut child = SleepChild::spawn();
        let pid = child.0.id();

        assert_eq!(kill_process(pid), Ok(()));

        let status = child.0.wait().unwrap();
        assert_eq!(status.signal(), Some(libc::SIGKILL));
    }

    #[test]
    fn lists_a_spawned_child_with_its_details() {
        let child = SleepChild::spawn();
        let users = Users::new_with_refreshed_list().ok();
        let mut system = System::new().unwrap();
        list_processes(&mut system, users.as_ref());
        thread::sleep(Duration::from_millis(200));

        let processes = list_processes(&mut system, users.as_ref());

        let info = processes
            .iter()
            .find(|process| process.pid == child.0.id())
            .expect("spawned child is listed");
        assert_eq!(info.name, "sleep");
        assert_eq!(info.parent_pid, Some(std::process::id()));
        assert!(info
            .exe_path
            .as_deref()
            .is_some_and(|path| path.ends_with("sleep")));
        assert!(info.user.is_some());
        assert!(info.memory_bytes > 0);
        assert!(info.started_at.is_some());
    }

    #[test]
    fn maps_permission_and_missing_process_errors() {
        assert_eq!(
            kill_error(io::Error::from_raw_os_error(libc::EPERM)),
            KillError::PermissionDenied
        );
        assert_eq!(
            kill_error(io::Error::from_raw_os_error(libc::ESRCH)),
            KillError::NotFound
        );
    }
}
