//! Order-preserving parallel map on scoped threads.
//!
//! Workers pull small blocks of indices from a shared counter; every result
//! is tagged with its index and the results are put back in index order, so
//! the output never depends on the number of workers or on scheduling.

use std::sync::atomic::{AtomicUsize, Ordering};

/// Indices handed to a worker at a time.
const BLOCK: usize = 8;

/// `(0..count).map(|i| f(&mut state, i))` on up to `jobs` threads, each
/// with its own `init()` state. `jobs` of 0 is treated as 1.
pub(crate) fn map_with<S, T, I, F>(count: usize, jobs: usize, init: I, f: F) -> Vec<T>
where
    T: Send,
    I: Fn() -> S + Sync,
    F: Fn(&mut S, usize) -> T + Sync,
{
    let jobs = jobs.clamp(1, count.div_ceil(BLOCK).max(1));
    if jobs == 1 {
        let mut state = init();
        return (0..count).map(|i| f(&mut state, i)).collect();
    }
    let next = AtomicUsize::new(0);
    let mut tagged: Vec<(usize, T)> = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..jobs)
            .map(|_| {
                scope.spawn(|| {
                    let mut state = init();
                    let mut out = Vec::new();
                    loop {
                        let start = next.fetch_add(BLOCK, Ordering::Relaxed);
                        if start >= count {
                            break;
                        }
                        for i in start..(start + BLOCK).min(count) {
                            out.push((i, f(&mut state, i)));
                        }
                    }
                    out
                })
            })
            .collect();
        handles
            .into_iter()
            .flat_map(|h| match h.join() {
                Ok(part) => part,
                Err(payload) => std::panic::resume_unwind(payload),
            })
            .collect()
    });
    tagged.sort_unstable_by_key(|(i, _)| *i);
    tagged.into_iter().map(|(_, t)| t).collect()
}

/// `(0..count).map(f)` on up to `jobs` threads.
pub(crate) fn map<T, F>(count: usize, jobs: usize, f: F) -> Vec<T>
where
    T: Send,
    F: Fn(usize) -> T + Sync,
{
    map_with(count, jobs, || (), |_, i| f(i))
}

#[cfg(test)]
mod tests {
    #[test]
    fn results_come_back_in_order_for_any_job_count() {
        let serial = super::map(1000, 1, |i| i * i);
        for jobs in [0, 2, 3, 7, 64] {
            assert_eq!(super::map(1000, jobs, |i| i * i), serial);
        }
        assert!(super::map(0, 4, |i| i).is_empty());
    }
}
