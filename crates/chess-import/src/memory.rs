//! What the machine has: total and available memory, for choosing between keeping
//! an index in memory and reading it from disk (and for the bench report).

use sysinfo::{MemoryRefreshKind, RefreshKind, System};

fn system() -> System {
    System::new_with_specifics(RefreshKind::nothing().with_memory(MemoryRefreshKind::everything()))
}

/// Bytes of memory the system reports as available to new allocations without
/// swapping (Linux `MemAvailable`; the other systems' equivalent). `None` when unknown.
/// `CP_MEMORY_LIMIT=<MB>` caps it (a dev hook: the low-memory paths on a big machine).
pub fn available() -> Option<u64> {
    let real = Some(system().available_memory()).filter(|n| *n > 0);
    match std::env::var("CP_MEMORY_LIMIT")
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
    {
        Some(limit) => Some(real.unwrap_or(u64::MAX).min(limit * 1_000_000)),
        None => real,
    }
}

/// Bytes of physical memory.
pub fn total() -> Option<u64> {
    Some(system().total_memory()).filter(|n| *n > 0)
}
