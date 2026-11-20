//! Platform-neutral view state for the menu bar popover.

use std::path::{Path, PathBuf};

use crate::actions::{ActionError, ItemAction};
use crate::disk::{display_name, ScanEntry, ScanSnapshot, ScanState};
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

    /// Describes the outcome of an action on a disk item.
    pub fn for_item(action: ItemAction, name: &str, result: &Result<(), ActionError>) -> Self {
        match result {
            Ok(()) => Self {
                text: action.success_text(name),
                is_error: false,
            },
            Err(error) => Self {
                text: action.failure_text(name, error),
                is_error: true,
            },
        }
    }
}

/// An action on a disk item that has been handed to the worker and has not reported back yet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingItem {
    pub action: ItemAction,
    pub path: PathBuf,
}

#[derive(Debug, Default)]
pub struct AppModel {
    tab: Tab,
    scan: Option<ScanSnapshot>,
    processes: Vec<ProcessInfo>,
    selected_pid: Option<u32>,
    notice: Option<Notice>,
    selected_path: Option<PathBuf>,
    pending_item: Option<PendingItem>,
    item_notice: Option<Notice>,
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

    /// Replaces the disk list. A selected item that the new list no longer shows is deselected.
    pub fn set_scan(&mut self, snapshot: ScanSnapshot) {
        self.scan = Some(snapshot);
        if self.selected_item().is_none() {
            self.selected_path = None;
        }
    }

    pub fn reset_scan(&mut self) {
        self.scan = None;
        self.selected_path = None;
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

    /// Second line of the disk view: the running action, the latest outcome, or the scan summary.
    pub fn disk_status_line(&self) -> (String, bool) {
        if let Some(pending) = &self.pending_item {
            return (
                pending.action.progress_text(&display_name(&pending.path)),
                false,
            );
        }
        if let Some(notice) = &self.item_notice {
            return (notice.text.clone(), notice.is_error);
        }
        (self.disk_detail(), false)
    }

    /// The disk item the user selected, if the list still shows it.
    pub fn selected_item(&self) -> Option<&ScanEntry> {
        let path = self.selected_path.as_ref()?;
        self.disk_entries().iter().find(|entry| &entry.path == path)
    }

    /// Selects the disk item at `index`, or clears the selection if that item is already selected.
    pub fn toggle_item(&mut self, index: usize) {
        let Some(path) = self
            .disk_entries()
            .get(index)
            .map(|entry| entry.path.clone())
        else {
            return;
        };
        self.selected_path = if self.selected_path.as_ref() == Some(&path) {
            None
        } else {
            Some(path)
        };
    }

    /// Items can be removed only after the scan finishes, because a running scan still adds to their sizes.
    pub fn can_change_items(&self) -> bool {
        self.scan
            .as_ref()
            .is_some_and(|scan| scan.state != ScanState::Scanning)
    }

    pub fn pending_item(&self) -> Option<&PendingItem> {
        self.pending_item.as_ref()
    }

    pub fn start_item_action(&mut self, action: ItemAction, path: &Path) {
        self.pending_item = Some(PendingItem {
            action,
            path: path.to_path_buf(),
        });
    }

    /// Records how an action on the item at `path` ended, and applies a success to the list.
    pub fn report_item_result(
        &mut self,
        action: ItemAction,
        path: &Path,
        result: &Result<(), ActionError>,
    ) {
        if self
            .pending_item
            .as_ref()
            .is_some_and(|pending| pending.action == action && pending.path == path)
        {
            self.pending_item = None;
        }
        if result.is_ok() {
            match action {
                ItemAction::Open => {}
                ItemAction::Remove => self.apply_removal(path),
                ItemAction::Lock => self.set_item_locked(path, true),
                ItemAction::Unlock => self.set_item_locked(path, false),
            }
        }
        self.item_notice = Some(Notice::for_item(action, &display_name(path), result));
    }

    pub fn item_notice(&self) -> Option<&Notice> {
        self.item_notice.as_ref()
    }

    /// Drops the removed item and everything under it from the list. Its size and item count leave the
    /// totals, and the folders that held it shrink by the same amounts.
    fn apply_removal(&mut self, path: &Path) {
        let Some(scan) = self.scan.as_mut() else {
            return;
        };
        let Some((size, items)) = scan
            .entries
            .iter()
            .find(|entry| entry.path == path)
            .map(|entry| (entry.size, entry.items))
        else {
            return;
        };
        scan.bytes_scanned = scan.bytes_scanned.saturating_sub(size);
        scan.items_scanned = scan.items_scanned.saturating_sub(items);
        scan.entries.retain_mut(|entry| {
            if entry.path.starts_with(path) {
                return false;
            }
            if path.starts_with(&entry.path) {
                entry.size = entry.size.saturating_sub(size);
                entry.items = entry.items.saturating_sub(items);
            }
            entry.size > 0
        });
        scan.entries
            .sort_by(|a, b| b.size.cmp(&a.size).then_with(|| a.path.cmp(&b.path)));
        if self
            .selected_path
            .as_ref()
            .is_some_and(|selected| selected.starts_with(path))
        {
            self.selected_path = None;
        }
    }

    fn set_item_locked(&mut self, path: &Path, locked: bool) {
        if let Some(entry) = self
            .scan
            .as_mut()
            .and_then(|scan| scan.entries.iter_mut().find(|entry| entry.path == path))
        {
            entry.locked = locked;
        }
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
    use crate::disk::EntryKind;
    use std::time::Duration;

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

    fn disk_entry(path: &str, size: u64, items: u64, kind: EntryKind) -> ScanEntry {
        ScanEntry {
            path: PathBuf::from(path),
            size,
            kind,
            items,
            locked: false,
        }
    }

    fn disk_snapshot(bytes: u64, items: u64, entries: Vec<ScanEntry>) -> ScanSnapshot {
        ScanSnapshot {
            sequence: 1,
            state: ScanState::Completed,
            items_scanned: items,
            bytes_scanned: bytes,
            skipped_no_access: 0,
            other_errors: 0,
            current_path: None,
            entries,
            elapsed: Duration::ZERO,
        }
    }

    #[test]
    fn removing_a_folder_updates_the_totals_and_the_folders_around_it() {
        let mut model = AppModel::default();
        model.set_scan(disk_snapshot(
            1_000,
            5,
            vec![
                disk_entry("/data", 1_000, 5, EntryKind::Directory),
                disk_entry("/data/cache", 900, 3, EntryKind::Directory),
                disk_entry("/data/cache/a.bin", 500, 1, EntryKind::File),
                disk_entry("/data/cache/b.bin", 400, 1, EntryKind::File),
                disk_entry("/data/keep.bin", 100, 1, EntryKind::File),
            ],
        ));

        model.report_item_result(ItemAction::Remove, Path::new("/data/cache"), &Ok(()));

        let scan = model.scan().unwrap();
        assert_eq!((scan.bytes_scanned, scan.items_scanned), (100, 2));
        let remaining: Vec<(&str, u64)> = scan
            .entries
            .iter()
            .map(|entry| (entry.path.to_str().unwrap(), entry.size))
            .collect();
        assert_eq!(remaining, [("/data", 100), ("/data/keep.bin", 100)]);
        assert_eq!(scan.entries[0].items, 2);
    }

    #[test]
    fn locking_marks_the_item_and_removing_the_selected_item_clears_the_selection() {
        let mut model = AppModel::default();
        model.set_scan(disk_snapshot(
            300,
            2,
            vec![
                disk_entry("/data/a.bin", 200, 1, EntryKind::File),
                disk_entry("/data/b.bin", 100, 1, EntryKind::File),
            ],
        ));
        model.toggle_item(1);
        assert_eq!(
            model.selected_item().map(ScanEntry::name),
            Some("b.bin".to_string())
        );

        model.report_item_result(ItemAction::Lock, Path::new("/data/b.bin"), &Ok(()));
        assert!(model.selected_item().is_some_and(|entry| entry.locked));

        model.report_item_result(ItemAction::Remove, Path::new("/data/b.bin"), &Ok(()));
        assert!(model.selected_item().is_none());
    }

    #[test]
    fn a_failed_action_keeps_the_list_and_explains_why() {
        let mut model = AppModel::default();
        model.set_scan(disk_snapshot(
            100,
            1,
            vec![disk_entry("/data/a.bin", 100, 1, EntryKind::File)],
        ));
        model.start_item_action(ItemAction::Remove, Path::new("/data/a.bin"));

        model.report_item_result(
            ItemAction::Remove,
            Path::new("/data/a.bin"),
            &Err(ActionError::Locked),
        );

        assert!(model.pending_item().is_none());
        assert_eq!(model.disk_entries().len(), 1);
        let notice = model.item_notice().unwrap();
        assert!(notice.is_error);
        assert!(notice.text.ends_with("Locked. Unlock it first."));
    }
}
