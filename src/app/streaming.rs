//! Streaming audio reader that downloads while playing.
//!
//! Implements a Read + Seek trait that blocks when data isn't yet downloaded.

use crate::app::api::error_for_status;
use anyhow::{Context, Result, bail};
use compio::runtime::{JoinHandle, spawn};
use cyper::Response;
use futures::StreamExt;
use see::sync::Sender;
use std::fs::{File, OpenOptions};
use std::io::{Error, ErrorKind, Read, Seek, SeekFrom, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

/// A streaming reader that downloads data while allowing reads.
/// Blocks on read() when the requested position hasn't been downloaded yet.
pub struct StreamingReader {
    /// Shared state between the reader and downloader
    state: Arc<StreamingState>,
    /// The file being written to and read from (shared with downloader)
    file: File,
    /// Temp path for cleanup on drop
    tmp_path: PathBuf,
    /// Handle to cancel writer
    _writer: JoinHandle<()>,
}

#[derive(Default)]
struct StreamingState {
    /// How many bytes have been written to the file
    downloaded: AtomicU64,
    /// Total content length (0 if unknown until headers parsed)
    total: u64,
    /// Mutex + Condvar for efficient blocking waits
    condvar: Condvar,
    /// Mutex to serialize file operations
    file_lock: Mutex<()>,
    /// Whether download has completed or failed
    done: AtomicU64, // 0 = in_progress, 1 = done, 2 = error
    /// Error message if done == 2
    error: Mutex<Option<String>>,
}

impl Drop for StreamingReader {
    fn drop(&mut self) {
        // Clean up temp file if download not complete
        let done = self.state.done.load(Ordering::SeqCst);
        if done != 1 {
            let _ = std::fs::remove_file(&self.tmp_path);
        }
    }
}

impl StreamingState {
    fn of(total: u64) -> Self {
        StreamingState {
            total,
            ..Default::default()
        }
    }
}

impl StreamingReader {
    /// Create a new streaming reader, starting the background download.
    /// Progress updates are sent to `progress_tx` via a watch channel.
    pub async fn new(
        http: &cyper::Client,
        url: &str,
        cache_path: PathBuf,
        cookie: Option<&str>,
        progress_tx: Sender<(u64, u64)>,
    ) -> Result<Self> {
        // Create temp file
        if let Some(parent) = cache_path.parent() {
            std::fs::create_dir_all(parent).context("create streaming cache dir")?;
        }
        let tmp_path = cache_path.with_extension("part");
        let file = OpenOptions::new()
            .create(true)
            .write(true)
            .read(true)
            .truncate(true)
            .open(&tmp_path)
            .context("create streaming temp file")?;

        let mut request = http.get(url)?;
        if let Some(cookie) = cookie {
            request = request.header("Cookie", cookie)?;
        }
        let response = request.send().await?;
        let total = response
            .content_length()
            .context("Music no content_length!")?;
        let state = Arc::new(StreamingState::of(total));

        // Start background download
        let download = download_streaming(
            response,
            tmp_path.clone(),
            cache_path,
            state.clone(),
            progress_tx,
        );

        let state_clone = state.clone();
        let hnd = spawn(async move {
            if let Err(e) = download.await {
                let mut err = state.error.lock().unwrap();
                *err = Some(e.to_string());
                state.done.store(2, Ordering::SeqCst);
                state.condvar.notify_all();
            }
        });

        let reader = Self {
            state: state_clone,
            file,
            tmp_path: tmp_path,
            _writer: hnd,
        };

        Ok(reader)
    }

    /// Wait efficiently until position is available or download completes/fails.
    fn wait_for_position(&self, pos: u64) -> std::io::Result<()> {
        let mut file_lock = self.state.file_lock.lock().unwrap();

        loop {
            let downloaded = self.state.downloaded.load(Ordering::SeqCst);
            let done = self.state.done.load(Ordering::SeqCst);

            if done == 1 && pos >= downloaded {
                // Download complete and we've read all data
                return Ok(());
            }
            if done == 2 {
                let err = self.state.error.lock().unwrap();
                if let Some(msg) = err.as_ref() {
                    return Err(Error::new(ErrorKind::Other, msg.clone()));
                }
                return Err(Error::new(ErrorKind::Other, "download failed"));
            }
            if pos < downloaded {
                // Data at position is available
                return Ok(());
            }

            // Block efficiently until notified
            file_lock = self
                .state
                .condvar
                .wait_timeout(file_lock, Duration::from_secs(1))
                .unwrap()
                .0;
        }
    }

    pub fn total(&self) -> u64 {
        self.state.total
    }
}

impl Read for StreamingReader {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        // Wait for data to be available at current position
        let pos = {
            let _guard = self.state.file_lock.lock().unwrap();
            self.file.stream_position()?
        };

        // Efficiently wait for data at this position
        self.wait_for_position(pos)?;

        // Now read under lock
        let _guard = self.state.file_lock.lock().unwrap();
        self.file.read(buf)
    }
}

impl Seek for StreamingReader {
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        let new_pos = match pos {
            SeekFrom::Start(p) => p,
            SeekFrom::End(p) => {
                // Wait for download to complete to know total size
                loop {
                    let done = self.state.done.load(Ordering::SeqCst);
                    if done == 1 {
                        let total = self.state.total;
                        break total.wrapping_add_signed(p);
                    }
                    if done == 2 {
                        return Err(std::io::Error::new(
                            std::io::ErrorKind::Other,
                            "download failed",
                        ));
                    }

                    // Efficiently wait for download to complete
                    let mut file_lock = self.state.file_lock.lock().unwrap();
                    file_lock = self
                        .state
                        .condvar
                        .wait_timeout(file_lock, Duration::from_secs(1))
                        .unwrap()
                        .0;
                }
            }
            SeekFrom::Current(p) => {
                let _guard = self.state.file_lock.lock().unwrap();
                let current = self.file.stream_position()?;
                current.wrapping_add_signed(p)
            }
        };

        // Wait for the target position if it's beyond downloaded data
        loop {
            let downloaded = self.state.downloaded.load(Ordering::SeqCst);
            let done = self.state.done.load(Ordering::SeqCst);

            if done == 1 {
                // Download done, any position is valid
                break;
            }
            if done == 2 {
                return Err(Error::new(ErrorKind::Other, "download failed"));
            }
            if new_pos <= downloaded {
                break;
            }

            // Efficiently wait for data
            let mut file_lock = self.state.file_lock.lock().unwrap();
            file_lock = self
                .state
                .condvar
                .wait_timeout(file_lock, Duration::from_secs(1))
                .unwrap()
                .0;
        }

        let _guard = self.state.file_lock.lock().unwrap();
        self.file.seek(SeekFrom::Start(new_pos))
    }
}

async fn download_streaming(
    response: Response,
    tmp_path: PathBuf,
    cache_path: PathBuf,
    state: Arc<StreamingState>,
    progress_tx: Sender<(u64, u64)>,
) -> Result<()> {
    let response = error_for_status(response)?;

    // Open file with append mode
    let mut file = {
        let _guard = state.file_lock.lock().unwrap();
        OpenOptions::new()
            .write(true)
            .append(true)
            .open(&tmp_path)
            .context("open streaming temp file for write")?
    };

    let mut downloaded: u64 = 0;
    let mut stream = response.bytes_stream();

    loop {
        let chunk = match stream.next().await {
            Some(Ok(chunk)) => chunk,
            None => break,
            Some(Err(e)) => bail!(e),
        };

        if chunk.is_empty() {
            break;
        }

        // Must hold lock when writing to ensure atomic append and proper read visibility
        {
            let _guard = state.file_lock.lock().unwrap();
            file.write_all(&chunk).context("write streaming chunk")?;
            file.flush().context("flush streaming chunk")?;
        }

        downloaded = downloaded.wrapping_add(chunk.len() as u64);
        state.downloaded.store(downloaded, Ordering::SeqCst);
        let _ = progress_tx.send((downloaded, state.total));
        state.condvar.notify_all();
    }

    // Rename to final cache path (do this with lock held to ensure no readers in middle of read)
    {
        let _guard = state.file_lock.lock().unwrap();
        drop(file);
        std::fs::rename(&tmp_path, cache_path).context("rename streaming temp file")?;
    }

    state.done.store(1, Ordering::SeqCst);
    state.condvar.notify_all();

    Ok(())
}
