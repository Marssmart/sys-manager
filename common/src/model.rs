//! Platform-neutral view state for the menu bar popover.

use crate::disk::{ScanEntry, ScanSnapshot, ScanState};
use crate::format::{format_bytes, format_count};
use crate::process::{sort_for_display, KillError, ProcessInfo};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Tab {
    #[default]
    DiskCleaner,
    Processes,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Notice {
    pub text: String,
    pub is_error: bool,
}

impl Notice {
    /// Describes the outcome of a kill request for the process viewer.
    pub fn for_kill(pid: u32, name: &str, result: &Result<(), KillError>) -> Self {
        match result {
            Ok(()) => Self {
                text: format!("Killed {name} (PID {pid})"),
                is_error: false,
            },
            Err(error) => Self {
                text: format!("Couldn't kill {name} (PID {pid}): {error}"),
                is_error: true,
            },
        }
    }
}

#[derive(Debug, Default)]
pub struct AppModel {
    tab: Tab,
    scan: Option<ScanSnapshot>,
    processes: Vec<ProcessInfo>,
    selected_pid: Option<u32>,
    notice: Option<Notice>,
}

impl AppModel {
    pub fn tab(&self) -> Tab {
        self.tab
    }

    pub fn select_tab(&mut self, tab: Tab) {
        self.tab = tab;
    }

    pub fn scan(&self) -> Option<&ScanSnapshot> {
        self.scan.as_ref()
    }

    pub fn set_scan(&mut self, snapshot: ScanSnapshot) {
        self.scan = Some(snapshot);
    }

    pub fn reset_scan(&mut self) {
        self.scan = None;
    }

    pub fn disk_entries(&self) -> &[ScanEntry] {
        self.scan
            .as_ref()
            .map_or(&[], |scan| scan.entries.as_slice())
    }

    /// Headline for the disk view, such as `Scanning · 12,345 items · 35.2 GB`.
    pub fn disk_title(&self) -> String {
        let Some(scan) = &self.scan else {
            return "Preparing scan…".to_string();
        };
        let items = format_count(scan.items_scanned);
        let bytes = format_bytes(scan.bytes_scanned);
        match scan.state {
            ScanState::Scanning => format!("Scanning · {items} items · {bytes}"),
            ScanState::Completed => format!("Scan complete · {items} items · {bytes}"),
            ScanState::Cancelled => format!("Scan stopped · {items} items · {bytes}"),
        }
    }

    /// Second line of the disk view: the path being read while scanning, otherwise what was skipped.
    pub fn disk_detail(&self) -> String {
        let Some(scan) = &self.scan else {
            return String::new();
        };
        if scan.state == ScanState::Scanning {
            return scan
                .current_path
                .as_ref()
                .map(|path| path.display().to_string())
                .unwrap_or_default();
        }
        let mut parts = vec!["Largest first".to_string()];
        if scan.skipped_no_access > 0 {
            parts.push(format!(
                "{} skipped: no access",
                format_count(scan.skipped_no_access)
            ));
        }
        if scan.other_errors > 0 {
            parts.push(format!("{} unreadable", format_count(scan.other_errors)));
        }
        parts.join(" · ")
    }

    pub fn processes(&self) -> &[ProcessInfo] {
        &self.processes
    }

    /// Replaces the process list. A selection whose process has exited is cleared.
    pub fn set_processes(&mut self, mut processes: Vec<ProcessInfo>) {
        sort_for_display(&mut processes);
        self.processes = processes;
        if let Some(pid) = self.selected_pid {
            if !self.processes.iter().any(|process| process.pid == pid) {
                self.selected_pid = None;
            }
        }
    }

    pub fn process_title(&self) -> String {
        format!("{} running · sorted by CPU", self.processes.len())
    }

    pub fn selected_process(&self) -> Option<&ProcessInfo> {
        let pid = self.selected_pid?;
        self.processes.iter().find(|process| process.pid == pid)
    }

    /// Selects the process with `pid`, or clears the selection if it is already selected.
    pub fn toggle_selection(&mut self, pid: u32) {
        self.selected_pid = if self.selected_pid == Some(pid) {
            None
        } else {
            Some(pid)
        };
    }

    pub fn notice(&self) -> Option<&Notice> {
        self.notice.as_ref()
    }

    pub fn set_notice(&mut self, notice: Option<Notice>) {
        self.notice = notice;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn process(pid: u32) -> ProcessInfo {
        ProcessInfo {
            pid,
            parent_pid: Some(1),
            name: format!("proc-{pid}"),
            exe_path: None,
            user: None,
            cpu_percent: 0.0,
            memory_bytes: 0,
            started_at: None,
        }
    }

    #[test]
    fn selection_survives_refresh_only_while_the_process_exists() {
        let mut model = AppModel::default();
        model.set_processes(vec![process(10), process(20)]);
        model.toggle_selection(20);
        assert_eq!(model.selected_process().map(|p| p.pid), Some(20));

        model.set_processes(vec![process(10)]);
        assert!(model.selected_process().is_none());

        model.set_processes(vec![process(10), process(20)]);
        assert!(model.selected_process().is_none());
    }

    #[test]
    fn toggling_the_selected_process_clears_it() {
        let mut model = AppModel::default();
        model.set_processes(vec![process(10)]);
        model.toggle_selection(10);
        model.toggle_selection(10);
        assert!(model.selected_process().is_none());
    }
}
