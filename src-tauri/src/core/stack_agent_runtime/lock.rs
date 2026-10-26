//! A lock file so two Atomic Chat processes never prepare runtimes at once.
//! A lock whose process is gone, or that is older than any refresh can run,
//! is stale and taken over.

use std::io::Write;
use std::path::{Path, PathBuf};

/// Longer than the longest build the source allows.
const STALE_AFTER: std::time::Duration = std::time::Duration::from_secs(3 * 60 * 60);

pub struct RefreshLock {
    path: PathBuf,
}

impl RefreshLock {
    pub fn acquire(root: &Path) -> Option<Self> {
        let _ = std::fs::create_dir_all(root);
        let path = root.join("refresh.lock");
        for _ in 0..2 {
            match std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
            {
                Ok(mut f) => {
                    let _ = write!(f, "{}", std::process::id());
                    return Some(Self { path });
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    if !is_stale(&path) {
                        return None;
                    }
                    log::warn!("stack-agent runtime: taking over a stale refresh lock");
                    let _ = std::fs::remove_file(&path);
                }
                Err(e) => {
                    log::error!("stack-agent runtime: could not create the refresh lock: {e}");
                    return None;
                }
            }
        }
        None
    }
}

impl Drop for RefreshLock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

fn is_stale(path: &Path) -> bool {
    let old = std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.elapsed().ok())
        .is_some_and(|age| age > STALE_AFTER);
    if old {
        return true;
    }
    let pid = std::fs::read_to_string(path)
        .ok()
        .and_then(|s| s.trim().parse::<u32>().ok());
    match pid {
        // This process holds no lock it does not know about: the in-process
        // mutex is taken before the file, so a file naming us is a leftover.
        Some(p) if p == std::process::id() => true,
        Some(p) => !process_alive(p),
        // Being written right now, or unreadable: leave it.
        None => false,
    }
}

fn process_alive(pid: u32) -> bool {
    use sysinfo::{Pid, ProcessesToUpdate, System};
    let mut sys = System::new();
    let pid = Pid::from_u32(pid);
    sys.refresh_processes(ProcessesToUpdate::Some(&[pid]), true);
    sys.process(pid).is_some()
}
