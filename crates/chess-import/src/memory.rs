//! What the machine has: total and available memory, for choosing between keeping
//! an index in memory and reading it from disk (and for the bench report).

use sysinfo::{MemoryRefreshKind, RefreshKind, System};

fn system() -> System {
    System::new_with_specifics(RefreshKind::nothing().with_memory(MemoryRefreshKind::everything()))
}

/// Bytes of memory the system reports as available to new allocations without
/// swapping (Linux `MemAvailable`; the other systems' equivalent). `None` when unknown.
pub fn available() -> Option<u64> {
    Some(system().available_memory()).filter(|n| *n > 0)
}

/// Bytes of physical memory.
pub fn total() -> Option<u64> {
    Some(system().total_memory()).filter(|n| *n > 0)
}
