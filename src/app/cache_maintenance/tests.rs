use super::*;
use std::sync::atomic::{AtomicU64, Ordering};

struct Directory(PathBuf);
impl Directory {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "cnmplayer-runtime-cache-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
    fn file(&self, name: &str, bytes: usize, age: Duration) -> PathBuf {
        let path = self.0.join(name);
        fs::write(&path, vec![0u8; bytes]).unwrap();
        let file = fs::OpenOptions::new().write(true).open(&path).unwrap();
        file.set_times(fs::FileTimes::new().set_modified(SystemTime::now() - age))
            .unwrap();
        path
    }
}
impl Drop for Directory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn policy(strategy: CacheCleanStrategy) -> CacheConfig {
    CacheConfig {
        path: None,
        clean_strategy: strategy,
        max_size_mb: 1,
        max_age_days: 1,
        clean_on_startup: false,
    }
}

#[test]
fn completed_writes_enforce_budget_without_restart_and_preserve_parts() {
    let directory = Directory::new();
    let old = directory.file("old.audio", 700 * 1024, Duration::from_secs(20));
    let new = directory.file("new.audio", 700 * 1024, Duration::ZERO);
    let part = directory.file(
        "download.flac.part",
        2 * 1024 * 1024,
        Duration::from_secs(30),
    );
    let maintenance = CacheMaintenance::new(policy(CacheCleanStrategy::Size));
    for _ in 0..16 {
        maintenance.request(directory.0.clone());
    }
    maintenance.flush();
    assert!(!old.exists());
    assert_eq!(fs::metadata(new).unwrap().len(), 700 * 1024);
    assert_eq!(fs::metadata(part).unwrap().len(), 2 * 1024 * 1024);
}

#[test]
fn active_atomic_temps_are_not_counted_or_removed() {
    let directory = Directory::new();
    let old = directory.file("old.audio", 2 * 1024 * 1024, Duration::ZERO);
    let temp_name = format!(
        ".cover.tmp-{}-{}-0",
        std::process::id(),
        SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let temp = directory.file(&temp_name, 2 * 1024 * 1024, Duration::from_secs(2 * 86400));
    let maintenance = CacheMaintenance::new(policy(CacheCleanStrategy::Both));
    maintenance.request(directory.0.clone());
    maintenance.flush();
    assert!(!old.exists());
    assert!(temp.exists());
}

#[test]
fn active_audio_survives_size_and_age_until_last_lease_is_released() {
    let directory = Directory::new();
    let active = directory.file(
        "active.audio",
        2 * 1024 * 1024,
        Duration::from_secs(3 * 86400),
    );
    let other = directory.file("other.audio", 64, Duration::ZERO);
    let maintenance = CacheMaintenance::new(policy(CacheCleanStrategy::Both));
    let first = maintenance.protect(active.clone());
    let second = maintenance.protect(active.clone());
    maintenance.request(directory.0.clone());
    maintenance.flush();
    assert_eq!(fs::metadata(&active).unwrap().len(), 2 * 1024 * 1024);
    assert!(!other.exists());
    drop(first);
    maintenance.flush();
    assert!(active.exists());
    drop(second);
    maintenance.flush();
    assert!(!active.exists());
}

#[test]
fn runtime_age_cleanup_preserves_fresh_files_in_multiple_directories() {
    let first = Directory::new();
    let second = Directory::new();
    let old_audio = first.file("old.audio", 64, Duration::from_secs(2 * 86400));
    let fresh_audio = first.file("fresh.audio", 64, Duration::ZERO);
    let old_cover = second.file("old.img", 64, Duration::from_secs(2 * 86400));
    let fresh_cover = second.file("fresh.img", 64, Duration::ZERO);
    let maintenance = CacheMaintenance::new(policy(CacheCleanStrategy::Age));
    maintenance.request(first.0.clone());
    maintenance.request(second.0.clone());
    maintenance.flush();
    assert!(!old_audio.exists());
    assert!(!old_cover.exists());
    assert!(fresh_audio.is_file());
    assert!(fresh_cover.is_file());
}

#[test]
fn concurrent_replacement_is_not_deleted_using_stale_age() {
    let directory = Directory::new();
    let path = directory.file("cover.img", 3, Duration::from_secs(2 * 86400));
    cleanup(&directory.0, &policy(CacheCleanStrategy::Age), |entry| {
        fs::write(&entry.path, b"fresh cover bytes").unwrap();
        remove_unchanged(entry)
    })
    .unwrap();
    assert_eq!(fs::read(path).unwrap(), b"fresh cover bytes");
}
