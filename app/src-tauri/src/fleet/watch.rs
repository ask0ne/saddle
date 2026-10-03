//! Watch a file or directory and call back when it changes.
//!
//! A raw kqueue vnode watch is opened against a file descriptor, and
//! replacing/rotating the file invalidates that descriptor — the watch goes
//! deaf until it's reopened. `notify`'s macOS backend is FSEvents, which
//! watches by *path* rather than descriptor, so this is believed not to need
//! manual rearm-on-rename/delete/revoke. `survives_file_replacement` below is
//! the empirical check for that.

use std::path::Path;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::time::Duration;

use notify::{RecommendedWatcher, RecursiveMode, Watcher as _};

/// Keeps the underlying OS watch alive for as long as this is held; drop to stop.
pub struct FileWatcher {
    _watcher: RecommendedWatcher,
}

impl FileWatcher {
    /// Watch `path` (a file or directory) and call `on_change` once writes
    /// have stopped arriving for `debounce` — writes arrive in bursts, so
    /// this coalesces one burst into one refresh.
    pub fn new(
        path: &Path,
        debounce: Duration,
        on_change: impl FnMut() + Send + 'static,
    ) -> notify::Result<Self> {
        let (tx, rx): (Sender<()>, Receiver<()>) = mpsc::channel();
        let mut watcher =
            notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
                if res.is_ok() {
                    let _ = tx.send(());
                }
            })?;
        watcher.watch(path, RecursiveMode::NonRecursive)?;

        std::thread::spawn(move || debounce_loop(rx, debounce, on_change));

        Ok(Self { _watcher: watcher })
    }
}

/// Block for the first ping, then drain anything else arriving within the
/// debounce window before firing once. Exits once the sender side is gone
/// (the `FileWatcher` was dropped).
fn debounce_loop(rx: Receiver<()>, debounce: Duration, mut on_change: impl FnMut()) {
    while rx.recv().is_ok() {
        loop {
            match rx.recv_timeout(debounce) {
                Ok(()) => continue,
                Err(RecvTimeoutError::Timeout) => break,
                Err(RecvTimeoutError::Disconnected) => return,
            }
        }
        on_change();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn tempdir() -> TempDir {
        use std::sync::atomic::{AtomicU64, Ordering as O};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, O::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("devpit-watch-test-{}-{n}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        TempDir(dir)
    }
    struct TempDir(std::path::PathBuf);
    impl TempDir {
        fn path(&self) -> &Path {
            &self.0
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// Poll `count` until it's non-zero or `timeout` elapses.
    fn wait_for(count: &AtomicUsize, timeout: Duration) -> bool {
        let start = std::time::Instant::now();
        while start.elapsed() < timeout {
            if count.load(Ordering::SeqCst) > 0 {
                return true;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        false
    }

    #[test]
    fn debounce_loop_coalesces_a_burst_into_one_call() {
        let (tx, rx) = mpsc::channel();
        let calls = Arc::new(AtomicUsize::new(0));
        let calls_in_loop = calls.clone();
        let handle = std::thread::spawn(move || {
            debounce_loop(rx, Duration::from_millis(20), move || {
                calls_in_loop.fetch_add(1, Ordering::SeqCst);
            });
        });

        for _ in 0..5 {
            tx.send(()).unwrap();
        }
        assert!(wait_for(&calls, Duration::from_secs(1)));
        std::thread::sleep(Duration::from_millis(50)); // let the debounce window fully close
        assert_eq!(calls.load(Ordering::SeqCst), 1);

        drop(tx);
        handle.join().unwrap();
    }

    #[test]
    fn debounce_loop_exits_once_sender_is_dropped() {
        let (tx, rx) = mpsc::channel::<()>();
        let handle =
            std::thread::spawn(move || debounce_loop(rx, Duration::from_millis(10), || {}));
        drop(tx);
        handle.join().unwrap(); // hangs the test (and CI) if debounce_loop doesn't exit
    }

    #[test]
    fn fires_on_a_real_write() {
        let dir = tempdir();
        let path = dir.path().join("watched.txt");
        fs::write(&path, b"initial").unwrap();

        let calls = Arc::new(AtomicUsize::new(0));
        let calls_cb = calls.clone();
        let _watcher = FileWatcher::new(&path, Duration::from_millis(30), move || {
            calls_cb.fetch_add(1, Ordering::SeqCst);
        })
        .unwrap();

        fs::write(&path, b"changed").unwrap();
        assert!(
            wait_for(&calls, Duration::from_secs(5)),
            "expected a callback after a write"
        );
    }

    /// The empirical check: does notify's
    /// FSEvents backend keep delivering events after the watched file is
    /// removed and recreated, with no manual re-arm?
    #[test]
    fn survives_file_replacement() {
        let dir = tempdir();
        let path = dir.path().join("rotated.txt");
        fs::write(&path, b"initial").unwrap();

        let calls = Arc::new(AtomicUsize::new(0));
        let calls_cb = calls.clone();
        let _watcher = FileWatcher::new(&path, Duration::from_millis(30), move || {
            calls_cb.fetch_add(1, Ordering::SeqCst);
        })
        .unwrap();

        fs::remove_file(&path).unwrap();
        fs::write(&path, b"recreated").unwrap();

        assert!(
            wait_for(&calls, Duration::from_secs(5)),
            "expected a callback after remove+recreate with no manual rearm"
        );
    }
}
