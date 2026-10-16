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

#[cfg(test)]
mod tests {
    use super::api::ApiState;
    use super::author::merge_artist_album_pages;
    use super::cover_presentation::CoverPresentation;
    use super::cover_state::CoverFetchState;
    use super::input_controller::HitRect;
    use super::likes::LikeMachine;
    use super::likes::LikeToggleFuture;
    use super::likes::LikeToggleTask;
    use super::likes::LikeVerifyFuture;
    use super::likes::LikeVerifyTask;
    use super::likes::ToggleOutcome;
    use super::metadata::validate_cached_cover;
    use super::owned_task::SharedTask;
    use super::owned_task::peek_shared_future;
    use super::playlist::PlaylistState;
    use super::presentation::VU_LUFS_FLOOR;
    use super::presentation::lufs_to_bar_level;
    use super::presentation::lufs_to_mean_square;
    use super::presentation::mean_square_to_lufs;
    use super::roam::merge_private_roam_refresh;
    use super::roam::normalize_fm_song_items;
    use super::search_controller::SearchController;
    use super::search_controller::SearchItem;
    use super::search_controller::SearchItemKind;
    use super::search_controller::SearchScope;
    use super::search_controller::parse_search_input;
    use super::sidebar::HomeSidebarPageFetch;
    use super::sidebar::HomeSidebarPlaylist;
    use super::sidebar::HomeSidebarSection;
    use super::sidebar::HomeSidebarState;
    use super::sidebar::home_sidebar_wheel_target;
    use super::startup_controller::startup_loading_progress;
    use super::tracks::PlaylistTrack;
    use super::tracks::PlaylistTrackKind;
    use super::tracks::artist_name_segments;
    use super::tracks::pick_artist_id;
    use cyper::Client;
    use ncm_api::ApiResponse;
    use ratatui::style::Style;
    use serde_json::Value;
    use std::time::Duration;
    use std::time::Instant;

    #[compio::test]
    async fn home_cover_loads_and_paints_when_multiple_tiles_complete_together() {
        use std::io::{Read, Write};
        let image = image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
            24,
            24,
            image::Rgb([220, 60, 30]),
        ));
        let mut encoded = std::io::Cursor::new(Vec::new());
        image
            .write_to(&mut encoded, image::ImageFormat::Png)
            .unwrap();
        let bytes = encoded.into_inner();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            for _ in 0..3 {
                let (mut socket, _) = listener.accept().unwrap();
                let mut request = [0; 2048];
                assert!(socket.read(&mut request).unwrap() > 0);
                write!(
                    socket,
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    bytes.len()
                )
                .unwrap();
                socket.write_all(&bytes).unwrap();
            }
        });
        let api = ApiState::new(None, Client::builder().no_proxy().build().unwrap()).unwrap();
        let mut tiles = [
            CoverFetchState::default(),
            CoverFetchState::default(),
            CoverFetchState::default(),
        ];
        for (index, cover) in tiles.iter_mut().enumerate() {
            cover.load(api.clone(), format!("http://{address}/{index}"));
        }
        let deadline = Instant::now() + Duration::from_secs(3);
        while tiles
            .iter()
            .any(|cover| peek_shared_future(&cover.image).is_none())
        {
            assert!(Instant::now() < deadline);
            compio::time::sleep(Duration::from_millis(1)).await;
        }
        let mut presentation = CoverPresentation::new(api.wake_signal());
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(8, 4)).unwrap();
        loop {
            presentation.poll();
            terminal
                .draw(|frame| {
                    presentation.begin_frame();
                    tiles[2].render(
                        frame,
                        &mut presentation,
                        frame.area(),
                        Style::default(),
                        None,
                        false,
                    );
                })
                .unwrap();
            presentation.prepare();
            if terminal
                .backend()
                .buffer()
                .content
                .iter()
                .any(|cell| matches!(cell.fg, ratatui::style::Color::Rgb(..)))
            {
                break;
            }
            assert!(Instant::now() < deadline);
            compio::time::sleep(Duration::from_millis(1)).await;
        }
        server.join().unwrap();
    }

    fn test_artist_album_response(items: &[Value], more: bool) -> ApiResponse {
        ApiResponse {
            status: 200,
            body: serde_json::json!({
                "code": 200,
                "hotAlbums": items,
                "more": more,
            }),
            cookie: Vec::new(),
        }
    }

    #[test]
    fn artist_album_pages_merge_all_release_metadata() {
        let first = vec![serde_json::json!({"id": 1, "name": "first"})];
        let second = vec![
            serde_json::json!({"id": 2, "name": "second"}),
            serde_json::json!({"id": 3, "name": "third"}),
        ];

        let merged = merge_artist_album_pages(vec![
            test_artist_album_response(&first, true),
            test_artist_album_response(&second, false),
        ])
        .unwrap();

        let items = merged
            .body
            .get("hotAlbums")
            .and_then(Value::as_array)
            .unwrap();
        assert_eq!(items.len(), 3);
        assert_eq!(items[0]["name"], "first");
        assert_eq!(items[2]["name"], "third");
    }

    #[test]
    fn artist_album_pages_reject_incomplete_page_instead_of_returning_partial_data() {
        let first = vec![serde_json::json!({"id": 1, "name": "first"})];
        let malformed = ApiResponse {
            status: 200,
            body: serde_json::json!({"code": 200, "more": false}),
            cookie: Vec::new(),
        };

        let result =
            merge_artist_album_pages(vec![test_artist_album_response(&first, true), malformed]);

        assert!(result.is_err());
    }

    #[test]
    fn cached_cover_requires_decodable_image_and_terminal_marker() {
        for format in [image::ImageFormat::Png, image::ImageFormat::Jpeg] {
            let mut encoded = std::io::Cursor::new(Vec::new());
            image::DynamicImage::new_rgb8(2, 2)
                .write_to(&mut encoded, format)
                .unwrap();
            let complete = encoded.into_inner();
            assert_eq!(
                validate_cached_cover(complete.clone()),
                Some(complete.clone())
            );

            let mut incomplete = complete;
            incomplete.pop();
            assert!(validate_cached_cover(incomplete).is_none());
        }

        assert!(validate_cached_cover(Vec::new()).is_none());
        assert!(validate_cached_cover(b"not an image\xff\xd9".to_vec()).is_none());
    }

    /// 无后缀默认走混合搜索；后缀命中时只搜该类型（`@artist` 与 `@author` 同义）。
    #[test]
    fn parse_search_input_resolves_scope() {
        assert_eq!(
            parse_search_input("test"),
            ("test".to_string(), SearchScope::Mixed)
        );
        assert_eq!(
            parse_search_input("test @list"),
            ("test".to_string(), SearchScope::Playlist)
        );
        assert_eq!(
            parse_search_input("test @artist"),
            ("test".to_string(), SearchScope::Author)
        );
        assert_eq!(
            parse_search_input("@author"),
            (String::new(), SearchScope::Author)
        );
    }

    fn search_item(kind: SearchItemKind, label: &str) -> SearchItem {
        SearchItem {
            kind,
            left_label: label.to_string(),
            right_label: String::new(),
            song_id: None,
            album_id: None,
            playlist_id: None,
            artist_id: None,
            title: None,
            artist: None,
            album: None,
            cover_url: None,
            duration_ms: None,
            cover: CoverFetchState::default(),
        }
    }

    /// 视口按行滚动：底部推进多少行，顶部就退多少行（作者卡片被裁切而不是整块移出）。
    #[test]
    fn search_scroll_moves_by_rows() {
        let mut state = SearchController::default();
        state.set_results(
            vec![
                search_item(SearchItemKind::Artist, "artist-1"), // 行 0..4（卡片）
                search_item(SearchItemKind::Artist, "artist-2"), // 行 4..8
                search_item(SearchItemKind::Song, "song-1"),     // 行 8..10（分区线 1 + 单曲 1）
                search_item(SearchItemKind::Song, "song-2"),     // 行 10..11
                search_item(SearchItemKind::Song, "song-3"),     // 行 11..12
            ],
            0,
            false,
        );
        state.set_viewport(6, true);

        // 进入第二张卡片：底边对齐 8，顶部退 2 行（底边前进 2 行）。
        state.set_focus(1);
        assert_eq!(state.effective_scroll_rows(), 2);

        // 进入带分区线的单曲：底边 8 → 10，顶部同样只退 2 行。
        assert!(state.focus_next());
        assert_eq!(state.effective_scroll_rows(), 4);

        // 纯单曲：底部只推进 1 行，顶部也只退 1 行（卡片只被裁掉 1 行）。
        assert!(state.focus_next());
        assert_eq!(state.effective_scroll_rows(), 5);

        // 回退：目标条目仍完整可见时不滚动。
        assert!(state.focus_prev());
        assert_eq!(state.effective_scroll_rows(), 5);

        // 继续回退到被裁掉的那张卡片（起点 4 已在视口上方）：顶部只回退 1 行。
        assert!(state.focus_prev());
        assert_eq!(state.effective_scroll_rows(), 4);
    }

    /// 列表全是卡片时，一次推进就是一个卡片高度，顶部也退一个卡片高度。
    #[test]
    fn search_scroll_moves_by_card_height_on_author_list() {
        let mut state = SearchController::default();
        state.set_results(
            vec![
                search_item(SearchItemKind::Artist, "artist-1"),
                search_item(SearchItemKind::Artist, "artist-2"),
                search_item(SearchItemKind::Artist, "artist-3"),
            ],
            0,
            false,
        );
        state.set_viewport(6, true);

        state.set_focus(1);
        assert_eq!(state.effective_scroll_rows(), 2);
        assert!(state.focus_next());
        assert_eq!(state.effective_scroll_rows(), 6);
    }

    /// 视口行数变化后聚焦条目仍然完整可见（窗口缩放 / 小窗模式）。
    #[test]
    fn search_viewport_resize_keeps_focus_visible() {
        let mut state = SearchController::default();
        state.set_results(
            (0..20)
                .map(|i| search_item(SearchItemKind::Song, &format!("song-{i}")))
                .collect(),
            0,
            false,
        );
        state.set_viewport(5, true);
        state.set_focus(9);
        assert_eq!(state.effective_scroll_rows(), 5);

        state.set_viewport(3, true);
        assert_eq!(state.effective_scroll_rows(), 7);
        assert_eq!(state.page_items(), 3);
    }

    fn track(id: &str) -> PlaylistTrack {
        PlaylistTrack {
            kind: PlaylistTrackKind::Song,
            id: Some(id.to_string()),
            title: format!("title-{}", id),
            artist: "artist".to_string(),
            album: "album".to_string(),
            cover_url: Some(format!("https://example.com/{}.jpg", id)),
            duration_ms: 1000,
            duration: "00:01".to_string(),
        }
    }
    #[test]
    fn playlist_state_appends_pages_without_resetting_focus() {
        let mut state = PlaylistState::default();
        state.set_tracks(vec![track("a"), track("b")]);
        state.set_focus(1);
        state.total_tracks = Some(4);

        state.append_tracks(vec![track("c"), track("d")]);

        let ids: Vec<Option<String>> = state.tracks.iter().map(|item| item.id.clone()).collect();
        assert_eq!(
            ids,
            vec![
                Some("a".to_string()),
                Some("b".to_string()),
                Some("c".to_string()),
                Some("d".to_string()),
            ]
        );
        assert_eq!(state.focused_idx, 1);
        assert_eq!(state.total_tracks, Some(4));
    }

    #[test]
    fn sidebar_state_appends_requested_section_page() {
        let mut state = HomeSidebarState {
            created_has_more: true,
            created_next_offset: 100,
            created_playlists: vec![HomeSidebarPlaylist {
                id: Some("first".to_string()),
                title: "first".to_string(),
                creator: "creator".to_string(),
                track_count: 1,
            }],
            ..HomeSidebarState::default()
        };
        state.append_section_page(HomeSidebarPageFetch {
            section: HomeSidebarSection::Created,
            items: vec![HomeSidebarPlaylist {
                id: Some("second".to_string()),
                title: "second".to_string(),
                creator: "creator".to_string(),
                track_count: 2,
            }],
            next_offset: 101,
            has_more: false,
        });

        assert_eq!(state.created_playlists.len(), 2);
        assert_eq!(state.created_next_offset, 101);
        assert!(!state.created_has_more);
    }

    #[test]
    fn merge_refresh_keeps_last_played_at_head() {
        let old = vec![track("a"), track("b"), track("c")];
        let fetched = vec![track("x"), track("y"), track("b")];

        let (merged, index) = merge_private_roam_refresh(&old, Some(1), fetched);

        // 最后播放的 b 保留在首位，x/y 追加，重复的 b 跳过
        let ids: Vec<Option<String>> = merged.iter().map(|t| t.id.clone()).collect();
        assert_eq!(
            ids,
            vec![
                Some("b".to_string()),
                Some("x".to_string()),
                Some("y".to_string())
            ]
        );
        assert_eq!(index, Some(0));
    }

    #[test]
    fn merge_refresh_without_playback_replaces_all() {
        let old = vec![track("a"), track("b")];
        let fetched = vec![track("x"), track("y")];

        let (merged, index) = merge_private_roam_refresh(&old, None, fetched);

        let ids: Vec<Option<String>> = merged.iter().map(|t| t.id.clone()).collect();
        assert_eq!(ids, vec![Some("x".to_string()), Some("y".to_string())]);
        assert_eq!(index, None);
    }

    #[test]
    fn startup_progress_never_rewinds_and_stops_before_completion() {
        // 5 步计划：进度只随真实完成的步数与当前步的缓动单调推进，
        // 且在收尾 ramp 之前不越过 96%（留给「即将完成」的视觉余量）。
        let total = 5;
        let mut last = 0.0_f32;
        for done in 0..=total {
            for micros in [0.0_f32, 0.02, 0.31, 0.62, 1.5] {
                let progress = startup_loading_progress(done, total, micros, None, false);
                assert!(
                    progress >= last - 1.0e-6,
                    "进度回退: done={done} step_elapsed={micros} progress={progress} last={last}"
                );
                assert!(progress <= 0.96 + 1.0e-6, "未完成时越过 96%: {progress}");
                last = progress;
            }
        }
        // 最后一步跑满也只到 96%，不会提前显示 100%。
        assert!(
            startup_loading_progress(total, total, 5.0, None, false) <= 0.96 + 1.0e-6,
            "全部步骤完成后未收尾就已到 100%"
        );
    }

    #[test]
    fn startup_progress_reaches_full_after_completion_ramp() {
        let ramping = startup_loading_progress(5, 5, 0.62, Some(0.0), true);
        let done = startup_loading_progress(5, 5, 0.62, Some(1.0), true);

        assert!(done >= ramping, "收尾 ramp 必须单调向上");
        assert!((done - 1.0).abs() < 1.0e-6, "收尾后应为 1.0，实际 {done}");
    }

    #[test]
    fn merge_refresh_stale_last_played_falls_back_to_fetched() {
        // 最后播放歌曲已不在旧列表（数据异常），退化为全量替换
        let old = vec![track("a")];
        let fetched = vec![track("x")];

        let (merged, index) = merge_private_roam_refresh(&old, Some(5), fetched);

        let ids: Vec<Option<String>> = merged.iter().map(|t| t.id.clone()).collect();
        assert_eq!(ids, vec![Some("x".to_string())]);
        assert_eq!(index, None);
    }

    #[test]
    fn normalize_fm_items_maps_album_and_artists() {
        let items = serde_json::json!([{
            "name": "song",
            "id": 1,
            "album": {"name": "al-name", "picUrl": "http://cover"},
            "artists": [{"name": "artist-a"}],
            "dt": 2000
        }]);

        let normalized = normalize_fm_song_items(items.as_array().unwrap());
        assert_eq!(normalized[0]["al"]["name"], "al-name");
        assert_eq!(normalized[0]["al"]["picUrl"], "http://cover");
        assert_eq!(normalized[0]["ar"][0]["name"], "artist-a");
        // 原有字段不受影响
        assert_eq!(normalized[0]["name"], "song");
    }

    #[test]
    fn lufs_mapping_uses_minus_60_to_zero_range() {
        assert!((mean_square_to_lufs(0.5) - (-3.701)).abs() < 0.01);
        assert!((lufs_to_bar_level(-30.0) - 0.5).abs() < 1.0e-6);
        assert_eq!(lufs_to_bar_level(VU_LUFS_FLOOR), 0.0);
        assert_eq!(lufs_to_bar_level(0.0), 1.0);
        assert_eq!(lufs_to_mean_square(VU_LUFS_FLOOR), 0.0);
    }

    fn pending_toggle_future() -> LikeToggleFuture {
        let fut: LikeToggleTask = Box::pin(async { None });
        SharedTask::unstarted(fut)
    }

    fn pending_verify_future() -> LikeVerifyFuture {
        let fut: LikeVerifyTask = Box::pin(async { None });
        SharedTask::unstarted(fut)
    }

    /// 点击必须立刻改变显示值（乐观），并把请求排进派发队列。
    #[test]
    fn like_click_is_optimistic_and_queues_one_request() {
        let mut machine = LikeMachine::default();
        machine.set_intent("s1".to_string(), true);

        assert!(machine.displayed("s1"), "点击后立即显示为已收藏");
        assert_eq!(machine.pending_dispatch(), Some(("s1".to_string(), true)));
    }

    /// 连点两次回到原状态：一个请求都不该发。
    #[test]
    fn like_double_click_back_to_start_sends_nothing() {
        let mut machine = LikeMachine::default();
        machine.set_intent("s1".to_string(), true);
        machine.set_intent("s1".to_string(), false);

        assert!(!machine.displayed("s1"));
        assert_eq!(machine.pending_dispatch(), None);
        assert_eq!(machine.drop_satisfied_intent(), Some("s1".to_string()));
        assert!(machine.desired.is_none());
    }

    /// 在途请求未收敛前不再并发派发，但显示跟随最后一次意图。
    #[test]
    fn like_inflight_toggle_blocks_a_second_request() {
        let mut machine = LikeMachine::default();
        machine.set_intent("s1".to_string(), true);
        machine.begin_toggle("s1".to_string(), true, pending_toggle_future());

        machine.set_intent("s1".to_string(), false);

        assert_eq!(machine.pending_dispatch(), None, "串行化：一次只发一个");
        assert_eq!(
            machine.drop_satisfied_intent(),
            None,
            "在途写入不能满足新意图"
        );
        assert!(!machine.displayed("s1"));
    }

    /// 旧回包不得覆盖更新的意图：确认值照写，显示与补发按新意图走。
    #[test]
    fn like_stale_response_keeps_newer_intent() {
        let mut machine = LikeMachine::default();
        machine.set_intent("s1".to_string(), true);
        machine.begin_toggle("s1".to_string(), true, pending_toggle_future());
        machine.set_intent("s1".to_string(), false);
        assert_eq!(machine.drop_satisfied_intent(), None);
        // 真实路径里 pump 先取走回包句柄，再交给状态机收敛
        machine.toggle = None;

        let outcome = machine.on_toggle_result("s1", true, Ok(()));

        assert_eq!(outcome, ToggleOutcome::Superseded);
        assert!(machine.is_confirmed("s1"), "回包写入已确认值");
        assert!(!machine.displayed("s1"), "显示仍按最后一次意图");
        assert_eq!(machine.pending_dispatch(), Some(("s1".to_string(), false)));
    }

    /// 仍是最新意图的失败要回滚显示并报错。
    #[test]
    fn like_failure_rolls_back_to_confirmed() {
        let mut machine = LikeMachine::default();
        machine.set_intent("s1".to_string(), true);

        let outcome = machine.on_toggle_result("s1", true, Err("502".to_string()));

        assert_eq!(
            outcome,
            ToggleOutcome::Failed {
                message: "502".to_string()
            }
        );
        assert!(!machine.is_confirmed("s1"));
        assert!(!machine.displayed("s1"), "失败后回滚到已确认值");
        assert_eq!(machine.pending_dispatch(), None, "失败的意图已被放弃");
    }

    /// 已被取代的旧失败直接忽略，不影响新意图。
    #[test]
    fn like_stale_failure_is_ignored() {
        let mut machine = LikeMachine::default();
        machine.set_intent("s1".to_string(), true);
        machine.begin_toggle("s1".to_string(), true, pending_toggle_future());
        machine.set_intent("s1".to_string(), false);
        machine.toggle = None;

        let outcome = machine.on_toggle_result("s1", true, Err("timeout".to_string()));

        assert_eq!(outcome, ToggleOutcome::StaleFailure, "旧失败不打断新意图");
        assert!(!machine.displayed("s1"));
        assert!(!machine.is_confirmed("s1"));
        // 新意图（取消收藏）与已确认状态一致，无需再发请求
        assert_eq!(machine.pending_dispatch(), None);
        assert_eq!(machine.drop_satisfied_intent(), Some("s1".to_string()));
    }

    /// 服务端确认不能覆盖未决意图。
    #[test]
    fn like_verify_does_not_override_pending_intent() {
        let mut machine = LikeMachine::default();
        machine.begin_verify("s1".to_string(), pending_verify_future());
        machine.set_intent("s1".to_string(), true);

        assert!(
            !machine.on_verify_result("s1", Ok(false)),
            "确认被未决意图挡住"
        );
        assert!(!machine.is_confirmed("s1"));
        assert!(machine.displayed("s1"));
    }

    /// 无未决意图时确认写入已确认集合；确认失败保持本地缓存。
    #[test]
    fn like_verify_applies_without_pending_intent() {
        let mut machine = LikeMachine::default();

        assert!(machine.on_verify_result("s1", Ok(true)));
        assert!(machine.is_confirmed("s1"));
        assert!(machine.displayed("s1"));
        assert!(!machine.on_verify_result("s1", Err(())));
        assert!(machine.is_confirmed("s1"), "确认失败不改动本地缓存");
    }

    /// 未决意图只作用于它自己的曲目。
    #[test]
    fn like_display_is_per_song() {
        let mut machine = LikeMachine::default();
        machine.set_confirmed("s1", true);
        machine.set_intent("s2".to_string(), true);

        assert!(machine.displayed("s1"), "其他曲目仍按已确认值");
        assert!(machine.displayed("s2"));
        assert!(!machine.is_confirmed("s2"));
    }

    fn sidebar_item(title: &str) -> HomeSidebarPlaylist {
        HomeSidebarPlaylist {
            id: Some(title.to_string()),
            title: title.to_string(),
            creator: String::new(),
            track_count: 1,
        }
    }

    fn sidebar_state(created: usize, collected: usize) -> HomeSidebarState {
        let mut state = HomeSidebarState {
            created_playlists: (0..created)
                .map(|i| sidebar_item(&format!("created-{i}")))
                .collect(),
            collected_playlists: (0..collected)
                .map(|i| sidebar_item(&format!("collected-{i}")))
                .collect(),
            ..HomeSidebarState::default()
        };
        state.clamp_focus();
        state
    }

    /// 滚轮一格一步、到端点即停（不回卷），且焦点记忆跟着走。
    #[test]
    fn home_sidebar_wheel_steps_and_clamps_at_ends() {
        let mut state = sidebar_state(3, 0);
        state.expanded = true;

        state.scroll_by(true);
        assert_eq!(state.focused_index, 1);
        assert_eq!(state.created_focused_index, 1, "焦点位置写回分区记忆");

        state.scroll_by(true);
        state.scroll_by(true);
        assert_eq!(state.focused_index, 2, "到底即停，不像键盘那样绕回开头");

        state.scroll_by(false);
        assert_eq!(state.focused_index, 1);

        state.focused_index = 0;
        state.scroll_by(false);
        assert_eq!(state.focused_index, 0, "到顶即停");
    }

    /// 空分区没有可滚的内容，不动焦点。
    #[test]
    fn home_sidebar_wheel_without_items_does_nothing() {
        let mut state = sidebar_state(0, 0);
        state.expanded = true;

        state.scroll_by(true);

        assert_eq!(state.focused_index, 0);
        assert_eq!(state.focused_section, HomeSidebarSection::Created);
    }

    /// 指着另一个分区滚：先切过去（沿用该分区的位置记忆）再走一格。
    #[test]
    fn home_sidebar_wheel_switches_section_then_steps() {
        let mut state = sidebar_state(3, 4);
        state.expanded = true;
        state.collected_focused_index = 2;

        state.scroll_section_by(HomeSidebarSection::Collected, true);

        assert_eq!(state.focused_section, HomeSidebarSection::Collected);
        assert_eq!(state.focused_index, 3, "先回到记忆位置 2，再前进一格");
        assert_eq!(state.created_focused_index, 0, "原分区焦点已存回");
        assert_eq!(state.collected_focused_index, 3);
    }

    /// 指到空分区时不会把焦点放进空列表：仍留在有内容的分区里走一格。
    #[test]
    fn home_sidebar_wheel_on_empty_section_keeps_full_one() {
        let mut state = sidebar_state(2, 0);
        state.expanded = true;

        state.scroll_section_by(HomeSidebarSection::Collected, true);

        assert_eq!(state.focused_section, HomeSidebarSection::Created);
        assert_eq!(state.focused_index, 1);
    }

    /// 分区判定：光标所在分区优先，落在分区之间的空隙时用当前聚焦分区，
    /// 面板外或面板未登记则不响应（收起态由调用方先挡掉）。
    #[test]
    fn home_sidebar_wheel_targets_section_under_cursor() {
        let panel = HitRect {
            x: 0,
            y: 0,
            width: 30,
            height: 20,
        };
        let sections = [
            (
                HitRect {
                    x: 0,
                    y: 5,
                    width: 30,
                    height: 6,
                },
                HomeSidebarSection::Created,
            ),
            (
                HitRect {
                    x: 0,
                    y: 11,
                    width: 30,
                    height: 6,
                },
                HomeSidebarSection::Collected,
            ),
        ];

        assert_eq!(
            home_sidebar_wheel_target(Some(panel), &sections, HomeSidebarSection::Created, 3, 12),
            Some(HomeSidebarSection::Collected),
            "指到收藏区就滚收藏区"
        );
        assert_eq!(
            home_sidebar_wheel_target(Some(panel), &sections, HomeSidebarSection::Collected, 3, 2),
            Some(HomeSidebarSection::Collected),
            "落在分区外的面板空白处 → 用聚焦分区"
        );
        assert_eq!(
            home_sidebar_wheel_target(Some(panel), &sections, HomeSidebarSection::Created, 40, 12),
            None,
            "面板外不响应"
        );
        assert_eq!(
            home_sidebar_wheel_target(None, &sections, HomeSidebarSection::Created, 3, 12),
            None,
            "面板未登记（侧边栏宽度不足）不响应"
        );
    }

    /// 作者行按连接符切段：下标与显示位置一一对应（空段保留占位）。
    #[test]
    fn artist_name_segments_split_the_display_line() {
        assert_eq!(artist_name_segments("Jay"), vec!["Jay"]);
        assert_eq!(
            artist_name_segments("Caffeine / 初音ミク"),
            vec!["Caffeine", "初音ミク"]
        );
        assert_eq!(artist_name_segments("A / B / C"), vec!["A", "B", "C"]);
        assert_eq!(artist_name_segments(""), vec![""]);
    }

    /// 段序号 → 作者 ID：先认名字（`ar` 顺序不同也对），再按位置兜底，都没有就 None。
    #[test]
    fn pick_artist_id_matches_the_name_then_the_position() {
        let artists = vec![
            ("初音ミク".to_string(), Some("9001".to_string())),
            ("Caffeine".to_string(), Some("9002".to_string())),
        ];

        assert_eq!(
            pick_artist_id(&artists, "Caffeine / 初音ミク", 0),
            Some("9002".to_string()),
            "点第 1 段：名字匹配到表里的第 2 位"
        );
        assert_eq!(
            pick_artist_id(&artists, "Caffeine / 初音ミク", 1),
            Some("9001".to_string())
        );

        let only_other = vec![("Other".to_string(), Some("1".to_string()))];
        assert_eq!(
            pick_artist_id(&only_other, "A / B", 0),
            Some("1".to_string()),
            "名字对不上时按位置兜底"
        );
        assert_eq!(
            pick_artist_id(&only_other, "A / B", 1),
            None,
            "名字对不上且位置越界"
        );
        assert_eq!(pick_artist_id(&[], "A", 0), None, "没有作者表");
    }
}
