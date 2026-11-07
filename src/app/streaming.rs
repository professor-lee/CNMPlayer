//! Streaming audio reader: async writes on the reactor, blocking reads/seeks only
//! on decoder/audio workers. Published bytes are fully written before readers see them.

use super::api::NoStreamUrl;
use super::api::error_for_status;
use super::cache_maintenance::CacheMaintenance;
use anyhow::Context;
use anyhow::Result;
use anyhow::bail;
use compio::io::AsyncWrite;
use compio::io::AsyncWriteExt;
use cyper::Response;
use futures::StreamExt;
use futures::future::LocalBoxFuture;
use parking_lot::Condvar;
use parking_lot::Mutex;
use see::sync::Sender;
use std::collections::HashSet;
use std::fs::File;
use std::io::Cursor;
use std::io::Error;
use std::io::ErrorKind;
use std::io::Read;
use std::io::Seek;
use std::io::SeekFrom;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::LazyLock;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;
use std::time::Duration;
use std::time::Instant;

static ACTIVE_STREAMING_TEMPS: LazyLock<Mutex<HashSet<PathBuf>>> =
    LazyLock::new(|| Mutex::new(HashSet::new()));

fn active_streaming_temps() -> &'static Mutex<HashSet<PathBuf>> {
    &ACTIVE_STREAMING_TEMPS
}

fn reserve_streaming_temp(path: &Path) {
    active_streaming_temps().lock().insert(path.to_path_buf());
}

fn release_streaming_temp(path: &Path) {
    active_streaming_temps().lock().remove(path);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProcessLiveness {
    Live,
    Dead,
    Unknown,
}

fn parse_streaming_temp_pid(path: &Path) -> Option<u32> {
    let name = path.file_name()?.to_str()?;
    let name = name.strip_suffix(".part")?;
    let (cache_key, job) = name.rsplit_once('.')?;
    if !cache_key.contains("__") {
        return None;
    }
    let (pid, counter) = job.split_once('-')?;
    if counter.is_empty() {
        return None;
    }
    counter.parse::<u64>().ok()?;
    pid.parse::<u32>().ok()
}

fn process_liveness(pid: u32) -> ProcessLiveness {
    if pid == 0 {
        return ProcessLiveness::Unknown;
    }
    #[cfg(unix)]
    {
        let Ok(pid) = libc::pid_t::try_from(pid) else {
            return ProcessLiveness::Unknown;
        };
        // SAFETY: kill(pid, 0) only probes process existence and sends no signal.
        let result = unsafe { libc::kill(pid, 0) };
        if result == 0 {
            return ProcessLiveness::Live;
        }
        match std::io::Error::last_os_error().raw_os_error() {
            Some(code) if code == libc::ESRCH => ProcessLiveness::Dead,
            Some(code) if code == libc::EPERM => ProcessLiveness::Live,
            _ => ProcessLiveness::Unknown,
        }
    }

    #[cfg(not(unix))]
    {
        let _ = pid;
        ProcessLiveness::Unknown
    }
}

fn should_reclaim_streaming_temp(
    pid: u32,
    current_pid: u32,
    active: bool,
    liveness: ProcessLiveness,
) -> bool {
    if active {
        return false;
    }
    if pid == current_pid {
        // A current-PID file not in the in-process registry is from an earlier
        // incarnation of this PID, so it is safe to treat as an orphan.
        return true;
    }
    matches!(liveness, ProcessLiveness::Dead)
}

/// Remove a temporary audio file only when its streaming naming convention and
/// owning process prove that it is orphaned. The registry lock covers the
/// check-and-remove sequence so a new stream cannot create the same path after
/// the check and before removal.
pub(super) fn reclaim_orphan_streaming_temp(path: &Path) -> bool {
    let Some(pid) = parse_streaming_temp_pid(path) else {
        return false;
    };
    let mut active = active_streaming_temps().lock();
    let should_reclaim = should_reclaim_streaming_temp(
        pid,
        std::process::id(),
        active.contains(path),
        process_liveness(pid),
    );
    if !should_reclaim {
        return false;
    }
    let removed = std::fs::remove_file(path).is_ok();
    if removed {
        active.remove(path);
    }
    removed
}

struct StreamingTempRegistration(PathBuf);

impl Drop for StreamingTempRegistration {
    fn drop(&mut self) {
        release_streaming_temp(&self.0);
    }
}
struct StreamingTempCleanup(Option<PathBuf>);

impl StreamingTempCleanup {
    fn new(path: PathBuf) -> Self {
        Self(Some(path))
    }

    fn disarm(&mut self) {
        self.0.take();
    }
}

impl Drop for StreamingTempCleanup {
    fn drop(&mut self) {
        let Some(path) = self.0.take() else {
            return;
        };
        // Cleanup can run while the reader future is being cancelled on the
        // reactor. Keep the potentially blocking filesystem operation in a
        // detached async task rather than doing it from Drop.
        compio::runtime::spawn(async move {
            let _ = compio::fs::remove_file(path).await;
        })
        .detach();
    }
}
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
    done: AtomicU64, // 0 = in progress, 1 = complete, 2 = error, 3 = cancelled
    error: Mutex<Option<String>>,
    cancelled: AtomicBool,
    /// 与 `Origin` 共享：后台重试/断点续传期间为 true。
    retrying: Arc<AtomicBool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum StreamingStatus {
    Active,
    Complete,
    Cancelled,
    Failed,
}

impl StreamingState {
    fn has_complete_body(&self) -> bool {
        let total = self.total.load(Ordering::SeqCst);
        total > 0 && self.downloaded.load(Ordering::SeqCst) >= total
    }

    fn cancel(&self) {
        let _guard = self.wait_lock.lock();
        self.cancelled.store(true, Ordering::SeqCst);
        self.condvar.notify_all();
    }

    fn finish(&self, result: &Result<()>) {
        self.retrying.store(false, Ordering::SeqCst);
        let _guard = self.wait_lock.lock();
        if result.is_ok() && self.has_complete_body() {
            self.done.store(1, Ordering::SeqCst);
        } else if self.cancelled.load(Ordering::SeqCst) {
            self.done.store(3, Ordering::SeqCst);
        } else if let Err(err) = result {
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

    pub(super) fn status(&self) -> StreamingStatus {
        match self.state.done.load(Ordering::SeqCst) {
            1 => StreamingStatus::Complete,
            2 => StreamingStatus::Failed,
            3 => StreamingStatus::Cancelled,
            _ if self.state.cancelled.load(Ordering::SeqCst) => StreamingStatus::Cancelled,
            _ => StreamingStatus::Active,
        }
    }

    pub(super) fn error_message(&self) -> Option<String> {
        self.state.error.lock().clone()
    }

    pub(super) fn has_complete_body(&self) -> bool {
        self.state.has_complete_body()
    }

    /// 下载正在后台重试（断线重连、换直链）；界面据此显示加载态。
    pub(super) fn is_retrying(&self) -> bool {
        self.state.retrying.load(Ordering::SeqCst)
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
        // after close. Do not cancel/drop its future mid-write or clean up from here
        // once the complete body is already published to the reader.
        if !self.state.has_complete_body() {
            self.state.cancel();
        }
    }
}

/// 取一个新的签名直链。签名会过期、不同次取到的边缘节点也不同，
/// 所以每次重连失败后都换一个新链接。
pub(super) type UrlSource = Box<dyn FnMut() -> LocalBoxFuture<'static, Result<String>>>;

/// 后台重试与断点续传的参数。重试全程不改动任何界面文字。
#[derive(Debug, Clone, Copy)]
pub(super) struct RetryPolicy {
    /// 单次取直链的上限。
    pub(super) url_timeout: Duration,
    /// 单次等响应头的上限。
    pub(super) header_timeout: Duration,
    /// 单个连接多久没有新字节就断开、从断点重连。
    pub(super) body_stall: Duration,
    /// 连续多少次尝试都没有新字节才放弃；有进展就重新计数。
    pub(super) max_attempts: u32,
    /// 第 n 次失败后等待 `base * 2^(n-2)`（首次立即重试），封顶 `max_backoff`。
    pub(super) base_backoff: Duration,
    pub(super) max_backoff: Duration,
}

impl RetryPolicy {
    pub(super) const DEFAULT: Self = Self {
        url_timeout: Duration::from_secs(15),
        header_timeout: Duration::from_secs(15),
        body_stall: Duration::from_secs(15),
        max_attempts: 6,
        base_backoff: Duration::from_secs(1),
        max_backoff: Duration::from_secs(8),
    };

    fn backoff(&self, failures: u32) -> Duration {
        if failures <= 1 {
            return Duration::ZERO;
        }
        let factor = 1u32 << (failures - 2).min(16);
        self.base_backoff
            .saturating_mul(factor)
            .min(self.max_backoff)
    }
}

/// 一次尝试失败的性质：`Retry` 换链接/从断点重连，`Fatal` 立即放弃。
enum Failure {
    Retry(anyhow::Error),
    Fatal(anyhow::Error),
}

/// 等 `fut` 至多 `limit`，期间每 200ms 看一次取消标志；超时或取消返回 `None`。
async fn bounded<F: Future>(fut: F, limit: Duration, cancelled: &AtomicBool) -> Option<F::Output> {
    let deadline = Instant::now() + limit;
    let mut fut = std::pin::pin!(fut);
    loop {
        if cancelled.load(Ordering::SeqCst) {
            return None;
        }
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return None;
        }
        let slice = left.min(Duration::from_millis(200));
        if let Ok(output) = compio::time::timeout(slice, fut.as_mut()).await {
            return Some(output);
        }
    }
}

async fn pause(duration: Duration, cancelled: &AtomicBool) {
    let _ = bounded(std::future::pending::<()>(), duration, cancelled).await;
}

/// `bytes 1000-2023/14210656` → (起点, 总长)。
fn parse_content_range(value: &str) -> Option<(u64, u64)> {
    let range = value.trim().strip_prefix("bytes ")?;
    let (span, total) = range.split_once('/')?;
    let (start, _) = span.split_once('-')?;
    Some((start.trim().parse().ok()?, total.trim().parse().ok()?))
}

/// 一首歌的下载来源。跨重连保留：当前直链、首个响应的 ETag 与重试计数。
pub(super) struct Origin {
    http: cyper::Client,
    cookie: Option<String>,
    source: UrlSource,
    url: Option<String>,
    etag: Option<String>,
    policy: RetryPolicy,
    failures: u32,
    /// 进入退避即置位，拿到新响应头或新字节即清除；首连阶段（还没有
    /// `StreamingReaderHandle`）由播放准备方持有同一个标志读取。
    retrying: Arc<AtomicBool>,
}

impl Origin {
    /// `url` 是已经取到的第一个直链；`source` 用于之后换新链接。
    pub(super) fn new(
        http: cyper::Client,
        cookie: Option<String>,
        url: Option<String>,
        source: UrlSource,
        policy: RetryPolicy,
    ) -> Self {
        Self {
            http,
            cookie,
            source,
            url,
            etag: None,
            policy,
            failures: 0,
            retrying: Arc::new(AtomicBool::new(false)),
        }
    }

    /// 共享的重试标志：首连阶段界面据此显示加载态。
    pub(super) fn retrying(&self) -> Arc<AtomicBool> {
        self.retrying.clone()
    }

    /// 换入调用方预先建好的重试标志（首连阶段界面要读同一个标志）。
    pub(super) fn with_retrying(mut self, retrying: Arc<AtomicBool>) -> Self {
        self.retrying = retrying;
        self
    }

    /// 失败一次：预算用尽返回 `false`，否则按退避等待后返回 `true`。
    async fn backoff_after(&mut self, cancelled: &AtomicBool) -> bool {
        self.failures += 1;
        if self.failures >= self.policy.max_attempts {
            return false;
        }
        self.retrying.store(true, Ordering::SeqCst);
        pause(self.policy.backoff(self.failures), cancelled).await;
        true
    }

    /// 一次连接：从 `offset` 起请求，`total == 0` 表示首个连接（由它决定总长）。
    async fn connect(
        &mut self,
        offset: u64,
        total: u64,
        cancelled: &AtomicBool,
    ) -> std::result::Result<(Response, u64), Failure> {
        let policy = self.policy;
        let url = match self.url.clone() {
            Some(url) => url,
            None => match bounded((self.source)(), policy.url_timeout, cancelled).await {
                Some(Ok(url)) => url,
                Some(Err(err)) if err.is::<NoStreamUrl>() => return Err(Failure::Fatal(err)),
                Some(Err(err)) => return Err(Failure::Retry(err)),
                None => return Err(Failure::Retry(anyhow::anyhow!("stream url timed out"))),
            },
        };
        let result = self.request(&url, offset, total, cancelled).await;
        // 失败后丢掉这个直链：签名可能过期，换一个也常换到更快的节点。
        self.url = result.is_ok().then_some(url);
        result
    }

    async fn request(
        &mut self,
        url: &str,
        offset: u64,
        total: u64,
        cancelled: &AtomicBool,
    ) -> std::result::Result<(Response, u64), Failure> {
        let retry = |err: anyhow::Error| Failure::Retry(err);
        let mut request = self.http.get(url).map_err(|err| retry(err.into()))?;
        if let Some(cookie) = self.cookie.as_deref() {
            request = request
                .header("Cookie", cookie)
                .map_err(|e| retry(e.into()))?;
        }
        if offset > 0 {
            request = request
                .header("Range", format!("bytes={offset}-"))
                .map_err(|e| retry(e.into()))?;
        }
        let response = match bounded(request.send(), self.policy.header_timeout, cancelled).await {
            Some(Ok(response)) => response,
            Some(Err(err)) => return Err(retry(err.into())),
            None => {
                return Err(retry(anyhow::anyhow!(
                    "streaming response headers timed out"
                )));
            }
        };
        let response = error_for_status(response).map_err(retry)?;
        let header = |name: &str| {
            response
                .headers()
                .get(name)
                .and_then(|value| value.to_str().ok())
                .map(str::to_owned)
        };
        let etag = header("ETag");
        if let (Some(known), Some(now)) = (self.etag.as_deref(), etag.as_deref())
            && known != now
        {
            return Err(Failure::Fatal(anyhow::anyhow!("streaming source changed")));
        }
        let skip = if response.status().as_u16() == 206 {
            let (start, size) = header("Content-Range")
                .as_deref()
                .and_then(parse_content_range)
                .ok_or_else(|| retry(anyhow::anyhow!("invalid Content-Range")))?;
            if start != offset || (total > 0 && size != total) {
                return Err(retry(anyhow::anyhow!("unexpected Content-Range")));
            }
            if total == 0 {
                return Err(retry(anyhow::anyhow!("unrequested partial response")));
            }
            0
        } else {
            // 服务器忽略了 Range：整份重发，跳过已有的前缀。
            let length = response
                .content_length()
                .ok_or_else(|| Failure::Fatal(anyhow::anyhow!("Music no content_length!")))?;
            if length == 0 {
                return Err(Failure::Fatal(anyhow::anyhow!("empty streaming response")));
            }
            if total > 0 && length != total {
                return Err(Failure::Fatal(anyhow::anyhow!("streaming source changed")));
            }
            offset
        };
        if self.etag.is_none() {
            self.etag = etag;
        }
        Ok((response, skip))
    }

    /// 重试直到连上、遇到不可重试的错误、被取消或预算用尽。
    async fn open(
        &mut self,
        offset: u64,
        total: u64,
        cancelled: &AtomicBool,
    ) -> Result<(Response, u64)> {
        loop {
            if cancelled.load(Ordering::SeqCst) {
                bail!("streaming cancelled");
            }
            match self.connect(offset, total, cancelled).await {
                Ok(connected) => return Ok(connected),
                Err(Failure::Fatal(err)) => return Err(err),
                Err(Failure::Retry(err)) => {
                    if !self.backoff_after(cancelled).await {
                        return Err(err);
                    }
                }
            }
        }
    }
}

impl StreamingReader {
    pub(super) async fn new(
        mut origin: Origin,
        cache_path: PathBuf,
        progress_tx: Sender<(u64, u64)>,
        cache_maintenance: CacheMaintenance,
    ) -> Result<Self> {
        let writer_lease = cache_maintenance.protect(cache_path.clone());
        let state = Arc::new(StreamingState {
            retrying: origin.retrying(),
            ..StreamingState::default()
        });
        // 首个连接也走重试：取直链、等响应头各自限时，失败换新链接再来。
        let (response, _) = origin.open(0, 0, &state.cancelled).await?;
        let total = response
            .content_length()
            .context("Music no content_length!")?;
        if let Some(parent) = cache_path.parent() {
            compio::fs::create_dir_all(parent)
                .await
                .context("create streaming cache dir")?;
        }
        // A cancelled old reader may still be finishing an async write. Never let
        // its close/cleanup truncate or unlink the next reader's temporary file.
        static NEXT_ID: AtomicU64 = AtomicU64::new(0);
        let (tmp_path, writer_file, registration, cleanup) = loop {
            let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
            let candidate = cache_path.with_extension(format!("{}-{id}.part", std::process::id()));
            reserve_streaming_temp(&candidate);
            let mut cleanup = StreamingTempCleanup::new(candidate.clone());
            let registration = StreamingTempRegistration(candidate.clone());
            match compio::fs::OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(&candidate)
                .await
            {
                Ok(file) => break (candidate, file, registration, cleanup),
                Err(err) if err.kind() == ErrorKind::AlreadyExists => cleanup.disarm(),
                Err(err) => return Err(err.into()),
            }
        };
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
        state.total.store(total, Ordering::SeqCst);
        let task_state = state.clone();
        compio::runtime::spawn(async move {
            let _cleanup = cleanup;
            let _registration = registration;
            let _writer_lease = writer_lease;
            let result = download_streaming(
                response,
                &mut origin,
                writer_file,
                &tmp_path,
                cache_path.clone(),
                &task_state,
                progress_tx,
            )
            .await;
            task_state.finish(&result);
            if result.is_err() {
                let _ = compio::fs::remove_file(&tmp_path).await;
            } else if let Some(parent) = cache_path.parent() {
                cache_maintenance.request(parent.to_path_buf());
            }
        })
        .detach();
        Ok(Self { state, file })
    }

    fn wait_for_position(&self, pos: u64, reading: bool) -> std::io::Result<()> {
        let mut guard = self.state.wait_lock.lock();
        loop {
            let done = self.state.done.load(Ordering::SeqCst);
            if done == 2 {
                let error = self.state.error.lock();
                return Err(Error::other(
                    error.as_deref().unwrap_or("download failed").to_owned(),
                ));
            }
            if done == 1 {
                return Ok(());
            }
            if self.state.cancelled.load(Ordering::SeqCst) {
                return Err(Error::new(ErrorKind::Interrupted, "streaming cancelled"));
            }
            let downloaded = self.state.downloaded.load(Ordering::SeqCst);
            if pos < downloaded || (!reading && pos == downloaded) {
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

/// 下载整首歌。某个连接断开、出错或超过 `body_stall` 没有新字节时，
/// 从已发布的断点用 `Range` 重连（必要时换新直链）续写同一个临时文件；
/// 只有连续 `max_attempts` 次尝试都没有新字节才判失败。
async fn download_streaming(
    response: Response,
    origin: &mut Origin,
    file: compio::fs::File,
    tmp_path: &PathBuf,
    cache_path: PathBuf,
    state: &StreamingState,
    progress_tx: Sender<(u64, u64)>,
) -> Result<()> {
    let result: Result<()> = async {
        let mut cursor = Cursor::new(&file);
        let total = state.total.load(Ordering::SeqCst);
        let mut connection = Some((response, 0));
        loop {
            let (response, skip) = match connection.take() {
                Some(connected) => connected,
                None => {
                    let offset = state.downloaded.load(Ordering::SeqCst);
                    origin.open(offset, total, &state.cancelled).await?
                }
            };
            let before = state.downloaded.load(Ordering::SeqCst);
            let pumped = pump(
                response,
                skip,
                &mut cursor,
                state,
                &progress_tx,
                origin.policy.body_stall,
            )
            .await;
            if state.downloaded.load(Ordering::SeqCst) > before {
                origin.failures = 0;
            }
            match pumped {
                Ok(()) => break,
                Err(Failure::Fatal(err)) => return Err(err),
                Err(Failure::Retry(err)) => {
                    if !origin.backoff_after(&state.cancelled).await {
                        return Err(err);
                    }
                }
            }
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

/// 把一个响应的正文写到 `cursor`（其位置始终等于已发布字节数）。
/// `skip` 是服务器忽略 Range、整份重发时要丢掉的已有前缀。
async fn pump(
    response: Response,
    mut skip: u64,
    cursor: &mut Cursor<&compio::fs::File>,
    state: &StreamingState,
    progress_tx: &Sender<(u64, u64)>,
    body_stall: Duration,
) -> std::result::Result<(), Failure> {
    let mut stream = response.bytes_stream();
    let total = state.total.load(Ordering::SeqCst);
    let mut downloaded = state.downloaded.load(Ordering::SeqCst);
    let mut last_progress = Instant::now();
    loop {
        if state.cancelled.load(Ordering::SeqCst) {
            return Err(Failure::Fatal(anyhow::anyhow!("streaming cancelled")));
        }
        if last_progress.elapsed() >= body_stall {
            return Err(Failure::Retry(anyhow::anyhow!(
                "streaming body stalled with no byte progress"
            )));
        }
        let wait =
            Duration::from_millis(500).min(body_stall.saturating_sub(last_progress.elapsed()));
        let mut chunk = match compio::time::timeout(wait, stream.next()).await {
            Ok(Some(Ok(chunk))) if !chunk.is_empty() => chunk,
            Ok(Some(Ok(_))) => continue,
            Ok(Some(Err(err))) => return Err(Failure::Retry(err.into())),
            Ok(None) => break,
            Err(_) => continue,
        };
        if skip > 0 {
            let dropped = skip.min(chunk.len() as u64);
            skip -= dropped;
            last_progress = Instant::now();
            chunk = chunk.slice(dropped as usize..);
            if chunk.is_empty() {
                continue;
            }
        }
        let next = downloaded + chunk.len() as u64;
        if next > total {
            return Err(Failure::Fatal(anyhow::anyhow!(
                "streaming response exceeds Content-Length"
            )));
        }
        cursor
            .write_all(chunk)
            .await
            .0
            .map_err(|err| Failure::Fatal(err.into()))?;
        downloaded = next;
        last_progress = Instant::now();
        // Pair publication with the wait mutex to prevent missed wakeups.
        {
            let _guard = state.wait_lock.lock();
            state.downloaded.store(downloaded, Ordering::SeqCst);
            state.condvar.notify_all();
        }
        state.retrying.store(false, Ordering::SeqCst);
        let _ = progress_tx.send((downloaded, total));
    }
    if downloaded != total {
        return Err(Failure::Retry(anyhow::anyhow!(
            "streaming Content-Length mismatch: expected {total}, received {downloaded}"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests;
