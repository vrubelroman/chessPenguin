//! Worker threads of the parallel passes (booster, position search, opening tallies).
//!
//! A target without threads (WebAssembly in a browser: `std::thread::spawn` panics
//! there) runs every pass on the calling thread; so does a pass with one worker.

/// Workers for a parallel pass: the threads the system offers, `fallback` when it
/// cannot tell; always one where there are no threads.
pub(crate) fn threads(fallback: usize) -> usize {
    if cfg!(target_family = "wasm") {
        return 1;
    }
    std::thread::available_parallelism().map_or(fallback, |n| n.get())
}

/// `work` for every item, each on its own scoped thread, results in the items' order
/// (`None`: that worker panicked). One item runs on the calling thread.
pub(crate) fn map<I, T, F>(items: Vec<I>, work: F) -> Vec<Option<T>>
where
    I: Send,
    T: Send,
    F: Fn(I) -> T + Sync,
{
    if items.len() <= 1 {
        return items.into_iter().map(|i| Some(work(i))).collect();
    }
    let work = &work;
    std::thread::scope(|s| {
        let handles: Vec<_> = items
            .into_iter()
            .map(|i| s.spawn(move || work(i)))
            .collect();
        handles.into_iter().map(|h| h.join().ok()).collect()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_order_and_runs_one_inline() {
        let out = map(vec![3, 1, 2], |n| n * 10);
        assert_eq!(out, vec![Some(30), Some(10), Some(20)]);
        let here = std::thread::current().id();
        let one = map(vec![()], |()| std::thread::current().id());
        assert_eq!(one, vec![Some(here)]);
        assert!(map(Vec::<u8>::new(), |n| n).is_empty());
    }

    #[test]
    fn a_panicking_worker_gives_none() {
        let out = map(vec![0, 1], |n| {
            assert!(n == 0, "worker {n}");
            n
        });
        assert_eq!(out, vec![Some(0), None]);
    }
}
