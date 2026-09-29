//! 下载：目标目录解析、任务管理（取流 → 落盘 → 写标签/封面/歌词）、取消与状态查询。
//!
//! 单线程 compio runtime：所有阻塞 IO 都经 `spawn_blocking`，网络与文件写全程 await。

use crate::app::api::ApiState;
use crate::data::config::AudioQuality;
use crate::launch;
use anyhow::{Context, Result};
use compio::io::{AsyncWrite, AsyncWriteExt};
use directories::{BaseDirs, UserDirs};
use futures::StreamExt;
use futures::channel::mpsc::{TryRecvError, UnboundedReceiver, UnboundedSender, unbounded};
use parking_lot::Mutex;
use std::collections::HashMap;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// 默认下载目录名：`{系统音乐目录}/cnmplayer/`。
pub const DOWNLOAD_DIR_NAME: &str = "cnmplayer";

/// 系统既没有音乐目录也没有家目录时的显示值（设置弹窗里照原样显示这个字面量）。
pub const DOWNLOAD_PATH_NULL: &str = "Null";

/// 认识这两种载荷（`type` 字段只会有这两个值；网易云的九个档位都落在这两容器里）。
pub const AUDIO_EXTENSIONS: [&str; 2] = ["mp3", "flac"];

/// 取消标志的轮询间隔（读取流时的超时切片）。
const CANCEL_POLL_INTERVAL: Duration = Duration::from_millis(500);

/// 未下载（Nerd Font `ec74`）。
pub const ICON_DOWNLOAD: char = '\u{ec74}';
/// 已下载（Nerd Font `f00c`）。
pub const ICON_DONE: char = '\u{f00c}';
/// 下载中的旋转帧：Braille 转轮（`⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏`）。
const SPINNER_FRAMES: [char; 10] = [
    '\u{280b}', '\u{2819}', '\u{2839}', '\u{2838}', '\u{283c}', '\u{2834}', '\u{2826}', '\u{2827}',
    '\u{2807}', '\u{280f}',
];
const SPINNER_FRAME_MS: u128 = 100;

/// 下载按钮/图标三态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DownloadState {
    NotDownloaded,
    Downloading,
    Done,
}

/// 三态对应的字形；`Downloading` 按相位取旋转帧（time-based，空闲节流下也自洽）。
pub fn state_glyph(state: DownloadState, phase: Duration) -> char {
    match state {
        DownloadState::NotDownloaded => ICON_DOWNLOAD,
        DownloadState::Done => ICON_DONE,
        DownloadState::Downloading => {
            let frame = (phase.as_millis() / SPINNER_FRAME_MS) as usize;
            SPINNER_FRAMES[frame % SPINNER_FRAMES.len()]
        }
    }
}

/// 下载目录不可用的原因（文案由两端各自的 `lang_text` 生成）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DownloadPathError {
    /// 不是绝对路径。
    NotAbsolute,
    /// 建不出来或不可写。
    NotWritable,
}

/// 用户在设置里填的下载目录。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DownloadPathChoice {
    /// 显式禁用下载（填的就是字面量 `Null`）。
    Disabled,
    /// 可写的绝对路径。
    Dir(PathBuf),
}

/// 解析实际使用的下载目录。
///
/// 优先级：用户自定义（`Null` = 显式禁用）→ 系统音乐目录下的 `cnmplayer/`
/// → `~/Music/cnmplayer/` → `None`（下载整体禁用，UI 不显示下载入口）。
pub fn resolve_download_root(custom: Option<&str>) -> Option<PathBuf> {
    if let Some(raw) = custom.map(str::trim).filter(|value| !value.is_empty()) {
        if is_null_download_path(raw) {
            return None;
        }
        return Some(PathBuf::from(raw));
    }

    if let Some(audio) = UserDirs::new().and_then(|dirs| dirs.audio_dir().map(Path::to_path_buf)) {
        return Some(audio.join(DOWNLOAD_DIR_NAME));
    }

    BaseDirs::new().map(|dirs| dirs.home_dir().join("Music").join(DOWNLOAD_DIR_NAME))
}

/// 填的是不是"禁用下载"的哨兵值（`Null`，忽略大小写与首尾空白）。
pub fn is_null_download_path(raw: &str) -> bool {
    raw.trim().eq_ignore_ascii_case(DOWNLOAD_PATH_NULL)
}

/// 解析并校验用户填写的下载目录。
///
/// 留空或填 `Null`（忽略大小写与首尾空白）都表示**显式禁用下载**；
/// 其余必须是非空的绝对路径、能建出来且可写。
pub fn parse_download_path(raw: &str) -> Result<DownloadPathChoice, DownloadPathError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || is_null_download_path(trimmed) {
        return Ok(DownloadPathChoice::Disabled);
    }

    let path = PathBuf::from(trimmed);
    if !path.is_absolute() {
        return Err(DownloadPathError::NotAbsolute);
    }

    if !is_writable_dir(&path) {
        return Err(DownloadPathError::NotWritable);
    }

    Ok(DownloadPathChoice::Dir(path))
}

/// 目录可写性用一次真实的建/删探针判定：`create_dir_all` 在"目录已存在但只读"时也会成功。
fn is_writable_dir(dir: &Path) -> bool {
    if std::fs::create_dir_all(dir).is_err() {
        return false;
    }

    let probe = dir.join(format!(".cnmplayer-write-test-{}", std::process::id()));
    match std::fs::write(&probe, b"") {
        Ok(()) => {
            let _ = std::fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

/// 文件名片段清理：路径分隔符换成 `-`（`AC/DC` → `AC-DC`）、其余非法字符丢掉、
/// 连续空白压成一个空格，空串回落 `fallback`。
fn sanitize_component(raw: &str, fallback: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for ch in raw.chars() {
        if ch.is_control() {
            continue;
        }
        if matches!(ch, '/' | '\\' | ':') {
            out.push('-');
            continue;
        }
        if matches!(ch, '*' | '?' | '"' | '<' | '>' | '|') {
            continue;
        }
        out.push(ch);
    }

    // 空白压缩：分隔符替换后容易留下连续空格（"A / B" → "A - B" 之外的情况）。
    let mut collapsed = String::with_capacity(out.len());
    let mut last_space = false;
    for ch in out.chars() {
        let is_space = ch.is_whitespace();
        if is_space && last_space {
            continue;
        }
        collapsed.push(if is_space { ' ' } else { ch });
        last_space = is_space;
    }

    let trimmed = collapsed.trim().trim_matches('.').trim();
    let mut text = if trimmed.is_empty() {
        fallback.to_string()
    } else {
        trimmed.to_string()
    };

    // 单个组件别超过 150 字符，避免 CJK 长标题撞上文件系统的 255 字节上限。
    if text.chars().count() > 150 {
        text = text.chars().take(150).collect();
    }
    text
}

/// 文件名主干：`标题 - 作者 - 专辑`（缺失的段直接省略，专辑名里已含标题时也保留）。
pub fn download_file_stem(title: &str, artist: &str, album: &str) -> String {
    let mut parts = vec![sanitize_component(title, "未知歌曲")];
    for raw in [artist, album] {
        let part = sanitize_component(raw, "");
        if !part.is_empty() {
            parts.push(part);
        }
    }
    parts.join(" - ")
}

/// 一次下载的目标位置：下载根目录 + 文件名主干（所有下载都直接落根目录）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DownloadTarget {
    pub dir: PathBuf,
    pub base: String,
}

impl DownloadTarget {
    pub fn file_path(&self, ext: &str) -> PathBuf {
        self.dir.join(format!("{}.{ext}", self.base))
    }

    pub fn part_path(&self, ext: &str) -> PathBuf {
        self.dir.join(format!("{}.{ext}.part", self.base))
    }

    /// 已经落盘的音频（两个扩展名都认），用于"已下载"图标。
    pub fn existing_file(&self) -> Option<PathBuf> {
        AUDIO_EXTENSIONS
            .iter()
            .map(|ext| self.file_path(ext))
            .find(|path| path.is_file())
    }
}

/// 下载请求：UI 侧决定落点，任务侧只负责取流与写标签。
#[derive(Debug, Clone)]
pub struct DownloadRequest {
    pub song_id: String,
    pub level: AudioQuality,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub target: DownloadTarget,
}

/// 任务结束的通知（由 `DownloadManager::poll` 交给 UI 写状态行）。
#[derive(Debug, Clone)]
pub enum DownloadEvent {
    /// 排队中的任务真正开始写盘（先来后到，队列里可能有多个）。
    Started {
        title: String,
        level: AudioQuality,
    },
    Finished {
        title: String,
        path: PathBuf,
        level: String,
        file_type: String,
        tag_error: Option<String>,
    },
    Failed {
        title: String,
        error: String,
    },
    Cancelled {
        title: String,
    },
}

/// 任务结果。
#[derive(Debug, Clone)]
pub struct DownloadOutcome {
    pub path: PathBuf,
    pub level: String,
    pub file_type: String,
    /// 标签/封面写入失败不影响音频落盘，单独报告。
    pub tag_error: Option<String>,
}

#[derive(Debug, Clone)]
enum TaskError {
    Cancelled,
    Failed(String),
}

#[derive(Default)]
struct JobShared {
    cancelled: AtomicBool,
    /// 队列里的任务真正轮到自己（开始写盘）时置位。
    started: AtomicBool,
    outcome: Mutex<Option<Result<DownloadOutcome, TaskError>>>,
}

struct JobHandle {
    title: String,
    level: AudioQuality,
    cancelling: bool,
    start_reported: bool,
    shared: Arc<JobShared>,
}

/// 行内图标的预计算行：文件名与磁盘缓存 key 随列表一起生成一次，
/// 之后每帧只做一次 HashMap 查，不再重建 key。
#[derive(Debug, Clone)]
pub struct DownloadRow {
    pub song_id: String,
    pub target: DownloadTarget,
    key: String,
}

impl DownloadRow {
    pub fn new(song_id: String, target: DownloadTarget) -> Self {
        let key = target.dir.join(&target.base).display().to_string();
        Self {
            song_id,
            target,
            key,
        }
    }

    /// 磁盘缓存 key（与 `DownloadManager::state_of` 内部一致）。
    #[cfg(test)]
    pub fn key(&self) -> &str {
        &self.key
    }
}

/// 行内图标状态的 memo。
///
/// - `epoch`（列表代 × 下载根目录代）变化 → 重建行数据（做一次 `sanitize`/`join`）；
/// - `DownloadManager::version`（任务开始 / 结束 / 取消 / 磁盘缓存作废）变化 → 重算状态。
///
/// 两个都不变时每帧只做一次 u64 比较，行内取值是一次切片索引。
#[derive(Default)]
pub struct DownloadRowCache {
    epoch: u64,
    initialized: bool,
    rows: Vec<Option<DownloadRow>>,
    states: Vec<Option<DownloadState>>,
    states_version: u64,
}

impl DownloadRowCache {
    pub fn refresh(
        &mut self,
        epoch: u64,
        manager: &mut DownloadManager,
        build: impl FnOnce() -> Vec<Option<DownloadRow>>,
    ) {
        let version = manager.version();
        self.refresh_inner(epoch, version, build, |row| manager.state_of_row(row));
    }

    fn refresh_inner(
        &mut self,
        epoch: u64,
        version: u64,
        build: impl FnOnce() -> Vec<Option<DownloadRow>>,
        mut resolve: impl FnMut(&DownloadRow) -> DownloadState,
    ) {
        if !self.initialized || self.epoch != epoch {
            self.rows = build();
            self.states = vec![None; self.rows.len()];
            self.epoch = epoch;
            self.initialized = true;
            // 版本对不上就重算一次：换列表后必须重新查一遍任务表。
            self.states_version = u64::MAX;
        }

        if self.states_version != version {
            self.states = self
                .rows
                .iter()
                .map(|row| row.as_ref().map(&mut resolve))
                .collect();
            self.states_version = version;
        }
    }

    pub fn state_at(&self, index: usize) -> Option<DownloadState> {
        self.states.get(index).copied().flatten()
    }
}

/// 队列消息：一次下载请求 + 它的结果槽。
#[derive(Clone)]
struct DownloadMessage {
    request: DownloadRequest,
    shared: Arc<JobShared>,
}

/// 下载任务表 + 落盘状态缓存。
///
/// 全局只有一个下载任务（`loop_downloads`）：`enqueue` 只把请求塞进队列，
/// 队列按先来后到顺序执行——同一时刻最多一个在写盘，取消在途任务后下一个
/// 立刻顶上。队列排空后任务自己退出（空闲不留常驻 async 任务），
/// 下一次 `enqueue` 再起一个。进度与结果通过每个任务自己的 `JobShared` 回传。
pub struct DownloadManager {
    /// 当前在途的下载任务；`None` 或已关闭 = 没有任务占着（下次按需现起）。
    worker_tx: Option<UnboundedSender<DownloadMessage>>,
    /// 起新任务时用的 API 句柄（`enqueue` 时刷新，保证带最新 cookie）。
    api: ApiState,
    jobs: HashMap<String, JobHandle>,
    /// `目标前缀 -> 是否已落盘`：避免每帧对每一行都 stat 磁盘。
    disk_cache: HashMap<String, bool>,
    /// 任务表 / 磁盘缓存的变化计数：行内 memo 据此决定是否重算状态。
    version: u64,
}

impl DownloadManager {
    pub fn new(api: ApiState) -> Self {
        Self {
            worker_tx: None,
            api,
            jobs: HashMap::new(),
            disk_cache: HashMap::new(),
            version: 0,
        }
    }

    /// 任务表 / 磁盘缓存的变化计数（事件驱动 memo 的版本号）。
    pub fn version(&self) -> u64 {
        self.version
    }

    fn bump_version(&mut self) {
        self.version = self.version.wrapping_add(1);
    }

    /// 发起下载（进队列）；同一首歌已在队列 / 下载中时返回错误文案。
    pub fn enqueue(&mut self, api: &ApiState, request: DownloadRequest) -> Result<(), String> {
        if let Some(job) = self.jobs.get(&request.song_id) {
            return Err(if job.cancelling {
                "正在取消上一任务，请稍候".to_string()
            } else {
                "该歌曲已在下载中".to_string()
            });
        }

        self.api = api.clone();
        let shared = Arc::new(JobShared::default());
        self.jobs.insert(
            request.song_id.clone(),
            JobHandle {
                title: request.title.clone(),
                level: request.level,
                cancelling: false,
                start_reported: false,
                shared: shared.clone(),
            },
        );
        self.disk_cache.remove(
            &request
                .target
                .dir
                .join(&request.target.base)
                .display()
                .to_string(),
        );
        self.bump_version();

        self.dispatch(DownloadMessage { request, shared })
    }

    /// 把消息送进队列；没有活着的任务就现起一个。
    ///
    /// 任务在队列排空后会自行退出（接收端随之 drop）：这里在建/发之前先看
    /// `is_closed`，发送失败再重建一次——同一条线程内两者都不含 await，
    /// 不存在"消息已入队但接收端恰好退出"的交错。
    fn dispatch(&mut self, message: DownloadMessage) -> Result<(), String> {
        self.ensure_worker();
        let retry = message.clone();
        if let Some(mut tx) = self.worker_tx.clone()
            && tx.start_send(message).is_ok()
        {
            return Ok(());
        }

        // 任务刚好退出（极罕见）：换一个任务把同一条消息再投一次。
        self.worker_tx = None;
        self.ensure_worker();
        if let Some(mut tx) = self.worker_tx.clone()
            && tx.start_send(retry).is_ok()
        {
            return Ok(());
        }
        Err("下载任务未能启动".to_string())
    }

    /// 确保有一个活着的下载任务（`None` 或已关闭时现起一个）。
    fn ensure_worker(&mut self) {
        if self.worker_tx.as_ref().is_some_and(|tx| !tx.is_closed()) {
            return;
        }
        let (tx, rx) = unbounded();
        launch(loop_downloads(rx, self.api.clone()));
        self.worker_tx = Some(tx);
    }

    /// 请求取消：立刻把图标恢复成"未下载"，任务退出前不允许对同一首歌再发起下载。
    pub fn cancel(&mut self, song_id: &str) -> bool {
        let Some(job) = self.jobs.get_mut(song_id) else {
            return false;
        };
        job.cancelling = true;
        job.shared.cancelled.store(true, Ordering::SeqCst);
        // 图标由"下载中"变回"未下载"，memo 需要重算。
        self.bump_version();
        true
    }

    /// 是否有任务在途（含正在取消的），用于高频重绘判定。
    pub fn is_active(&self) -> bool {
        self.jobs.values().any(|job| !job.cancelling)
    }

    /// 该歌曲是否有在途任务（含正在取消的）。
    pub fn is_busy(&self, song_id: &str) -> bool {
        self.jobs.contains_key(song_id)
    }

    /// 该歌曲是否处于可取消的下载中。
    pub fn is_downloading(&self, song_id: &str) -> bool {
        self.jobs.get(song_id).is_some_and(|job| !job.cancelling)
    }

    /// 每帧搬运完成的任务。
    pub fn poll(&mut self) -> Vec<DownloadEvent> {
        let mut events = Vec::new();

        // 队列里轮到自己开始写盘的任务：报一次"开始下载"（先来后到）。
        for job in self.jobs.values_mut() {
            if !job.start_reported && job.shared.started.load(Ordering::SeqCst) {
                job.start_reported = true;
                events.push(DownloadEvent::Started {
                    title: job.title.clone(),
                    level: job.level,
                });
            }
        }

        let finished: Vec<String> = self
            .jobs
            .iter()
            .filter(|(_, job)| job.shared.outcome.lock().is_some())
            .map(|(song_id, _)| song_id.clone())
            .collect();

        for song_id in finished {
            let Some(job) = self.jobs.remove(&song_id) else {
                continue;
            };
            let Some(result) = job.shared.outcome.lock().take() else {
                continue;
            };
            // 任务离场（完成 / 失败 / 取消）都会让图标换态，memo 需要重算。
            self.bump_version();

            match result {
                Ok(outcome) => {
                    if let Some(parent) = outcome.path.parent() {
                        let stem = outcome
                            .path
                            .file_stem()
                            .map(|value| value.to_string_lossy().to_string())
                            .unwrap_or_default();
                        self.disk_cache
                            .insert(parent.join(&stem).display().to_string(), true);
                    }
                    events.push(DownloadEvent::Finished {
                        title: job.title,
                        path: outcome.path,
                        level: outcome.level,
                        file_type: outcome.file_type,
                        tag_error: outcome.tag_error,
                    });
                }
                Err(TaskError::Cancelled) => {
                    events.push(DownloadEvent::Cancelled { title: job.title })
                }
                Err(TaskError::Failed(error)) => events.push(DownloadEvent::Failed {
                    title: job.title,
                    error,
                }),
            }
        }

        events
    }

    /// 图标三态：任务在途 → 下载中；否则查磁盘（带缓存）。
    pub fn state_of(&mut self, song_id: &str, target: &DownloadTarget) -> DownloadState {
        let key = target.dir.join(&target.base).display().to_string();
        self.state_of_key(song_id, &key, target)
    }

    /// 用预计算行查状态：key 不重建，命中缓存时零分配。
    pub fn state_of_row(&mut self, row: &DownloadRow) -> DownloadState {
        self.state_of_key(&row.song_id, &row.key, &row.target)
    }

    fn state_of_key(&mut self, song_id: &str, key: &str, target: &DownloadTarget) -> DownloadState {
        if let Some(job) = self.jobs.get(song_id) {
            return if job.cancelling {
                DownloadState::NotDownloaded
            } else {
                DownloadState::Downloading
            };
        }

        if let Some(done) = self.disk_cache.get(key) {
            return if *done {
                DownloadState::Done
            } else {
                DownloadState::NotDownloaded
            };
        }

        let done = target.existing_file().is_some();
        // 缓存别无限涨：超过阈值就整体清空（下次按需重查）。
        if self.disk_cache.len() > 4096 {
            self.disk_cache.clear();
        }
        self.disk_cache.insert(key.to_string(), done);
        if done {
            DownloadState::Done
        } else {
            DownloadState::NotDownloaded
        }
    }

    /// 下载根目录变化（设置里改了路径）后作废磁盘缓存。
    pub fn clear_disk_cache(&mut self) {
        self.disk_cache.clear();
        self.bump_version();
    }
}

/// 唯一的下载任务：顺序消费队列，队列排空即退出（空闲不占 async 任务）。
///
/// 一次只跑一个下载；队列里的任务在轮到自己时先看取消标志（排队期间被取消的
/// 就不再发起），随后写盘并回填结果。取消在途任务会立刻中断当前写盘，
/// 队列里的下一个紧接着开始。
async fn loop_downloads(mut rx: UnboundedReceiver<DownloadMessage>, mut api: ApiState) {
    loop {
        // 队列空（或发送端已关）就释放这个任务；下次 enqueue 会另起一个。
        let message = match rx.try_recv() {
            Ok(message) => message,
            Err(TryRecvError::Empty) | Err(TryRecvError::Closed) => break,
        };

        let DownloadMessage { request, shared } = message;
        if shared.cancelled.load(Ordering::SeqCst) {
            *shared.outcome.lock() = Some(Err(TaskError::Cancelled));
            continue;
        }

        shared.started.store(true, Ordering::SeqCst);
        let result = download_task(&mut api, request, &shared).await;
        *shared.outcome.lock() = Some(result);
    }
}

/// 任务主体：取链 → 边下边写 `.part` → 改名 → 标签/封面 → 专辑封面。
async fn download_task(
    api: &mut ApiState,
    request: DownloadRequest,
    shared: &JobShared,
) -> Result<DownloadOutcome, TaskError> {
    let cancelled = || shared.cancelled.load(Ordering::SeqCst);

    let source = match api
        .audio_download_url(&request.song_id, request.level.as_api_level())
        .await
    {
        Ok(source) => source,
        Err(err) => return Err(TaskError::Failed(err.to_string())),
    };

    if cancelled() {
        return Err(TaskError::Cancelled);
    }

    if let Err(err) = std::fs::create_dir_all(&request.target.dir) {
        return Err(TaskError::Failed(format!("创建下载目录失败: {err}")));
    }

    let ext = normalize_extension(&source.file_type);
    let final_path = request.target.file_path(ext);
    let part_path = request.target.part_path(ext);

    match stream_to_file(api, &source.url, &part_path, shared).await {
        Ok(()) => {}
        Err(TaskError::Cancelled) => {
            let _ = std::fs::remove_file(&part_path);
            return Err(TaskError::Cancelled);
        }
        Err(err) => {
            let _ = std::fs::remove_file(&part_path);
            return Err(err);
        }
    }

    if cancelled() {
        let _ = std::fs::remove_file(&part_path);
        return Err(TaskError::Cancelled);
    }

    // 覆盖同名文件；顺带清掉同名但扩展名不同的旧文件，避免一份歌两份体积。
    if let Err(err) = compio::fs::rename(&part_path, &final_path).await {
        let _ = std::fs::remove_file(&part_path);
        return Err(TaskError::Failed(format!("重命名失败: {err}")));
    }
    for other in AUDIO_EXTENSIONS {
        if other == ext {
            continue;
        }
        let stale = request.target.file_path(other);
        if stale.is_file() {
            let _ = std::fs::remove_file(&stale);
        }
    }

    let tag_error = write_metadata(api, &request, &final_path, cancelled())
        .await
        .err()
        .map(|err| err.to_string());

    if cancelled() {
        return Err(TaskError::Cancelled);
    }

    Ok(DownloadOutcome {
        path: final_path,
        level: source.level,
        file_type: ext.to_string(),
        tag_error,
    })
}

/// 把响应流写进 `.part` 文件；取消时立即退出并由调用方清理半成品。
async fn stream_to_file(
    api: &ApiState,
    url: &str,
    part_path: &Path,
    shared: &JobShared,
) -> Result<(), TaskError> {
    let mut request = match api.http_client().get(url) {
        Ok(request) => request,
        Err(err) => return Err(TaskError::Failed(err.to_string())),
    };
    if let Some(cookie) = api.session_cookie() {
        request = match request.header("Cookie", cookie) {
            Ok(request) => request,
            Err(err) => return Err(TaskError::Failed(err.to_string())),
        };
    }

    let response = match request.send().await {
        Ok(response) => response,
        Err(err) => return Err(TaskError::Failed(err.to_string())),
    };
    let response = match crate::app::api::error_for_status(response) {
        Ok(response) => response,
        Err(err) => return Err(TaskError::Failed(err.to_string())),
    };

    let file = match compio::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(part_path)
        .await
    {
        Ok(file) => file,
        Err(err) => return Err(TaskError::Failed(format!("打开临时文件失败: {err}"))),
    };
    let mut cursor = Cursor::new(&file);
    let mut stream = response.bytes_stream();

    loop {
        if shared.cancelled.load(Ordering::SeqCst) {
            let _ = file.close().await;
            return Err(TaskError::Cancelled);
        }

        // 用超时切片等数据：网速再慢也能在 ~500ms 内响应取消，
        // 并且不会因为"长时间无数据"误判失败（超时只回到循环头再查一次标志）。
        let chunk = match compio::time::timeout(CANCEL_POLL_INTERVAL, stream.next()).await {
            Ok(Some(Ok(chunk))) if !chunk.is_empty() => chunk,
            Ok(Some(Ok(_))) => continue,
            Ok(Some(Err(err))) => {
                let _ = file.close().await;
                return Err(TaskError::Failed(err.to_string()));
            }
            Ok(None) => break,
            Err(_) => continue,
        };

        if let Err(err) = cursor.write_all(chunk).await.0 {
            let _ = file.close().await;
            return Err(TaskError::Failed(err.to_string()));
        }
        if let Err(err) = cursor.flush().await {
            let _ = file.close().await;
            return Err(TaskError::Failed(err.to_string()));
        }
    }

    if let Err(err) = file.close().await {
        return Err(TaskError::Failed(err.to_string()));
    }
    Ok(())
}

/// 详情 + 歌词 + 封面 → 写标签（同步 IO 放阻塞线程池）。
async fn write_metadata(
    api: &ApiState,
    request: &DownloadRequest,
    path: &Path,
    cancelled: bool,
) -> Result<()> {
    if cancelled {
        return Ok(());
    }

    let metadata = fetch_song_metadata(api, request).await;
    let lyrics = fetch_lyrics(api, &request.song_id).await;
    let cover = match metadata.album_cover_url.as_deref() {
        Some(url) => api.fetch_cover_bytes(url).await.ok(),
        None => None,
    };

    let path = path.to_path_buf();
    let result = compio::runtime::spawn_blocking(move || {
        write_tags_blocking(&path, &metadata, cover.as_deref(), lyrics.as_deref())
    })
    .await
    .unwrap();
    result
}

/// 歌曲详情里的元数据（拿不到就回落到请求里带的展示字段）。
struct SongMetadata {
    title: String,
    artists: String,
    album: String,
    album_cover_url: Option<String>,
    track_number: Option<u32>,
    date: Option<String>,
}

async fn fetch_song_metadata(api: &ApiState, request: &DownloadRequest) -> SongMetadata {
    let mut metadata = SongMetadata {
        title: request.title.clone(),
        artists: request.artist.clone(),
        album: request.album.clone(),
        album_cover_url: None,
        track_number: None,
        date: None,
    };

    let mut api = api.clone();
    let Ok(response) = api.song_detail(&request.song_id).await else {
        return metadata;
    };
    let Some(song) = response.body.pointer("/songs/0") else {
        return metadata;
    };

    if let Some(name) = song.get("name").and_then(|value| value.as_str()) {
        if !name.trim().is_empty() {
            metadata.title = name.to_string();
        }
    }
    let artists = song
        .get("ar")
        .and_then(|value| value.as_array())
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.get("name").and_then(|value| value.as_str()))
                .filter(|name| !name.trim().is_empty())
                .collect::<Vec<_>>()
                .join(" / ")
        })
        .unwrap_or_default();
    if !artists.is_empty() {
        metadata.artists = artists;
    }
    if let Some(album) = song.pointer("/al/name").and_then(|value| value.as_str()) {
        if !album.trim().is_empty() {
            metadata.album = album.to_string();
        }
    }
    metadata.album_cover_url = song
        .pointer("/al/picUrl")
        .and_then(|value| value.as_str())
        .map(str::to_string);
    metadata.track_number = song
        .get("no")
        .and_then(|value| value.as_u64())
        .filter(|value| *value > 0)
        .map(|value| value as u32);
    metadata.date = song
        .get("publishTime")
        .and_then(|value| value.as_i64())
        .and_then(civil_date_from_unix_ms);

    metadata
}

/// 歌词：拿不到（纯音乐）就返回 `None`，不写空标签。
async fn fetch_lyrics(api: &ApiState, song_id: &str) -> Option<String> {
    let mut api = api.clone();
    let response = api.lyric(song_id).await.ok()?;
    let lyric = response
        .body
        .pointer("/lrc/lyric")
        .and_then(|value| value.as_str())
        .unwrap_or_default()
        .trim()
        .to_string();
    if lyric.is_empty() { None } else { Some(lyric) }
}

/// 专辑封面（专辑页下载时额外落一张 `cover.*`）。
/// `type` 字段归一化：只认 mp3/flac，其余（含空）按 mp3。
fn normalize_extension(file_type: &str) -> &'static str {
    match file_type.trim().to_ascii_lowercase().as_str() {
        "flac" => "flac",
        _ => "mp3",
    }
}

/// 写标签：标题/作者/专辑/曲目号/日期/歌词/内嵌封面（ID3v2 与 Vorbis Comment 都走这一套）。
fn write_tags_blocking(
    path: &Path,
    metadata: &SongMetadata,
    cover: Option<&[u8]>,
    lyrics: Option<&str>,
) -> Result<()> {
    use lofty::config::WriteOptions;
    use lofty::picture::{MimeType, Picture, PictureType};
    use lofty::tag::{Accessor, ItemKey, Tag, TagExt, TagType};

    // 扩展名即容器类型（文件是本次刚下下来的）：mp3 → ID3v2，flac → Vorbis Comment。
    let tag_type = match path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "flac" => TagType::VorbisComments,
        _ => TagType::Id3v2,
    };

    let mut tag = Tag::new(tag_type);
    tag.set_title(metadata.title.clone());
    tag.set_artist(metadata.artists.clone());
    tag.set_album(metadata.album.clone());
    if let Some(track) = metadata.track_number {
        tag.set_track(track);
    }
    if let Some(date) = metadata.date.as_deref() {
        tag.insert_text(ItemKey::RecordingDate, date.to_string());
    }
    if let Some(lyrics) = lyrics {
        tag.insert_text(ItemKey::UnsyncLyrics, lyrics.to_string());
    }

    if let Some(bytes) = cover {
        let mime = if bytes.starts_with(&[0x89, b'P', b'N', b'G']) {
            MimeType::Png
        } else {
            MimeType::Jpeg
        };
        tag.remove_picture_type(PictureType::CoverFront);
        tag.push_picture(
            Picture::unchecked(bytes.to_vec())
                .pic_type(PictureType::CoverFront)
                .mime_type(mime)
                .build(),
        );
    }

    tag.save_to_path(path, WriteOptions::default())
        .with_context(|| format!("save tags failed: {}", path.display()))?;
    Ok(())
}

/// `publishTime`（毫秒）→ `YYYY-MM-DD`；越界或非法值返回 `None`。
fn civil_date_from_unix_ms(ms: i64) -> Option<String> {
    if ms <= 0 {
        return None;
    }
    let days = ms.div_euclid(86_400_000);
    // Howard Hinnant 的 civil_from_days。
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if month <= 2 { year + 1 } else { year };
    if !(1000..=9999).contains(&year) {
        return None;
    }
    Some(format!("{year:04}-{month:02}-{day:02}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stem_skips_missing_parts() {
        assert_eq!(
            download_file_stem("歌名", "作者", "专辑"),
            "歌名 - 作者 - 专辑"
        );
        assert_eq!(download_file_stem("歌名", "作者", ""), "歌名 - 作者");
        assert_eq!(download_file_stem("歌名", "", ""), "歌名");
        assert_eq!(download_file_stem("  ", "", ""), "未知歌曲");
    }

    #[test]
    fn stem_strips_path_separators_and_controls() {
        // 路径分隔符换 `-`（比直接吞掉可读），非法字符丢掉，控制字符丢掉。
        let stem = download_file_stem("a/b:c*d", "AC/DC", "g\nh");
        assert_eq!(stem, "a-b-cd - AC-DC - gh");
    }

    #[test]
    fn stem_collapses_repeated_spaces() {
        assert_eq!(
            download_file_stem("Title", "Woodkid   /   Connelly", "Album"),
            "Title - Woodkid - Connelly - Album"
        );
    }

    #[test]
    fn civil_date_matches_known_timestamps() {
        assert_eq!(civil_date_from_unix_ms(0), None);
        assert_eq!(
            civil_date_from_unix_ms(1_600_000_000_000).as_deref(),
            Some("2020-09-13")
        );
        assert_eq!(
            civil_date_from_unix_ms(1_700_000_000_000).as_deref(),
            Some("2023-11-14")
        );
    }

    #[test]
    fn target_paths_use_extension_and_part_suffix() {
        let target = DownloadTarget {
            dir: PathBuf::from("/tmp/cnm"),
            base: "A - B".to_string(),
        };
        assert_eq!(
            target.file_path("flac"),
            PathBuf::from("/tmp/cnm/A - B.flac")
        );
        assert_eq!(
            target.part_path("mp3"),
            PathBuf::from("/tmp/cnm/A - B.mp3.part")
        );
    }

    #[test]
    fn state_glyph_uses_expected_frames() {
        let phase = Duration::from_millis(0);
        assert_eq!(
            state_glyph(DownloadState::NotDownloaded, phase),
            ICON_DOWNLOAD
        );
        assert_eq!(state_glyph(DownloadState::Done, phase), ICON_DONE);
        assert_eq!(
            state_glyph(DownloadState::Downloading, phase),
            SPINNER_FRAMES[0]
        );
        assert_eq!(
            state_glyph(
                DownloadState::Downloading,
                Duration::from_millis(SPINNER_FRAME_MS as u64)
            ),
            SPINNER_FRAMES[1]
        );
    }

    /// 加载态是 Braille 转轮：每帧都是 Braille 块，且十帧互不相同。
    #[test]
    fn spinner_frames_are_braille_and_distinct() {
        for frame in SPINNER_FRAMES {
            assert!(
                ('\u{2800}'..='\u{28ff}').contains(&frame),
                "{frame:?} 不是 Braille 字符"
            );
        }
        let unique: std::collections::HashSet<char> = SPINNER_FRAMES.iter().copied().collect();
        assert_eq!(unique.len(), SPINNER_FRAMES.len());
    }

    /// 预计算行的 key 必须与 `state_of` 内部构造的一致（否则磁盘缓存查不到）。
    #[test]
    fn row_key_matches_the_manager_key() {
        let target = DownloadTarget {
            dir: PathBuf::from("/tmp/cnm"),
            base: "A - B".to_string(),
        };
        let row = DownloadRow::new("42".to_string(), target.clone());
        assert_eq!(
            row.key(),
            target.dir.join(&target.base).display().to_string()
        );
    }

    /// memo 只在"列表代变化"时重建行数据、只在"任务版本变化"时重算状态。
    #[test]
    fn row_cache_rebuilds_only_on_epoch_and_version_changes() {
        use std::cell::Cell;

        fn refresh(
            cache: &mut DownloadRowCache,
            epoch: u64,
            version: u64,
            builds: &Cell<u32>,
            resolves: &Cell<u32>,
        ) {
            cache.refresh_inner(
                epoch,
                version,
                || {
                    builds.set(builds.get() + 1);
                    vec![Some(DownloadRow::new(
                        "1".to_string(),
                        DownloadTarget {
                            dir: PathBuf::from("/tmp/cnm"),
                            base: "A".to_string(),
                        },
                    ))]
                },
                |_row| {
                    resolves.set(resolves.get() + 1);
                    DownloadState::NotDownloaded
                },
            );
        }

        let mut cache = DownloadRowCache::default();
        let builds = Cell::new(0);
        let resolves = Cell::new(0);

        refresh(&mut cache, 1, 7, &builds, &resolves);
        assert_eq!((builds.get(), resolves.get()), (1, 1));
        assert_eq!(cache.state_at(0), Some(DownloadState::NotDownloaded));

        // 同代同版本：什么都不做。
        refresh(&mut cache, 1, 7, &builds, &resolves);
        assert_eq!((builds.get(), resolves.get()), (1, 1), "不该重建/重算");

        // 任务事件（版本 +1）：只重算状态。
        refresh(&mut cache, 1, 8, &builds, &resolves);
        assert_eq!((builds.get(), resolves.get()), (1, 2), "只重算状态");

        // 换列表（代 +1）：重建行数据并重算状态。
        refresh(&mut cache, 2, 8, &builds, &resolves);
        assert_eq!((builds.get(), resolves.get()), (2, 3), "换列表要重建行数据");

        assert_eq!(cache.state_at(9), None, "越界行没有图标");
    }

    #[test]
    fn parse_rejects_relative_paths() {
        assert_eq!(
            parse_download_path("relative/dir"),
            Err(DownloadPathError::NotAbsolute)
        );
        assert_eq!(
            parse_download_path("  相对/路径  "),
            Err(DownloadPathError::NotAbsolute)
        );
    }

    /// 留空 = 按需求回填成 `Null`（禁用下载），不是"保留旧值"。
    #[test]
    fn empty_path_falls_back_to_null() {
        assert_eq!(parse_download_path(""), Ok(DownloadPathChoice::Disabled));
        assert_eq!(
            parse_download_path("   \t "),
            Ok(DownloadPathChoice::Disabled)
        );
    }

    /// `Null` 是"显式禁用下载"的哨兵：解析成 Disabled，解析根目录得到 None。
    #[test]
    fn null_path_disables_downloads() {
        assert_eq!(
            parse_download_path("Null"),
            Ok(DownloadPathChoice::Disabled)
        );
        assert_eq!(
            parse_download_path("  null  "),
            Ok(DownloadPathChoice::Disabled)
        );
        assert!(is_null_download_path("NULL"));
        assert!(!is_null_download_path("Nullx"));
        assert_eq!(resolve_download_root(Some("Null")), None);
        assert_eq!(resolve_download_root(Some("null")), None);
    }
}
