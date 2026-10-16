//! Streaming audio reader: async writes on the reactor, blocking reads/seeks only
//! on decoder/audio workers. Published bytes are fully written before readers see them.

use super::api::error_for_status;
use anyhow::Context;
use anyhow::Result;
use anyhow::bail;
use compio::io::AsyncWrite;
use compio::io::AsyncWriteExt;
use cyper::Response;
use futures::StreamExt;
use parking_lot::Condvar;
use parking_lot::Mutex;
use see::sync::Sender;
use std::fs::File;
use std::io::Cursor;
use std::io::Error;
use std::io::ErrorKind;
use std::io::Read;
use std::io::Seek;
use std::io::SeekFrom;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;
use std::time::Duration;

/// Its synchronous Read/Seek implementation must only run on blocking workers.
pub(super) struct StreamingReader {
    state: Arc<StreamingState>,
    file: File,
}

#[derive(Default)]
struct StreamingState {
    downloaded: AtomicU64,
    total: AtomicU64,
    condvar: Condvar,
    // Protects condition changes/waits, NOT file I/O; never held across await.
    wait_lock: Mutex<()>,
    done: AtomicU64, // 0 = in progress, 1 = complete, 2 = error
    error: Mutex<Option<String>>,
    cancelled: AtomicBool,
}

impl StreamingState {
    fn cancel(&self) {
        let _guard = self.wait_lock.lock();
        self.cancelled.store(true, Ordering::SeqCst);
        self.condvar.notify_all();
    }

    fn finish(&self, result: &Result<()>) {
        let _guard = self.wait_lock.lock();
        if let Err(err) = result {
            *self.error.lock() = Some(err.to_string());
            self.done.store(2, Ordering::SeqCst);
        } else {
            self.done.store(1, Ordering::SeqCst);
        }
        self.condvar.notify_all();
    }
}

#[derive(Clone)]
pub(super) struct StreamingReaderHandle {
    state: Arc<StreamingState>,
}

impl StreamingReaderHandle {
    pub(super) fn cancel(&self) {
        self.state.cancel();
    }
}

impl From<&StreamingReader> for StreamingReaderHandle {
    fn from(reader: &StreamingReader) -> Self {
        Self {
            state: reader.state.clone(),
        }
    }
}

impl Drop for StreamingReader {
    fn drop(&mut self) {
        // The detached writer owns its file and removes its unique temporary path
        // after close. Do not cancel/drop its future mid-write or clean up from here.
        self.state.cancel();
    }
}

impl StreamingReader {
    pub(super) async fn new(
        http: &cyper::Client,
        url: &str,
        cache_path: PathBuf,
        cookie: Option<&str>,
        progress_tx: Sender<(u64, u64)>,
    ) -> Result<Self> {
        let mut request = http.get(url)?;
        if let Some(cookie) = cookie {
            request = request.header("Cookie", cookie)?;
        }
        let response = compio::time::timeout(Duration::from_secs(30), request.send())
            .await
            .context("streaming response headers timed out after 30s")??;
        let response = error_for_status(response)?;
        let total = response
            .content_length()
            .context("Music no content_length!")?;
        if total == 0 {
            bail!("empty streaming response");
        }
        if let Some(parent) = cache_path.parent() {
            compio::fs::create_dir_all(parent)
                .await
                .context("create streaming cache dir")?;
        }
        // A cancelled old reader may still be finishing an async write. Never let
        // its close/cleanup truncate or unlink the next reader's temporary file.
        static NEXT_ID: AtomicU64 = AtomicU64::new(0);
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let tmp_path = cache_path.with_extension(format!("{}-{id}.part", std::process::id()));
        let writer_file = compio::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&tmp_path)
            .await?;
        let reader_path = tmp_path.clone();
        let reader_file = compio::runtime::spawn_blocking(move || File::open(reader_path))
            .await
            .map_err(|_| anyhow::anyhow!("streaming file open task panicked"))
            .and_then(|result| result.map_err(Into::into));
        let file = match reader_file {
            Ok(file) => file,
            Err(err) => {
                let _ = writer_file.close().await;
                let _ = compio::fs::remove_file(&tmp_path).await;
                return Err(err);
            }
        };
        let state = Arc::new(StreamingState::default());
        state.total.store(total, Ordering::SeqCst);
        let task_state = state.clone();
        compio::runtime::spawn(async move {
            let result = download_streaming(
                response,
                writer_file,
                &tmp_path,
                cache_path,
                &task_state,
                progress_tx,
            )
            .await;
            task_state.finish(&result);
            if result.is_err() {
                let _ = compio::fs::remove_file(&tmp_path).await;
            }
        })
        .detach();
        Ok(Self { state, file })
    }

    fn wait_for_position(&self, pos: u64, reading: bool) -> std::io::Result<()> {
        let mut guard = self.state.wait_lock.lock();
        loop {
            if self.state.cancelled.load(Ordering::SeqCst) {
                return Err(Error::new(ErrorKind::Interrupted, "streaming cancelled"));
            }
            let done = self.state.done.load(Ordering::SeqCst);
            if done == 2 {
                let error = self.state.error.lock();
                return Err(Error::other(
                    error.as_deref().unwrap_or("download failed").to_owned(),
                ));
            }
            let downloaded = self.state.downloaded.load(Ordering::SeqCst);
            if done == 1 || pos < downloaded || (!reading && pos == downloaded) {
                return Ok(());
            }
            self.state.condvar.wait(&mut guard);
        }
    }

    pub(super) fn total(&self) -> u64 {
        self.state.total.load(Ordering::SeqCst)
    }
}

impl Read for StreamingReader {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        let pos = self.file.stream_position()?;
        self.wait_for_position(pos, true)?;
        // A write may be in flight beyond the published prefix. Limit reads to
        // that prefix, rather than relying on the file's physical length.
        let available = self
            .state
            .downloaded
            .load(Ordering::SeqCst)
            .saturating_sub(pos);
        let len = available.min(buf.len() as u64) as usize;
        self.file.read(&mut buf[..len])
    }
}

impl Seek for StreamingReader {
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        let new_pos = match pos {
            SeekFrom::Start(pos) => pos,
            SeekFrom::End(offset) => {
                self.wait_for_position(self.total(), true)?;
                checked_seek(self.total(), offset)?
            }
            SeekFrom::Current(offset) => checked_seek(self.file.stream_position()?, offset)?,
        };
        self.wait_for_position(new_pos, false)?;
        self.file.seek(SeekFrom::Start(new_pos))
    }
}

fn checked_seek(base: u64, offset: i64) -> std::io::Result<u64> {
    base.checked_add_signed(offset)
        .ok_or_else(|| Error::new(ErrorKind::InvalidInput, "invalid streaming seek position"))
}

async fn download_streaming(
    response: Response,
    file: compio::fs::File,
    tmp_path: &PathBuf,
    cache_path: PathBuf,
    state: &StreamingState,
    progress_tx: Sender<(u64, u64)>,
) -> Result<()> {
    let result: Result<()> = async {
        let mut cursor = Cursor::new(&file);
        let mut stream = response.bytes_stream();
        let total = state.total.load(Ordering::SeqCst);
        let mut downloaded = 0u64;
        loop {
            if state.cancelled.load(Ordering::SeqCst) {
                bail!("streaming cancelled");
            }
            // Timed polling lets cancellation terminate a stalled CDN response.
            let chunk = match compio::time::timeout(Duration::from_millis(500), stream.next()).await
            {
                Ok(Some(Ok(chunk))) if !chunk.is_empty() => chunk,
                Ok(Some(Ok(_))) => continue,
                Ok(Some(Err(err))) => return Err(err.into()),
                Ok(None) => break,
                Err(_) => continue,
            };
            let next = downloaded
                .checked_add(chunk.len() as u64)
                .context("streaming length overflow")?;
            if next > total {
                bail!("streaming response exceeds Content-Length");
            }
            cursor.write_all(chunk).await.0?;
            downloaded = next;
            // Complete positional writes are visible to the independent reader.
            // Pair publication with the wait mutex to prevent missed wakeups.
            {
                let _guard = state.wait_lock.lock();
                state.downloaded.store(downloaded, Ordering::SeqCst);
                state.condvar.notify_all();
            }
            let _ = progress_tx.send((downloaded, total));
        }
        if downloaded != total {
            bail!("streaming Content-Length mismatch: expected {total}, received {downloaded}");
        }
        cursor.flush().await?;
        file.sync_all().await?;
        Ok(())
    }
    .await;
    // Close before propagating errors/cleanup/rename, including cancellation.
    let close = file.close().await;
    result?;
    close?;
    if state.cancelled.load(Ordering::SeqCst) {
        bail!("streaming cancelled");
    }
    compio::fs::rename(tmp_path, cache_path).await?;
    Ok(())
}

#[cfg(test)]
mod tests;
