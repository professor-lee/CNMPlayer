use anyhow::{Context, Result};
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);
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
            let path = parent.join(format!(".{base}-{pid}-{now}-{n}.tmp"));
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
