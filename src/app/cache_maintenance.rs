//! Coalesced disk-cache maintenance, independent of the UI/reactor thread.
//! Leases protect cached audio while preparation/playback owns it. A released
//! lease schedules another pass, so temporarily oversized active files do not
//! permanently bypass the configured budget.

use super::streaming::reclaim_orphan_streaming_temp;
use crate::data::config::{CacheCleanStrategy, CacheConfig};
use anyhow::{Context, Result};
use parking_lot::{Condvar, Mutex};
use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

// Merge bursts of completed writes without delaying maintenance indefinitely.
const COALESCE_INTERVAL: Duration = Duration::from_secs(1);

#[derive(Debug, Default)]
struct State {
    pending: BTreeSet<PathBuf>,
    protected: HashMap<PathBuf, usize>,
    running: bool,
}

#[derive(Debug)]
struct Shared {
    policy: CacheConfig,
    state: Mutex<State>,
    settled: Condvar,
}

#[derive(Clone, Debug)]
pub(super) struct CacheMaintenance {
    shared: Arc<Shared>,
}

#[derive(Debug)]
pub(super) struct CacheLease {
    maintenance: CacheMaintenance,
    path: PathBuf,
}

impl CacheMaintenance {
    pub(super) fn new(policy: CacheConfig) -> Self {
        Self {
            shared: Arc::new(Shared {
                policy,
                state: Mutex::new(State::default()),
                settled: Condvar::new(),
            }),
        }
    }

    /// Acquire before probing/opening a cache path, not after the file is open.
    pub(super) fn protect(&self, path: PathBuf) -> CacheLease {
        *self
            .shared
            .state
            .lock()
            .protected
            .entry(path.clone())
            .or_default() += 1;
        CacheLease {
            maintenance: self.clone(),
            path,
        }
    }

    /// Safe from either the reactor or the persistence worker. Only the first
    /// request starts a worker; subsequent writes merge into its next pass.
    pub(super) fn request(&self, directory: PathBuf) {
        {
            let mut state = self.shared.state.lock();
            state.pending.insert(directory);
            if state.running {
                return;
            }
            state.running = true;
        }
        let shared = self.shared.clone();
        if let Err(error) = std::thread::Builder::new()
            .name("cnmplayer-cache-maintenance".into())
            .spawn(move || maintain(shared))
        {
            let mut state = self.shared.state.lock();
            state.running = false;
            self.shared.settled.notify_all();
            log::warn!("cache maintenance worker unavailable: {error}");
        }
    }

    #[cfg(test)]
    fn flush(&self) {
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        let mut state = self.shared.state.lock();
        while state.running {
            assert!(
                !self
                    .shared
                    .settled
                    .wait_until(&mut state, deadline)
                    .timed_out(),
                "cache maintenance did not settle"
            );
        }
        assert!(state.pending.is_empty());
    }
}

impl Drop for CacheLease {
    fn drop(&mut self) {
        {
            let mut state = self.maintenance.shared.state.lock();
            if let Some(count) = state.protected.get_mut(&self.path) {
                *count -= 1;
                if *count == 0 {
                    state.protected.remove(&self.path);
                }
            }
        }
        if let Some(directory) = self.path.parent() {
            self.maintenance.request(directory.to_path_buf());
        }
    }
}

fn maintain(shared: Arc<Shared>) {
    loop {
        std::thread::sleep(COALESCE_INTERVAL);
        let directories = {
            let mut state = shared.state.lock();
            std::mem::take(&mut state.pending)
        };
        for directory in directories {
            let remove = |entry: &CacheEntry| {
                // Hold the short protection lock across check + unlink. A new
                // prepare cannot acquire a lease between those two operations.
                let state = shared.state.lock();
                !state.protected.contains_key(&entry.path) && remove_unchanged(entry)
            };
            if let Err(error) = cleanup(&directory, &shared.policy, remove) {
                log::warn!(
                    "cache maintenance failed for {}: {error:#}",
                    directory.display()
                );
            }
        }
        let mut state = shared.state.lock();
        if state.pending.is_empty() {
            state.running = false;
            shared.settled.notify_all();
            return;
        }
    }
}

#[derive(Debug)]
struct CacheEntry {
    path: PathBuf,
    size: u64,
    modified: SystemTime,
}

pub(super) fn cleanup_orphan_streaming_temps(cache_dir: &Path) -> Result<()> {
    if !cache_dir.is_dir() {
        return Ok(());
    }
    for entry in fs::read_dir(cache_dir)
        .with_context(|| format!("read cache dir failed: {}", cache_dir.display()))?
    {
        let path = entry?.path();
        if path.extension().is_some_and(|ext| ext == "part") {
            reclaim_orphan_streaming_temp(&path);
        }
    }
    Ok(())
}

/// Unleased directories (e.g. exported MPRIS artwork) retain the same policy.
#[cfg(target_os = "linux")]
pub(super) fn cleanup_cache_dir(cache_dir: &Path, policy: &CacheConfig) -> Result<()> {
    cleanup(cache_dir, policy, remove_unchanged)
}

fn remove_unchanged(entry: &CacheEntry) -> bool {
    // Atomic writers can replace a path after the directory scan. Do not evict
    // fresh content based on the old file's age or size.
    fs::metadata(&entry.path).is_ok_and(|metadata| {
        metadata.len() == entry.size
            && metadata.modified().ok() == Some(entry.modified)
            && fs::remove_file(&entry.path).is_ok()
    })
}

fn cleanup(
    cache_dir: &Path,
    policy: &CacheConfig,
    remove: impl Fn(&CacheEntry) -> bool,
) -> Result<()> {
    cleanup_orphan_streaming_temps(cache_dir)?;
    let mut entries = list_cache_entries(cache_dir)?;
    if matches!(
        policy.clean_strategy,
        CacheCleanStrategy::Age | CacheCleanStrategy::Both
    ) && policy.max_age_days > 0
    {
        let now = SystemTime::now();
        let ttl = Duration::from_secs(policy.max_age_days.saturating_mul(24 * 60 * 60));
        entries.retain(|entry| {
            let expired = now
                .duration_since(entry.modified)
                .is_ok_and(|age| age > ttl);
            !expired || !remove(entry)
        });
    }
    if matches!(
        policy.clean_strategy,
        CacheCleanStrategy::Size | CacheCleanStrategy::Both
    ) && policy.max_size_mb > 0
    {
        let limit = policy.max_size_mb.saturating_mul(1024 * 1024);
        let mut total = entries
            .iter()
            .fold(0u64, |sum, entry| sum.saturating_add(entry.size));
        if total > limit {
            entries.sort_by(|a, b| {
                a.modified
                    .cmp(&b.modified)
                    .then_with(|| a.path.cmp(&b.path))
            });
            for entry in entries {
                if total <= limit {
                    break;
                }
                if remove(&entry) {
                    total = total.saturating_sub(entry.size);
                }
            }
        }
    }
    Ok(())
}

fn list_cache_entries(cache_dir: &Path) -> Result<Vec<CacheEntry>> {
    let mut entries = Vec::new();
    if !cache_dir.is_dir() {
        return Ok(entries);
    }
    for entry in fs::read_dir(cache_dir)
        .with_context(|| format!("read cache dir failed: {}", cache_dir.display()))?
    {
        let entry = entry?;
        let path = entry.path();
        if path.extension().is_some_and(|ext| ext == "part") {
            continue;
        }
        let metadata = match entry.metadata() {
            Ok(metadata) => metadata,
            // A writer/another process may remove an entry after read_dir.
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error.into()),
        };
        if metadata.is_file() {
            entries.push(CacheEntry {
                path,
                size: metadata.len(),
                modified: metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH),
            });
        }
    }
    Ok(entries)
}

#[cfg(test)]
mod tests;
