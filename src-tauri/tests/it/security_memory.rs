//! Argon2's working memory goes back to the system when a run ends
//! (`security::hashing::Workspace`), Linux only.
//!
//! Each run needs 19 MiB. Taken from glibc's allocator, the first freed
//! block raised malloc's mmap threshold, so every later block came from the
//! running thread's malloc arena and stayed there after it was freed: the
//! weekly soak of 2026-10-18 grew 115 MB this way, one block at a time. A
//! sign-in pads its Argon2 work (KEY-01), so it runs several. macOS and
//! Windows return large frees by themselves; their check is that hashing
//! gives the same answers (`security::hashing::tests`).

#[cfg(target_os = "linux")]
use openalgo_desktop_lib::security::hashing;

/// One run's working memory, in KiB (m_cost 19456 with 1 KiB blocks).
#[cfg(target_os = "linux")]
const ONE_RUN_KIB: u64 = 19_456;

/// Resident set of this process in KiB.
#[cfg(target_os = "linux")]
fn rss_kib() -> u64 {
    let statm = std::fs::read_to_string("/proc/self/statm").unwrap();
    let pages: u64 = statm.split_whitespace().nth(1).unwrap().parse().unwrap();
    // SAFETY: sysconf only reads a system constant.
    let page = unsafe { libc::sysconf(libc::_SC_PAGESIZE) } as u64;
    pages * page / 1024
}

#[cfg(target_os = "linux")]
#[test]
fn argon2_runs_on_many_threads_give_their_memory_back() {
    crate::isolated!(argon2_runs_on_many_threads_give_their_memory_back);
    let pepper = [7u8; 32];
    // A first run, as at start-up: with the allocator's memory, freeing it
    // raised the mmap threshold for every later run.
    let hash = hashing::hash_password(&pepper, "Secret@123").unwrap();
    assert!(hashing::verify_password(&pepper, "Secret@123", &hash).unwrap());
    let before = rss_kib();
    // Four threads at once, three times: concurrent runs land in separate
    // malloc arenas, as sign-ins, API keys and the key vault do.
    for _ in 0..3 {
        let runs: Vec<_> = (0..4)
            .map(|_| {
                let hash = hash.clone();
                std::thread::spawn(move || {
                    hashing::verify_password(&[7u8; 32], "Secret@123", &hash).unwrap()
                })
            })
            .collect();
        for r in runs {
            assert!(r.join().unwrap());
        }
    }
    let after = rss_kib();
    eprintln!(
        "RSS around 12 Argon2 runs, 4 threads at a time: {} -> {} KiB",
        before, after
    );
    assert!(
        after.saturating_sub(before) < ONE_RUN_KIB,
        "Argon2 runs kept their working memory: RSS {} -> {} KiB (one run is {} KiB)",
        before,
        after,
        ONE_RUN_KIB
    );
}
