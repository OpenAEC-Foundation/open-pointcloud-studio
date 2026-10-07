//! Resident memory used by this application process, refreshed for the status bar.

use sysinfo::{get_current_pid, Pid, ProcessRefreshKind, ProcessesToUpdate, System};

pub(crate) struct MemoryUsage {
    system: System,
    pid: Option<Pid>,
    bytes: Option<u64>,
}

impl Default for MemoryUsage {
    fn default() -> Self {
        Self {
            system: System::new(),
            pid: get_current_pid().ok(),
            bytes: None,
        }
    }
}

impl MemoryUsage {
    pub(crate) fn refresh(&mut self) {
        let Some(pid) = self.pid else {
            return;
        };
        self.system.refresh_processes_specifics(
            ProcessesToUpdate::Some(&[pid]),
            false,
            ProcessRefreshKind::nothing().with_memory(),
        );
        self.bytes = self.system.process(pid).map(|process| process.memory());
    }

    pub(crate) fn label(&self) -> String {
        self.bytes.map_or_else(|| "—".to_owned(), format_bytes)
    }
}

fn format_bytes(bytes: u64) -> String {
    const MIB: f64 = 1_048_576.0;
    const GIB: f64 = 1_073_741_824.0;
    if bytes >= GIB as u64 {
        format!("{:.1} GiB", bytes as f64 / GIB)
    } else {
        format!("{:.0} MiB", bytes as f64 / MIB)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shows_process_memory_in_readable_units() {
        assert_eq!(format_bytes(512 * 1_048_576), "512 MiB");
        assert_eq!(format_bytes(1_610_612_736), "1.5 GiB");
        let mut usage = MemoryUsage::default();
        usage.refresh();
        assert!(usage.bytes.is_some_and(|bytes| bytes > 0));
    }
}
