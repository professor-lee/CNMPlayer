//! Streaming audio reader that downloads while playing.
//!
//! Implements a Read + Seek trait that blocks when data isn't yet downloaded.

use crate::app::api::error_for_status;
use anyhow::{Context, Result, bail};
use compio::fs::rename;
use compio::io::{AsyncWrite, AsyncWriteExt};
use compio::runtime::{JoinHandle, spawn};
use cyper::Response;
use futures::StreamExt;
use see::sync::Sender;
use std::fs::File;
use std::io::{Cursor, Error, ErrorKind, Read, Seek, SeekFrom};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

/// A streaming reader that downloads data while allowing reads.
/// Blocks on read() when the requested position hasn't been downloaded yet.
pub struct StreamingReader {
    /// Shared state between the reader and downloader
    state: Arc<StreamingState>,
    /// The reader's own file handle. The writer uses a separate handle, so
    /// this cursor is only ever advanced by the reader itself and file
    /// operations never contend with the writer.
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
    /// Mutex + Condvar for efficient blocking waits. The mutex is only used
    /// for the condvar predicate protocol (check-then-wait must be atomic);
    /// file I/O itself is lock-free thanks to the separate reader/writer
    /// handles.
    condvar: Condvar,
    file_lock: Mutex<()>,
    /// Whether download has completed or failed
    done: AtomicU64, // 0 = in_progress, 1 = done, 2 = error
    /// Set when the reader is dropped; the writer stops fetching and never
    /// renames the temp file into the cache.
    cancelled: AtomicBool,
    /// Error message if done == 2
    error: Mutex<Option<String>>,
}

impl Drop for StreamingReader {
    fn drop(&mut self) {
        // Cancel the background download: stop fetching and never rename the
        // temp file into the cache after the reader is gone.
        self.state.cancelled.store(true, Ordering::Release);

        // Clean up temp file if download not complete
        let done = self.state.done.load(Ordering::Acquire);
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

        use std::fs::OpenOptions;
        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(true)
            .open(&tmp_path)?;

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
                state.done.store(2, Ordering::Release);
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
            let downloaded = self.state.downloaded.load(Ordering::Acquire);
            let done = self.state.done.load(Ordering::Acquire);

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
        let pos = self.file.stream_position()?;

        // Efficiently wait for data at this position
        self.wait_for_position(pos)?;

        // The reader owns this handle's cursor, so no lock is needed here.
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
                    let done = self.state.done.load(Ordering::Acquire);
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
                let current = self.file.stream_position()?;
                current.wrapping_add_signed(p)
            }
        };

        // Wait for the target position if it's beyond downloaded data
        loop {
            let downloaded = self.state.downloaded.load(Ordering::Acquire);
            let done = self.state.done.load(Ordering::Acquire);

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

    // The writer opens its own handle; the reader keeps a separate one. File
    // data is visible across handles (page cache), so I/O never needs the
    // state mutex - that mutex is only used for the condvar wait protocol.
    let file = {
        use compio::fs::OpenOptions;
        OpenOptions::new().write(true).open(&tmp_path).await?
    };
    let mut cursor = Cursor::new(&file);

    let mut downloaded: u64 = 0;
    let mut stream = response.bytes_stream();

    loop {
        // Stop early when the reader has been dropped (e.g. track switched).
        if state.cancelled.load(Ordering::Acquire) {
            let _ = std::fs::remove_file(&tmp_path);
            return Ok(());
        }

        let chunk = match stream.next().await {
            Some(Err(e)) => bail!(e),
            Some(Ok(chunk)) if !chunk.is_empty() => chunk,
            _ => break,
        };

        let len = chunk.len();
        cursor.write_all(chunk).await.0?;

        downloaded += len as u64;
        {
            let _guard = state.file_lock.lock().unwrap();
            state.downloaded.store(downloaded, Ordering::Release);
        }
        let _ = progress_tx.send((downloaded, state.total));
        state.condvar.notify_all();
    }

    // Don't move the partial file into the cache if the reader is gone.
    if state.cancelled.load(Ordering::Acquire) {
        let _ = std::fs::remove_file(&tmp_path);
        return Ok(());
    }

    // Flush once at the end (write_all already lands data in the page cache,
    // which readers observe immediately) before renaming into the cache.
    cursor.flush().await?;
    file.close().await?;
    rename(&tmp_path, cache_path).await?;

    {
        let _guard = state.file_lock.lock().unwrap();
        state.done.store(1, Ordering::Release);
    }
    state.condvar.notify_all();

    Ok(())
}
