use anyhow::{Context, Result};
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);
pub(crate) fn is_atomic_temp_path(path: &Path) -> bool {
    atomic_temp_pid(path).is_some()
}

/// Remove atomic-write leftovers owned by a process that is no longer alive.
/// Current-process files are always retained because an active writer can be
/// between creation and registration with any in-process tracking structure.
pub(crate) fn cleanup_orphan_atomic_temps(cache_dir: &Path) -> Result<()> {
    if !cache_dir.is_dir() {
        return Ok(());
    }
    for entry in fs::read_dir(cache_dir)
        .with_context(|| format!("read cache dir failed: {}", cache_dir.display()))?
    {
        let path = entry?.path();
        let Some(pid) = atomic_temp_pid(&path) else {
            continue;
        };
        if pid == std::process::id() || !process_is_dead(pid) {
            continue;
        }
        let _ = fs::remove_file(path);
    }
    Ok(())
}

fn atomic_temp_pid(path: &Path) -> Option<u32> {
    let name = path.file_name()?.to_str()?;
    let (base, suffix) = name.rsplit_once(".tmp-")?;
    if !base.starts_with('.') {
        return None;
    }
    let mut fields = suffix.split('-');
    let pid = fields.next()?.parse().ok()?;
    fields.next()?.parse::<u128>().ok()?;
    fields.next()?.parse::<u64>().ok()?;
    fields.next().is_none().then_some(pid)
}

#[cfg(unix)]
fn process_is_dead(pid: u32) -> bool {
    let Ok(pid) = libc::pid_t::try_from(pid) else {
        return false;
    };
    // SAFETY: kill(pid, 0) only probes process existence and sends no signal.
    let result = unsafe { libc::kill(pid, 0) };
    if result == 0 {
        return false;
    }
    matches!(
        std::io::Error::last_os_error().raw_os_error(),
        Some(code) if code == libc::ESRCH
    )
}

#[cfg(not(unix))]
fn process_is_dead(_pid: u32) -> bool {
    false
}
/// Replace a file without ever truncating the previous contents.
///
/// The temporary is created in the destination directory with `create_new`,
/// flushed to stable storage, and removed automatically if replacement fails.
pub fn write_atomic(path: &Path, contents: &[u8]) -> Result<()> {
    write_atomic_with_mode(path, contents, None)
}

/// Atomic replacement with an optional Unix mode for the temporary and target.
pub fn write_atomic_with_mode(path: &Path, contents: &[u8], mode: Option<u32>) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("path has no parent: {}", path.display()))?;
    fs::create_dir_all(parent).with_context(|| format!("mkdir {}", parent.display()))?;
    let effective_mode = mode.or_else(|| existing_mode(path));

    let mut temp = TempFile::create(parent, path, effective_mode)?;
    let temp_path = &temp.path;

    (|| {
        temp.file
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("temporary file is closed"))?
            .write_all(contents)
            .with_context(|| format!("write {}", temp_path.display()))?;
        temp.file
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("temporary file is closed"))?
            .sync_all()
            .with_context(|| format!("sync {}", temp_path.display()))?;
        drop(temp.file.take());
        fs::rename(temp_path, path).with_context(|| format!("replace {}", path.display()))?;
        #[cfg(unix)]
        {
            let directory = File::open(parent)
                .with_context(|| format!("open {} for sync", parent.display()))?;
            directory
                .sync_all()
                .with_context(|| format!("sync {}", parent.display()))?;
        }
        Ok(())
    })()
}

#[cfg(unix)]
fn existing_mode(path: &Path) -> Option<u32> {
    use std::os::unix::fs::PermissionsExt;
    fs::metadata(path)
        .ok()
        .map(|meta| meta.permissions().mode() & 0o7777)
}

#[cfg(not(unix))]
fn existing_mode(_path: &Path) -> Option<u32> {
    None
}

struct TempFile {
    path: PathBuf,
    file: Option<File>,
}

impl TempFile {
    fn create(parent: &Path, target: &Path, mode: Option<u32>) -> Result<Self> {
        let base = target
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("file");
        let pid = std::process::id();
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|value| value.as_nanos())
            .unwrap_or(0);
        for _ in 0..64 {
            let n = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
            let path = parent.join(format!(".{base}.tmp-{pid}-{now}-{n}"));
            let mut options = OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            if let Some(mode) = mode {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(mode);
            }
            match options.open(&path) {
                Ok(file) => {
                    let temp = Self {
                        path,
                        file: Some(file),
                    };
                    #[cfg(unix)]
                    if let Some(mode) = mode {
                        use std::os::unix::fs::PermissionsExt;
                        temp.file
                            .as_ref()
                            .ok_or_else(|| anyhow::anyhow!("temporary file is closed"))?
                            .set_permissions(fs::Permissions::from_mode(mode))
                            .with_context(|| format!("chmod {}", temp.path.display()))?;
                    }
                    return Ok(temp);
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => {
                    return Err(error).with_context(|| format!("create {}", path.display()));
                }
            }
        }
        Err(anyhow::anyhow!(
            "could not create unique temporary for {}",
            target.display()
        ))
    }
}

impl Drop for TempFile {
    fn drop(&mut self) {
        drop(self.file.take());
        let _ = fs::remove_file(&self.path);
    }
}

#[cfg(test)]
mod tests;
