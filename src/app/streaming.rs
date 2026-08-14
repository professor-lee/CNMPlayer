//! Streaming audio reader that downloads while playing.
//!
//! Implements a Read + Seek trait that blocks when data isn't yet downloaded.

use crate::app::api::error_for_status;
use anyhow::{Context, Result, bail};
use compio::fs::rename;
use compio::io::{AsyncWrite, AsyncWriteExt};
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
    /// The file being written to and read from (shared with downloader)
    file: File,
    /// Temp path for cleanup on drop
    tmp_path: PathBuf,
    /// 下载线程句柄（下载跑在专用 OS 线程 + 独立 compio 运行时上）
    _writer: std::thread::JoinHandle<()>,
}

#[derive(Default)]
struct StreamingState {
    /// How many bytes have been written to the file
    downloaded: AtomicU64,
    /// Total content length（0 表示响应头尚未到达）
    total: AtomicU64,
    /// Mutex + Condvar for efficient blocking waits
    condvar: Condvar,
    /// Mutex to serialize file operations
    file_lock: Mutex<()>,
    /// Whether download has completed or failed
    done: AtomicU64, // 0 = in_progress, 1 = done, 2 = error
    /// Error message if done == 2
    error: Mutex<Option<String>>,
    /// 切歌/丢弃时置位：唤醒阻塞中的 read/seek 等待，并让下载线程提前退出
    cancelled: AtomicBool,
}

/// 流媒体读取器的外部控制句柄：切歌时用于取消阻塞中的下载/seek 等待。
#[derive(Clone)]
pub struct StreamingReaderHandle {
    state: Arc<StreamingState>,
}

impl StreamingReaderHandle {
    /// 取消流：唤醒所有阻塞在 read/seek 等待上的线程并使其尽快返回错误，
    /// 同时通知下载线程退出。用于避免切歌时音频线程与下载互相等待形成死锁。
    pub fn cancel(&self) {
        self.state.cancelled.store(true, Ordering::SeqCst);
        self.state.condvar.notify_all();
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
        // 通知下载线程尽早退出（不把半截文件写入正式缓存），
        // 并唤醒可能阻塞在 read/seek 等待上的线程。
        self.state.cancelled.store(true, Ordering::SeqCst);
        self.state.condvar.notify_all();
        // Clean up temp file if download not complete
        let done = self.state.done.load(Ordering::SeqCst);
        if done != 1 {
            let _ = std::fs::remove_file(&self.tmp_path);
        }
    }
}

/// 在下载线程内新建 HTTP 客户端：cyper::Client 不是 Send（内部含 Rc），
/// 无法跨线程共享；默认头与 App::new 中保持一致。
fn build_download_client() -> Result<cyper::Client> {
    use http::header;
    let mut headers = header::HeaderMap::new();
    headers.insert(
        header::USER_AGENT,
        header::HeaderValue::from_static("Mozilla/5.0 CNMPlayer/0.1"),
    );
    headers.insert(
        header::REFERER,
        header::HeaderValue::from_static("https://music.163.com/"),
    );
    cyper::Client::builder()
        .default_headers(headers)
        .build()
        .context("build download http client")
}

impl StreamingReader {
    /// Create a new streaming reader, starting the background download.
    /// Progress updates are sent to `progress_tx` via a watch channel.
    pub async fn new(
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

        let state = Arc::new(StreamingState::default());

        // 下载放在专用 OS 线程 + 独立 compio 运行时上执行：主线程是单线程
        // 运行时，一旦被同步阻塞（例如 seek 等待加载）就会让挂在它上面的
        // 任务全部停摆。独立线程保证“播放冻结等待加载”必然被推进。
        // cyper 的响应流绑定创建它的运行时，因此整个请求也要在下载线程内发起。
        let state_for_thread = state.clone();
        let url = url.to_string();
        let cookie = cookie.map(str::to_owned);
        let tmp_path_for_thread = tmp_path.clone();
        let cache_path_for_thread = cache_path.clone();
        let writer = std::thread::spawn(move || {
            let result = (|| -> Result<()> {
                let runtime = compio::runtime::Runtime::builder()
                    .build()
                    .context("build download runtime")?;
                runtime.block_on(async {
                    // cyper::Client 不是 Send（内部含 Rc），因此在下载线程内新建
                    let http_client = build_download_client()?;
                    let mut request = http_client.get(&url)?;
                    if let Some(cookie) = cookie.as_deref() {
                        request = request.header("Cookie", cookie)?;
                    }
                    let response = request.send().await?;
                    let total = response
                        .content_length()
                        .context("Music no content_length!")?;
                    state_for_thread.total.store(total, Ordering::SeqCst);
                    state_for_thread.condvar.notify_all();

                    download_streaming(
                        response,
                        tmp_path_for_thread,
                        cache_path_for_thread,
                        state_for_thread.clone(),
                        progress_tx,
                    )
                    .await
                })
            })();
            if let Err(e) = result {
                let mut err = state_for_thread.error.lock().unwrap();
                *err = Some(e.to_string());
                state_for_thread.done.store(2, Ordering::SeqCst);
                state_for_thread.condvar.notify_all();
            }
        });

        // 等待响应头（content-length）就绪或失败后返回
        loop {
            if state.done.load(Ordering::SeqCst) == 2 {
                let msg = state
                    .error
                    .lock()
                    .unwrap()
                    .clone()
                    .unwrap_or_else(|| "download failed".to_string());
                return Err(anyhow::anyhow!("{msg}"));
            }
            if state.total.load(Ordering::SeqCst) > 0 {
                break;
            }
            let mut guard = state.file_lock.lock().unwrap();
            let (next, _) = state
                .condvar
                .wait_timeout(guard, Duration::from_secs(1))
                .unwrap();
            guard = next;
        }

        let reader = Self {
            state,
            file,
            tmp_path: tmp_path,
            _writer: writer,
        };

        Ok(reader)
    }

    /// Wait efficiently until position is available or download completes/fails.
    fn wait_for_position(&self, pos: u64) -> std::io::Result<()> {
        let mut file_lock = self.state.file_lock.lock().unwrap();

        loop {
            if self.state.cancelled.load(Ordering::SeqCst) {
                return Err(Error::new(ErrorKind::Interrupted, "streaming cancelled"));
            }

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
        self.state.total.load(Ordering::SeqCst)
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
                    if self.state.cancelled.load(Ordering::SeqCst) {
                        return Err(std::io::Error::new(
                            std::io::ErrorKind::Interrupted,
                            "streaming cancelled",
                        ));
                    }

                    let done = self.state.done.load(Ordering::SeqCst);
                    if done == 1 {
                        let total = self.state.total.load(Ordering::SeqCst);
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

        // Wait for the target position if it's beyond downloaded data.
        // 保持阻塞等待（播放冻结）：下载线程独立推进，追上目标后继续播放。
        loop {
            if self.state.cancelled.load(Ordering::SeqCst) {
                return Err(Error::new(ErrorKind::Interrupted, "streaming cancelled"));
            }

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
    let file = {
        let _guard = state.file_lock.lock().unwrap();
        use compio::fs::OpenOptions;
        OpenOptions::new().write(true).open(&tmp_path).await?
    };
    let mut cursor = Cursor::new(&file);

    let mut downloaded: u64 = 0;
    let mut stream = response.bytes_stream();

    loop {
        if state.cancelled.load(Ordering::SeqCst) {
            // 切歌取消下载：不写入正式缓存，保留 .part 由 Drop 清理
            return Ok(());
        }

        let chunk = match stream.next().await {
            Some(Err(e)) => bail!(e),
            Some(Ok(chunk)) if !chunk.is_empty() => chunk,
            _ => break,
        };

        let len = chunk.len();

        // Must hold lock when writing to ensure atomic append and proper read visibility
        {
            let _guard = state.file_lock.lock().unwrap();
            cursor.write_all(chunk).await.0?;
            cursor.flush().await?
        }

        downloaded += len as u64;
        state.downloaded.store(downloaded, Ordering::SeqCst);
        let _ = progress_tx.send((downloaded, state.total.load(Ordering::SeqCst)));
        state.condvar.notify_all();
    }

    if state.cancelled.load(Ordering::SeqCst) {
        // 已被丢弃：不把半截文件写入正式缓存
        return Ok(());
    }

    // Rename to final cache path (do this with lock held to ensure no readers in middle of read)
    {
        let _guard = state.file_lock.lock().unwrap();
        file.close().await?;
        rename(&tmp_path, cache_path).await?;
    }

    state.done.store(1, Ordering::SeqCst);
    state.condvar.notify_all();

    Ok(())
}
