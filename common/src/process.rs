//! Process data shared by the process viewer, and the rules for signalling a process.

use std::fmt;
use std::path::PathBuf;
use std::time::SystemTime;

#[derive(Clone, Debug, PartialEq)]
pub struct ProcessInfo {
    pub pid: u32,
    pub parent_pid: Option<u32>,
    pub name: String,
    pub exe_path: Option<PathBuf>,
    pub user: Option<String>,
    pub cpu_percent: f32,
    pub memory_bytes: u64,
    pub started_at: Option<SystemTime>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KillRefusal {
    /// PIDs 0 and 1 belong to the kernel and to init.
    SystemProcess,
    /// Sys Manager never terminates itself.
    OwnProcess,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KillError {
    Refused(KillRefusal),
    NotFound,
    PermissionDenied,
    Failed(String),
}

impl fmt::Display for KillError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Refused(KillRefusal::SystemProcess) => write!(f, "protected system process"),
            Self::Refused(KillRefusal::OwnProcess) => write!(f, "this is Sys Manager itself"),
            Self::NotFound => write!(f, "the process has already exited"),
            Self::PermissionDenied => write!(f, "permission denied"),
            Self::Failed(message) => write!(f, "{message}"),
        }
    }
}

/// Refuses PIDs that must never be signalled: the kernel and init, and this process.
pub fn check_kill_allowed(pid: u32, own_pid: u32) -> Result<(), KillError> {
    if pid <= 1 {
        Err(KillError::Refused(KillRefusal::SystemProcess))
    } else if pid == own_pid {
        Err(KillError::Refused(KillRefusal::OwnProcess))
    } else {
        Ok(())
    }
}

/// Busiest first, then largest memory, then lowest PID.
pub fn sort_for_display(processes: &mut [ProcessInfo]) {
    processes.sort_by(|a, b| {
        b.cpu_percent
            .total_cmp(&a.cpu_percent)
            .then(b.memory_bytes.cmp(&a.memory_bytes))
            .then(a.pid.cmp(&b.pid))
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn process(pid: u32, cpu_percent: f32, memory_bytes: u64) -> ProcessInfo {
        ProcessInfo {
            pid,
            parent_pid: None,
            name: format!("proc-{pid}"),
            exe_path: None,
            user: None,
            cpu_percent,
            memory_bytes,
            started_at: None,
        }
    }

    #[test]
    fn kill_guard_refuses_kernel_init_and_own_pid() {
        assert_eq!(
            check_kill_allowed(0, 500),
            Err(KillError::Refused(KillRefusal::SystemProcess))
        );
        assert_eq!(
            check_kill_allowed(1, 500),
            Err(KillError::Refused(KillRefusal::SystemProcess))
        );
        assert_eq!(
            check_kill_allowed(500, 500),
            Err(KillError::Refused(KillRefusal::OwnProcess))
        );
        assert_eq!(check_kill_allowed(2, 500), Ok(()));
    }

    #[test]
    fn sorts_by_cpu_then_memory_then_pid() {
        let mut processes = vec![
            process(30, 1.0, 100),
            process(10, 5.0, 100),
            process(20, 1.0, 900),
            process(40, 1.0, 900),
        ];

        sort_for_display(&mut processes);

        let pids: Vec<u32> = processes.iter().map(|p| p.pid).collect();
        assert_eq!(pids, [10, 20, 40, 30]);
    }
}
