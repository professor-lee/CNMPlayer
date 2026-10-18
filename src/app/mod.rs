pub(crate) mod about;
mod api;
pub(crate) mod auth;
mod author;
mod browse_controller;
mod cover_presentation;
pub(crate) mod cover_state;
pub(crate) mod download;
mod download_controller;
mod fullscreen;
mod home;
pub(crate) mod input_controller;
mod keybinds;
mod latest_fetch;
mod likes;
mod metadata;
mod mpris;
mod mpris_bridge;
mod owned_task;
mod page_transition;
pub(crate) mod playback_controller;
mod player;
mod playlist;
mod playlist_pagination;
pub(crate) mod presentation;
mod response;
mod roam;
pub(crate) mod search_controller;
pub(crate) mod settings_controller;
pub(crate) mod sidebar;
mod startup;
mod startup_controller;
mod streaming;
mod text;
pub(crate) mod tracks;

#[cfg(feature = "easter-egg")]
use self::about::AboutEasterEgg;
use self::api::ApiState;
use self::auth::LoginMethod;
use self::auth::LoginState;
use self::browse_controller::BrowseController;
use self::cover_presentation::CoverPresentation;
use self::download::DownloadManager;
use self::download::DownloadRowCache;
use self::download::DownloadState;
use self::download_controller::DownloadController;
use self::input_controller::HitRect;
use self::input_controller::InputController;
use self::input_controller::PageLyricsLayout;
use self::input_controller::PlayerBarHitTargets;
use self::metadata::COVER_CACHE_SUBDIR;
use self::metadata::CoverFetchRequest;
use self::metadata::CoverFetchResult;
use self::metadata::LyricFetchRequest;
use self::metadata::LyricFetchResult;
use self::metadata::loop_cover_fetch;
use self::metadata::loop_lyric_fetch;
use self::mpris_bridge::MprisBridge;
use self::playback_controller::PlaybackController;
use self::playback_controller::PlaybackRuntimeState;
use self::player::AudioPlayer;
use self::player::cleanup_cache_dir;
use self::player::resolve_cache_root;
use self::playlist::PlaylistPageKind;
use self::playlist::PlaylistState;
use self::presentation::FlatPanel;
use self::presentation::SmallWindowMode;
use self::presentation::VU_LUFS_FLOOR;
use self::search_controller::SearchController;
use self::settings_controller::SettingsController;
use self::sidebar::HomeSidebarHit;
use self::sidebar::HomeSidebarSection;
use self::startup::StartupInit;
use self::startup_controller::StartupController;
use self::text::lang_text;
use crate::data::config::Config;
use crate::data::config::GraphicsProtocol;
use crate::data::persistence::PersistenceKey;
use crate::data::persistence::PersistenceWorker;
use crate::data::session;
use crate::launch;
use crate::render::motion::Transition;
use crate::render::wake::WakeSignal;
use crate::tmplayer::audio::spectrum::MINI_BARS;
use crate::tmplayer::audio::spectrum::Spectrum;
use crate::ui::theme::Theme;
use anyhow::Result;
use anyhow::anyhow;
use cyper::Client;
use futures::channel::mpsc as async_mpsc;
use http::header;
use see::unsync as watch;
use std::path::PathBuf;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;
use std::sync::mpsc;
use std::sync::mpsc::Receiver;
use std::sync::mpsc::TryRecvError;
use std::time::Duration;
use std::time::Instant;

/// 列表代的全局计数器：任何一次列表内容替换都换一个新号（不复用），
/// 行内图标的行数据缓存据此失效——比逐帧比对内容便宜且不会漏。
static LIST_GENERATION: AtomicU64 = AtomicU64::new(1);

fn next_list_generation() -> u64 {
    LIST_GENERATION.fetch_add(1, Ordering::Relaxed)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Page {
    Login,
    Loading,
    Home,
    Playlist,
    Author,
    Search,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Overlay {
    Settings,
    SettingsPlayback,
    SettingsKeybinds,
    SettingsLyrics,
    SettingsDownload,
    SettingsAbout,
    SearchBox,
}

pub(crate) struct App {
    pub(crate) wake: WakeSignal,
    pub(crate) config: Config,
    pub(crate) theme: Theme,
    pub(crate) lyric_render_cache: std::cell::RefCell<crate::ui::lyric_view::LyricRenderCache>,
    pub(crate) page: Page,
    pub(crate) overlay: Option<Overlay>,
    pub(crate) login: LoginState,
    pub(crate) browse: BrowseController,
    /// 上次检查 stderr 日志体积的时刻。
    stderr_trim_checked_at: Option<Instant>,
    stderr_trim_inflight: Option<Receiver<()>>,
    home_sidebar_anim_span_cells: u16,
    pub(crate) search: SearchController,
    pub(crate) playback: PlaybackController,
    startup: StartupController,
    page_transition: page_transition::PageTransitionController,
    player_bar_hits: PlayerBarHitTargets,
    /// 最近一次同步到的终端尺寸（单元格）。
    term_width: u16,
    term_height: u16,
    /// 当前生效的小窗口模式；None 表示普通内容页或未启用小窗口显示。
    pub(crate) small_window_mode: Option<SmallWindowMode>,
    /// 扁窗高度恰为播放栏高度时，Alt+X 切换的当前目标面板。
    pub(crate) flat_panel: FlatPanel,
    /// Visual transition, separate from the logical target in `flat_panel`.
    flat_switch_anim: Option<Transition>,
    /// 上一帧是否处于“扁窗高度恰为播放栏高度”的子状态。
    flat_exact_height: bool,
    /// 窄窗音量条的显示端平滑值（LUFS）。以 VU_FLOOR 表示静音。
    pub(crate) vu_left_lufs: f32,
    pub(crate) vu_right_lufs: f32,
    vu_animating: bool,
    vu_last_tick_at: Option<Instant>,
    vu_last_meter_generation: u64,
    home_sidebar_panel_hit: Option<HitRect>,
    /// 上一帧歌词浮窗的几何（拖拽 / 点击拦截共用）。
    page_lyrics_layout: Option<PageLyricsLayout>,
    /// 正在拖动歌词浮窗时，光标相对浮窗左上角的偏移。
    page_lyrics_grab: Option<(u16, u16)>,
    home_sidebar_playlist_hits: Vec<(HitRect, HomeSidebarHit)>,
    /// 侧边栏两个分区的矩形（滚轮据此判断光标落在哪个分区）。
    home_sidebar_section_hits: Vec<(HitRect, HomeSidebarSection)>,
    home_tile_hits: Vec<(HitRect, usize)>,
    playlist_track_hits: Vec<(HitRect, usize)>,
    /// 歌单页 / 专辑页行内下载图标的命中区（先于整行命中判定）。
    playlist_track_download_hits: Vec<(HitRect, usize)>,
    author_tile_hits: Vec<(HitRect, usize)>,
    search_item_hits: Vec<(HitRect, usize)>,
    /// 搜索页单曲行内下载图标的命中区。
    search_item_download_hits: Vec<(HitRect, usize)>,
    pub(crate) input: InputController,
    pub(crate) settings: SettingsController,
    /// about 弹窗里的形象彩蛋状态。
    #[cfg(feature = "easter-egg")]
    pub(crate) about_egg: AboutEasterEgg,
    session_cookie: Option<String>,
    pub(crate) should_quit: bool,
    launch_fullscreen_requested: bool,
    pub(crate) vip_audio_unlocked: bool,
    config_revision: u64,
    search_return_page: Page,
    playlist_return_page: Page,
    /// 作者页的上一级：从搜索页进是搜索页，从全屏页点作者名进是首页。
    author_return_page: Page,
    playlist_section_return_snapshot: Option<PlaylistState>,
    qr_last_poll_at: Option<Instant>,
    last_global_hotkey_at: Option<Instant>,
    last_content_click: Option<(Instant, Page, usize)>,
    mini_spectrum: Spectrum,
    cover_cache_dir: PathBuf,
    cover_fetch_tx: watch::Sender<Option<CoverFetchRequest>>,
    cover_fetch_rx: async_mpsc::Receiver<CoverFetchResult>,
    cover_fetch_inflight_url: Option<String>,
    cover_fetch_generation: u64,
    cover_fetch_last_attempt_at: Option<Instant>,
    queue_cover_fetch_last_attempt_at: Option<Instant>,
    lyric_fetch_tx: watch::Sender<Option<LyricFetchRequest>>,
    lyric_fetch_rx: async_mpsc::Receiver<LyricFetchResult>,
    lyric_fetch_inflight_song_id: Option<String>,
    lyric_fetch_generation: u64,
    lyric_fetch_last_attempt_at: Option<Instant>,
    mpris_bridge: MprisBridge,
    mpris_last_sync_at: Instant,
    mpris_last_signature: Option<u64>,
    mpris_last_playback: PlaybackRuntimeState,
    api: ApiState,
    persistence: PersistenceWorker,
    /// 下载任务表（异步后台任务；状态行与图标都从这里读）。
    downloads: DownloadController,
    pub(crate) covers: CoverPresentation,
}

impl App {
    pub(crate) fn draw_ascii(&self) -> bool {
        self.config.graphics_protocol == GraphicsProtocol::Off
    }

    /// 构造 App 并启动后台初始化：本地设置同步完成，网络初始化交给
    /// [`StartupInit`]，加载页随即可以显示真实进度。
    pub(crate) async fn new(config: Config, theme: Theme) -> Result<Self> {
        let audio_player = AudioPlayer::new(&config)?;
        let persistence = PersistenceWorker::spawn("cnmplayer-persistence")?;
        let saved_cookie = compio::runtime::spawn_blocking(session::load_cookie)
            .await
            .map_err(|_| anyhow!("session read task panicked"))??;

        let mut headers = header::HeaderMap::new();
        headers.insert(
            header::USER_AGENT,
            header::HeaderValue::from_static("Mozilla/5.0 CNMPlayer/0.1"),
        );
        headers.insert(
            header::REFERER,
            header::HeaderValue::from_static("https://music.163.com/"),
        );
        let http_client = Client::builder().default_headers(headers).build()?;
        let api = ApiState::new(saved_cookie.clone(), http_client.clone())?;
        let wake = api.wake_signal();
        let cache_root = resolve_cache_root(&config);
        let cover_cache_dir = cache_root.join(COVER_CACHE_SUBDIR);
        let download_root =
            crate::app::download::resolve_download_root(config.download_path.as_deref());
        let mpris_bridge = MprisBridge::new(&cache_root, &config.cache, wake.clone());
        if config.cache.clean_on_startup {
            let startup_dir = cover_cache_dir.clone();
            let startup_policy = config.cache.clone();
            compio::runtime::spawn_blocking(move || {
                let _ = cleanup_cache_dir(&startup_dir, &startup_policy);
            })
            .detach();
        }

        // One active request plus one replaceable latest request, not a FIFO backlog.
        let (cover_fetch_tx, cover_fetch_req_rx) = watch::channel(None);
        let (cover_fetch_res_tx, cover_fetch_rx) = async_mpsc::channel(1);
        let worker = loop_cover_fetch(
            cover_fetch_req_rx,
            cover_fetch_res_tx,
            http_client.clone(),
            cover_cache_dir.clone(),
            persistence.handle(),
            wake.clone(),
        );
        launch(worker);

        // 下载任务全局只有一个：管理器起一次常驻 worker，之后只往队列里塞请求。
        let download_manager = DownloadManager::new(api.clone());

        let (lyric_fetch_tx, lyric_fetch_req_rx) = watch::channel(None);
        let (lyric_fetch_res_tx, lyric_fetch_rx) = async_mpsc::channel(1);
        let worker = loop_lyric_fetch(lyric_fetch_req_rx, lyric_fetch_res_tx, api.clone());
        launch(worker);

        let mut app = Self {
            wake: wake.clone(),
            config,
            theme,
            lyric_render_cache: Default::default(),
            page: Page::Login,
            overlay: None,
            login: LoginState::default(),
            browse: BrowseController::default(),
            stderr_trim_checked_at: None,
            stderr_trim_inflight: None,
            home_sidebar_anim_span_cells: 24,
            search: SearchController::default(),
            playback: PlaybackController::new(audio_player),
            startup: StartupController::detached(),
            page_transition: page_transition::PageTransitionController::default(),
            player_bar_hits: PlayerBarHitTargets::default(),
            term_width: 0,
            term_height: 0,
            small_window_mode: None,
            flat_panel: FlatPanel::Player,
            flat_switch_anim: None,
            flat_exact_height: false,
            vu_left_lufs: VU_LUFS_FLOOR,
            vu_right_lufs: VU_LUFS_FLOOR,
            vu_animating: false,
            vu_last_tick_at: None,
            vu_last_meter_generation: 0,
            home_sidebar_panel_hit: None,
            page_lyrics_layout: None,
            page_lyrics_grab: None,
            home_sidebar_playlist_hits: Vec::new(),
            home_sidebar_section_hits: Vec::new(),
            home_tile_hits: Vec::new(),
            playlist_track_hits: Vec::new(),
            playlist_track_download_hits: Vec::new(),
            author_tile_hits: Vec::new(),
            search_item_hits: Vec::new(),
            search_item_download_hits: Vec::new(),
            input: InputController::default(),
            settings: SettingsController::default(),
            #[cfg(feature = "easter-egg")]
            about_egg: AboutEasterEgg::default(),
            session_cookie: None,
            should_quit: false,
            launch_fullscreen_requested: false,
            vip_audio_unlocked: false,
            config_revision: 0,
            search_return_page: Page::Home,
            playlist_return_page: Page::Home,
            author_return_page: Page::Home,
            playlist_section_return_snapshot: None,
            qr_last_poll_at: None,
            last_global_hotkey_at: None,
            last_content_click: None,
            mini_spectrum: Spectrum::new(MINI_BARS),
            cover_cache_dir,
            cover_fetch_tx,
            cover_fetch_rx,
            cover_fetch_inflight_url: None,
            cover_fetch_generation: 0,
            cover_fetch_last_attempt_at: None,
            queue_cover_fetch_last_attempt_at: None,
            lyric_fetch_tx,
            lyric_fetch_rx,
            lyric_fetch_inflight_song_id: None,
            lyric_fetch_generation: 0,
            lyric_fetch_last_attempt_at: None,
            mpris_bridge,
            mpris_last_sync_at: Instant::now(),
            mpris_last_signature: None,
            mpris_last_playback: PlaybackRuntimeState::Stopped,
            api,
            persistence,
            downloads: DownloadController {
                manager: download_manager,
                playlist_cache: DownloadRowCache::default(),
                search_cache: DownloadRowCache::default(),
                rows_epoch: 0,
                spinner_start: Instant::now(),
                now_playing_state: DownloadState::NotDownloaded,
                root: download_root,
                page_kind: PlaylistPageKind::Playlist,
                pending_intents: Default::default(),
            },
            covers: CoverPresentation::new(wake),
        };

        app.load_private_roam_memory().await;

        app.sync_terminal_size();

        // 先出加载页，网络初始化交给后台任务：登录恢复这几步在旧实现里是
        // 进备用屏幕之前同步 await 的，终端因此有一段时间毫无反馈。
        let skip_roam = app.private_roam_refreshed_today();
        let (steps, target) = startup::initial_plan(saved_cookie.is_some(), skip_roam);
        app.startup.init = StartupInit::spawn(
            app.config.clone(),
            app.api.clone(),
            saved_cookie,
            skip_roam,
            steps,
        );
        app.begin_startup_loading(target);
        Ok(app)
    }

    fn persist_config(&mut self) {
        self.config_revision = self.config_revision.wrapping_add(1);
        let snapshot = self.config.clone();
        let _ = self
            .persistence
            .enqueue_latest(PersistenceKey::Config, move || snapshot.save());
    }

    pub(crate) fn flush_persistence(&self) -> Result<()> {
        self.persistence.flush().map_err(anyhow::Error::msg)
    }

    pub(crate) async fn tick(&mut self) {
        if let Some(error) = self.persistence.pending_error() {
            self.set_runtime_status(format!(
                "{}: {error}",
                self.lang_text("保存失败", "Save failed")
            ));
        }
        self.tick_audio().await;
        self.tick_cover_fetch();
        self.tick_lyric_fetch();
        self.apply_mpris_control_events().await;
        self.sync_mpris_exposure();
        let now = Instant::now();
        self.tick_page_transition(now);
        self.tick_flat_switch(now);
        self.tick_vu_meter(now);
        self.tick_search_box_animation(now);
        self.tick_home_sidebar_animation(now);
        self.tick_home_sidebar_fetch();
        self.tick_home_sidebar_page_fetch();
        self.tick_author_fetch();
        self.tick_playlist_fetch();
        self.tick_like_sync();
        self.tick_download();
        self.tick_stderr_log_trim();
        trim_allocator_periodically();
        self.tick_startup_init().await;
        self.tick_startup_loading();
        #[cfg(feature = "easter-egg")]
        self.tick_about_easter_egg();

        if self.page == Page::Login && self.login.method == LoginMethod::Qr {
            if self.login.qr_key.trim().is_empty() {
                return;
            }

            let now = Instant::now();
            if let Some(last) = self.qr_last_poll_at
                && now.duration_since(last) < Duration::from_millis(1400)
            {
                return;
            }

            self.qr_last_poll_at = Some(now);
            self.check_qr_status_and_login().await;
        }
    }

    /// Periodic stderr maintenance runs off the UI reactor, with one job at a time.
    fn tick_stderr_log_trim(&mut self) {
        if let Some(rx) = self.stderr_trim_inflight.as_ref() {
            match rx.try_recv() {
                Ok(()) | Err(TryRecvError::Disconnected) => self.stderr_trim_inflight = None,
                Err(TryRecvError::Empty) => return,
            }
        }
        const CHECK_INTERVAL: Duration = Duration::from_secs(60);
        let due = match self.stderr_trim_checked_at {
            Some(at) => at.elapsed() >= CHECK_INTERVAL,
            None => true,
        };
        if !due {
            return;
        }
        self.stderr_trim_checked_at = Some(Instant::now());
        let (tx, rx) = mpsc::channel();
        self.stderr_trim_inflight = Some(rx);
        compio::runtime::spawn_blocking(move || {
            crate::trim_stderr_log_if_needed();
            let _ = tx.send(());
        })
        .detach();
    }

    pub(crate) fn set_runtime_status(&mut self, text: impl Into<String>) {
        let text = text.into();
        self.browse.home.status_line = text.clone();
        self.search.status_line = text;
    }

    fn lang_text<'a>(&self, zh: &'a str, en: &'a str) -> &'a str {
        lang_text(self.config.language, zh, en)
    }
}

/// glibc keeps freed arenas resident after image/JSON bursts. Return the free
/// pages at a low frequency; this does not reduce the peak, only the plateau.
#[cfg(all(unix, target_env = "gnu"))]
fn trim_allocator_periodically() {
    use std::sync::Mutex;

    unsafe extern "C" {
        fn malloc_trim(pad: usize) -> libc::c_int;
    }

    const INTERVAL: Duration = Duration::from_secs(60);
    static LAST: Mutex<Option<Instant>> = Mutex::new(None);

    let Ok(mut last) = LAST.try_lock() else {
        return;
    };
    let now = Instant::now();
    match *last {
        Some(at) if now.duration_since(at) < INTERVAL => return,
        Some(_) => *last = Some(now),
        None => {
            *last = Some(now);
            return;
        }
    }
    drop(last);
    // SAFETY: malloc_trim only releases unused allocator pages; it does not
    // invalidate live Rust allocations.
    unsafe {
        malloc_trim(0);
    }
}

#[cfg(not(all(unix, target_env = "gnu")))]
fn trim_allocator_periodically() {}

#[cfg(test)]
mod tests;
