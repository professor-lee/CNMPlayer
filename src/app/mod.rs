mod api;
pub(crate) mod download;
mod mpris_bridge;
pub(crate) mod player;
mod startup;
pub(crate) mod streaming;

use crate::app::api::error_for_status;
use crate::app::player::is_nonempty_file;
use crate::data::config::{AudioQuality, BarChannels, BarNumber, Language, VisualizeMode};
use crate::data::config::{Config, GraphicsProtocol};
use crate::data::playback_session;
use crate::data::private_roam;
use crate::data::session;
use crate::data::theme_loader::ThemeLoader;
use crate::launch;
use crate::render::cover_renderer::render_cover_ascii;
use crate::render::graphics_overlay::cover_viewport;
use crate::tmplayer::app::state::LyricLine;
use crate::tmplayer::audio::cava::{CavaChannels, CavaConfig, MiniCavaState};
use crate::tmplayer::audio::pcm_tap::PcmRing;
use crate::tmplayer::playback::metadata::{parse_lrc, parse_plain_lyrics};
use crate::ui::page_lyrics;
use crate::ui::theme::Theme;
use anyhow::{Context, Result, anyhow, bail};
use crossterm::event::{
    KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use cyper::Client;
use futures::channel::mpsc::{UnboundedReceiver, UnboundedSender, unbounded};
use futures::{FutureExt, future::Shared};
use http::header;
use image::DynamicImage;
use ncm_api::ApiResponse;
use parking_lot::Mutex;
use ratatui::Frame;
use ratatui::layout::{Rect, Size};
use ratatui::style::Style;
use ratatui::widgets::{Block, Paragraph};
use ratatui_image::StatefulImage;
use ratatui_image::picker::Picker;
use ratatui_image::protocol::StatefulProtocol;
use serde_json::Value;
use std::collections::HashSet;
use std::collections::hash_map::DefaultHasher;
use std::fs;
use std::hash::{Hash, Hasher};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use unicode_width::UnicodeWidthChar;

use api::ApiState;
use download::{
    DownloadEvent, DownloadManager, DownloadRequest, DownloadRow, DownloadRowCache, DownloadState,
    DownloadTarget,
};
use mpris_bridge::{MprisBridge, MprisControlEvent, MprisSyncPayload};
use player::{AudioPlayer, AudioPlayerState, cleanup_cache_dir, resolve_cache_root};
use startup::StartupInit;
use streaming::StreamingReader;

const MAX_INPUT_LEN: usize = 64;
/// 下载路径输入框的长度上限（字符）。
const DOWNLOAD_PATH_MAX_CHARS: usize = 4096;

/// 列表代的全局计数器：任何一次列表内容替换都换一个新号（不复用），
/// 行内图标的行数据缓存据此失效——比逐帧比对内容便宜且不会漏。
static LIST_GENERATION: AtomicU64 = AtomicU64::new(1);

fn next_list_generation() -> u64 {
    LIST_GENERATION.fetch_add(1, Ordering::Relaxed)
}
const SEARCH_RESULT_PAGE_SIZE: usize = 50;
/// 无后缀（混合）搜索里作者 / 歌单分区只取最相关的少量条目，不参与分页。
const MIXED_AUX_RESULT_LIMIT: usize = 5;
const SEARCH_BOX_TARGET_HEIGHT: u16 = 3;
/// 搜索框滑出动画时长（time-based，与帧率解耦）
const SEARCH_BOX_ANIM_DURATION: Duration = Duration::from_millis(180);
/// 侧边栏滑出动画时长（time-based，与帧率解耦）。主页与全屏播放页共用。
pub(crate) const SIDEBAR_ANIM_DURATION: Duration = Duration::from_millis(200);
const HOME_SIDEBAR_PLAYLIST_LIMIT: usize = 100;
const SETTINGS_ROOT_ITEMS: usize = 13;
const SETTINGS_PLAYBACK_ITEMS: usize = 8;
const SETTINGS_LYRICS_ITEMS: usize = 3;
pub(crate) const SETTINGS_DOWNLOAD_ITEMS: usize = 3;
pub(crate) const SETTINGS_KEYBIND_ITEMS: usize = 22;
/// 主程序内容页小窗口模式的统一触发阈值，与主页既有判定一致。
pub(crate) const SMALL_WINDOW_MIN_WIDTH: u16 = 32;
pub(crate) const SMALL_WINDOW_MIN_HEIGHT: u16 = 12;
/// 全屏页普通布局的最小宽度；主程序小于该宽度时不响应打开全屏。
const FULLSCREEN_MIN_WIDTH: u16 = 50;
/// 扁窗视口高度下限，即折叠播放栏区域高度。
const FLAT_SMALL_HEIGHT: u16 = 5;
/// 扁窗播放栏/歌词栏切换动画时长。
const FLAT_SWITCH_ANIM_DURATION: Duration = Duration::from_millis(220);
const CONTENT_DOUBLE_CLICK_MS: u64 = 400;
const GLOBAL_HOTKEY_COOLDOWN_MS: u64 = 120;
const STARTUP_LOADING_MIN_VISIBLE_SECS: f32 = 0.75;
const STARTUP_LOADING_FILL_SECS: f32 = 0.62;
const STARTUP_LOADING_COMPLETE_RAMP_SECS: f32 = 0.26;
const RESERVED_RESET_KEYBIND: &str = "Ctrl+Alt+R";
const COVER_CACHE_SUBDIR: &str = "cover";
const COVER_FETCH_RETRY_MS: u64 = 1500;
const LYRICS_FETCH_RETRY_MS: u64 = 1500;
/// 窄窗 LUFS 表显示范围：-60..0 LUFS。
const VU_LUFS_FLOOR: f32 = -120.0;
const VU_LUFS_MIN: f32 = -60.0;
const VU_LUFS_MAX: f32 = 0.0;
const VU_ATTACK_SECS: f32 = 0.08;
const VU_RELEASE_SECS: f32 = 0.35;
const VU_SETTLED_EPSILON: f32 = 0.05;

const DEFAULT_KEYBIND_SEARCH_BOX: &str = "Ctrl+S";
const DEFAULT_KEYBIND_FULLSCREEN: &str = "Ctrl+F";
const DEFAULT_KEYBIND_SETTINGS: &str = "T";
const DEFAULT_KEYBIND_SIDEBAR: &str = "P";
const DEFAULT_KEYBIND_QUIT: &str = "Q";
const DEFAULT_KEYBIND_PAGE_UP: &str = "pageUP";
const DEFAULT_KEYBIND_PAGE_DOWN: &str = "pageDown";
const DEFAULT_KEYBIND_PREV: &str = "Alt+Left";
const DEFAULT_KEYBIND_NEXT: &str = "Alt+Right";
const DEFAULT_KEYBIND_TOGGLE_PLAY_PAUSE: &str = "Alt+Space";
const DEFAULT_KEYBIND_TOGGLE_MODE: &str = "Alt+M";
const DEFAULT_KEYBIND_FULLSCREEN_PREV: &str = "Left";
const DEFAULT_KEYBIND_FULLSCREEN_NEXT: &str = "Right";
const DEFAULT_KEYBIND_FULLSCREEN_TOGGLE_PLAY_PAUSE: &str = "Space";
const DEFAULT_KEYBIND_FULLSCREEN_TOGGLE_MODE: &str = "M";
const DEFAULT_KEYBIND_FULLSCREEN_EQ: &str = "E";
const DEFAULT_KEYBIND_FULLSCREEN_EQ_RESET: &str = "Alt+R";
const DEFAULT_KEYBIND_TOGGLE_LIKE_FULLSCREEN: &str = "L";
const DEFAULT_KEYBIND_TOGGLE_LIKE_COLLAPSED: &str = "Alt+L";
const DEFAULT_KEYBIND_SMALL_WINDOW_TOGGLE: &str = "Alt+X";
const DEFAULT_KEYBIND_DOWNLOAD: &str = "Ctrl+Alt+D";
const DEFAULT_KEYBIND_DOWNLOAD_FULLSCREEN: &str = "Ctrl+D";

#[derive(Debug, Clone, Copy)]
enum KeybindAction {
    SearchBox,
    Fullscreen,
    Settings,
    Sidebar,
    Quit,
    PageUp,
    PageDown,
    Prev,
    Next,
    TogglePlayPause,
    ToggleMode,
    FullscreenPrev,
    FullscreenNext,
    FullscreenTogglePlayPause,
    FullscreenToggleMode,
    FullscreenEq,
    FullscreenEqReset,
    ToggleLikeFullscreen,
    ToggleLikeCollapsed,
    SmallWindowToggle,
    /// 主应用：下载聚焦的单曲。
    Download,
    /// 全屏页：下载当前播放的单曲（宿主侧不处理）。
    DownloadFullscreen,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Page {
    Login,
    Loading,
    Home,
    Playlist,
    Author,
    Search,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Overlay {
    Settings,
    SettingsPlayback,
    SettingsKeybinds,
    SettingsLyrics,
    SettingsDownload,
    SettingsAbout,
    SearchBox,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SmallWindowMode {
    Flat,
    Narrow,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlatPanel {
    Player,
    Lyrics,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct FlatSwitchAnim {
    pub from_x: f32,
    pub to_x: f32,
    pub started_at: Instant,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoginMethod {
    Qr,
    Username,
    Phone,
}

pub struct LoginState {
    pub method: LoginMethod,
    pub focus_index: usize,
    pub username: String,
    pub password: String,
    pub phone: String,
    pub captcha: String,
    pub qr_key: String,
    pub qr_url: String,
    pub status_line: String,
}

impl Default for LoginState {
    fn default() -> Self {
        Self {
            method: LoginMethod::Qr,
            focus_index: 0,
            username: String::new(),
            password: String::new(),
            phone: String::new(),
            captcha: String::new(),
            qr_key: String::new(),
            qr_url: String::new(),
            status_line: "按 F1 刷新二维码后扫码登录".to_string(),
        }
    }
}

impl LoginState {
    pub fn set_method(&mut self, method: LoginMethod) {
        if self.method != method {
            self.method = method;
            self.focus_index = 0;
        }
    }

    pub fn field_count(&self) -> usize {
        match self.method {
            LoginMethod::Qr => 2,
            LoginMethod::Username => 3,
            LoginMethod::Phone => 4,
        }
    }

    pub fn next_focus(&mut self) {
        let total = self.field_count();
        if total == 0 {
            return;
        }
        self.focus_index = (self.focus_index + 1) % total;
    }

    pub fn prev_focus(&mut self) {
        let total = self.field_count();
        if total == 0 {
            return;
        }
        self.focus_index = if self.focus_index == 0 {
            total - 1
        } else {
            self.focus_index - 1
        };
    }

    fn is_input_focused(&self) -> bool {
        match self.method {
            LoginMethod::Qr => false,
            LoginMethod::Username => self.focus_index <= 1,
            LoginMethod::Phone => self.focus_index <= 1,
        }
    }

    fn active_input_mut(&mut self) -> Option<&mut String> {
        match self.method {
            LoginMethod::Qr => None,
            LoginMethod::Username => match self.focus_index {
                0 => Some(&mut self.username),
                1 => Some(&mut self.password),
                _ => None,
            },
            LoginMethod::Phone => match self.focus_index {
                0 => Some(&mut self.phone),
                1 => Some(&mut self.captcha),
                _ => None,
            },
        }
    }

    pub fn push_char(&mut self, ch: char) {
        if ch.is_control() || !self.is_input_focused() {
            return;
        }
        if let Some(value) = self.active_input_mut() {
            if value.chars().count() < MAX_INPUT_LEN {
                value.push(ch);
            }
        }
    }

    pub fn pop_char(&mut self) {
        if !self.is_input_focused() {
            return;
        }
        if let Some(value) = self.active_input_mut() {
            value.pop();
        }
    }
}

type SharedFuture<T> = Shared<Pin<Box<dyn Future<Output = Option<T>>>>>;

/// 侧边栏歌单的一次拉取结果。异步任务不持有 `&mut App`，
/// 取完由 `tick_home_sidebar_fetch` 搬进状态。
#[derive(Clone)]
struct HomeSidebarFetch {
    user_id: String,
    liked_playlist_id: Option<String>,
    user_name: String,
    created: Vec<HomeSidebarPlaylist>,
    collected: Vec<HomeSidebarPlaylist>,
}

type HomeSidebarFetchFuture = SharedFuture<Result<HomeSidebarFetch, String>>;

async fn fetch_home_sidebar_playlists(
    mut api: ApiState,
    language: Language,
) -> Result<HomeSidebarFetch, String> {
    let zh = matches!(language, Language::Zh);
    let pick = |z: &'static str, e: &'static str| if zh { z } else { e };

    let account = match api.user_account().await {
        Ok(v) => v,
        Err(_) => api.login_status().await.map_err(|err| {
            format!(
                "{}: {err}",
                pick("账号信息请求失败", "Account request failed")
            )
        })?,
    };
    let account_code = response_code(&account);
    if account_code != 200 {
        return Err(format!(
            "{}({}): {}",
            pick("账号信息请求失败", "Failed to fetch account profile"),
            account_code,
            response_message(&account)
        ));
    }

    let user_id = extract_current_user_id(&account)
        .ok_or_else(|| pick("未找到当前用户 ID", "Current user id not found").to_string())?;
    let user_name = extract_current_user_name(&account)
        .unwrap_or_else(|| pick("当前用户", "Current User").to_string());

    let created_response = api
        .user_playlist_create(&user_id, HOME_SIDEBAR_PLAYLIST_LIMIT, 0)
        .await
        .map_err(|err| {
            format!(
                "{}: {err}",
                pick("创建歌单请求失败", "Created playlists request failed")
            )
        })?;
    let created_code = response_code(&created_response);
    if created_code != 200 {
        return Err(format!(
            "{}({}): {}",
            pick("创建歌单请求失败", "Created playlists request failed"),
            created_code,
            response_message(&created_response)
        ));
    }

    let collected_response = api
        .user_playlist_collect(&user_id, HOME_SIDEBAR_PLAYLIST_LIMIT, 0)
        .await
        .map_err(|err| {
            format!(
                "{}: {err}",
                pick("收藏歌单请求失败", "Collected playlists request failed")
            )
        })?;
    let collected_code = response_code(&collected_response);
    if collected_code != 200 {
        return Err(format!(
            "{}({}): {}",
            pick("收藏歌单请求失败", "Collected playlists request failed"),
            collected_code,
            response_message(&collected_response)
        ));
    }

    Ok(HomeSidebarFetch {
        user_id,
        liked_playlist_id: extract_liked_playlist_id(&account),
        user_name,
        created: parse_home_sidebar_playlists(&created_response),
        collected: parse_home_sidebar_playlists(&collected_response),
    })
}
type CoverFuture = SharedFuture<Arc<DynamicImage>>;
type AsciiFuture = SharedFuture<String>;

type AuthorFetchFuture = SharedFuture<Result<AuthorFetch, String>>;
/// 装箱后的任务体（`shot_and_share` 的入参类型）。
type AuthorFetchTask = Pin<Box<dyn Future<Output = Option<Result<AuthorFetch, String>>>>>;

/// 作者页四个接口的原始回包（`None` = 该请求失败）；解析见 `App::build_author_page`。
struct AuthorResponses {
    detail: Option<ApiResponse>,
    desc: Option<ApiResponse>,
    top_song: Option<ApiResponse>,
    album: Option<ApiResponse>,
}

/// 作者页的一次拉取结果：`AuthorState` 里除封面句柄与视口字段外的全部字段。
///
/// 结果要经 `shot_and_share` 搬运，故实现 `Clone`（封面句柄是 `Shared`，克隆很廉价）。
#[derive(Clone)]
struct AuthorFetch {
    id: String,
    title: String,
    artist: String,
    description: String,
    cover_url: Option<String>,
    tiles: Vec<AuthorTile>,
    hot_songs: Vec<PlaylistTrack>,
    albums: Vec<PlaylistTrack>,
    eps: Vec<PlaylistTrack>,
    singles: Vec<PlaylistTrack>,
}

/// 四个 `artist/*` 接口一次并发拉取。
///
/// 它们彼此独立，按仓库既有做法用 `futures::join!`：`cyper::Client` 是 `!Send`，
/// 只能同一个 runtime 里并发，不能各自 spawn。
async fn fetch_artist_responses(api: &ApiState, artist_id: &str) -> AuthorResponses {
    let mut detail_api = api.clone();
    let mut desc_api = api.clone();
    let mut top_song_api = api.clone();
    let mut album_api = api.clone();
    let (detail, desc, top_song, album) = futures::join!(
        detail_api.artist_detail(artist_id),
        desc_api.artist_desc(artist_id),
        top_song_api.artist_top_song(artist_id),
        album_api.artist_album(artist_id, 60, 0),
    );

    AuthorResponses {
        detail: detail.ok(),
        desc: desc.ok(),
        top_song: top_song.ok(),
        album: album.ok(),
    }
}

/// 全屏页点作者名要拉的东西：先 `song/detail` 解析出段对应的作者 ID，再拉作者页数据。
///
/// 整段不借 `&mut App`，交给 `shot_and_share` 后台跑，宿主循环照常重绘。
async fn fetch_author_page(
    api: ApiState,
    language: Language,
    song_id: String,
    index: usize,
    artist_line: String,
) -> Result<AuthorFetch, String> {
    let refs = fetch_song_page_refs(api.clone(), &song_id).await;
    let artist_id = pick_artist_id(&refs.artists, &artist_line, index).ok_or_else(|| {
        lang_text(
            language,
            "无法解析当前歌曲的作者",
            "Failed to resolve the artist of the current song",
        )
        .to_string()
    })?;

    // 全屏页只有显示名，没有搜索结果那行的封面可以兜底。
    fetch_author_page_by_id(api, language, artist_id, None).await
}

/// 拉一次作者页数据（作者 ID 已确定）。
///
/// `fallback_cover_url` 是搜索结果里那行的封面：接口没给头像时用它兜底。
async fn fetch_author_page_by_id(
    api: ApiState,
    language: Language,
    artist_id: String,
    fallback_cover_url: Option<String>,
) -> Result<AuthorFetch, String> {
    let responses = fetch_artist_responses(&api, &artist_id).await;
    let mut fetch = App::build_author_page(&api, language, &artist_id, responses)?;
    if fetch.cover_url.is_none() {
        fetch.cover_url = fallback_cover_url;
    }
    Ok(fetch)
}

/// `song/detail` →「点名字进页面」要用的作者 / 专辑 ID。
///
/// 队列与搜索结果只带歌曲 ID 与拼好的显示名（曲目行没有 `ar`/`al` 的 ID），
/// 所以按需解析一次。
async fn fetch_song_page_refs(mut api: ApiState, song_id: &str) -> SongPageRefs {
    let Ok(detail) = api.song_detail(song_id).await else {
        return SongPageRefs::default();
    };

    let Some(song) = detail
        .body
        .get("songs")
        .and_then(|value| value.as_array())
        .and_then(|items| items.first())
    else {
        return SongPageRefs::default();
    };

    let artists = song
        .get("ar")
        .and_then(|value| value.as_array())
        .map(|items| {
            items
                .iter()
                .filter_map(|item| {
                    let name = item.get("name").and_then(|value| value.as_str())?;
                    Some((name.to_string(), parse_value_as_string(item.get("id"))))
                })
                .collect()
        })
        .unwrap_or_default();
    let album_id = parse_value_as_string(song.pointer("/al/id"));

    SongPageRefs { artists, album_id }
}

type PlaylistFetchFuture = SharedFuture<Result<PlaylistFetch, String>>;
/// 装箱后的任务体（`shot_and_share` 的入参类型）。
type PlaylistFetchTask = Pin<Box<dyn Future<Output = Option<Result<PlaylistFetch, String>>>>>;

/// 歌单页 / 专辑页的在途拉取：句柄旁边记下是哪种页面（成功后文案不同）。
struct PlaylistFetchSlot {
    kind: PlaylistPageKind,
    future: PlaylistFetchFuture,
}

/// 一次性生成歌单页 / 专辑页的整页行数据：文件名与磁盘缓存 key 都在这里做完，
/// 之后每帧只查状态表（列表代或下载根目录变化时才重建）。所有下载都直接落根目录。
fn playlist_download_rows(
    tracks: &[PlaylistTrack],
    root: Option<&Path>,
) -> Vec<Option<DownloadRow>> {
    let Some(root) = root else {
        return vec![None; tracks.len()];
    };

    tracks
        .iter()
        .map(|track| {
            if track.kind != PlaylistTrackKind::Song {
                return None;
            }
            let song_id = track.id.clone()?;
            let target = DownloadTarget {
                dir: root.to_path_buf(),
                base: crate::app::download::download_file_stem(
                    &track.title,
                    &track.artist,
                    &track.album,
                ),
            };
            Some(DownloadRow::new(song_id, target))
        })
        .collect()
}

/// 搜索页同理：只有单曲行有图标，且不落专辑子文件夹。
fn search_download_rows(results: &[SearchItem], root: Option<&Path>) -> Vec<Option<DownloadRow>> {
    let Some(root) = root else {
        return vec![None; results.len()];
    };
    let dir = root.to_path_buf();

    results
        .iter()
        .map(|item| {
            if item.kind != SearchItemKind::Song {
                return None;
            }
            let song_id = item.song_id.clone()?;
            let target = DownloadTarget {
                dir: dir.clone(),
                base: crate::app::download::download_file_stem(
                    item.title.as_deref().unwrap_or(&item.left_label),
                    item.artist.as_deref().unwrap_or_default(),
                    item.album.as_deref().unwrap_or_default(),
                ),
            };
            Some(DownloadRow::new(song_id, target))
        })
        .collect()
}

/// 打开的是歌单还是专辑：端点与文案不同，落状态是同一套。
#[derive(Clone, Copy, PartialEq, Eq)]
enum PlaylistPageKind {
    Playlist,
    Album,
}

/// 下载路径行的行内编辑状态（光标按字符计数，横向窗口按显示列算）。
#[derive(Debug, Clone, Default)]
pub struct DownloadPathEdit {
    pub buffer: String,
    pub cursor: usize,
    /// 可见窗口左边界所在的显示列：只在光标撞到窗口边界时才挪动。
    pub window_col: usize,
}

/// 一次下载的候选歌曲：UI 侧决定落点所需的全部信息。
struct DownloadCandidate {
    song_id: String,
    title: String,
    artist: String,
    album: String,
}

impl PlaylistPageKind {
    fn opened(self, language: Language) -> &'static str {
        match (self, language) {
            (Self::Playlist, Language::Zh) => "已打开歌单",
            (Self::Playlist, Language::En) => "Opened playlist",
            (Self::Album, Language::Zh) => "已打开专辑",
            (Self::Album, Language::En) => "Opened album",
        }
    }

    fn failed(self, language: Language) -> &'static str {
        match (self, language) {
            (Self::Playlist, Language::Zh) => "打开歌单失败",
            (Self::Playlist, Language::En) => "Failed to open playlist",
            (Self::Album, Language::Zh) => "打开专辑失败",
            (Self::Album, Language::En) => "Failed to open album",
        }
    }
}

/// 「我喜欢的音乐」全量 id 的刷新结果（打开该歌单时顺带做一次）。
#[derive(Clone)]
struct LikedRefresh {
    ids: HashSet<String>,
    /// 只有当场取过账号档案时才有值（否则沿用已有 uid，不动档案）。
    profile: Option<AccountProfile>,
}

/// 歌单页 / 专辑页的一次拉取结果：`PlaylistState` 里除封面句柄与视口字段外的全部字段。
///
/// 结果要经 `shot_and_share` 搬运，故实现 `Clone`（封面句柄是 `Shared`，克隆很廉价）。
#[derive(Clone)]
struct PlaylistFetch {
    id: String,
    title: String,
    artist: String,
    description: String,
    cover_url: Option<String>,
    tracks: Vec<PlaylistTrack>,
    liked: Option<LikedRefresh>,
}

/// 打开的歌单是不是「我喜欢的音乐」：ID 对得上，或标题命中（中英文两种叫法）。
fn is_liked_playlist(
    liked_playlist_id: Option<&str>,
    playlist_id: &str,
    title: Option<&str>,
) -> bool {
    if liked_playlist_id == Some(playlist_id) {
        return true;
    }

    let title = title.unwrap_or_default().trim();
    !title.is_empty()
        && (title.contains("我喜欢的音乐") || title.to_ascii_lowercase().contains("liked songs"))
}

/// 「我喜欢的音乐」全量 id：uid 已知就直接用，否则顺带取一次账号档案。
///
/// 任一步失败都返回 `None`（旧行为是忽略这里的错误，不影响歌单页打开）。
async fn fetch_liked_refresh(
    api: &mut ApiState,
    language: Language,
    uid_hint: Option<String>,
) -> Option<LikedRefresh> {
    let (uid, profile) = match uid_hint {
        Some(uid) => (uid, None),
        None => {
            let profile = fetch_account_profile(api, language).await.ok()?;
            (profile.uid.clone(), Some(profile))
        }
    };

    let ids = fetch_liked_song_ids(api, &uid, language).await.ok()?;
    Some(LikedRefresh { ids, profile })
}

/// 拉一次歌单页数据（不借 `&mut App`，可交给 `shot_and_share` 后台跑）。
///
/// `fallback_cover_url` 是搜索结果里那行的封面：接口没给封面时用它兜底。
/// `liked_playlist_id` / `uid_hint` 只用于判断要不要顺带刷新「我喜欢的音乐」。
async fn fetch_playlist_page(
    mut api: ApiState,
    language: Language,
    playlist_id: String,
    fallback_cover_url: Option<String>,
    liked_playlist_id: Option<String>,
    uid_hint: Option<String>,
) -> Result<PlaylistFetch, String> {
    let response = api
        .playlist_detail(&playlist_id)
        .await
        .map_err(|err| err.to_string())?;
    let code = response_code(&response);
    if code != 200 {
        return Err(format!(
            "请求失败({}): {}",
            code,
            response_message(&response)
        ));
    }

    let playlist = response
        .body
        .get("playlist")
        .ok_or_else(|| "歌单数据缺失".to_string())?;

    let title = playlist
        .get("name")
        .and_then(|value| value.as_str())
        .unwrap_or("未命名歌单")
        .to_string();

    let liked = if is_liked_playlist(liked_playlist_id.as_deref(), &playlist_id, Some(&title)) {
        fetch_liked_refresh(&mut api, language, uid_hint).await
    } else {
        None
    };

    let artist = playlist
        .pointer("/creator/nickname")
        .and_then(|value| value.as_str())
        .unwrap_or("网易云音乐")
        .to_string();

    let description = first_non_empty(
        playlist,
        &["/description", "/copywriter", "/creator/signature"],
    )
    .unwrap_or_else(|| "暂无简介".to_string());

    let cover_url = first_non_empty(playlist, &["/coverImgUrl", "/picUrl"]).or(fallback_cover_url);

    let tracks = playlist
        .get("tracks")
        .and_then(|value| value.as_array())
        .map(|items| parse_tracks(items))
        .unwrap_or_default();

    Ok(PlaylistFetch {
        id: playlist_id,
        title,
        artist,
        description,
        cover_url,
        tracks,
        liked,
    })
}

/// 拉一次专辑页数据（歌单页样式复用同一套落状态）。
///
/// 专辑接口不给每首歌的封面，缺封面的曲目用专辑封面兜底。
async fn fetch_album_page(
    mut api: ApiState,
    language: Language,
    album_id: String,
    fallback_cover_url: Option<String>,
) -> Result<PlaylistFetch, String> {
    let response = api.album(&album_id).await.map_err(|err| err.to_string())?;
    let code = response_code(&response);
    if code != 200 {
        return Err(format!(
            "请求失败({}): {}",
            code,
            response_message(&response)
        ));
    }

    let album = response
        .body
        .get("album")
        .or_else(|| response.body.pointer("/data/album"))
        .ok_or_else(|| "专辑数据缺失".to_string())?;

    let title = album
        .get("name")
        .and_then(|value| value.as_str())
        .unwrap_or("未命名专辑")
        .to_string();

    let artist = first_non_empty(album, &["/artist/name", "/artists/0/name"])
        .unwrap_or_else(|| "网易云音乐".to_string());

    let description = first_non_empty(album, &["/description", "/company", "/type", "/subType"])
        .unwrap_or_else(|| lang_text(language, "暂无简介", "No description").to_string());

    let cover_url = first_non_empty(album, &["/picUrl", "/blurPicUrl"]).or(fallback_cover_url);

    let mut tracks = response
        .body
        .get("songs")
        .or_else(|| response.body.pointer("/data/songs"))
        .and_then(|value| value.as_array())
        .map(|items| parse_tracks(items))
        .unwrap_or_default();

    if let Some(album_cover_url) = cover_url.as_ref() {
        for track in &mut tracks {
            let missing_song_cover = track
                .cover_url
                .as_deref()
                .map(|value| value.trim().is_empty())
                .unwrap_or(true);
            if missing_song_cover {
                track.cover_url = Some(album_cover_url.clone());
            }
        }
    }

    Ok(PlaylistFetch {
        id: album_id,
        title,
        artist,
        description,
        cover_url,
        tracks,
        liked: None,
    })
}

/// 全屏页点专辑名要拉的东西：先 `song/detail` 解析出 `al.id`，再拉专辑页数据。
///
/// 与作者页同理：整段不借 `&mut App`，交给 `shot_and_share` 后台跑；
/// 全屏页只有显示名，本机音频 / 无播放时解析不出来，错误写进占位页与状态行。
async fn fetch_album_page_from_song(
    api: ApiState,
    language: Language,
    song_id: String,
) -> Result<PlaylistFetch, String> {
    let refs = fetch_song_page_refs(api.clone(), &song_id).await;
    let album_id = refs.album_id.ok_or_else(|| {
        lang_text(
            language,
            "无法解析当前歌曲的专辑",
            "Failed to resolve the album of the current song",
        )
        .to_string()
    })?;

    // 全屏页只有显示名，没有搜索结果那行的封面可以兜底。
    fetch_album_page(api, language, album_id, None).await
}

fn shot_and_share<F>(fut: F) -> Shared<F>
where
    F: Future + Sized + 'static,
    F::Output: Clone,
{
    let shared = fut.shared();
    launch(shared.clone());
    shared
}

pub fn peek_shared_future<T>(cover_bytes: &Option<SharedFuture<T>>) -> Option<&T> {
    cover_bytes.as_ref()?.peek()?.as_ref()
}

/// 句柄不在 `Option` 里时的取值变体。
fn peek_shared<T>(fut: &SharedFuture<T>) -> Option<&T> {
    fut.peek()?.as_ref()
}

/// 收藏写入请求。成功 `Ok(())`；失败带可展示的原因（接口错误码或传输错误）。
///
/// 返回 `String` 而非 `anyhow::Error`：结果要跨 future 边界搬运，需 `Clone`。
async fn like_song_request(mut api: ApiState, song_id: String, target: bool) -> Result<(), String> {
    match api.like_song(&song_id, target).await {
        Ok(response) => {
            let code = response
                .body
                .get("code")
                .and_then(|value| value.as_i64())
                .unwrap_or(response.status);
            if code == 200 {
                Ok(())
            } else {
                Err(code.to_string())
            }
        }
        Err(err) => Err(err.to_string()),
    }
}

/// 服务端收藏态确认请求（切歌后核对账号里的真实状态）。
async fn song_like_check_request(mut api: ApiState, song_id: String) -> Result<bool, ()> {
    let Ok(song_id_num) = song_id.parse::<u64>() else {
        return Err(());
    };

    let ids_json = format!("[{song_id_num}]");
    let Ok(response) = api.song_like_check(&ids_json).await else {
        return Err(());
    };
    if response_code(&response) != 200 {
        return Err(());
    }

    parse_song_like_check_result(&response.body, &song_id).ok_or(())
}

type LikeToggleFuture = SharedFuture<Result<(), String>>;
type LikeVerifyFuture = SharedFuture<Result<bool, ()>>;
/// 装箱后的任务体（`shot_and_share` 的入参类型）。
type LikeToggleTask = Pin<Box<dyn Future<Output = Option<Result<(), String>>>>>;
type LikeVerifyTask = Pin<Box<dyn Future<Output = Option<Result<bool, ()>>>>>;

/// 在途的收藏写入。
struct PendingLikeToggle {
    song_id: String,
    target: bool,
    fut: LikeToggleFuture,
}

/// 在途的收藏确认。
struct PendingLikeVerify {
    song_id: String,
    fut: LikeVerifyFuture,
}

/// 一次收藏写入回包的收敛结果。
#[derive(Debug, PartialEq, Eq)]
enum ToggleOutcome {
    /// 仍是当前意图：确认值已写入，界面据此提示。
    Settled { liked: bool },
    /// 已被更新的意图取代：确认值已写入，不提示，由 reconcile 补发新意图。
    Superseded,
    /// 失败且仍是当前意图：意图已放弃，显示应回滚到已确认值。
    Failed { message: String },
    /// 失败但已被取代：忽略。
    StaleFailure,
}

/// 收藏的「期望 / 已确认」双轨状态机。
///
/// 点击只写 `desired` 并把界面切到期望值（乐观更新），真实请求由
/// `App::tick_like_sync` 在每帧派发与收敛——输入路径上不再 `await` 网络，
/// 收藏不会冻结单线程事件循环的动画。
///
/// - 连点：只保留最后一次期望；在途请求串行化，其回包不会覆盖更新的意图。
/// - 失败：回滚显示到已确认集合，并写状态行。
///
/// 本结构不依赖 `App` 与网络：回包由调用方（tick 或单测）喂进 `on_*_result`。
#[derive(Default)]
struct LikeMachine {
    /// 用户最后一次意图（song_id, target）。
    desired: Option<(String, bool)>,
    toggle: Option<PendingLikeToggle>,
    verify: Option<PendingLikeVerify>,
    /// 已确认的收藏集合（服务端口径）。
    confirmed: HashSet<String>,
}

impl LikeMachine {
    /// 服务端已确认该曲目被收藏。
    fn is_confirmed(&self, song_id: &str) -> bool {
        self.confirmed.contains(song_id)
    }

    /// 该曲目当前应显示的状态：未决意图优先，其次已确认值。
    fn displayed(&self, song_id: &str) -> bool {
        match self.desired.as_ref() {
            Some((id, target)) if id == song_id => *target,
            _ => self.is_confirmed(song_id),
        }
    }

    /// 用整份「喜欢的歌曲」列表覆盖已确认集合。
    fn replace_confirmed(&mut self, confirmed: HashSet<String>) {
        self.confirmed = confirmed;
    }

    /// 记录一次点击。新意图优先于任何在途的服务端确认。
    fn set_intent(&mut self, song_id: String, target: bool) {
        self.desired = Some((song_id, target));
        self.verify = None;
    }

    /// 现在该补发的写入请求；`None` 表示无需发（无意图、已满足、或有在途）。
    fn pending_dispatch(&self) -> Option<(String, bool)> {
        if self.toggle.is_some() {
            return None;
        }
        let (song_id, target) = self.desired.as_ref()?;
        if self.is_confirmed(song_id) == *target {
            return None;
        }
        Some((song_id.clone(), *target))
    }

    /// 期望已被满足（例如连点两次回到原状态）时清掉意图，返回该曲目 id。
    fn drop_satisfied_intent(&mut self) -> Option<String> {
        let (song_id, target) = self.desired.as_ref()?;
        if self.is_confirmed(song_id) != *target {
            return None;
        }
        let song_id = song_id.clone();
        self.desired = None;
        Some(song_id)
    }

    fn begin_toggle(&mut self, song_id: String, target: bool, fut: LikeToggleFuture) {
        self.toggle = Some(PendingLikeToggle {
            song_id,
            target,
            fut,
        });
    }

    fn begin_verify(&mut self, song_id: String, fut: LikeVerifyFuture) {
        self.verify = Some(PendingLikeVerify { song_id, fut });
    }

    /// 收敛一次写入回包。
    fn on_toggle_result(
        &mut self,
        song_id: &str,
        target: bool,
        result: Result<(), String>,
    ) -> ToggleOutcome {
        let still_wanted = matches!(
            self.desired.as_ref(),
            Some((id, value)) if id == song_id && *value == target
        );
        if still_wanted {
            self.desired = None;
        }

        match result {
            Ok(()) => {
                self.set_confirmed(song_id, target);
                if still_wanted {
                    ToggleOutcome::Settled { liked: target }
                } else {
                    ToggleOutcome::Superseded
                }
            }
            Err(message) => {
                if still_wanted {
                    ToggleOutcome::Failed { message }
                } else {
                    ToggleOutcome::StaleFailure
                }
            }
        }
    }

    /// 收敛一次确认回包；返回是否真的写入了确认值。
    fn on_verify_result(&mut self, song_id: &str, result: Result<bool, ()>) -> bool {
        let Ok(liked) = result else {
            return false;
        };
        // 更新的意图 / 在途写入优先，别被旧确认覆盖。
        let superseded = matches!(self.desired.as_ref(), Some((id, _)) if id == song_id)
            || self
                .toggle
                .as_ref()
                .is_some_and(|pending| pending.song_id == song_id);
        if superseded {
            return false;
        }

        self.set_confirmed(song_id, liked);
        true
    }

    fn set_confirmed(&mut self, song_id: &str, liked: bool) {
        if liked {
            self.confirmed.insert(song_id.to_string());
        } else {
            self.confirmed.remove(song_id);
        }
    }

    /// 登出等场景：连已确认集合一起丢弃。
    fn clear(&mut self) {
        *self = Self::default();
    }
}

#[derive(Clone, Default)]
pub struct CoverFetchState {
    pub url: Option<String>,
    pub image: Option<CoverFuture>,
    ascii: Option<AsciiFuture>,
    size: Size,
    protocol: Option<Arc<Mutex<StatefulProtocol>>>,
    /// 协议缓存键：整块图尺寸 + 可见行区间。部分可见时按可见比例裁源图，
    /// 键包含可见区间，避免缩放时复用错切片。
    protocol_key: Option<(Size, u16, u16)>,
}

impl CoverFetchState {
    pub fn load(&mut self, api: ApiState, url: String) {
        let cover_url = url.clone();
        let fut = async move {
            let bytes = api.fetch_cover_bytes(&cover_url).await.ok();
            let flatten = bytes.filter(|x| !x.is_empty());
            let image = flatten.and_then(|x| image::load_from_memory(&x).ok());

            // Downsampling to 500px to save memory.
            image.map(|x| x.thumbnail(500, 500)).map(Arc::new)
        };
        let fut = Box::pin(fut);
        self.image = Some(shot_and_share(fut));
        self.url = Some(url);
        self.size = Size::ZERO;
        self.protocol = None;
        self.protocol_key = None;
    }

    pub fn render(
        &mut self,
        frame: &mut Frame,
        picker: &mut Picker,
        area: Rect,
        text_style: Style,
        bg_style: Option<Style>,
        draw_ascii: bool,
    ) {
        self.render_rows(
            frame,
            picker,
            area,
            area.height,
            0..area.height,
            text_style,
            bg_style,
            draw_ascii,
        );
    }

    /// 只渲染 `visible` 行（行号相对整块图，`area` 是这些行的落点），其余行不写入。
    ///
    /// 部分可见时**按可见比例裁源图**：ASCII 路径取对应的文本行，图形路径先把
    /// 可见比例换算成 `cover_viewport` 结果里的行切片再生成协议——所以是"裁"而不是
    /// "压进子矩形"，也不需要画完整块图再擦除。
    // 参数各管一件事（落点/整块行数/可见区间/三种绘制开关），打包成结构体反而更难读。
    #[allow(clippy::too_many_arguments)]
    pub fn render_rows(
        &mut self,
        frame: &mut Frame,
        picker: &mut Picker,
        area: Rect,
        full_rows: u16,
        visible: Range<u16>,
        text_style: Style,
        bg_style: Option<Style>,
        draw_ascii: bool,
    ) {
        let visible_rows = visible
            .end
            .min(full_rows)
            .saturating_sub(visible.start.min(full_rows));
        if area.is_empty() || full_rows == 0 || visible_rows == 0 {
            return;
        }
        let area = Rect {
            height: area.height.min(visible_rows),
            ..area
        };
        if area.is_empty() {
            return;
        }

        if let Some(bg) = bg_style {
            frame.render_widget(Block::default().style(bg), area);
        }

        // 缓存按**整块图**尺寸键控：部分可见时不会每帧重建。
        let size = Size::new(area.width, full_rows);
        if draw_ascii {
            if (self.ascii.is_none() || self.size != size)
                && let Some(bytes) = peek_shared_future(&self.image)
            {
                self.ascii = Some(make_ascii_future(bytes.clone(), area.width, full_rows));
                self.size = size;
            }
            let ascii = match peek_shared_future(&self.ascii) {
                Some(x) => x.clone(),
                None => placeholder_cover_ascii(area.width, full_rows, '░'),
            };
            frame.render_widget(
                Paragraph::new(ascii)
                    .style(text_style)
                    .scroll((visible.start, 0)),
                area,
            );
            return;
        }

        let Some(img) = peek_shared_future(&self.image) else {
            return;
        };

        let key = (size, visible.start, area.height);
        if self.protocol_key.as_ref() != Some(&key) {
            let (crop_x, crop_y, view_w, view_h) =
                cover_viewport(img.width(), img.height(), area.width, full_rows);
            let (slice_y, slice_h) =
                source_rows_for_visible(view_h, full_rows, visible.start, area.height);
            let slice = img.crop_imm(crop_x, crop_y + slice_y, view_w, slice_h);
            self.protocol = Some(Arc::new(Mutex::new(picker.new_resize_protocol(slice))));
            self.protocol_key = Some(key);
        }

        if let Some(proto) = &self.protocol {
            let mut proto = proto.lock();
            let widget = StatefulImage::<StatefulProtocol>::default();
            frame.render_stateful_widget(widget, area, &mut proto);
        }
    }
}

/// 可见行区间对应的源图行区间（相对 viewport 顶部），按比例取，保证是裁切而非压缩。
fn source_rows_for_visible(
    view_h: u32,
    full_rows: u16,
    skip: u16,
    visible_rows: u16,
) -> (u32, u32) {
    if view_h == 0 || full_rows == 0 || visible_rows == 0 {
        return (0, view_h.max(1));
    }

    let full = u32::from(full_rows);
    let start = (view_h * u32::from(skip) / full).min(view_h - 1);
    let end = (view_h * (u32::from(skip) + u32::from(visible_rows)) / full).max(start + 1);
    (start, end.min(view_h) - start)
}

fn make_ascii_future(bytes: Arc<DynamicImage>, width: u16, height: u16) -> AsciiFuture {
    let fut = Box::pin(async move { render_cover_ascii(bytes, width, height) });
    shot_and_share(fut)
}

pub struct HomeTile {
    pub id: Option<String>,
    pub title: String,
    pub subtitle: String,
    pub cover: CoverFetchState,
}

impl HomeTile {
    fn placeholder_daily() -> Self {
        Self {
            id: Some(HOME_DAILY_RECOMMEND_TILE_ID.to_string()),
            title: "每日推荐".to_string(),
            subtitle: String::new(),
            cover: CoverFetchState::default(),
        }
    }

    fn from_recommendation(
        api: &ApiState,
        id: Option<String>,
        title: String,
        subtitle: String,
        cover_url: Option<String>,
    ) -> Self {
        let mut cover = CoverFetchState::default();
        cover_url.map(|x| cover.load(api.clone(), x));
        Self {
            id,
            title,
            subtitle,
            cover,
        }
    }
}

pub struct HomeState {
    pub focused_idx: usize,
    pub columns: usize,
    pub tiles: Vec<HomeTile>,
    pub status_line: String,
    pub scroll_row_offset: usize,
    pub visible_rows: usize,
}

impl Default for HomeState {
    fn default() -> Self {
        Self {
            focused_idx: 0,
            columns: 1,
            tiles: vec![HomeTile::placeholder_daily()],
            status_line: "方向键/Tab 切换，Enter 进入".to_string(),
            scroll_row_offset: 0,
            visible_rows: 1,
        }
    }
}

impl HomeState {
    fn total_virtual_rows(&self) -> usize {
        if self.tiles.is_empty() {
            return 0;
        }

        let columns = self.columns.max(1);
        let last_virtual = home_tile_real_to_virtual_index(self.tiles.len() - 1, columns);
        last_virtual / columns + 1
    }

    fn max_scroll_row_offset(&self) -> usize {
        self.total_virtual_rows()
            .saturating_sub(self.visible_rows.max(1))
    }

    fn clamp_scroll_row_offset(&mut self) {
        self.scroll_row_offset = self.scroll_row_offset.min(self.max_scroll_row_offset());
    }

    fn ensure_focus_visible(&mut self) {
        if self.tiles.is_empty() {
            self.focused_idx = 0;
            self.scroll_row_offset = 0;
            return;
        }

        self.focused_idx = self.focused_idx.min(self.tiles.len() - 1);
        let columns = self.columns.max(1);
        let focused_row = home_tile_real_to_virtual_index(self.focused_idx, columns) / columns;
        let visible_rows = self.visible_rows.max(1);

        if focused_row < self.scroll_row_offset {
            self.scroll_row_offset = focused_row;
        } else {
            let bottom_row = self
                .scroll_row_offset
                .saturating_add(visible_rows.saturating_sub(1));
            if focused_row > bottom_row {
                self.scroll_row_offset = focused_row.saturating_add(1).saturating_sub(visible_rows);
            }
        }

        self.clamp_scroll_row_offset();
    }

    pub fn set_columns(&mut self, columns: usize) {
        self.columns = columns.max(1);
        self.ensure_focus_visible();
    }

    pub fn set_visible_rows(&mut self, visible_rows: usize) {
        self.visible_rows = visible_rows.max(1);
        self.ensure_focus_visible();
    }

    pub fn effective_scroll_row_offset(&self) -> usize {
        self.scroll_row_offset.min(self.max_scroll_row_offset())
    }

    pub fn set_tiles(&mut self, mut tiles: Vec<HomeTile>) {
        if tiles.is_empty() {
            tiles.push(HomeTile::placeholder_daily());
        }
        self.tiles = tiles;
        self.focused_idx = 0;
        self.scroll_row_offset = 0;
        self.ensure_focus_visible();
    }

    pub fn focus_next(&mut self) {
        if self.tiles.is_empty() {
            return;
        }
        self.focused_idx = (self.focused_idx + 1) % self.tiles.len();
        self.ensure_focus_visible();
    }

    pub fn focus_prev(&mut self) {
        if self.tiles.is_empty() {
            return;
        }
        self.focused_idx = if self.focused_idx == 0 {
            self.tiles.len() - 1
        } else {
            self.focused_idx - 1
        };
        self.ensure_focus_visible();
    }

    pub fn focus_left(&mut self) {
        self.focus_prev();
    }

    pub fn focus_right(&mut self) {
        self.focus_next();
    }

    pub fn focus_up(&mut self) {
        if self.tiles.is_empty() {
            return;
        }

        let step = self.columns.max(1);
        let focused_virtual = home_tile_real_to_virtual_index(self.focused_idx, step);
        if focused_virtual < step {
            return;
        }

        let focused_row = focused_virtual / step;
        let target_virtual = focused_virtual - step;
        if let Some(target) =
            home_tile_virtual_to_real_index(target_virtual, step, self.tiles.len())
        {
            let is_top_edge = focused_row == self.scroll_row_offset;
            self.focused_idx = target;
            if is_top_edge && self.scroll_row_offset > 0 {
                self.scroll_row_offset -= 1;
            }
            self.ensure_focus_visible();
        }
    }

    pub fn focus_down(&mut self) {
        if self.tiles.is_empty() {
            return;
        }

        let step = self.columns.max(1);
        let focused_virtual = home_tile_real_to_virtual_index(self.focused_idx, step);
        let focused_row = focused_virtual / step;
        let target_virtual = focused_virtual.saturating_add(step);

        if let Some(target) =
            home_tile_virtual_to_real_index(target_virtual, step, self.tiles.len())
        {
            let bottom_edge_row = self
                .scroll_row_offset
                .saturating_add(self.visible_rows.max(1).saturating_sub(1));
            let is_bottom_edge = focused_row >= bottom_edge_row;
            self.focused_idx = target;
            if is_bottom_edge {
                self.scroll_row_offset = self
                    .scroll_row_offset
                    .saturating_add(1)
                    .min(self.max_scroll_row_offset());
            }
            self.ensure_focus_visible();
        }
    }
}

const HOME_DAILY_RECOMMEND_TILE_ID: &str = "__cnm_daily_recommend_songs__";
const HOME_PRIVATE_ROAM_TILE_ID: &str = "__cnm_private_roam__";
const HOME_PINNED_TITLES: [&str; 3] = ["每日推荐", "私人雷达", "私人漫游"];

fn home_tile_real_to_virtual_index(index: usize, columns: usize) -> usize {
    let cols = columns.max(1);
    if cols <= 3 || index < 3 {
        index
    } else {
        index.saturating_add(cols - 3)
    }
}

fn home_tile_virtual_to_real_index(
    virtual_index: usize,
    columns: usize,
    tile_len: usize,
) -> Option<usize> {
    let cols = columns.max(1);

    if cols <= 3 {
        return (virtual_index < tile_len).then_some(virtual_index);
    }

    if virtual_index < 3 {
        return (virtual_index < tile_len).then_some(virtual_index);
    }

    if virtual_index < cols {
        return None;
    }

    let real_index = virtual_index.saturating_sub(cols - 3);
    (real_index < tile_len).then_some(real_index)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HomeSidebarSection {
    Created,
    Collected,
}

#[derive(Debug, Clone)]
pub struct HomeSidebarPlaylist {
    pub id: Option<String>,
    pub title: String,
    pub creator: String,
    pub track_count: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HomeSidebarHit {
    pub section: HomeSidebarSection,
    pub index: usize,
}

/// 滚轮落在侧边栏时该滚哪个分区：光标所在分区优先（分区按整块算，列表短时
/// 下方的空白也命中），否则用当前聚焦分区；光标不在侧边栏面板内返回 `None`。
fn home_sidebar_wheel_target(
    panel: Option<HitRect>,
    sections: &[(HitRect, HomeSidebarSection)],
    focused: HomeSidebarSection,
    col: u16,
    row: u16,
) -> Option<HomeSidebarSection> {
    let panel = panel?;
    if !panel.contains(col, row) {
        return None;
    }

    Some(
        sections
            .iter()
            .find(|(rect, _)| rect.contains(col, row))
            .map(|(_, section)| *section)
            .unwrap_or(focused),
    )
}

pub struct HomeSidebarState {
    pub expanded: bool,
    pub loading: bool,
    pub user_id: Option<String>,
    pub liked_playlist_id: Option<String>,
    pub user_name: String,
    pub created_playlists: Vec<HomeSidebarPlaylist>,
    pub collected_playlists: Vec<HomeSidebarPlaylist>,
    pub focused_section: HomeSidebarSection,
    pub focused_index: usize,
    pub created_focused_index: usize,
    pub collected_focused_index: usize,
    pub created_scroll_offset: usize,
    pub collected_scroll_offset: usize,
    pub anim_progress: f32,
    /// 滑出/收起动画起始时刻（None = 无进行中动画）
    pub anim_started_at: Option<Instant>,
    /// 动画起始时的 progress 值（支持动画中途反向切换）
    pub anim_from: f32,
    pub status_line: String,
}

impl Default for HomeSidebarState {
    fn default() -> Self {
        Self {
            expanded: false,
            loading: false,
            user_id: None,
            liked_playlist_id: None,
            user_name: String::new(),
            created_playlists: Vec::new(),
            collected_playlists: Vec::new(),
            focused_section: HomeSidebarSection::Created,
            focused_index: 0,
            created_focused_index: 0,
            collected_focused_index: 0,
            created_scroll_offset: 0,
            collected_scroll_offset: 0,
            anim_progress: 0.0,
            anim_started_at: None,
            anim_from: 0.0,
            status_line: String::new(),
        }
    }
}

impl HomeSidebarState {
    fn section_memory(&self, section: HomeSidebarSection) -> usize {
        match section {
            HomeSidebarSection::Created => self.created_focused_index,
            HomeSidebarSection::Collected => self.collected_focused_index,
        }
    }

    fn set_section_memory(&mut self, section: HomeSidebarSection, index: usize) {
        match section {
            HomeSidebarSection::Created => {
                self.created_focused_index = index;
            }
            HomeSidebarSection::Collected => {
                self.collected_focused_index = index;
            }
        }
    }

    pub fn section_scroll_offset(&self, section: HomeSidebarSection) -> usize {
        match section {
            HomeSidebarSection::Created => self.created_scroll_offset,
            HomeSidebarSection::Collected => self.collected_scroll_offset,
        }
    }

    pub fn set_section_scroll_offset(&mut self, section: HomeSidebarSection, offset: usize) {
        match section {
            HomeSidebarSection::Created => {
                self.created_scroll_offset = offset;
            }
            HomeSidebarSection::Collected => {
                self.collected_scroll_offset = offset;
            }
        }
    }

    fn sync_memory_from_current(&mut self) {
        self.set_section_memory(self.focused_section, self.focused_index);
    }

    fn section_len(&self, section: HomeSidebarSection) -> usize {
        match section {
            HomeSidebarSection::Created => self.created_playlists.len(),
            HomeSidebarSection::Collected => self.collected_playlists.len(),
        }
    }

    pub fn clamp_focus(&mut self) {
        let created_len = self.created_playlists.len();
        let collected_len = self.collected_playlists.len();

        self.created_focused_index = if created_len == 0 {
            0
        } else {
            self.created_focused_index
                .min(created_len.saturating_sub(1))
        };
        self.collected_focused_index = if collected_len == 0 {
            0
        } else {
            self.collected_focused_index
                .min(collected_len.saturating_sub(1))
        };

        if created_len == 0 && collected_len == 0 {
            self.focused_section = HomeSidebarSection::Created;
            self.focused_index = 0;
            return;
        }

        match self.focused_section {
            HomeSidebarSection::Created if created_len == 0 => {
                self.focused_section = HomeSidebarSection::Collected;
            }
            HomeSidebarSection::Collected if collected_len == 0 => {
                self.focused_section = HomeSidebarSection::Created;
            }
            _ => {}
        }

        self.focused_index = self.section_memory(self.focused_section);

        let created_max_start = created_len.saturating_sub(1);
        let collected_max_start = collected_len.saturating_sub(1);
        self.created_scroll_offset = self.created_scroll_offset.min(created_max_start);
        self.collected_scroll_offset = self.collected_scroll_offset.min(collected_max_start);
    }

    pub fn reset_focus(&mut self) {
        self.created_focused_index = 0;
        self.collected_focused_index = 0;
        self.created_scroll_offset = 0;
        self.collected_scroll_offset = 0;
        self.focused_section = if !self.created_playlists.is_empty() {
            HomeSidebarSection::Created
        } else if !self.collected_playlists.is_empty() {
            HomeSidebarSection::Collected
        } else {
            HomeSidebarSection::Created
        };
        self.focused_index = 0;
        self.clamp_focus();
    }

    pub fn focus_next(&mut self) {
        let len = self.section_len(self.focused_section);
        if len == 0 {
            return;
        }
        self.focused_index = (self.focused_index + 1) % len;
        self.sync_memory_from_current();
    }

    pub fn focus_prev(&mut self) {
        let len = self.section_len(self.focused_section);
        if len == 0 {
            return;
        }
        self.focused_index = if self.focused_index == 0 {
            len - 1
        } else {
            self.focused_index - 1
        };
        self.sync_memory_from_current();
    }

    /// 滚轮滚动一格：在当前分区内移动焦点（到顶/到底即停，不回卷——键盘的
    /// `focus_next/prev` 会绕回，滚轮从列表末尾跳回开头会很突兀）。
    /// 侧边栏的视图跟随焦点，所以这就是它唯一的滚动方式。
    pub fn scroll_by(&mut self, forward: bool) {
        let len = self.section_len(self.focused_section);
        if len == 0 {
            return;
        }

        let next = if forward {
            self.focused_index.saturating_add(1).min(len - 1)
        } else {
            self.focused_index.saturating_sub(1)
        };
        self.focused_index = next;
        self.sync_memory_from_current();
    }

    /// 滚轮落到某个分区：先切到该分区（沿用它的位置记忆），再走一格，
    /// 这样"指着收藏区滚滚轮"不会把创建区的焦点带走。
    pub fn scroll_section_by(&mut self, section: HomeSidebarSection, forward: bool) {
        if section != self.focused_section {
            let index = self.section_memory(section);
            self.set_focus(section, index);
        }
        self.scroll_by(forward);
    }

    pub fn switch_section_prev(&mut self) {
        self.sync_memory_from_current();
        self.focused_section = match self.focused_section {
            HomeSidebarSection::Created => HomeSidebarSection::Collected,
            HomeSidebarSection::Collected => HomeSidebarSection::Created,
        };
        self.clamp_focus();
    }

    pub fn switch_section_next(&mut self) {
        self.sync_memory_from_current();
        self.focused_section = match self.focused_section {
            HomeSidebarSection::Created => HomeSidebarSection::Collected,
            HomeSidebarSection::Collected => HomeSidebarSection::Created,
        };
        self.clamp_focus();
    }

    pub fn set_focus(&mut self, section: HomeSidebarSection, index: usize) {
        self.sync_memory_from_current();
        self.focused_section = section;
        self.focused_index = index;
        self.sync_memory_from_current();
        self.clamp_focus();
    }

    pub fn focused_playlist(&self) -> Option<&HomeSidebarPlaylist> {
        match self.focused_section {
            HomeSidebarSection::Created => self.created_playlists.get(self.focused_index),
            HomeSidebarSection::Collected => self.collected_playlists.get(self.focused_index),
        }
    }

    pub fn is_visible(&self) -> bool {
        self.expanded || self.anim_progress > 0.0
    }
}

#[derive(Debug, Clone)]
pub struct PlaylistTrack {
    pub kind: PlaylistTrackKind,
    pub id: Option<String>,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub cover_url: Option<String>,
    pub duration_ms: i64,
    pub duration: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaylistTrackKind {
    Song,
    Album,
    Ep,
    Single,
}

/// 私人漫游状态：歌曲列表、最后播放位置与展示封面
#[derive(Debug, Clone, Default)]
pub struct PrivateRoamState {
    /// 漫游歌曲列表（权威副本，进入漫游时填充到 playlist）
    pub tracks: Vec<PlaylistTrack>,
    /// 最后播放歌曲在列表中的索引（每日刷新后若保留在首位则为 0）
    pub last_played_index: Option<usize>,
    /// 最后播放的漫游歌曲封面（切到别的列表播放后仍保留）
    pub last_played_cover_url: Option<String>,
    /// 当前应展示的封面：未播放过时为首歌封面，播放后为播放中的歌曲封面
    pub cover_url: Option<String>,
    /// 每日刷新标记：上次刷新的 UTC 天数
    pub last_refresh_day: Option<i64>,
}

/// 搜索结果条目的种类。带后缀的搜索只产出单一种类；无后缀搜索混合作者 / 歌单 / 单曲。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchItemKind {
    Song,
    Album,
    Artist,
    Playlist,
}

/// 作者卡片的行数：上边框 + 头像两行 + 下边框。
pub const ARTIST_CARD_ROWS: usize = 4;

impl SearchItemKind {
    /// 行右侧的类型标签（与 `SearchScope` 的后缀同源，避免两处字面量）。
    /// 单曲行的右侧位让给时长，故为 None。
    pub fn tag(self) -> Option<&'static str> {
        let scope = self.scope();
        (scope != SearchScope::Single).then(|| scope.suffix())
    }

    /// 条目占用的行数：作者卡片 4 行，其余 1 行。
    pub fn rows(self, card: bool) -> usize {
        if card && self == Self::Artist {
            ARTIST_CARD_ROWS
        } else {
            1
        }
    }

    fn scope(self) -> SearchScope {
        match self {
            Self::Song => SearchScope::Single,
            Self::Album => SearchScope::Album,
            Self::Artist => SearchScope::Author,
            Self::Playlist => SearchScope::Playlist,
        }
    }
}

pub struct SearchItem {
    pub kind: SearchItemKind,
    pub left_label: String,
    pub right_label: String,
    pub song_id: Option<String>,
    pub album_id: Option<String>,
    pub playlist_id: Option<String>,
    pub artist_id: Option<String>,
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub cover_url: Option<String>,
    pub duration_ms: Option<i64>,
    /// 作者条目的头像，复用封面管线。
    pub cover: CoverFetchState,
}

/// 搜索请求的作用域。`Mixed` 对应无后缀搜索（作者 + 歌单 + 单曲并发拉取后合并）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchScope {
    Mixed,
    Single,
    Album,
    Author,
    Playlist,
}

impl SearchScope {
    /// 带后缀的类型；`Mixed` 无后缀，不在此表。
    const SUFFIXED: [SearchScope; 4] = [Self::Single, Self::Album, Self::Author, Self::Playlist];

    fn suffix(self) -> &'static str {
        match self {
            Self::Mixed => "",
            Self::Single => "@single",
            Self::Album => "@album",
            Self::Author => "@author",
            Self::Playlist => "@list",
        }
    }

    /// NCM cloudsearch 的 `type`；`Mixed` 由 `execute_search` 拆成多请求，没有单一 id。
    fn search_type(self) -> Option<i32> {
        match self {
            Self::Mixed => None,
            Self::Single => Some(1),
            Self::Album => Some(10),
            Self::Author => Some(100),
            Self::Playlist => Some(1000),
        }
    }

    fn display_name(self) -> &'static str {
        match self {
            Self::Mixed => "综合",
            Self::Single => "单曲",
            Self::Album => "专辑",
            Self::Author => "作者",
            Self::Playlist => "歌单",
        }
    }
}

pub struct SearchState {
    pub query: String,
    pub focused_idx: usize,
    pub results: Vec<SearchItem>,
    pub status_line: String,
    pub scope: SearchScope,
    pub next_offset: usize,
    pub has_more: bool,
    /// 视口顶部距列表起点的**行数**。条目高度不一（作者卡片 4 行），按行滚动
    /// 才能让顶部与底部同步移动，而不是整条整条地跳。
    pub scroll_rows: usize,
    /// 视口高度（行），渲染侧每帧写入。
    view_rows: usize,
    /// 作者条目是否按卡片渲染（面板够宽够高），渲染侧每帧写入。
    card_mode: bool,
    /// 列表代：结果被替换 / 追加时换号（行内图标缓存据此失效）。
    generation: u64,
}

impl Default for SearchState {
    fn default() -> Self {
        Self {
            query: String::new(),
            focused_idx: 0,
            results: Vec::new(),
            status_line: "输入关键词后按 Enter 搜索".to_string(),
            scope: SearchScope::Mixed,
            next_offset: 0,
            has_more: false,
            scroll_rows: 0,
            view_rows: 1,
            card_mode: false,
            generation: next_list_generation(),
        }
    }
}

impl SearchState {
    /// 条目的行跨度：分区线（若有）+ 条目自身高度。
    fn row_span(&self, index: usize) -> usize {
        let kind = self.results[index].kind;
        let divider = usize::from(index > 0 && self.results[index - 1].kind != kind);
        divider + kind.rows(self.card_mode)
    }

    /// 条目（不含其分区线）首行在列表行空间中的位置。
    pub fn item_start_row(&self, index: usize) -> usize {
        (0..index.min(self.results.len()))
            .map(|i| self.row_span(i))
            .sum()
    }

    pub fn item_end_row(&self, index: usize) -> usize {
        self.item_start_row(index) + self.row_span(index)
    }

    /// 条目前的分区线占用的行数（0 或 1）。
    pub fn divider_rows(&self, index: usize) -> usize {
        usize::from(index > 0 && self.results[index - 1].kind != self.results[index].kind)
    }

    fn total_rows(&self) -> usize {
        (0..self.results.len()).map(|i| self.row_span(i)).sum()
    }

    fn max_scroll_rows(&self) -> usize {
        self.total_rows().saturating_sub(self.view_rows.max(1))
    }

    fn clamp_scroll(&mut self) {
        self.scroll_rows = self.scroll_rows.min(self.max_scroll_rows());
    }

    /// 聚焦条目必须完整可见：底边对齐就按**该条目推进的行数**下移视口，
    /// 于是顶部也退同样多的行（卡片被裁切而不是整块移出，底部不会跳变）。
    fn ensure_focus_visible(&mut self) {
        if self.results.is_empty() {
            self.focused_idx = 0;
            self.scroll_rows = 0;
            return;
        }

        self.focused_idx = self.focused_idx.min(self.results.len() - 1);
        let view = self.view_rows.max(1);
        let start = self.item_start_row(self.focused_idx);
        let end = self.item_end_row(self.focused_idx);
        if end > self.scroll_rows.saturating_add(view) {
            self.scroll_rows = end - view;
        }
        if start < self.scroll_rows {
            self.scroll_rows = start;
        }
        self.clamp_scroll();
    }

    /// 渲染侧每帧写入视口行数与卡片模式。
    pub fn set_viewport(&mut self, view_rows: usize, card_mode: bool) {
        self.view_rows = view_rows.max(1);
        self.card_mode = card_mode;
        self.ensure_focus_visible();
    }

    pub fn effective_scroll_rows(&self) -> usize {
        self.scroll_rows.min(self.max_scroll_rows())
    }

    /// 视口内可见的条目数（翻页步长）；卡片只算一条。
    pub fn page_items(&self) -> usize {
        let top = self.effective_scroll_rows();
        let bottom = top.saturating_add(self.view_rows.max(1));
        let mut row = 0usize;
        let mut count = 0usize;
        for index in 0..self.results.len() {
            let span = self.row_span(index);
            if row.saturating_add(span) > top && row < bottom {
                count += 1;
            }
            row = row.saturating_add(span);
        }
        count.max(1)
    }

    pub fn set_focus(&mut self, index: usize) {
        if self.results.is_empty() {
            self.focused_idx = 0;
            self.scroll_rows = 0;
            return;
        }

        self.focused_idx = index.min(self.results.len() - 1);
        self.ensure_focus_visible();
    }

    pub fn focus_next(&mut self) -> bool {
        if self.results.is_empty() || self.focused_idx + 1 >= self.results.len() {
            return false;
        }

        self.focused_idx += 1;
        self.ensure_focus_visible();
        true
    }

    pub fn focus_prev(&mut self) -> bool {
        if self.results.is_empty() || self.focused_idx == 0 {
            return false;
        }

        self.focused_idx -= 1;
        self.ensure_focus_visible();
        true
    }

    /// 整体替换结果。`next_offset` / `has_more` 只描述**可继续分页的分区**
    /// （混合搜索下即单曲分区，混合列表只有它在末尾追加）。
    pub fn set_results(&mut self, results: Vec<SearchItem>, next_offset: usize, has_more: bool) {
        self.results = results;
        self.focused_idx = 0;
        self.next_offset = next_offset;
        self.has_more = has_more;
        self.scroll_rows = 0;
        self.generation = next_list_generation();
        self.ensure_focus_visible();
    }

    /// 列表代（行内图标的行数据缓存据此失效）。
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// 追加分区分页结果。游标与 `has_more` 由调用方按分区语义推进。
    pub fn append_results(&mut self, mut results: Vec<SearchItem>) -> usize {
        let added = results.len();
        self.results.append(&mut results);
        self.generation = next_list_generation();
        self.clamp_scroll();
        added
    }
}

#[derive(Clone)]
pub struct PlaylistState {
    pub id: Option<String>,
    pub title: String,
    pub artist: String,
    pub description: String,
    pub cover: CoverFetchState,
    pub focused_idx: usize,
    pub scroll_offset: usize,
    pub visible_rows: usize,
    pub tracks: Vec<PlaylistTrack>,
    /// 列表代：内容被整体替换时换号（行内图标缓存据此决定是否重建行数据）。
    generation: u64,
}

impl Default for PlaylistState {
    fn default() -> Self {
        Self {
            id: None,
            title: "歌单详情".to_string(),
            artist: "网易云音乐".to_string(),
            description: "从主页进入歌单后加载真实数据。".to_string(),
            cover: CoverFetchState::default(),
            focused_idx: 0,
            scroll_offset: 0,
            visible_rows: 1,
            tracks: Vec::new(),
            generation: next_list_generation(),
        }
    }
}

impl PlaylistState {
    fn max_scroll_offset(&self) -> usize {
        self.tracks.len().saturating_sub(self.visible_rows.max(1))
    }

    fn clamp_scroll_offset(&mut self) {
        self.scroll_offset = self.scroll_offset.min(self.max_scroll_offset());
    }

    fn ensure_focus_visible(&mut self) {
        if self.tracks.is_empty() {
            self.focused_idx = 0;
            self.scroll_offset = 0;
            return;
        }

        self.focused_idx = self.focused_idx.min(self.tracks.len() - 1);
        if self.focused_idx < self.scroll_offset {
            self.scroll_offset = self.focused_idx;
        } else {
            let bottom = self
                .scroll_offset
                .saturating_add(self.visible_rows.max(1).saturating_sub(1));
            if self.focused_idx > bottom {
                self.scroll_offset = self
                    .focused_idx
                    .saturating_add(1)
                    .saturating_sub(self.visible_rows.max(1));
            }
        }

        self.clamp_scroll_offset();
    }

    pub fn set_visible_rows(&mut self, visible_rows: usize) {
        self.visible_rows = visible_rows.max(1);
        self.ensure_focus_visible();
    }

    pub fn effective_scroll_offset(&self) -> usize {
        self.scroll_offset.min(self.max_scroll_offset())
    }

    pub fn set_focus(&mut self, index: usize) {
        if self.tracks.is_empty() {
            self.focused_idx = 0;
            self.scroll_offset = 0;
            return;
        }

        self.focused_idx = index.min(self.tracks.len() - 1);
        self.ensure_focus_visible();
    }

    pub fn set_tracks(&mut self, tracks: Vec<PlaylistTrack>) {
        self.tracks = tracks;
        self.focused_idx = 0;
        self.scroll_offset = 0;
        self.generation = next_list_generation();
        self.ensure_focus_visible();
    }

    /// 列表代（行内图标的行数据缓存据此失效）。
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// 搜索页点歌单/专辑后先落地的占位状态：标题用结果行的名字，数据由
    /// `App::tick_playlist_fetch` 搬进来（数据没到之前 `App::playlist_fetch` 为 `Some`，
    /// 歌单页不响应翻页键）。
    pub fn placeholder(title: String, description: String) -> Self {
        Self {
            title,
            artist: String::new(),
            description,
            tracks: Vec::new(),
            ..Self::default()
        }
    }

    pub fn focus_next(&mut self) -> bool {
        if self.tracks.is_empty() || self.focused_idx + 1 >= self.tracks.len() {
            return false;
        }

        let bottom = self
            .scroll_offset
            .saturating_add(self.visible_rows.max(1).saturating_sub(1));
        let is_bottom_edge = self.focused_idx >= bottom;

        self.focused_idx += 1;
        if is_bottom_edge {
            self.scroll_offset = self
                .scroll_offset
                .saturating_add(1)
                .min(self.max_scroll_offset());
        }
        self.ensure_focus_visible();
        true
    }

    pub fn focus_prev(&mut self) -> bool {
        if self.tracks.is_empty() || self.focused_idx == 0 {
            return false;
        }

        let is_top_edge = self.focused_idx == self.scroll_offset;
        self.focused_idx -= 1;
        if is_top_edge && self.scroll_offset > 0 {
            self.scroll_offset -= 1;
        }
        self.ensure_focus_visible();
        true
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthorTileKind {
    HotSong,
    Album,
    Ep,
    Single,
}

#[derive(Clone)]
pub struct AuthorTile {
    pub kind: AuthorTileKind,
    pub title: String,
    pub subtitle: String,
    pub cover: CoverFetchState,
}

impl AuthorTile {
    fn placeholder() -> Self {
        Self {
            kind: AuthorTileKind::Album,
            title: "暂无内容".to_string(),
            subtitle: "No content".to_string(),
            cover: CoverFetchState::default(),
        }
    }

    fn from_album(
        api: &ApiState,
        title: String,
        subtitle: String,
        cover_url: Option<String>,
        kind: AuthorTileKind,
    ) -> Self {
        let mut cover = CoverFetchState::default();
        cover_url.map(|x| cover.load(api.clone(), x));
        Self {
            kind,
            title,
            subtitle,
            cover,
        }
    }
}

pub struct AuthorState {
    pub id: Option<String>,
    pub title: String,
    pub artist: String,
    pub description: String,
    pub cover: CoverFetchState,
    pub focused_idx: usize,
    pub columns: usize,
    pub scroll_row_offset: usize,
    pub visible_rows: usize,
    pub tiles: Vec<AuthorTile>,
    pub hot_songs: Vec<PlaylistTrack>,
    pub albums: Vec<PlaylistTrack>,
    pub eps: Vec<PlaylistTrack>,
    pub singles: Vec<PlaylistTrack>,
}

impl Default for AuthorState {
    fn default() -> Self {
        Self {
            id: None,
            title: "作者页".to_string(),
            artist: "网易云音乐".to_string(),
            description: "从搜索结果进入作者页后加载真实数据。".to_string(),
            cover: CoverFetchState::default(),
            focused_idx: 0,
            columns: 1,
            scroll_row_offset: 0,
            visible_rows: 1,
            tiles: vec![AuthorTile::placeholder()],
            hot_songs: Vec::new(),
            albums: Vec::new(),
            eps: Vec::new(),
            singles: Vec::new(),
        }
    }
}

impl AuthorState {
    /// 点作者名后先落地的占位状态：标题就是点中的那段名字，数据由 `App::tick_author_fetch`
    /// 搬进来（数据没到之前 `App::author_fetch` 为 `Some`，作者页不响应翻页键）。
    pub fn placeholder(title: String, description: String) -> Self {
        Self {
            title,
            artist: String::new(),
            description,
            tiles: Vec::new(),
            ..Self::default()
        }
    }

    fn total_rows(&self) -> usize {
        if self.tiles.is_empty() {
            0
        } else {
            (self.tiles.len() - 1) / self.columns.max(1) + 1
        }
    }

    fn max_scroll_row_offset(&self) -> usize {
        self.total_rows().saturating_sub(self.visible_rows.max(1))
    }

    fn clamp_scroll_row_offset(&mut self) {
        self.scroll_row_offset = self.scroll_row_offset.min(self.max_scroll_row_offset());
    }

    fn ensure_focus_visible(&mut self) {
        if self.tiles.is_empty() {
            self.focused_idx = 0;
            self.scroll_row_offset = 0;
            return;
        }

        self.focused_idx = self.focused_idx.min(self.tiles.len() - 1);
        let focused_row = self.focused_idx / self.columns.max(1);
        if focused_row < self.scroll_row_offset {
            self.scroll_row_offset = focused_row;
        } else {
            let bottom_row = self
                .scroll_row_offset
                .saturating_add(self.visible_rows.max(1).saturating_sub(1));
            if focused_row > bottom_row {
                self.scroll_row_offset = focused_row
                    .saturating_add(1)
                    .saturating_sub(self.visible_rows.max(1));
            }
        }

        self.clamp_scroll_row_offset();
    }

    pub fn set_tiles(&mut self, mut tiles: Vec<AuthorTile>) {
        if tiles.is_empty() {
            tiles.push(AuthorTile::placeholder());
        }
        self.tiles = tiles;
        self.focused_idx = 0;
        self.scroll_row_offset = 0;
        self.ensure_focus_visible();
    }

    pub fn set_columns(&mut self, columns: usize) {
        self.columns = columns.max(1);
        self.ensure_focus_visible();
    }

    pub fn set_visible_rows(&mut self, visible_rows: usize) {
        self.visible_rows = visible_rows.max(1);
        self.ensure_focus_visible();
    }

    pub fn effective_scroll_row_offset(&self) -> usize {
        self.scroll_row_offset.min(self.max_scroll_row_offset())
    }

    pub fn set_focus(&mut self, index: usize) {
        if self.tiles.is_empty() {
            self.focused_idx = 0;
            self.scroll_row_offset = 0;
            return;
        }

        self.focused_idx = index.min(self.tiles.len() - 1);
        self.ensure_focus_visible();
    }

    pub fn focus_next(&mut self) -> bool {
        if self.tiles.is_empty() {
            return false;
        }

        self.focused_idx = if self.focused_idx + 1 < self.tiles.len() {
            self.focused_idx + 1
        } else {
            0
        };
        self.ensure_focus_visible();
        true
    }

    pub fn focus_prev(&mut self) -> bool {
        if self.tiles.is_empty() {
            return false;
        }

        self.focused_idx = if self.focused_idx == 0 {
            self.tiles.len() - 1
        } else {
            self.focused_idx - 1
        };
        self.ensure_focus_visible();
        true
    }

    pub fn focus_left(&mut self) {
        self.focus_prev();
    }

    pub fn focus_right(&mut self) {
        self.focus_next();
    }

    pub fn focus_up(&mut self) -> bool {
        if self.tiles.is_empty() {
            return false;
        }

        let step = self.columns.max(1);
        if self.focused_idx < step {
            return false;
        }

        let focused_row = self.focused_idx / step;
        let is_top_edge = focused_row == self.scroll_row_offset;
        self.focused_idx -= step;
        if is_top_edge && self.scroll_row_offset > 0 {
            self.scroll_row_offset -= 1;
        }
        self.ensure_focus_visible();
        true
    }

    pub fn focus_down(&mut self) -> bool {
        if self.tiles.is_empty() {
            return false;
        }

        let step = self.columns.max(1);
        let target = self.focused_idx + step;
        if target >= self.tiles.len() {
            return false;
        }

        let focused_row = self.focused_idx / step;
        let bottom_edge_row = self
            .scroll_row_offset
            .saturating_add(self.visible_rows.max(1).saturating_sub(1));
        let is_bottom_edge = focused_row >= bottom_edge_row;
        self.focused_idx = target;
        if is_bottom_edge {
            self.scroll_row_offset = self
                .scroll_row_offset
                .saturating_add(1)
                .min(self.max_scroll_row_offset());
        }
        self.ensure_focus_visible();
        true
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaybackRepeatMode {
    Sequence,
    Shuffle,
    LoopAll,
    LoopOne,
}

impl PlaybackRepeatMode {
    pub fn next(self) -> Self {
        match self {
            Self::Sequence => Self::Shuffle,
            Self::Shuffle => Self::LoopAll,
            Self::LoopAll => Self::LoopOne,
            Self::LoopOne => Self::Sequence,
        }
    }

    /// 播放模式符号（Nerd Font PUA）。
    ///
    /// 用码位转义书写：直接粘贴字形会被复制/编辑流程吞掉，源码看不出异常，
    /// 运行时却变成空串——播放栏会因此错位，并留下"点不动"的按钮。
    pub fn symbol(self) -> &'static str {
        match self {
            Self::Sequence => "\u{f08f}",
            Self::Shuffle => "\u{f074}",
            Self::LoopAll => "\u{f0b6}",
            Self::LoopOne => "\u{f01e}",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaybackRuntimeState {
    Playing,
    Paused,
    Stopped,
}

#[derive(Debug, Clone)]
pub struct PlaybackTrack {
    pub song_id: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub duration_ms: i64,
    pub cover_url: Option<String>,
    pub cover: Option<Vec<u8>>,
    pub lyrics: Option<Vec<LyricLine>>,
}

impl PlaybackTrack {
    fn from_playlist_track(track: &PlaylistTrack) -> Option<Self> {
        if track.kind != PlaylistTrackKind::Song {
            return None;
        }

        let song_id = track.id.as_ref()?.trim().to_string();
        if song_id.is_empty() {
            return None;
        }

        Some(Self {
            song_id,
            title: track.title.clone(),
            artist: track.artist.clone(),
            album: track.album.clone(),
            duration_ms: track.duration_ms,
            cover_url: track.cover_url.clone(),
            cover: None,
            lyrics: None,
        })
    }

    fn from_search_item(item: &SearchItem) -> Option<Self> {
        let song_id = item.song_id.as_ref()?.trim().to_string();
        if song_id.is_empty() {
            return None;
        }

        Some(Self {
            song_id,
            title: item
                .title
                .clone()
                .unwrap_or_else(|| item.left_label.clone()),
            artist: item
                .artist
                .clone()
                .unwrap_or_else(|| "Unknown Artist".to_string()),
            album: item
                .album
                .clone()
                .unwrap_or_else(|| "Unknown Album".to_string()),
            duration_ms: item.duration_ms.unwrap_or_default(),
            cover_url: item.cover_url.clone(),
            cover: None,
            lyrics: None,
        })
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct HitRect {
    pub x: u16,
    pub y: u16,
    pub width: u16,
    pub height: u16,
}

impl HitRect {
    pub fn contains(self, col: u16, row: u16) -> bool {
        self.width > 0
            && self.height > 0
            && col >= self.x
            && col < self.x.saturating_add(self.width)
            && row >= self.y
            && row < self.y.saturating_add(self.height)
    }
}

impl From<ratatui::layout::Rect> for HitRect {
    fn from(rect: ratatui::layout::Rect) -> Self {
        Self {
            x: rect.x,
            y: rect.y,
            width: rect.width,
            height: rect.height,
        }
    }
}

/// 上一帧歌词浮窗的几何：内容区 + 浮窗本体。
///
/// 拖拽与"点击浮窗不穿透到下面的 tile"都以这份为准（每帧由
/// `page_lyrics::draw_page_lyrics_overlay` 重登记）。
#[derive(Debug, Clone, Copy)]
pub struct PageLyricsLayout {
    pub content: HitRect,
    pub panel: HitRect,
}

impl PageLyricsLayout {
    pub fn content_rect(self) -> Rect {
        Rect {
            x: self.content.x,
            y: self.content.y,
            width: self.content.width,
            height: self.content.height,
        }
    }

    pub fn panel_rect(self) -> Rect {
        Rect {
            x: self.panel.x,
            y: self.panel.y,
            width: self.panel.width,
            height: self.panel.height,
        }
    }
}

/// about 彩蛋的推进阶段。
#[cfg(feature = "easter-egg")]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EasterEggPhase {
    /// 未触发：about 弹窗是平常的样子。
    #[default]
    Idle,
    /// 蓄力：版本行背景的噪点由稀到密。
    Charging,
    /// 迸发：噪点带自下向上划过内容区并离开。
    Bursting,
    /// 已激活：内容区显示可点击的形象。
    Active,
}

/// about 彩蛋的全部运行时状态。
///
/// 动画帧是编译期烘焙好的常量（见 `render::mascot_frames`），这里不持有帧数据。
/// 退出 about 时整体 `Default::default()` 复位。
#[cfg(feature = "easter-egg")]
#[derive(Debug, Clone, Copy, Default)]
pub struct AboutEasterEgg {
    /// 版本行已被点击的次数，累计到 `TRIGGER_CLICKS` 触发。
    pub version_clicks: u8,
    pub phase: EasterEggPhase,
    /// 当前阶段的起始时刻（蓄力、迸发各自计时）。
    pub phase_started_at: Option<Instant>,
    /// 进入激活态的时刻，用于驱动边框噪点的环形流动。
    pub mascot_activated_at: Option<Instant>,
    /// 最近一次点击形象的时刻；`None` 表示形象静止。
    pub jelly_started_at: Option<Instant>,
    /// 版本行与形象的命中区域，绘制时写回、鼠标事件时查询。
    pub version_hit: Option<HitRect>,
    pub mascot_hit: Option<HitRect>,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct PlayerBarHitTargets {
    pub prev: Option<HitRect>,
    pub play_pause: Option<HitRect>,
    pub next: Option<HitRect>,
    pub progress: Option<HitRect>,
    /// 收藏爱心（左列右端）。
    pub like: Option<HitRect>,
    /// 下载按钮（爱心左侧隔一格；下载不可用时不登记）。
    pub download: Option<HitRect>,
    /// 播放模式符号（控制串最后一个字符）。
    pub mode: Option<HitRect>,
}

#[derive(Debug, Clone)]
pub struct FullscreenPlaybackSnapshot {
    pub queue: Vec<PlaybackTrack>,
    pub current_index: Option<usize>,
    pub now_playing: Option<PlaybackTrack>,
    pub now_playing_liked: bool,
    pub state: PlaybackRuntimeState,
    pub repeat_mode: PlaybackRepeatMode,
    pub position: Duration,
}

#[derive(Debug, Clone, Copy)]
pub struct FullscreenRuntimeSnapshot {
    pub current_index: Option<usize>,
    pub now_playing_liked: bool,
    pub state: PlaybackRuntimeState,
    pub repeat_mode: PlaybackRepeatMode,
    pub position: Duration,
    pub volume: f32,
    pub seeking: bool,
    /// 当前播放歌曲的下载图标状态；`None` = 下载不可用或没有播放中的歌曲。
    pub download: Option<DownloadState>,
}

#[derive(Debug, Clone)]
struct CoverFetchRequest {
    song_id: String,
    url: String,
}

#[derive(Debug, Clone)]
struct CoverFetchResult {
    song_id: String,
    url: String,
    bytes: Option<Vec<u8>>,
}

#[derive(Debug, Clone)]
struct LyricFetchRequest {
    song_id: String,
    cookie: Option<String>,
}

#[derive(Debug, Clone)]
struct LyricFetchResult {
    song_id: String,
    lyrics: Option<Vec<LyricLine>>,
}

async fn loop_cover_fetch(
    mut rx: UnboundedReceiver<CoverFetchRequest>,
    tx: Sender<CoverFetchResult>,
    client: Client,
) {
    let process_fn = async move |req: &CoverFetchRequest| {
        if req.url.is_empty() {
            return None;
        }
        let resp = client.get(req.url.as_str()).ok()?.send().await.ok()?;
        let resp = error_for_status(resp).ok()?;
        let bytes = resp.bytes().await.ok()?;
        (!bytes.is_empty()).then(|| bytes.to_vec())
    };
    while let Ok(req) = rx.recv().await {
        let bytes = process_fn(&req).await;
        let _ = tx.send(CoverFetchResult {
            song_id: req.song_id,
            url: req.url,
            bytes: bytes,
        });
    }
}

async fn loop_lyric_fetch(
    mut rx: UnboundedReceiver<LyricFetchRequest>,
    tx: Sender<LyricFetchResult>,
    mut api: ApiState,
) {
    let mut process_fn = async move |req: &LyricFetchRequest| {
        if let Some(cookie) = &req.cookie {
            api.set_cookie(cookie.to_string());
        }

        let lyric = api.lyric(&req.song_id).await.ok()?;
        let lrc = lyric.body.pointer("/lrc/lyric")?.as_str()?;
        parse_lrc(lrc).or_else(|| parse_plain_lyrics(lrc))
    };
    while let Ok(req) = rx.recv().await {
        let lyrics = process_fn(&req).await;
        let _ = tx.send(LyricFetchResult {
            song_id: req.song_id,
            lyrics,
        });
    }
}

pub struct App {
    pub config: Config,
    pub theme: Theme,
    pub page: Page,
    pub overlay: Option<Overlay>,
    pub login: LoginState,
    pub home: HomeState,
    pub home_sidebar: HomeSidebarState,
    /// 侧边栏歌单的在途拉取（异步填充，不阻塞展开动画）。
    home_sidebar_fetch: Option<HomeSidebarFetchFuture>,
    /// 作者页的在途拉取（全屏页点作者名：页面先落地，数据由 `tick_author_fetch` 搬进来）。
    author_fetch: Option<AuthorFetchFuture>,
    /// 歌单页 / 专辑页的在途拉取（搜索页打开：页面先落地，数据由 `tick_playlist_fetch` 搬进来）。
    playlist_fetch: Option<PlaylistFetchSlot>,
    /// 上次检查 stderr 日志体积的时刻。
    stderr_trim_checked_at: Option<Instant>,
    home_sidebar_anim_span_cells: u16,
    pub playlist: PlaylistState,
    pub private_roam: PrivateRoamState,
    pub author: AuthorState,
    pub search: SearchState,
    pub now_playing: Option<PlaybackTrack>,
    pub now_playing_liked: bool,
    /// 收藏的期望/已确认双轨状态机（乐观更新 + 每帧收敛）。
    like_machine: LikeMachine,
    pub playback_queue: Vec<PlaybackTrack>,
    /// 当前播放队列来源列表（专辑/歌单）的封面 URL。
    /// 与 `self.playlist` 解耦：后者是"最后访问的页面"，会随浏览漂移。
    playback_queue_cover_url: Option<String>,
    /// 当前播放队列来源列表的 id（如私人漫游的 tile id）。
    ///
    /// 与 `self.playlist.id` 的区别：后者是「当前浏览的页面」，会随浏览漂移，
    /// 且重启后为 None。来源相关行为（漫游的尾部追加、续播、封面跟随）
    /// 一律以此字段为判据，并随播放记忆一起持久化。
    playback_queue_source_id: Option<String>,
    pub playback_index: Option<usize>,
    pub playback_repeat_mode: PlaybackRepeatMode,
    pub playback_state: PlaybackRuntimeState,
    pub startup_loading_progress: f32,
    /// 启动初始化任务与加载页进度（真实步数）。
    pub startup: StartupInit,
    /// 加载页收尾后进入的页面（登录态可用为 Home，否则 Login）。
    startup_loading_target: Page,
    pub player_bar_hits: PlayerBarHitTargets,
    /// 最近一次同步到的终端尺寸（单元格）。
    pub term_width: u16,
    pub term_height: u16,
    /// 当前生效的小窗口模式；None 表示普通内容页或未启用小窗口显示。
    pub small_window_mode: Option<SmallWindowMode>,
    /// 扁窗高度恰为播放栏高度时，Alt+X 切换的当前目标面板。
    pub flat_panel: FlatPanel,
    /// 扁窗切换动画；None 表示已停在目标面板。
    pub flat_switch_anim: Option<FlatSwitchAnim>,
    /// 上一帧是否处于“扁窗高度恰为播放栏高度”的子状态。
    flat_exact_height: bool,
    /// 窄窗音量条的显示端平滑值（LUFS）。以 VU_FLOOR 表示静音。
    pub vu_left_lufs: f32,
    pub vu_right_lufs: f32,
    pub vu_animating: bool,
    vu_last_tick_at: Option<Instant>,
    vu_last_meter_generation: u64,
    pub home_sidebar_panel_hit: Option<HitRect>,
    /// 上一帧歌词浮窗的几何（拖拽 / 点击拦截共用）。
    page_lyrics_layout: Option<PageLyricsLayout>,
    /// 正在拖动歌词浮窗时，光标相对浮窗左上角的偏移。
    page_lyrics_grab: Option<(u16, u16)>,
    pub home_sidebar_playlist_hits: Vec<(HitRect, HomeSidebarHit)>,
    /// 侧边栏两个分区的矩形（滚轮据此判断光标落在哪个分区）。
    pub home_sidebar_section_hits: Vec<(HitRect, HomeSidebarSection)>,
    pub home_tile_hits: Vec<(HitRect, usize)>,
    pub playlist_track_hits: Vec<(HitRect, usize)>,
    /// 歌单页 / 专辑页行内下载图标的命中区（先于整行命中判定）。
    pub playlist_track_download_hits: Vec<(HitRect, usize)>,
    pub author_tile_hits: Vec<(HitRect, usize)>,
    pub search_item_hits: Vec<(HitRect, usize)>,
    /// 搜索页单曲行内下载图标的命中区。
    pub search_item_download_hits: Vec<(HitRect, usize)>,
    pub search_box_input: String,
    pub search_box_cursor: usize,
    pub search_box_anim_height: u16,
    /// 搜索框滑出动画的起始时刻（time-based 动画基准）
    pub search_box_anim_started_at: Option<Instant>,
    pub settings_selected: usize,
    pub settings_playback_selected: usize,
    pub settings_lyrics_selected: usize,
    pub settings_keybind_selected: usize,
    pub settings_keybind_rebinding: Option<usize>,
    /// 「下载设置」页的选中行。
    pub settings_download_selected: usize,
    /// 下载路径行的编辑状态（Some = 正在编辑该行）。
    pub download_path_edit: Option<DownloadPathEdit>,
    /// 「恢复默认」的两段式确认：首次选择后进入待确认态（文字换成警戒色）。
    pub download_reset_armed: bool,
    /// 设置弹窗当前页的行命中区（每帧由 `draw_settings_modal` 重注册）。
    pub settings_item_hits: Vec<(HitRect, usize)>,
    /// 设置弹窗内上一次点击（用于双击判定）。
    last_settings_click: Option<(Instant, Overlay, usize)>,
    /// about 弹窗里的形象彩蛋状态。
    #[cfg(feature = "easter-egg")]
    pub about_egg: AboutEasterEgg,
    pub session_cookie: Option<String>,
    pub should_quit: bool,
    pub launch_fullscreen_requested: bool,
    pub vip_audio_unlocked: bool,
    search_return_page: Page,
    playlist_return_page: Page,
    /// 作者页的上一级：从搜索页进是搜索页，从全屏页点作者名进是首页。
    author_return_page: Page,
    playlist_section_return_snapshot: Option<PlaylistState>,
    qr_last_poll_at: Option<Instant>,
    startup_loading_started_at: Option<Instant>,
    startup_loading_complete_started_at: Option<Instant>,
    startup_loading_complete_requested: bool,
    last_global_hotkey_at: Option<Instant>,
    last_content_click: Option<(Instant, Page, usize)>,
    pub cava: Option<MiniCavaState>,
    cover_cache_dir: PathBuf,
    cover_fetch_tx: UnboundedSender<CoverFetchRequest>,
    cover_fetch_rx: Receiver<CoverFetchResult>,
    cover_fetch_inflight_url: Option<String>,
    cover_fetch_last_attempt_at: Option<Instant>,
    lyric_fetch_tx: UnboundedSender<LyricFetchRequest>,
    lyric_fetch_rx: Receiver<LyricFetchResult>,
    lyric_fetch_inflight_song_id: Option<String>,
    lyric_fetch_last_attempt_at: Option<Instant>,
    mpris_bridge: MprisBridge,
    mpris_last_sync_at: Instant,
    mpris_last_signature: Option<u64>,
    mpris_last_playback: PlaybackRuntimeState,
    api: ApiState,
    audio_player: AudioPlayer,
    /// 下载任务表（异步后台任务；状态行与图标都从这里读）。
    pub download_manager: DownloadManager,
    /// 行内图标的 memo（歌单页 / 专辑页）：行数据随列表代重建，状态随任务版本重算。
    playlist_download_cache: DownloadRowCache,
    /// 行内图标的 memo（搜索页）。
    search_download_cache: DownloadRowCache,
    /// 下载根目录代：路径变化时 +1，行数据缓存据此失效。
    download_rows_epoch: u64,
    /// 下载图标的动画相位基准（time-based 旋转帧的起点）。
    download_spinner_start: Instant,
    /// 当前播放歌曲的下载图标状态缓存（全屏页每帧读，写入在 tick 里）。
    now_playing_download_state: DownloadState,
    /// 解析后的下载根目录；`None` = 系统里既没有音乐目录也没有家目录 → 下载禁用。
    pub download_root: Option<PathBuf>,
    /// 当前歌单页实际是歌单还是专辑（专辑页下载要落子文件夹 + 写 `cover.*`）。
    playlist_page_kind: PlaylistPageKind,
    pub graphics_picker: Picker,
}

impl App {
    pub fn draw_ascii(&self) -> bool {
        self.config.graphics_protocol == GraphicsProtocol::Off
    }

    /// 构造 App 并启动后台初始化：本地设置同步完成，网络初始化交给
    /// [`StartupInit`]，加载页随即可以显示真实进度。
    pub fn new(config: Config, theme: Theme) -> Result<Self> {
        let audio_player = AudioPlayer::new(&config)?;
        let saved_cookie = session::load_cookie().ok().flatten();

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
        let cache_root = resolve_cache_root(&config);
        let cover_cache_dir = cache_root.join(COVER_CACHE_SUBDIR);
        let download_root =
            crate::app::download::resolve_download_root(config.download_path.as_deref());
        let mpris_bridge = MprisBridge::new(&cache_root, &config.cache);
        if config.cache.clean_on_startup {
            let _ = cleanup_cache_dir(&cover_cache_dir, &config.cache);
        }
        let _ = fs::create_dir_all(&cover_cache_dir);

        let (cover_fetch_tx, cover_fetch_req_rx) = unbounded();
        let (cover_fetch_res_tx, cover_fetch_rx) = mpsc::channel::<CoverFetchResult>();
        let worker = loop_cover_fetch(cover_fetch_req_rx, cover_fetch_res_tx, http_client.clone());
        launch(worker);

        let api = ApiState::new(saved_cookie.clone(), http_client.clone())?;
        // 下载任务全局只有一个：管理器起一次常驻 worker，之后只往队列里塞请求。
        let download_manager = DownloadManager::new(api.clone());

        let (lyric_fetch_tx, lyric_fetch_req_rx) = unbounded();
        let (lyric_fetch_res_tx, lyric_fetch_rx) = mpsc::channel::<LyricFetchResult>();
        let worker = loop_lyric_fetch(lyric_fetch_req_rx, lyric_fetch_res_tx, api.clone());
        launch(worker);

        let mut app = Self {
            config,
            theme,
            page: Page::Login,
            overlay: None,
            login: LoginState::default(),
            home: HomeState::default(),
            home_sidebar: HomeSidebarState::default(),
            home_sidebar_fetch: None,
            author_fetch: None,
            playlist_fetch: None,
            stderr_trim_checked_at: None,
            home_sidebar_anim_span_cells: 24,
            playlist: PlaylistState::default(),
            private_roam: PrivateRoamState::default(),
            author: AuthorState::default(),
            search: SearchState::default(),
            now_playing: None,
            now_playing_liked: false,
            like_machine: LikeMachine::default(),
            playback_queue: Vec::new(),
            playback_queue_cover_url: None,
            playback_queue_source_id: None,
            playback_index: None,
            playback_repeat_mode: PlaybackRepeatMode::Sequence,
            playback_state: PlaybackRuntimeState::Stopped,
            startup_loading_progress: 0.0,
            startup: StartupInit::detached(),
            startup_loading_target: Page::Login,
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
            search_box_input: String::new(),
            search_box_cursor: 0,
            search_box_anim_height: 0,
            search_box_anim_started_at: None,
            settings_selected: 0,
            settings_playback_selected: 0,
            settings_lyrics_selected: 0,
            settings_keybind_selected: 0,
            settings_keybind_rebinding: None,
            settings_download_selected: 0,
            download_path_edit: None,
            download_reset_armed: false,
            settings_item_hits: Vec::new(),
            last_settings_click: None,
            #[cfg(feature = "easter-egg")]
            about_egg: AboutEasterEgg::default(),
            session_cookie: None,
            should_quit: false,
            launch_fullscreen_requested: false,
            vip_audio_unlocked: false,
            search_return_page: Page::Home,
            playlist_return_page: Page::Home,
            author_return_page: Page::Home,
            playlist_section_return_snapshot: None,
            qr_last_poll_at: None,
            startup_loading_started_at: None,
            startup_loading_complete_started_at: None,
            startup_loading_complete_requested: false,
            last_global_hotkey_at: None,
            last_content_click: None,
            cava: None,
            cover_cache_dir,
            cover_fetch_tx,
            cover_fetch_rx,
            cover_fetch_inflight_url: None,
            cover_fetch_last_attempt_at: None,
            lyric_fetch_tx,
            lyric_fetch_rx,
            lyric_fetch_inflight_song_id: None,
            lyric_fetch_last_attempt_at: None,
            mpris_bridge,
            mpris_last_sync_at: Instant::now(),
            mpris_last_signature: None,
            mpris_last_playback: PlaybackRuntimeState::Stopped,
            api,
            audio_player,
            download_manager,
            playlist_download_cache: DownloadRowCache::default(),
            search_download_cache: DownloadRowCache::default(),
            download_rows_epoch: 0,
            download_spinner_start: Instant::now(),
            now_playing_download_state: DownloadState::NotDownloaded,
            download_root,
            playlist_page_kind: PlaylistPageKind::Playlist,
            graphics_picker: Picker::halfblocks(),
        };

        app.load_private_roam_memory();

        if let Some(protocol) = app.config.graphics_protocol.to_ratatui_protocol() {
            app.graphics_picker.set_protocol_type(protocol);
        }

        app.sync_cava();
        app.sync_terminal_size();

        // 先出加载页，网络初始化交给后台任务：登录恢复这几步在旧实现里是
        // 进备用屏幕之前同步 await 的，终端因此有一段时间毫无反馈。
        let skip_roam = app.private_roam_refreshed_today();
        let (steps, target) = startup::initial_plan(saved_cookie.is_some(), skip_roam);
        app.startup = StartupInit::spawn(
            app.config.clone(),
            app.api.clone(),
            saved_cookie,
            skip_roam,
            steps,
        );
        app.begin_startup_loading(target);
        Ok(app)
    }

    pub async fn tick(&mut self) {
        self.tick_audio().await;
        self.tick_cover_fetch();
        self.tick_lyric_fetch();
        self.apply_mpris_control_events().await;
        self.sync_mpris_exposure();
        let now = Instant::now();
        self.tick_flat_switch();
        self.tick_vu_meter(now);
        self.tick_search_box_animation();
        self.tick_home_sidebar_animation();
        self.tick_home_sidebar_fetch();
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
            if let Some(last) = self.qr_last_poll_at {
                if now.duration_since(last) < Duration::from_millis(1400) {
                    return;
                }
            }

            self.qr_last_poll_at = Some(now);
            self.check_qr_status_and_login().await;
        }
    }

    pub async fn handle_key(&mut self, key: KeyEvent) {
        if key.kind != KeyEventKind::Press {
            return;
        }

        if key.modifiers.contains(KeyModifiers::CONTROL)
            && matches!(key.code, KeyCode::Char('c') | KeyCode::Char('C'))
        {
            self.should_quit = true;
            return;
        }

        if self.is_small_window_context() {
            self.handle_small_window_key(key).await;
            return;
        }

        if self.page != Page::Login
            && key.modifiers.contains(KeyModifiers::CONTROL)
            && matches!(key.code, KeyCode::Char('k') | KeyCode::Char('K'))
            && !matches!(self.overlay, Some(Overlay::SettingsKeybinds))
        {
            self.open_keybind_settings();
            return;
        }

        if let Some(overlay) = self.overlay {
            self.handle_overlay_key(overlay, key).await;
            return;
        }

        if self.page == Page::Loading {
            // 加载页不响应页面快捷键，但退出键必须留着：初始化万一被网络
            // 拖住，用户不能只剩 Ctrl+C。
            if matches!(
                self.keybind_action_from_event(key),
                Some(KeybindAction::Quit)
            ) {
                self.should_quit = true;
            }
            return;
        }

        if self.page != Page::Login && self.try_handle_configured_hotkey(key).await {
            return;
        }

        match self.page {
            Page::Login => self.handle_login_key(key).await,
            Page::Loading => {}
            Page::Home => self.handle_home_key(key).await,
            Page::Playlist => self.handle_playlist_key(key).await,
            Page::Author => self.handle_author_key(key).await,
            Page::Search => self.handle_search_key(key).await,
        }
    }

    async fn handle_small_window_key(&mut self, key: KeyEvent) {
        if keybind_matches(self.config.keybind_small_window_toggle.as_str(), key) {
            if self.small_window_mode == Some(SmallWindowMode::Flat) {
                self.toggle_flat_panel();
            }
            return;
        }

        let Some(action) = self.keybind_action_from_event(key) else {
            return;
        };
        match action {
            KeybindAction::Quit => {
                self.should_quit = true;
            }
            KeybindAction::Prev
            | KeybindAction::Next
            | KeybindAction::TogglePlayPause
            | KeybindAction::ToggleMode
            | KeybindAction::ToggleLikeCollapsed => {
                if self.can_execute_global_hotkey() {
                    self.trigger_keybind_action(action).await;
                }
            }
            _ => {}
        }
    }

    pub async fn handle_mouse(&mut self, mouse: MouseEvent) {
        // 松开左键总是收尾拖拽：即使中途切到小窗口/弹窗也不会卡住拖动状态
        // （卡住会让 should_continuous_redraw 一直按高帧率重绘）。
        if matches!(mouse.kind, MouseEventKind::Up(MouseButton::Left)) {
            self.page_lyrics_release();
            return;
        }

        if self.page == Page::Login || self.page == Page::Loading {
            return;
        }

        if self.is_small_window_context() {
            if matches!(mouse.kind, MouseEventKind::Down(_)) {
                self.dispatch_player_bar_click(mouse.column, mouse.row)
                    .await;
            }
            return;
        }

        let col = mouse.column;
        let row = mouse.row;

        match mouse.kind {
            MouseEventKind::ScrollUp => {
                if self.overlay.is_some() {
                    self.scroll_settings_modal(false);
                    return;
                }
                self.handle_content_scroll(col, row, false).await;
            }
            MouseEventKind::ScrollDown => {
                if self.overlay.is_some() {
                    self.scroll_settings_modal(true);
                    return;
                }
                self.handle_content_scroll(col, row, true).await;
            }
            MouseEventKind::Down(MouseButton::Left) => {
                if matches!(self.overlay, Some(Overlay::SearchBox)) {
                    self.handle_search_box_click(col, row);
                    return;
                }

                #[cfg(feature = "easter-egg")]
                if matches!(self.overlay, Some(Overlay::SettingsAbout)) {
                    self.handle_settings_about_click(col, row);
                    return;
                }

                if let Some(overlay) = self.overlay {
                    // 设置弹窗：命中行则聚焦/执行，其余位置一律吞掉。
                    self.handle_settings_modal_click(overlay, col, row).await;
                    return;
                }

                // 歌词浮窗盖在内容之上：命中即吞掉，顺带作为拖动把手。
                if self.page_lyrics_press(col, row) {
                    return;
                }

                if self.handle_content_click(col, row).await {
                    return;
                }

                self.dispatch_player_bar_click(col, row).await;
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                self.page_lyrics_drag(col, row);
            }
            _ => {}
        }
    }

    /// 折叠播放栏按钮的统一分派（小窗口与常规两条分支共用）。
    ///
    /// 命中区每帧由 `draw_collapsed_player_bar` 重注册，这里只查表；
    /// 新增按钮必须同时登记进 `PlayerBarHitTargets` 与 `player_bar_contains`。
    async fn dispatch_player_bar_click(&mut self, col: u16, row: u16) {
        let hits = self.player_bar_hits;

        if hits.download.is_some_and(|rect| rect.contains(col, row)) {
            // 与全屏页 Ctrl+D / 行内图标同一条路径：在途则取消。
            self.download_current_song();
            return;
        }
        if hits.like.is_some_and(|rect| rect.contains(col, row)) {
            self.toggle_like_hotkey();
            return;
        }
        if hits.mode.is_some_and(|rect| rect.contains(col, row)) {
            self.cycle_repeat_mode_hotkey();
            return;
        }
        if hits.prev.is_some_and(|rect| rect.contains(col, row)) {
            self.play_previous_hotkey().await;
            return;
        }
        if hits.play_pause.is_some_and(|rect| rect.contains(col, row)) {
            self.toggle_play_pause_hotkey().await;
            return;
        }
        if hits.next.is_some_and(|rect| rect.contains(col, row)) {
            self.play_next_hotkey().await;
            return;
        }
        if let Some(rect) = hits.progress {
            if rect.contains(col, row) {
                let relative_x = col.saturating_sub(rect.x) as f32;
                let ratio = if rect.width <= 1 {
                    0.0
                } else {
                    (relative_x / (rect.width - 1) as f32).clamp(0.0, 1.0)
                };
                self.seek_to_ratio(ratio);
            }
        }
    }

    fn player_bar_contains(&self, col: u16, row: u16) -> bool {
        let hits = self.player_bar_hits;
        [
            hits.prev,
            hits.play_pause,
            hits.next,
            hits.progress,
            hits.like,
            hits.download,
            hits.mode,
        ]
        .into_iter()
        .flatten()
        .any(|rect| rect.contains(col, row))
    }

    async fn handle_content_scroll(&mut self, col: u16, row: u16, forward: bool) {
        if self.overlay.is_some() || self.player_bar_contains(col, row) {
            return;
        }

        match self.page {
            Page::Search => {
                if forward {
                    self.advance_search_focus().await;
                } else {
                    let _ = self.search.focus_prev();
                }
            }
            Page::Playlist => {
                if forward {
                    let _ = self.playlist.focus_next();
                } else {
                    let _ = self.playlist.focus_prev();
                }
            }
            Page::Home => self.scroll_home_sidebar(col, row, forward),
            _ => {}
        };
    }

    /// 主页侧边栏的滚轮滚动：光标指到哪个分区就滚哪个（没有则滚当前聚焦分区），
    /// 一格一步、到端点即停。侧边栏收起或光标在面板外时不动。
    fn scroll_home_sidebar(&mut self, col: u16, row: u16, forward: bool) {
        if !self.home_sidebar.expanded {
            return;
        }

        let Some(section) = home_sidebar_wheel_target(
            self.home_sidebar_panel_hit,
            &self.home_sidebar_section_hits,
            self.home_sidebar.focused_section,
            col,
            row,
        ) else {
            return;
        };

        self.home_sidebar.scroll_section_by(section, forward);
    }

    async fn advance_search_focus(&mut self) {
        if self.search.results.is_empty() {
            return;
        }

        if self.search.focus_next() {
            if self.search.focused_idx + 1 == self.search.results.len() {
                match self.load_more_search_results().await {
                    Ok(_) => {}
                    Err(err) => {
                        self.search.status_line = format!("加载更多失败: {}", err);
                    }
                }
            }
            return;
        }

        let before = self.search.results.len();
        match self.load_more_search_results().await {
            Ok(added) if added > 0 => {
                self.search.set_focus(before);
            }
            Ok(_) => {}
            Err(err) => {
                self.search.status_line = format!("加载更多失败: {}", err);
            }
        }
    }

    pub fn clear_player_bar_hits(&mut self) {
        self.player_bar_hits = PlayerBarHitTargets::default();
    }

    pub fn set_player_bar_hits(&mut self, hits: PlayerBarHitTargets) {
        self.player_bar_hits = hits;
    }

    pub fn clear_content_hits(&mut self) {
        self.home_sidebar_panel_hit = None;
        self.home_sidebar_playlist_hits.clear();
        self.home_sidebar_section_hits.clear();
        self.home_tile_hits.clear();
        self.playlist_track_hits.clear();
        self.playlist_track_download_hits.clear();
        self.author_tile_hits.clear();
        self.search_item_hits.clear();
        self.search_item_download_hits.clear();
        self.page_lyrics_layout = None;
    }

    /// 歌词浮窗位置（归一化，内容区内左上角）。
    pub fn page_lyrics_pos(&self) -> (f32, f32) {
        (self.config.page_lyrics_pos_x, self.config.page_lyrics_pos_y)
    }

    /// 登记本帧歌词浮窗几何（由 `page_lyrics::draw_page_lyrics_overlay` 调用）。
    pub fn set_page_lyrics_layout(&mut self, content: Rect, panel: Rect) {
        self.page_lyrics_layout = Some(PageLyricsLayout {
            content: content.into(),
            panel: panel.into(),
        });
    }

    /// 鼠标按下：落在歌词浮窗上就吞掉这次点击（不穿透到下面的 tile），
    /// 并按配置决定是否开始拖动。返回是否已消费。
    fn page_lyrics_press(&mut self, col: u16, row: u16) -> bool {
        let Some(layout) = self.page_lyrics_layout else {
            return false;
        };
        if !layout.panel.contains(col, row) {
            return false;
        }

        self.page_lyrics_grab = self.config.page_lyrics_drag.then(|| {
            (
                col.saturating_sub(layout.panel.x),
                row.saturating_sub(layout.panel.y),
            )
        });
        true
    }

    /// 拖动中：把浮窗移到光标处（保持抓取偏移，钳在内容区内）。
    fn page_lyrics_drag(&mut self, col: u16, row: u16) {
        let (Some(layout), Some(grab)) = (self.page_lyrics_layout, self.page_lyrics_grab) else {
            return;
        };

        let (pos_x, pos_y) =
            page_lyrics::pos_after_drag(layout.content_rect(), layout.panel_rect(), col, row, grab);
        self.config.page_lyrics_pos_x = pos_x;
        self.config.page_lyrics_pos_y = pos_y;
    }

    /// 松开：按配置吸附到最近的角，并把位置写回配置。
    fn page_lyrics_release(&mut self) {
        if self.page_lyrics_grab.take().is_none() {
            return;
        }

        if self.config.page_lyrics_snap {
            if let Some(layout) = self.page_lyrics_layout {
                let (pos_x, pos_y) = page_lyrics::snap_pos(
                    layout.content_rect(),
                    layout.panel_rect(),
                    self.page_lyrics_pos(),
                );
                self.config.page_lyrics_pos_x = pos_x;
                self.config.page_lyrics_pos_y = pos_y;
            }
        }

        let _ = self.config.save();
    }

    pub fn clear_settings_item_hits(&mut self) {
        self.settings_item_hits.clear();
    }

    pub fn push_settings_item_hit(&mut self, rect: HitRect, index: usize) {
        self.settings_item_hits.push((rect, index));
    }

    pub fn set_home_sidebar_panel_hit(&mut self, rect: Option<HitRect>) {
        self.home_sidebar_panel_hit = rect;
    }

    pub fn set_home_sidebar_anim_span_cells(&mut self, span_cells: u16) {
        self.home_sidebar_anim_span_cells = span_cells.max(1);
    }

    pub fn push_home_sidebar_playlist_hit(&mut self, rect: HitRect, hit: HomeSidebarHit) {
        self.home_sidebar_playlist_hits.push((rect, hit));
    }

    pub fn push_home_sidebar_section_hit(&mut self, rect: HitRect, section: HomeSidebarSection) {
        self.home_sidebar_section_hits.push((rect, section));
    }

    pub fn push_home_tile_hit(&mut self, rect: HitRect, index: usize) {
        self.home_tile_hits.push((rect, index));
    }

    pub fn push_playlist_track_hit(&mut self, rect: HitRect, index: usize) {
        self.playlist_track_hits.push((rect, index));
    }

    pub fn push_playlist_track_download_hit(&mut self, rect: HitRect, index: usize) {
        self.playlist_track_download_hits.push((rect, index));
    }

    pub fn push_search_item_download_hit(&mut self, rect: HitRect, index: usize) {
        self.search_item_download_hits.push((rect, index));
    }

    pub fn push_author_tile_hit(&mut self, rect: HitRect, index: usize) {
        self.author_tile_hits.push((rect, index));
    }

    pub fn push_search_item_hit(&mut self, rect: HitRect, index: usize) {
        self.search_item_hits.push((rect, index));
    }

    pub fn playback_position(&self) -> Duration {
        self.audio_player.display_position()
    }

    /// 是否正在后台加载跳转目标（进度条据此显示脉冲加载动画）。
    pub fn is_seeking(&self) -> bool {
        self.audio_player.is_seeking()
    }

    /// 是否有进行中的动画需要高频重绘（进度条脉冲、搜索框滑出、侧边栏
    /// 滑出、启动加载）。主事件循环据此在动画期间从 1s 空闲节流切换
    /// 到 ~30fps 重绘。
    pub fn should_continuous_redraw(&self) -> bool {
        if self.is_seeking() {
            return true;
        }
        // 拖歌词浮窗时保持高帧率，鼠标跟手。
        if self.page_lyrics_grab.is_some() {
            return true;
        }
        if let Some(started_at) = self.search_box_anim_started_at {
            if started_at.elapsed() < SEARCH_BOX_ANIM_DURATION {
                return true;
            }
        }
        if self.home_sidebar.anim_started_at.is_some() {
            return true;
        }
        // 作者页数据在途：结果一到就上屏，别让 1s 空闲节流把它压住。
        if self.author_fetch.is_some() {
            return true;
        }
        // 歌单页 / 专辑页同理。
        if self.playlist_fetch.is_some() {
            return true;
        }
        // 下载中：图标要一直转（time-based 帧），别被 1s 空闲节流压成 1fps。
        if self.download_manager.is_active() {
            return true;
        }
        // 加载页全程保持高帧率：进度条本身在缓动，收尾还要等最短可见时长，
        // 交给 1s 空闲节流会把最后一步拖慢。
        if self.page == Page::Loading {
            return true;
        }
        if self.flat_switch_anim.is_some() {
            return true;
        }
        if self.small_window_mode == Some(SmallWindowMode::Flat)
            && self.playback_state == PlaybackRuntimeState::Playing
        {
            return true;
        }
        if self.small_window_mode == Some(SmallWindowMode::Narrow)
            && (self.playback_state == PlaybackRuntimeState::Playing || self.vu_animating)
        {
            return true;
        }
        // about 彩蛋：蓄力/迸发在动，激活后边框噪点也持续流动。
        #[cfg(feature = "easter-egg")]
        if self.about_egg.phase != EasterEggPhase::Idle {
            return true;
        }
        false
    }

    pub fn playback_duration(&self) -> Duration {
        if let Some(duration) = self.audio_player.duration() {
            return duration;
        }

        if let Some(track) = self.now_playing.as_ref() {
            return Duration::from_millis(track.duration_ms.max(0) as u64);
        }

        Duration::from_secs(0)
    }

    /// Returns (downloaded_bytes, total_bytes) for streaming buffer progress.
    /// Returns None if not streaming or if total is unknown.
    pub fn buffer_progress(&mut self) -> Option<(u64, u64)> {
        self.audio_player.recv_progress()
    }

    pub fn now_playing_artist_text(&self) -> String {
        self.now_playing
            .as_ref()
            .map(|track| track.artist.clone())
            .unwrap_or_default()
    }

    pub fn cava_bars(&self) -> [f32; 20] {
        self.cava.as_ref().map(|x| x.bars()).unwrap_or_default()
    }

    /// 播放链路上的 PCM 抽头环句柄，经 `HostPlaybackBridge` 交给全屏页示波器。
    pub fn pcm_ring(&self) -> Arc<PcmRing> {
        self.audio_player.pcm_ring()
    }

    /// 是否处于“小窗口相关”上下文：设置开启、已登录内容页、且终端低于统一阈值。
    /// 该上下文包含扁窗、窄窗，以及两个方向都过小而只显示提示的情况。
    pub fn is_small_window_context(&self) -> bool {
        if !self.config.small_window_display {
            return false;
        }
        if matches!(self.page, Page::Login | Page::Loading) {
            return false;
        }
        self.term_width < SMALL_WINDOW_MIN_WIDTH || self.term_height < SMALL_WINDOW_MIN_HEIGHT
    }

    pub fn set_terminal_size(&mut self, width: u16, height: u16) {
        let was_small_context = self.is_small_window_context();
        self.term_width = width;
        self.term_height = height;
        if !was_small_context && self.is_small_window_context() {
            // 普通尺寸进入小窗口上下文（含“双轴过小”）：立即关闭弹窗与侧边栏。
            self.close_panels_for_small_window();
        }
        self.recompute_small_window_mode();
    }

    pub fn sync_terminal_size(&mut self) {
        if let Ok((width, height)) = crossterm::terminal::size() {
            self.set_terminal_size(width, height);
        }
    }

    fn compute_small_window_mode(&self) -> Option<SmallWindowMode> {
        if !self.is_small_window_context() {
            return None;
        }

        let width = self.term_width;
        let height = self.term_height;
        if height >= SMALL_WINDOW_MIN_HEIGHT && width < SMALL_WINDOW_MIN_WIDTH {
            Some(SmallWindowMode::Narrow)
        } else if width >= SMALL_WINDOW_MIN_WIDTH
            && (FLAT_SMALL_HEIGHT..SMALL_WINDOW_MIN_HEIGHT).contains(&height)
        {
            Some(SmallWindowMode::Flat)
        } else {
            // 高度不足扁窗下限，或宽高同时过小：仍显示“终端窗口过小”。
            None
        }
    }

    fn recompute_small_window_mode(&mut self) {
        let mode = self.compute_small_window_mode();
        if mode == self.small_window_mode {
            if mode == Some(SmallWindowMode::Flat) {
                let width = self.term_width.max(1) as f32;
                let current = self.flat_switch_offset().min(width);
                if let Some(anim) = &mut self.flat_switch_anim {
                    anim.to_x = match self.flat_panel {
                        FlatPanel::Player => 0.0,
                        FlatPanel::Lyrics => width,
                    };
                    anim.from_x = current;
                }
                let exact = self.term_height == FLAT_SMALL_HEIGHT;
                if exact && !self.flat_exact_height {
                    // 从“上方有歌词”的扁窗缩到只剩播放栏高度时，重新以播放栏为默认面板。
                    self.flat_panel = FlatPanel::Player;
                    self.flat_switch_anim = None;
                }
                self.flat_exact_height = exact;
            }
            return;
        }

        self.small_window_mode = mode;
        match mode {
            Some(SmallWindowMode::Flat) => {
                // 每次进入扁窗都从缩略播放栏开始。
                self.flat_panel = FlatPanel::Player;
                self.flat_switch_anim = None;
                self.flat_exact_height = self.term_height == FLAT_SMALL_HEIGHT;
                self.close_panels_for_small_window();
            }
            Some(SmallWindowMode::Narrow) => {
                self.flat_exact_height = false;
                self.flat_switch_anim = None;
                self.vu_left_lufs = VU_LUFS_FLOOR;
                self.vu_right_lufs = VU_LUFS_FLOOR;
                self.vu_animating = false;
                self.vu_last_tick_at = None;
                self.close_panels_for_small_window();
            }
            None => {
                // 离开小窗口回到普通页面：清掉小窗口阶段可能残留的命中区域。
                self.flat_exact_height = false;
                self.flat_switch_anim = None;
                self.clear_content_hits();
                self.clear_player_bar_hits();
            }
        }

        // 宽高同时过小、只显示“终端窗口过小”时没有独立模式，
        // 但同样属于小窗口上下文：关闭已打开的弹窗与侧边栏。
        if mode.is_none() && self.is_small_window_context() {
            self.close_panels_for_small_window();
        }
    }

    /// 进入小窗口时关闭设置/搜索等弹窗与侧边栏，并清理隐藏页面的命中区域。
    fn close_panels_for_small_window(&mut self) {
        if self.overlay.is_some() {
            self.close_overlay();
        }
        self.home_sidebar.expanded = false;
        self.home_sidebar.anim_progress = 0.0;
        self.home_sidebar.anim_from = 0.0;
        self.home_sidebar.anim_started_at = None;
        self.clear_content_hits();
        self.clear_player_bar_hits();
    }

    /// 扁窗 5 行视口下两个面板的水平偏移（0=播放栏，width=歌词栏）。
    pub fn flat_switch_offset(&self) -> f32 {
        let width = self.term_width.max(1) as f32;
        if let Some(anim) = &self.flat_switch_anim {
            let elapsed = anim.started_at.elapsed().as_secs_f32();
            let t = if elapsed >= FLAT_SWITCH_ANIM_DURATION.as_secs_f32() {
                1.0
            } else {
                elapsed / FLAT_SWITCH_ANIM_DURATION.as_secs_f32()
            };
            let eased = cubic_bezier_y(t, 0.0, 0.7);
            return anim.from_x + (anim.to_x - anim.from_x) * eased;
        }
        match self.flat_panel {
            FlatPanel::Player => 0.0,
            FlatPanel::Lyrics => width,
        }
    }

    pub fn flat_switch_animating(&self) -> bool {
        self.flat_switch_anim.is_some()
    }

    pub fn toggle_flat_panel(&mut self) {
        if self.small_window_mode != Some(SmallWindowMode::Flat)
            || self.term_height != FLAT_SMALL_HEIGHT
        {
            return;
        }

        let width = self.term_width.max(1) as f32;
        let current = self.flat_switch_offset().clamp(0.0, width);
        let visually_lyrics = current >= width * 0.5;
        self.flat_panel = if visually_lyrics {
            FlatPanel::Player
        } else {
            FlatPanel::Lyrics
        };
        let to_x = match self.flat_panel {
            FlatPanel::Player => 0.0,
            FlatPanel::Lyrics => width,
        };
        self.flat_switch_anim = Some(FlatSwitchAnim {
            from_x: current,
            to_x,
            started_at: Instant::now(),
        });
    }

    fn tick_flat_switch(&mut self) {
        let Some(anim) = &mut self.flat_switch_anim else {
            return;
        };
        let width = self.term_width.max(1) as f32;
        let target = match self.flat_panel {
            FlatPanel::Player => 0.0,
            FlatPanel::Lyrics => width,
        };
        anim.to_x = target;
        if (anim.from_x - target).abs() < 0.5 {
            self.flat_switch_anim = None;
            return;
        }
        if anim.started_at.elapsed() >= FLAT_SWITCH_ANIM_DURATION {
            self.flat_switch_anim = None;
        }
    }

    fn tick_vu_meter(&mut self, now: Instant) {
        if self.small_window_mode != Some(SmallWindowMode::Narrow) {
            self.vu_animating = false;
            self.vu_last_tick_at = None;
            return;
        }

        let reading = self.audio_player.lufs_meter().latest();
        let dt = self
            .vu_last_tick_at
            .map(|at| now.saturating_duration_since(at).as_secs_f32().min(0.25))
            .unwrap_or(0.0);
        self.vu_last_tick_at = Some(now);

        let stale = self.vu_last_meter_generation == reading.generation
            && self.playback_state != PlaybackRuntimeState::Playing;
        self.vu_last_meter_generation = reading.generation;

        let target_left = if stale || self.now_playing.is_none() {
            VU_LUFS_FLOOR
        } else {
            mean_square_to_lufs(reading.left_mean_square)
        };
        let target_right = if stale || self.now_playing.is_none() {
            VU_LUFS_FLOOR
        } else {
            mean_square_to_lufs(reading.right_mean_square)
        };

        let (left, left_moving) = smooth_lufs_level(self.vu_left_lufs, target_left, dt);
        let (right, right_moving) = smooth_lufs_level(self.vu_right_lufs, target_right, dt);
        self.vu_left_lufs = left;
        self.vu_right_lufs = right;
        self.vu_animating = left_moving || right_moving;
    }

    pub fn main_spectrum_braille(&mut self) -> String {
        let mut out = String::with_capacity(10);
        for i in 0..10 {
            let bar = self.cava_bars();
            let left = bar[i * 2].clamp(0.0, 1.0);
            let right = bar[i * 2 + 1].clamp(0.0, 1.0);
            let left_h = (left * 4.0).round() as u8;
            let right_h = (right * 4.0).round() as u8;
            out.push(braille_from_two_bars(left_h.min(4), right_h.min(4)));
        }
        out
    }

    pub fn sync_on_change(&mut self) {
        self.sync_cava();
        self.sync_terminal_size();
    }

    fn sync_cava(&mut self) {
        let available = crate::tmplayer::audio::cava::is_available();
        // 两个无可视化档位都把 cava 停掉：折叠视图那 10 格迷你频谱也就没数据可画。
        let enable = !matches!(
            self.config.visualize,
            VisualizeMode::Lyrics | VisualizeMode::Hidden
        );
        if !available || !enable {
            self.cava = None;
            return;
        }

        if self.cava.is_none() {
            let cfg = CavaConfig {
                framerate_hz: self.config.spectrum_hz.clamp(1, 30),
                bars: 20,
                channels: CavaChannels::Mono,
                reverse: false,
            };

            self.cava = MiniCavaState::try_new(cfg).ok();
        }
    }

    pub fn suspend_main_cava_for_fullscreen(&mut self) {
        self.cava = None;
    }

    pub fn resume_main_cava_after_fullscreen(&mut self) {
        self.sync_cava();
    }

    fn seek_to_ratio(&mut self, ratio: f32) {
        if self.now_playing.is_none() {
            return;
        }

        let fallback_total = self
            .now_playing
            .as_ref()
            .map(|track| Duration::from_millis(track.duration_ms.max(0) as u64));
        let _ = self.audio_player.seek_to_ratio(ratio, fallback_total);
        self.playback_state = map_audio_state(self.audio_player.state());
    }

    pub async fn fullscreen_tick_playback(&mut self) {
        self.tick_audio().await;
        self.tick_cover_fetch();
        self.tick_lyric_fetch();
        self.apply_mpris_control_events().await;
        self.sync_mpris_exposure();
        // 全屏页不跑宿主主循环，收藏的派发/收敛要在这里推进。
        self.tick_like_sync();
        // 下载结果与图标状态同理（全屏页也能发起下载）。
        self.tick_download();
    }

    async fn apply_mpris_control_events(&mut self) {
        for event in self.mpris_bridge.drain_control_events() {
            match event {
                MprisControlEvent::Play => self.mpris_play().await,
                MprisControlEvent::Pause => self.mpris_pause(),
                MprisControlEvent::PlayPause => self.toggle_play_pause_hotkey().await,
                MprisControlEvent::Stop => {
                    self.audio_player.stop();
                    self.playback_state = PlaybackRuntimeState::Stopped;
                }
                MprisControlEvent::Next => self.play_next_hotkey().await,
                MprisControlEvent::Previous => self.play_previous_hotkey().await,
                MprisControlEvent::SeekRelativeMicros(delta) => self.mpris_seek_relative(delta),
                MprisControlEvent::SeekAbsoluteMicros(pos) => self.mpris_seek_absolute(pos),
            }
        }
    }

    async fn mpris_play(&mut self) {
        if self.now_playing.is_none() {
            return;
        }
        if self.playback_state == PlaybackRuntimeState::Stopped {
            if let Some(index) = self.playback_index {
                self.play_queue_index(index, false).await;
            }
            return;
        }
        if self.playback_state == PlaybackRuntimeState::Paused {
            self.audio_player.toggle_play_pause();
            self.playback_state = map_audio_state(self.audio_player.state());
        }
    }

    fn mpris_pause(&mut self) {
        if self.playback_state == PlaybackRuntimeState::Playing {
            self.audio_player.toggle_play_pause();
            self.playback_state = map_audio_state(self.audio_player.state());
        }
    }

    fn mpris_seek_relative(&mut self, delta_micros: i64) {
        let total = self.playback_duration();
        let total_micros = total.as_micros();
        if total_micros == 0 {
            return;
        }

        let current_micros = self.audio_player.position().as_micros() as i128;
        let target = (current_micros + delta_micros as i128).clamp(0, total_micros as i128);
        let ratio = (target as f64 / total_micros as f64) as f32;
        self.seek_to_ratio(ratio);
    }

    fn mpris_seek_absolute(&mut self, position_micros: i64) {
        let total = self.playback_duration();
        let total_micros = total.as_micros();
        if total_micros == 0 {
            return;
        }

        let target = (position_micros as i128).clamp(0, total_micros as i128);
        let ratio = (target as f64 / total_micros as f64) as f32;
        self.seek_to_ratio(ratio);
    }

    fn sync_mpris_exposure(&mut self) {
        let now = Instant::now();
        let signature = self
            .now_playing
            .as_ref()
            .map(mpris_metadata_signature)
            .unwrap_or(0);

        let metadata_changed = self.mpris_last_signature != Some(signature);
        let playback_changed = self.mpris_last_playback != self.playback_state;
        let periodic_tick =
            now.duration_since(self.mpris_last_sync_at) >= Duration::from_millis(900);

        if !metadata_changed && !playback_changed && !periodic_tick {
            return;
        }

        let payload = MprisSyncPayload {
            playback: self.playback_state,
            position: self.audio_player.display_position(),
            track: if metadata_changed {
                self.now_playing.clone()
            } else {
                None
            },
        };

        self.mpris_bridge.update(payload);
        self.mpris_last_sync_at = now;
        self.mpris_last_playback = self.playback_state;
        if metadata_changed {
            self.mpris_last_signature = Some(signature);
        }
    }

    pub fn fullscreen_playback_snapshot(&self) -> FullscreenPlaybackSnapshot {
        FullscreenPlaybackSnapshot {
            queue: self.playback_queue.clone(),
            current_index: self.playback_index,
            now_playing: self.now_playing.clone(),
            now_playing_liked: self.now_playing_liked,
            state: self.playback_state,
            repeat_mode: self.playback_repeat_mode,
            position: self.audio_player.display_position(),
        }
    }

    pub fn fullscreen_runtime_snapshot(&self) -> FullscreenRuntimeSnapshot {
        FullscreenRuntimeSnapshot {
            current_index: self.playback_index,
            now_playing_liked: self.now_playing_liked,
            state: self.playback_state,
            repeat_mode: self.playback_repeat_mode,
            position: self.audio_player.display_position(),
            volume: self.audio_player.volume(),
            seeking: self.audio_player.is_seeking(),
            download: self.current_download_state(),
        }
    }

    /// 当前播放歌曲的下载图标状态（每帧缓存一次，全屏页只读不再查磁盘）。
    pub fn current_download_state(&self) -> Option<DownloadState> {
        if self.download_root.is_none() || self.now_playing.is_none() {
            return None;
        }
        Some(self.now_playing_download_state)
    }

    /// 供全屏页每帧刷新的缓存：任务表与磁盘状态都封在这一处。
    fn refresh_current_download_state(&mut self) {
        let state = match self.now_playing.clone() {
            Some(track) => self
                .download_state_for_candidate(&DownloadCandidate {
                    song_id: track.song_id,
                    title: track.title,
                    artist: track.artist,
                    album: track.album,
                })
                .unwrap_or(DownloadState::NotDownloaded),
            None => DownloadState::NotDownloaded,
        };
        self.now_playing_download_state = state;
    }

    pub fn fullscreen_metadata_signature(&self) -> u64 {
        let mut hasher = DefaultHasher::new();
        self.playback_queue.len().hash(&mut hasher);
        self.playback_index.hash(&mut hasher);

        for track in &self.playback_queue {
            track.song_id.hash(&mut hasher);
            track.duration_ms.hash(&mut hasher);
        }

        if let Some(track) = self.now_playing.as_ref() {
            track.song_id.hash(&mut hasher);
            track.duration_ms.hash(&mut hasher);
            track
                .cover
                .as_ref()
                .map(|v| v.len())
                .unwrap_or(0)
                .hash(&mut hasher);
            track
                .lyrics
                .as_ref()
                .map(|v| v.len())
                .unwrap_or(0)
                .hash(&mut hasher);
            track
                .lyrics
                .as_ref()
                .and_then(|v| v.last().map(|line| line.start_ms))
                .unwrap_or(0)
                .hash(&mut hasher);
        }

        hasher.finish()
    }

    pub async fn fullscreen_toggle_play_pause(&mut self) {
        self.toggle_play_pause_hotkey().await;
    }

    pub async fn fullscreen_play_previous(&mut self) {
        self.play_previous_hotkey().await;
    }

    pub async fn fullscreen_play_next(&mut self) {
        self.play_next_hotkey().await;
    }

    pub async fn fullscreen_play_queue_index(&mut self, index: usize) {
        if index < self.playback_queue.len() {
            self.play_queue_index(index, false).await;
        }
    }

    pub fn fullscreen_seek_to_ratio(&mut self, ratio: f32) {
        self.seek_to_ratio(ratio);
    }

    pub fn fullscreen_set_volume(&mut self, volume: f32) {
        self.audio_player.set_volume(volume);
    }

    pub fn fullscreen_toggle_repeat_mode(&mut self) {
        self.cycle_repeat_mode_hotkey();
    }

    pub async fn fullscreen_toggle_like(&mut self) {
        self.toggle_like_hotkey();
    }

    async fn handle_overlay_key(&mut self, overlay: Overlay, key: KeyEvent) {
        match overlay {
            Overlay::Settings => self.handle_settings_root_key(key).await,
            Overlay::SettingsPlayback => self.handle_settings_playback_key(key),
            Overlay::SettingsKeybinds => self.handle_settings_keybinds_key(key),
            Overlay::SettingsLyrics => self.handle_settings_lyrics_key(key),
            Overlay::SettingsDownload => self.handle_settings_download_key(key),
            Overlay::SettingsAbout => self.handle_settings_about_key(key),
            Overlay::SearchBox => self.handle_search_box_key(key).await,
        }
    }

    async fn try_handle_configured_hotkey(&mut self, key: KeyEvent) -> bool {
        let Some(action) = self.keybind_action_from_event(key) else {
            return false;
        };

        if matches!(
            action,
            KeybindAction::ToggleLikeFullscreen
                | KeybindAction::FullscreenPrev
                | KeybindAction::FullscreenNext
                | KeybindAction::FullscreenTogglePlayPause
                | KeybindAction::FullscreenToggleMode
                | KeybindAction::FullscreenEq
                | KeybindAction::FullscreenEqReset
                | KeybindAction::SmallWindowToggle
        ) {
            return false;
        }

        if !self.can_execute_global_hotkey() {
            return true;
        }

        self.trigger_keybind_action(action).await;
        true
    }

    fn can_execute_global_hotkey(&mut self) -> bool {
        let now = Instant::now();
        if let Some(last_at) = self.last_global_hotkey_at {
            if now.duration_since(last_at) < Duration::from_millis(GLOBAL_HOTKEY_COOLDOWN_MS) {
                return false;
            }
        }
        self.last_global_hotkey_at = Some(now);
        true
    }

    async fn trigger_keybind_action(&mut self, action: KeybindAction) {
        match action {
            KeybindAction::SearchBox => self.open_search_box(),
            KeybindAction::Fullscreen => {
                // 全屏页普通布局最小宽度为 50；更窄的窗口直接忽略打开全屏，
                // 避免“进入全屏后立即因过小退出”。
                if self.term_width >= FULLSCREEN_MIN_WIDTH {
                    self.launch_fullscreen_requested = true;
                }
            }
            KeybindAction::Settings => self.open_settings(),
            KeybindAction::Sidebar => self.toggle_home_sidebar().await,
            KeybindAction::Quit => {
                self.should_quit = true;
            }
            KeybindAction::PageUp => self.quick_page_up_hotkey().await,
            KeybindAction::PageDown => self.quick_page_down_hotkey().await,
            KeybindAction::Prev => self.play_previous_hotkey().await,
            KeybindAction::Next => self.play_next_hotkey().await,
            KeybindAction::TogglePlayPause => self.toggle_play_pause_hotkey().await,
            KeybindAction::ToggleMode => self.cycle_repeat_mode_hotkey(),
            KeybindAction::FullscreenPrev => {}
            KeybindAction::FullscreenNext => {}
            KeybindAction::FullscreenTogglePlayPause => {}
            KeybindAction::FullscreenToggleMode => {}
            KeybindAction::FullscreenEq => {}
            KeybindAction::FullscreenEqReset => {}
            KeybindAction::ToggleLikeFullscreen => {}
            KeybindAction::ToggleLikeCollapsed => self.toggle_like_hotkey(),
            KeybindAction::SmallWindowToggle => {}
            KeybindAction::Download => self.download_focused_song().await,
            // 全屏页专用：宿主侧不处理（与其余 Fullscreen* 动作同理）。
            KeybindAction::DownloadFullscreen => {}
        }
    }

    async fn toggle_home_sidebar(&mut self) {
        if self.page != Page::Home || self.overlay.is_some() {
            return;
        }

        if self.home_sidebar.expanded {
            self.home_sidebar.expanded = false;
            self.animate_home_sidebar();
            return;
        }

        self.home_sidebar.expanded = true;
        self.animate_home_sidebar();

        if !self.home_sidebar.created_playlists.is_empty()
            || !self.home_sidebar.collected_playlists.is_empty()
        {
            self.home_sidebar.reset_focus();
            return;
        }

        // 异步填充：立刻返回，动画照常跑，数据由 tick 搬入。
        if self.home_sidebar_fetch.is_none() {
            self.home_sidebar.loading = true;
            let fut = fetch_home_sidebar_playlists(self.api.clone(), self.config.language);
            let fut: Pin<Box<dyn Future<Output = Option<Result<HomeSidebarFetch, String>>>>> =
                Box::pin(async move { Some(fut.await) });
            self.home_sidebar_fetch = Some(shot_and_share(fut));
        }
    }

    /// 周期性检查 stderr 日志体积。原生库可能持续刷 stderr，
    /// 而那个文件不经 ftail，需要自己设上限。
    fn tick_stderr_log_trim(&mut self) {
        const CHECK_INTERVAL: Duration = Duration::from_secs(60);
        let due = match self.stderr_trim_checked_at {
            Some(at) => at.elapsed() >= CHECK_INTERVAL,
            None => true,
        };
        if !due {
            return;
        }
        self.stderr_trim_checked_at = Some(Instant::now());
        crate::trim_stderr_log_if_needed();
    }

    /// 搬运侧边栏歌单的异步结果（每帧调用，结果就绪才动状态）。
    fn tick_home_sidebar_fetch(&mut self) {
        let Some(result) = peek_shared_future(&self.home_sidebar_fetch).cloned() else {
            return;
        };
        self.home_sidebar_fetch = None;
        self.home_sidebar.loading = false;

        match result {
            Ok(data) => {
                self.home_sidebar.user_id = Some(data.user_id);
                self.home_sidebar.liked_playlist_id = data.liked_playlist_id;
                self.home_sidebar.user_name = data.user_name;
                self.home_sidebar.created_playlists = data.created;
                self.home_sidebar.collected_playlists = data.collected;
                self.home_sidebar.clamp_focus();
                self.home_sidebar.status_line = match self.config.language {
                    Language::Zh => format!(
                        "创建 {} 个，收藏 {} 个",
                        self.home_sidebar.created_playlists.len(),
                        self.home_sidebar.collected_playlists.len()
                    ),
                    Language::En => format!(
                        "{} created, {} collected",
                        self.home_sidebar.created_playlists.len(),
                        self.home_sidebar.collected_playlists.len()
                    ),
                };
                self.home.status_line = self.home_sidebar.status_line.clone();
                self.home_sidebar.reset_focus();
            }
            Err(err) => {
                let text = format!(
                    "{}: {}",
                    self.lang_text("主页歌单加载失败", "Failed to load home playlists"),
                    err
                );
                self.home_sidebar.status_line = text.clone();
                self.home.status_line = text;
            }
        }
    }

    /// 搬运作者页的在途拉取（每帧调用，结果就绪才动状态）。
    fn tick_author_fetch(&mut self) {
        let Some(result) = peek_shared_future(&self.author_fetch).cloned() else {
            return;
        };
        self.author_fetch = None;

        match result {
            Ok(fetch) => {
                self.playlist_section_return_snapshot = None;
                let title = fetch.title.clone();
                self.apply_author_fetch(fetch);
                self.set_runtime_status(format!(
                    "{} {}",
                    self.lang_text("已打开作者", "Opened artist"),
                    title
                ));
            }
            Err(message) => {
                self.author.description = format!(
                    "{}: {message}",
                    self.lang_text("作者页加载失败", "Failed to load the artist page")
                );
                self.set_runtime_status(format!(
                    "{}: {message}",
                    self.lang_text("打开作者页失败", "Failed to open the artist page"),
                ));
            }
        }
    }

    /// 搬运歌单页 / 专辑页的在途拉取（每帧调用，结果就绪才动状态）。
    fn tick_playlist_fetch(&mut self) {
        let Some(slot) = self.playlist_fetch.as_ref() else {
            return;
        };
        let Some(result) = peek_shared(&slot.future).cloned() else {
            return;
        };
        let kind = slot.kind;
        self.playlist_fetch = None;
        self.playlist_page_kind = kind;

        match result {
            Ok(fetch) => {
                let title = fetch.title.clone();
                self.apply_playlist_fetch(fetch);
                self.set_runtime_status(format!("{} {}", kind.opened(self.config.language), title));
            }
            Err(message) => {
                self.playlist.description =
                    format!("{}: {message}", kind.failed(self.config.language));
                self.set_runtime_status(format!(
                    "{}: {message}",
                    kind.failed(self.config.language)
                ));
            }
        }
    }

    async fn open_focused_home_sidebar_playlist(&mut self) {
        let (playlist_id, title) = {
            let Some(item) = self.home_sidebar.focused_playlist() else {
                self.home.status_line = self
                    .lang_text("侧边栏暂无可打开歌单", "No sidebar playlist to open")
                    .to_string();
                return;
            };

            let Some(playlist_id) = item.id.clone() else {
                self.home.status_line = self
                    .lang_text(
                        "当前歌单缺少 ID，无法打开",
                        "The selected playlist has no ID",
                    )
                    .to_string();
                return;
            };

            (playlist_id, item.title.clone())
        };

        if self.is_liked_playlist(&playlist_id, Some(&title)) {
            let _ = self.refresh_liked_song_cache().await;
            self.refresh_now_playing_like_state();
        }

        self.home.status_line = format!("{} {}", self.lang_text("正在加载", "Loading"), title);

        match self.load_playlist_detail(&playlist_id).await {
            Ok(()) => {
                self.playlist_return_page = Page::Home;
                self.playlist_section_return_snapshot = None;
                self.home_sidebar.expanded = false;
                let target = if self.home_sidebar.expanded { 1.0 } else { 0.0 };
                self.home_sidebar.anim_progress = target;
                self.page = Page::Playlist;
                self.home.status_line = format!("{} {}", self.lang_text("已打开", "Opened"), title);
            }
            Err(err) => {
                self.home.status_line = format!(
                    "{}: {}",
                    self.lang_text("打开歌单失败", "Failed to open playlist"),
                    err
                );
            }
        }
    }

    fn keybind_action_from_event(&self, key: KeyEvent) -> Option<KeybindAction> {
        let actions = [
            KeybindAction::SearchBox,
            KeybindAction::Fullscreen,
            KeybindAction::Settings,
            KeybindAction::Sidebar,
            KeybindAction::Quit,
            KeybindAction::PageUp,
            KeybindAction::PageDown,
            KeybindAction::Prev,
            KeybindAction::Next,
            KeybindAction::TogglePlayPause,
            KeybindAction::ToggleMode,
            KeybindAction::FullscreenPrev,
            KeybindAction::FullscreenNext,
            KeybindAction::FullscreenTogglePlayPause,
            KeybindAction::FullscreenToggleMode,
            KeybindAction::FullscreenEq,
            KeybindAction::FullscreenEqReset,
            KeybindAction::ToggleLikeFullscreen,
            KeybindAction::ToggleLikeCollapsed,
            KeybindAction::SmallWindowToggle,
            KeybindAction::Download,
            KeybindAction::DownloadFullscreen,
        ];

        actions
            .into_iter()
            .find(|&action| keybind_matches(self.keybind_value_for_action(action), key))
    }

    fn keybind_value_for_action(&self, action: KeybindAction) -> &str {
        match action {
            KeybindAction::SearchBox => &self.config.keybind_search_box,
            KeybindAction::Fullscreen => &self.config.keybind_fullscreen,
            KeybindAction::Settings => &self.config.keybind_settings,
            KeybindAction::Sidebar => &self.config.keybind_sidebar,
            KeybindAction::Quit => &self.config.keybind_quit,
            KeybindAction::PageUp => &self.config.keybind_page_up,
            KeybindAction::PageDown => &self.config.keybind_page_down,
            KeybindAction::Prev => &self.config.keybind_prev,
            KeybindAction::Next => &self.config.keybind_next,
            KeybindAction::TogglePlayPause => &self.config.keybind_toggle_play_pause,
            KeybindAction::ToggleMode => &self.config.keybind_toggle_mode,
            KeybindAction::FullscreenPrev => &self.config.keybind_fullscreen_prev,
            KeybindAction::FullscreenNext => &self.config.keybind_fullscreen_next,
            KeybindAction::FullscreenTogglePlayPause => {
                &self.config.keybind_fullscreen_toggle_play_pause
            }
            KeybindAction::FullscreenToggleMode => &self.config.keybind_fullscreen_toggle_mode,
            KeybindAction::FullscreenEq => &self.config.keybind_fullscreen_eq,
            KeybindAction::FullscreenEqReset => &self.config.keybind_fullscreen_eq_reset,
            KeybindAction::ToggleLikeFullscreen => &self.config.keybind_toggle_like_fullscreen,
            KeybindAction::ToggleLikeCollapsed => &self.config.keybind_toggle_like_collapsed,
            KeybindAction::SmallWindowToggle => &self.config.keybind_small_window_toggle,
            KeybindAction::Download => &self.config.keybind_download,
            KeybindAction::DownloadFullscreen => &self.config.keybind_download_fullscreen,
        }
    }

    fn keybind_value_mut_for_index(&mut self, index: usize) -> Option<&mut String> {
        match index {
            0 => Some(&mut self.config.keybind_search_box),
            1 => Some(&mut self.config.keybind_fullscreen),
            2 => Some(&mut self.config.keybind_settings),
            3 => Some(&mut self.config.keybind_sidebar),
            4 => Some(&mut self.config.keybind_quit),
            5 => Some(&mut self.config.keybind_page_up),
            6 => Some(&mut self.config.keybind_page_down),
            7 => Some(&mut self.config.keybind_prev),
            8 => Some(&mut self.config.keybind_next),
            9 => Some(&mut self.config.keybind_toggle_play_pause),
            10 => Some(&mut self.config.keybind_fullscreen_prev),
            11 => Some(&mut self.config.keybind_fullscreen_next),
            12 => Some(&mut self.config.keybind_fullscreen_toggle_play_pause),
            13 => Some(&mut self.config.keybind_fullscreen_toggle_mode),
            14 => Some(&mut self.config.keybind_fullscreen_eq),
            15 => Some(&mut self.config.keybind_fullscreen_eq_reset),
            16 => Some(&mut self.config.keybind_toggle_like_fullscreen),
            17 => Some(&mut self.config.keybind_toggle_mode),
            18 => Some(&mut self.config.keybind_toggle_like_collapsed),
            19 => Some(&mut self.config.keybind_small_window_toggle),
            20 => Some(&mut self.config.keybind_download),
            21 => Some(&mut self.config.keybind_download_fullscreen),
            _ => None,
        }
    }

    fn keybind_value_for_index(&self, index: usize) -> Option<&str> {
        match index {
            0 => Some(self.config.keybind_search_box.as_str()),
            1 => Some(self.config.keybind_fullscreen.as_str()),
            2 => Some(self.config.keybind_settings.as_str()),
            3 => Some(self.config.keybind_sidebar.as_str()),
            4 => Some(self.config.keybind_quit.as_str()),
            5 => Some(self.config.keybind_page_up.as_str()),
            6 => Some(self.config.keybind_page_down.as_str()),
            7 => Some(self.config.keybind_prev.as_str()),
            8 => Some(self.config.keybind_next.as_str()),
            9 => Some(self.config.keybind_toggle_play_pause.as_str()),
            10 => Some(self.config.keybind_fullscreen_prev.as_str()),
            11 => Some(self.config.keybind_fullscreen_next.as_str()),
            12 => Some(self.config.keybind_fullscreen_toggle_play_pause.as_str()),
            13 => Some(self.config.keybind_fullscreen_toggle_mode.as_str()),
            14 => Some(self.config.keybind_fullscreen_eq.as_str()),
            15 => Some(self.config.keybind_fullscreen_eq_reset.as_str()),
            16 => Some(self.config.keybind_toggle_like_fullscreen.as_str()),
            17 => Some(self.config.keybind_toggle_mode.as_str()),
            18 => Some(self.config.keybind_toggle_like_collapsed.as_str()),
            19 => Some(self.config.keybind_small_window_toggle.as_str()),
            20 => Some(self.config.keybind_download.as_str()),
            21 => Some(self.config.keybind_download_fullscreen.as_str()),
            _ => None,
        }
    }

    fn find_keybind_conflict(&self, current_index: usize, binding: &str) -> Option<usize> {
        let normalized = normalize_keybind_text(binding)?;
        for other_index in 0..SETTINGS_KEYBIND_ITEMS {
            if other_index == current_index {
                continue;
            }
            let Some(other_binding) = self.keybind_value_for_index(other_index) else {
                continue;
            };
            let Some(other_normalized) = normalize_keybind_text(other_binding) else {
                continue;
            };
            if other_normalized.eq_ignore_ascii_case(normalized.as_str()) {
                return Some(other_index);
            }
        }
        None
    }

    fn keybind_name_for_index(&self, index: usize) -> &'static str {
        match index {
            0 => self.lang_text("搜索框", "Search Box"),
            1 => self.lang_text("全屏播放页", "Fullscreen"),
            2 => self.lang_text("设置弹窗", "Settings Modal"),
            3 => self.lang_text("侧边栏", "Sidebar"),
            4 => self.lang_text("退出应用", "Quit"),
            5 => self.lang_text("快速上翻页", "Quick Page Up"),
            6 => self.lang_text("快速下翻页", "Quick Page Down"),
            7 => self.lang_text("上一首", "Previous"),
            8 => self.lang_text("下一首", "Next"),
            9 => self.lang_text("播放/暂停", "Play/Pause"),
            10 => self.lang_text("全屏上一首", "Fullscreen Previous"),
            11 => self.lang_text("全屏下一首", "Fullscreen Next"),
            12 => self.lang_text("全屏暂停/播放", "Fullscreen Pause/Play"),
            13 => self.lang_text("全屏模式切换", "Fullscreen Mode Switch"),
            14 => self.lang_text("全屏页EQ", "Fullscreen EQ"),
            15 => self.lang_text("全屏EQ重置", "Fullscreen EQ Reset"),
            16 => self.lang_text("全屏收藏/取消收藏", "Fullscreen Like/Unlike"),
            17 => self.lang_text("折叠栏模式切换", "Collapsed Mode Switch"),
            18 => self.lang_text("折叠栏收藏/取消收藏", "Collapsed Like/Unlike"),
            19 => self.lang_text("小窗口切换显示", "Small Window Switch"),
            20 => self.lang_text("下载歌曲（主应用）", "Download Song (Host)"),
            21 => self.lang_text("下载歌曲（全屏页）", "Download Song (Fullscreen)"),
            _ => self.lang_text("未知", "Unknown"),
        }
    }

    fn reset_keybinds_to_default(&mut self) {
        self.config.keybind_search_box = DEFAULT_KEYBIND_SEARCH_BOX.to_string();
        self.config.keybind_fullscreen = DEFAULT_KEYBIND_FULLSCREEN.to_string();
        self.config.keybind_settings = DEFAULT_KEYBIND_SETTINGS.to_string();
        self.config.keybind_sidebar = DEFAULT_KEYBIND_SIDEBAR.to_string();
        self.config.keybind_quit = DEFAULT_KEYBIND_QUIT.to_string();
        self.config.keybind_page_up = DEFAULT_KEYBIND_PAGE_UP.to_string();
        self.config.keybind_page_down = DEFAULT_KEYBIND_PAGE_DOWN.to_string();
        self.config.keybind_prev = DEFAULT_KEYBIND_PREV.to_string();
        self.config.keybind_next = DEFAULT_KEYBIND_NEXT.to_string();
        self.config.keybind_toggle_play_pause = DEFAULT_KEYBIND_TOGGLE_PLAY_PAUSE.to_string();
        self.config.keybind_toggle_mode = DEFAULT_KEYBIND_TOGGLE_MODE.to_string();
        self.config.keybind_fullscreen_prev = DEFAULT_KEYBIND_FULLSCREEN_PREV.to_string();
        self.config.keybind_fullscreen_next = DEFAULT_KEYBIND_FULLSCREEN_NEXT.to_string();
        self.config.keybind_fullscreen_toggle_play_pause =
            DEFAULT_KEYBIND_FULLSCREEN_TOGGLE_PLAY_PAUSE.to_string();
        self.config.keybind_fullscreen_toggle_mode =
            DEFAULT_KEYBIND_FULLSCREEN_TOGGLE_MODE.to_string();
        self.config.keybind_fullscreen_eq = DEFAULT_KEYBIND_FULLSCREEN_EQ.to_string();
        self.config.keybind_fullscreen_eq_reset = DEFAULT_KEYBIND_FULLSCREEN_EQ_RESET.to_string();
        self.config.keybind_toggle_like_fullscreen =
            DEFAULT_KEYBIND_TOGGLE_LIKE_FULLSCREEN.to_string();
        self.config.keybind_toggle_like_collapsed =
            DEFAULT_KEYBIND_TOGGLE_LIKE_COLLAPSED.to_string();
        self.config.keybind_small_window_toggle = DEFAULT_KEYBIND_SMALL_WINDOW_TOGGLE.to_string();
        self.config.keybind_download = DEFAULT_KEYBIND_DOWNLOAD.to_string();
        self.config.keybind_download_fullscreen = DEFAULT_KEYBIND_DOWNLOAD_FULLSCREEN.to_string();
    }

    pub fn keybind_label_for_index(&self, index: usize) -> String {
        let value = self.keybind_value_for_action(match index {
            0 => KeybindAction::SearchBox,
            1 => KeybindAction::Fullscreen,
            2 => KeybindAction::Settings,
            3 => KeybindAction::Sidebar,
            4 => KeybindAction::Quit,
            5 => KeybindAction::PageUp,
            6 => KeybindAction::PageDown,
            7 => KeybindAction::Prev,
            8 => KeybindAction::Next,
            9 => KeybindAction::TogglePlayPause,
            10 => KeybindAction::FullscreenPrev,
            11 => KeybindAction::FullscreenNext,
            12 => KeybindAction::FullscreenTogglePlayPause,
            13 => KeybindAction::FullscreenToggleMode,
            14 => KeybindAction::FullscreenEq,
            15 => KeybindAction::FullscreenEqReset,
            16 => KeybindAction::ToggleLikeFullscreen,
            17 => KeybindAction::ToggleMode,
            18 => KeybindAction::ToggleLikeCollapsed,
            19 => KeybindAction::SmallWindowToggle,
            20 => KeybindAction::Download,
            21 => KeybindAction::DownloadFullscreen,
            _ => KeybindAction::SearchBox,
        });
        format!("{}: {}", self.keybind_name_for_index(index), value)
    }

    async fn toggle_play_pause_hotkey(&mut self) {
        if self.now_playing.is_none() {
            self.set_runtime_status(
                self.lang_text("当前没有可控制的播放", "No controllable playback right now"),
            );
            return;
        }

        if self.playback_state == PlaybackRuntimeState::Stopped {
            if let Some(index) = self.playback_index {
                self.play_queue_index(index, false).await;
                return;
            }
        }

        self.audio_player.toggle_play_pause();
        self.playback_state = map_audio_state(self.audio_player.state());
    }

    async fn play_previous_hotkey(&mut self) {
        if self.playback_queue.is_empty() {
            self.set_runtime_status(self.lang_text("当前播放队列为空", "Playback queue is empty"));
            return;
        }

        let current = self
            .playback_index
            .unwrap_or(0)
            .min(self.playback_queue.len() - 1);
        let target = match self.playback_repeat_mode {
            PlaybackRepeatMode::Sequence => current.checked_sub(1),
            PlaybackRepeatMode::LoopAll => {
                Some((current + self.playback_queue.len() - 1) % self.playback_queue.len())
            }
            PlaybackRepeatMode::LoopOne => Some(current),
            PlaybackRepeatMode::Shuffle => {
                Some(pick_shuffle_index(self.playback_queue.len(), current))
            }
        };

        if let Some(index) = target {
            self.play_queue_index(index, true).await;
        }
    }

    async fn play_next_hotkey(&mut self) {
        if self.playback_queue.is_empty() {
            self.set_runtime_status(self.lang_text("当前播放队列为空", "Playback queue is empty"));
            return;
        }

        let current = self
            .playback_index
            .unwrap_or(0)
            .min(self.playback_queue.len() - 1);
        let target = match self.playback_repeat_mode {
            PlaybackRepeatMode::Sequence => {
                if current + 1 < self.playback_queue.len() {
                    Some(current + 1)
                } else {
                    None
                }
            }
            PlaybackRepeatMode::LoopAll => Some((current + 1) % self.playback_queue.len()),
            PlaybackRepeatMode::LoopOne => Some(current),
            PlaybackRepeatMode::Shuffle => {
                Some(pick_shuffle_index(self.playback_queue.len(), current))
            }
        };

        if let Some(index) = target {
            self.play_queue_index(index, true).await;
        }
    }

    fn cycle_repeat_mode_hotkey(&mut self) {
        self.playback_repeat_mode = self.playback_repeat_mode.next();
        self.set_runtime_status(format!(
            "{}: {}",
            self.lang_text("播放模式", "Play Mode"),
            match self.playback_repeat_mode {
                PlaybackRepeatMode::Sequence => self.lang_text("顺序播放", "Sequence"),
                PlaybackRepeatMode::Shuffle => self.lang_text("随机播放", "Shuffle"),
                PlaybackRepeatMode::LoopAll => self.lang_text("列表循环", "Loop All"),
                PlaybackRepeatMode::LoopOne => self.lang_text("单曲循环", "Loop One"),
            }
        ));
        self.persist_playback_memory();
    }

    async fn quick_page_up_hotkey(&mut self) {
        match self.page {
            Page::Search => {
                let step = self.search.page_items();
                for _ in 0..step {
                    if !self.search.focus_prev() {
                        break;
                    }
                }
            }
            Page::Playlist => {
                let step = self.playlist.visible_rows.max(1);
                for _ in 0..step {
                    if !self.playlist.focus_prev() {
                        break;
                    }
                }
            }
            _ => {}
        }
    }

    async fn quick_page_down_hotkey(&mut self) {
        match self.page {
            Page::Search => {
                let step = self.search.page_items();
                for _ in 0..step {
                    let before_idx = self.search.focused_idx;
                    let before_len = self.search.results.len();
                    self.advance_search_focus().await;
                    if self.search.focused_idx == before_idx
                        && self.search.results.len() == before_len
                    {
                        break;
                    }
                }
            }
            Page::Playlist => {
                let step = self.playlist.visible_rows.max(1);
                for _ in 0..step {
                    if !self.playlist.focus_next() {
                        break;
                    }
                }
            }
            _ => {}
        }
    }

    /// 切歌后刷新收藏态：先用本地缓存（含未决意图）立即显示，服务端确认
    /// 交给 `tick_like_sync` 收敛——不再阻塞事件循环。
    fn refresh_now_playing_like_state(&mut self) {
        let Some(song_id) = self.current_song_id() else {
            self.now_playing_liked = false;
            return;
        };

        self.sync_like_display(&song_id);

        let fut = song_like_check_request(self.api.clone(), song_id.clone());
        let fut: LikeVerifyTask = Box::pin(async move { Some(fut.await) });
        self.like_machine.begin_verify(song_id, shot_and_share(fut));
    }

    /// 当前曲目 id。
    fn current_song_id(&self) -> Option<String> {
        self.now_playing.as_ref().map(|track| track.song_id.clone())
    }

    /// 重新解析下载根目录（设置里改了路径、或全屏页同步回来时调用）。
    pub fn refresh_download_root(&mut self) {
        let next =
            crate::app::download::resolve_download_root(self.config.download_path.as_deref());
        if next != self.download_root {
            self.download_root = next;
            self.download_manager.clear_disk_cache();
            // 行数据的目录部分变了：整页行数据重建。
            self.download_rows_epoch = self.download_rows_epoch.wrapping_add(1);
        }
    }

    /// 当前聚焦的单曲（歌单页 / 搜索页）与它所在页面的下载上下文。
    fn focused_download_song(&self) -> Option<DownloadCandidate> {
        match self.page {
            Page::Playlist => self.playlist_download_candidate(self.playlist.focused_idx),
            Page::Search => self.search_download_candidate(self.search.focused_idx),
            _ => None,
        }
    }

    /// 歌单页 / 专辑页某一行对应的下载候选（非单曲行返回 `None`）。
    fn playlist_download_candidate(&self, index: usize) -> Option<DownloadCandidate> {
        let track = self.playlist.tracks.get(index)?;
        if track.kind != PlaylistTrackKind::Song {
            return None;
        }
        Some(DownloadCandidate {
            song_id: track.id.clone()?,
            title: track.title.clone(),
            artist: track.artist.clone(),
            album: track.album.clone(),
        })
    }

    /// 搜索页某一行对应的下载候选（仅单曲行）。
    fn search_download_candidate(&self, index: usize) -> Option<DownloadCandidate> {
        let item = self.search.results.get(index)?;
        if item.kind != SearchItemKind::Song {
            return None;
        }
        Some(DownloadCandidate {
            song_id: item.song_id.clone()?,
            title: item
                .title
                .clone()
                .unwrap_or_else(|| item.left_label.clone()),
            artist: item.artist.clone().unwrap_or_default(),
            album: item.album.clone().unwrap_or_default(),
        })
    }

    /// 下载图标的动画相位（time-based：空闲节流下也按真实时间推进）。
    pub fn download_spinner_phase(&self) -> Duration {
        self.download_spinner_start.elapsed()
    }

    /// 歌单页 / 专辑页的行内图标：每帧调一次；列表代与任务版本都不变时几乎零成本。
    pub(crate) fn refresh_playlist_downloads(&mut self) {
        let epoch = self.playlist.generation() ^ self.download_rows_epoch;
        let root = self.download_root.clone();
        let tracks = &self.playlist.tracks;
        self.playlist_download_cache
            .refresh(epoch, &mut self.download_manager, || {
                playlist_download_rows(tracks, root.as_deref())
            });
    }

    /// 歌单页 / 专辑页某行的图标状态（先调 `refresh_playlist_downloads`）。
    pub(crate) fn playlist_download_state_at(&self, index: usize) -> Option<DownloadState> {
        self.playlist_download_cache.state_at(index)
    }

    /// 搜索页的行内图标：每帧调一次。
    pub(crate) fn refresh_search_downloads(&mut self) {
        let epoch = self.search.generation() ^ self.download_rows_epoch;
        let root = self.download_root.clone();
        let results = &self.search.results;
        self.search_download_cache
            .refresh(epoch, &mut self.download_manager, || {
                search_download_rows(results, root.as_deref())
            });
    }

    /// 搜索页某行的图标状态（先调 `refresh_search_downloads`）。
    pub(crate) fn search_download_state_at(&self, index: usize) -> Option<DownloadState> {
        self.search_download_cache.state_at(index)
    }

    fn download_state_for_candidate(
        &mut self,
        candidate: &DownloadCandidate,
    ) -> Option<DownloadState> {
        let root = self.download_root.clone()?;
        let target = DownloadTarget {
            dir: root,
            base: crate::app::download::download_file_stem(
                &candidate.title,
                &candidate.artist,
                &candidate.album,
            ),
        };
        Some(self.download_manager.state_of(&candidate.song_id, &target))
    }

    /// 主应用 Ctrl+Alt+D：下载聚焦的单曲（在途则取消）。
    async fn download_focused_song(&mut self) {
        let Some(candidate) = self.focused_download_song() else {
            self.set_runtime_status(self.lang_text(
                "当前列表里没有可下载的单曲",
                "No downloadable song in this list",
            ));
            return;
        };
        self.toggle_download(candidate);
    }

    /// 全屏页 Ctrl+D / 点下载图标：下载当前播放的单曲（在途则取消）。
    pub fn download_current_song(&mut self) {
        let Some(track) = self.now_playing.clone() else {
            self.set_runtime_status(
                self.lang_text("当前没有正在播放的歌曲", "Nothing is playing right now"),
            );
            return;
        };

        self.toggle_download(DownloadCandidate {
            song_id: track.song_id,
            title: track.title,
            artist: track.artist,
            album: track.album,
        });
    }

    /// 发起 / 取消下载（图标点击、两处快捷键共用这一条路径）。
    fn toggle_download(&mut self, candidate: DownloadCandidate) {
        if self.download_manager.is_downloading(&candidate.song_id) {
            self.download_manager.cancel(&candidate.song_id);
            self.set_runtime_status(format!(
                "{}: {}",
                self.lang_text("已取消下载", "Download cancelled"),
                candidate.title
            ));
            return;
        }

        if self.download_manager.is_busy(&candidate.song_id) {
            self.set_runtime_status(self.lang_text(
                "正在取消上一任务，请稍候",
                "Cancelling the previous download, please wait",
            ));
            return;
        }

        let Some(root) = self.download_root.clone() else {
            self.set_runtime_status(self.lang_text(
                "下载不可用：没有可用的下载目录",
                "Download unavailable: no usable download directory",
            ));
            return;
        };

        let target = DownloadTarget {
            dir: root,
            base: crate::app::download::download_file_stem(
                &candidate.title,
                &candidate.artist,
                &candidate.album,
            ),
        };
        let level = self
            .config
            .download_audio_quality
            .clamp_for_vip(self.vip_audio_unlocked);
        let request = DownloadRequest {
            song_id: candidate.song_id.clone(),
            level,
            title: candidate.title.clone(),
            artist: candidate.artist.clone(),
            album: candidate.album.clone(),
            target,
        };

        let queued = self.download_manager.is_active();
        match self.download_manager.enqueue(&self.api, request) {
            Ok(()) => {
                let status = if queued {
                    format!(
                        "{}: {} ({})",
                        self.lang_text("已加入下载队列", "Queued for download"),
                        candidate.title,
                        level.as_api_level()
                    )
                } else {
                    format!(
                        "{}: {} ({})",
                        self.lang_text("开始下载", "Downloading"),
                        candidate.title,
                        level.as_api_level()
                    )
                };
                self.set_runtime_status(status);
            }
            Err(message) => self.set_runtime_status(message),
        }
    }

    /// 每帧搬运下载结果：完成 / 失败 / 取消都写状态行。
    fn tick_download(&mut self) {
        self.refresh_current_download_state();
        for event in self.download_manager.poll() {
            match event {
                DownloadEvent::Started { title, level } => {
                    self.set_runtime_status(format!(
                        "{}: {title} ({})",
                        self.lang_text("开始下载", "Downloading"),
                        level.as_api_level()
                    ));
                }
                DownloadEvent::Finished {
                    title,
                    path,
                    level,
                    file_type,
                    tag_error,
                } => {
                    let mut text = format!(
                        "{}: {} [{}.{}] -> {}",
                        self.lang_text("已下载", "Downloaded"),
                        title,
                        level,
                        file_type,
                        path.display()
                    );
                    if let Some(error) = tag_error {
                        text.push_str(&format!(
                            " ({}{error})",
                            self.lang_text("标签写入失败: ", "tag write failed: ")
                        ));
                    }
                    self.set_runtime_status(text);
                }
                DownloadEvent::Failed { title, error } => {
                    self.set_runtime_status(format!(
                        "{}: {title}: {error}",
                        self.lang_text("下载失败", "Download failed")
                    ));
                }
                DownloadEvent::Cancelled { title } => {
                    self.set_runtime_status(format!(
                        "{}: {title}",
                        self.lang_text("已取消下载", "Download cancelled")
                    ));
                }
            }
        }
    }

    /// 把某首歌的显示值同步到状态机（未决意图优先，其次已确认值）。
    fn sync_like_display(&mut self, song_id: &str) {
        if self.current_song_id().as_deref() == Some(song_id) {
            self.now_playing_liked = self.like_machine.displayed(song_id);
        }
    }

    /// 收藏切换：立刻按期望值改界面（乐观更新），请求由 `tick_like_sync` 派发。
    fn toggle_like_hotkey(&mut self) {
        let Some(song_id) = self.current_song_id() else {
            self.set_runtime_status(self.lang_text(
                "当前没有可收藏的歌曲",
                "No song is available for like/unlike",
            ));
            return;
        };

        let target = !self.now_playing_liked;
        self.like_machine.set_intent(song_id, target);
        self.now_playing_liked = target;
    }

    /// 每帧收敛收藏状态：先搬在途结果，再按需补发请求。
    fn tick_like_sync(&mut self) {
        self.pump_like_toggle();
        self.pump_like_verify();
        self.dispatch_like_toggle();
    }

    fn pump_like_toggle(&mut self) {
        let Some(pending) = self.like_machine.toggle.as_ref() else {
            return;
        };
        let Some(result) = peek_shared(&pending.fut).cloned() else {
            return;
        };

        let song_id = pending.song_id.clone();
        let target = pending.target;
        self.like_machine.toggle = None;

        match self.like_machine.on_toggle_result(&song_id, target, result) {
            ToggleOutcome::Settled { liked } => {
                self.sync_like_display(&song_id);
                self.set_runtime_status(if liked {
                    self.lang_text("已收藏当前歌曲", "Liked current song")
                        .to_string()
                } else {
                    self.lang_text("已取消收藏当前歌曲", "Unliked current song")
                        .to_string()
                });
            }
            ToggleOutcome::Superseded => self.sync_like_display(&song_id),
            ToggleOutcome::Failed { message } => {
                // 意图已被状态机放弃，显示回滚到已确认值。
                self.sync_like_display(&song_id);
                self.set_runtime_status(format!(
                    "{}: {}",
                    self.lang_text("收藏操作失败", "Like operation failed"),
                    message
                ));
            }
            ToggleOutcome::StaleFailure => {}
        }
    }

    fn pump_like_verify(&mut self) {
        let Some(pending) = self.like_machine.verify.as_ref() else {
            return;
        };
        let Some(result) = peek_shared(&pending.fut).cloned() else {
            return;
        };

        let song_id = pending.song_id.clone();
        self.like_machine.verify = None;

        if self.like_machine.on_verify_result(&song_id, result) {
            self.sync_like_display(&song_id);
        }
    }

    fn dispatch_like_toggle(&mut self) {
        if let Some(song_id) = self.like_machine.drop_satisfied_intent() {
            self.sync_like_display(&song_id);
        }

        let Some((song_id, target)) = self.like_machine.pending_dispatch() else {
            return;
        };

        let fut = like_song_request(self.api.clone(), song_id.clone(), target);
        let fut: LikeToggleTask = Box::pin(async move { Some(fut.await) });
        self.like_machine
            .begin_toggle(song_id, target, shot_and_share(fut));
    }

    async fn tick_audio(&mut self) {
        let runtime = map_audio_state(self.audio_player.state());

        if self.playback_state == PlaybackRuntimeState::Playing
            && runtime == PlaybackRuntimeState::Stopped
        {
            self.play_next_after_finish().await;
            return;
        }

        self.playback_state = runtime;
    }

    async fn play_next_after_finish(&mut self) {
        if self.playback_queue.is_empty() {
            self.playback_state = PlaybackRuntimeState::Stopped;
            return;
        }

        let current = self
            .playback_index
            .unwrap_or(0)
            .min(self.playback_queue.len() - 1);

        // 私人漫游：队列（快照）播完后，若列表已追加新歌则从列表继续顺序播放
        if self.playback_repeat_mode == PlaybackRepeatMode::Sequence
            && self.playback_queue_is_roam()
            && current + 1 >= self.playback_queue.len()
            && self.private_roam.tracks.len() > self.playback_queue.len()
        {
            let start = self.private_roam.last_played_index.unwrap_or(0) + 1;
            if start < self.private_roam.tracks.len() {
                let queue: Vec<PlaybackTrack> = self.private_roam.tracks[start..]
                    .iter()
                    .filter_map(PlaybackTrack::from_playlist_track)
                    .collect();
                // 来源仍是漫游本身，封面沿用漫游当前封面（跟随播放歌曲）。
                let source_cover = self.private_roam.cover_url.clone();
                self.replace_queue_and_play(queue, 0, source_cover).await;
                return;
            }
        }

        let target = match self.playback_repeat_mode {
            PlaybackRepeatMode::Sequence => {
                if current + 1 < self.playback_queue.len() {
                    Some(current + 1)
                } else {
                    None
                }
            }
            PlaybackRepeatMode::LoopAll => Some((current + 1) % self.playback_queue.len()),
            PlaybackRepeatMode::LoopOne => Some(current),
            PlaybackRepeatMode::Shuffle => {
                Some(pick_shuffle_index(self.playback_queue.len(), current))
            }
        };

        if let Some(index) = target {
            self.play_queue_index(index, false).await;
        } else {
            self.playback_state = PlaybackRuntimeState::Stopped;
            self.set_runtime_status(self.lang_text("播放结束", "Playback finished"));
        }
    }

    async fn play_queue_index(&mut self, index: usize, announce: bool) {
        let Some(track) = self.playback_queue.get(index).cloned() else {
            return;
        };

        // 记录私人漫游播放位置/封面；播放到列表末尾时追加新歌
        self.track_private_roam_playback(&track).await;

        let mut enriched = track.clone();
        // Switch UI state immediately and avoid blocking network fetches here.
        self.enrich_track_metadata(&mut enriched, false).await;
        if let Some(slot) = self.playback_queue.get_mut(index) {
            slot.cover = enriched.cover.clone();
        }
        self.trim_non_current_cover_memory(index);
        self.now_playing = Some(enriched.clone());
        self.refresh_now_playing_like_state();
        self.playback_index = Some(index);
        self.cover_fetch_inflight_url = None;
        self.cover_fetch_last_attempt_at = None;
        self.maybe_schedule_now_playing_cover_fetch();
        self.lyric_fetch_inflight_song_id = None;
        self.lyric_fetch_last_attempt_at = None;
        self.maybe_schedule_now_playing_lyric_fetch();
        self.persist_playback_memory();

        let quality = self.config.audio_quality.as_api_level();
        let fail = |err, app: &mut Self| {
            app.now_playing_liked = false;
            app.playback_state = PlaybackRuntimeState::Stopped;
            app.set_runtime_status(format!(
                "{}: {err}",
                app.lang_text("播放失败", "Playback failed"),
            ));
        };
        let ok = |app: &mut Self| {
            app.playback_state = PlaybackRuntimeState::Playing;
            if announce {
                app.set_runtime_status(format!(
                    "{}: {} - {}",
                    app.lang_text("正在播放", "Now Playing"),
                    enriched.title,
                    enriched.artist
                ));
            };
        };

        let id = &track.song_id;
        let path = self.audio_player.cached_song_path(id, quality);

        if is_nonempty_file(&path) {
            return match self.audio_player.play_from_file(&path) {
                Ok(_) => ok(self),
                Err(err) => fail(err, self),
            };
        }

        // Song not cached - start streaming playback while prefetching in background.
        self.audio_player.stop();
        self.set_runtime_status(format!(
            "{}: {} - {}",
            self.lang_text("正在缓冲", "Buffering"),
            enriched.title,
            enriched.artist
        ));

        match self.api.song_stream_url_with_quality(id, quality).await {
            Ok(url) => {
                let (progress_tx, progress_rx) = see::sync::channel((0, 0));
                match StreamingReader::new(
                    self.api.http_client(),
                    &url,
                    path.clone(),
                    self.api.session_cookie(),
                    progress_tx,
                )
                .await
                {
                    Ok(reader) => match self.audio_player.play_streaming(reader, progress_rx).await
                    {
                        Ok(()) => ok(self),
                        Err(err) => fail(err, self),
                    },
                    Err(err) => fail(err, self),
                }
            }
            Err(err) => fail(err, self),
        }
    }

    fn cover_cache_path_for_url(&self, url: &str) -> Option<PathBuf> {
        let key = url.trim();
        if key.is_empty() {
            return None;
        }

        let mut hasher = DefaultHasher::new();
        key.hash(&mut hasher);
        let hash = hasher.finish();
        Some(self.cover_cache_dir.join(format!("{hash:016x}.img")))
    }

    fn load_cover_from_disk_cache(&self, url: &str) -> Option<Vec<u8>> {
        let path = self.cover_cache_path_for_url(url)?;
        let bytes = fs::read(path).ok()?;
        if bytes.is_empty() {
            return None;
        }
        Some(bytes)
    }

    fn persist_cover_to_disk_cache(&self, url: &str, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }

        let Some(path) = self.cover_cache_path_for_url(url) else {
            return;
        };

        let _ = fs::create_dir_all(&self.cover_cache_dir);
        let _ = fs::write(path, bytes);
    }

    async fn fetch_cover_with_disk_cache(&self, url: &str) -> Option<Vec<u8>> {
        if let Some(bytes) = self.load_cover_from_disk_cache(url) {
            return Some(bytes);
        }

        let bytes = self.api.fetch_cover_bytes(url).await.ok()?;
        if bytes.is_empty() {
            return None;
        }
        self.persist_cover_to_disk_cache(url, &bytes);
        Some(bytes)
    }

    fn apply_cover_fetch_result(&mut self, result: CoverFetchResult) {
        if self.cover_fetch_inflight_url.as_deref() == Some(result.url.as_str()) {
            self.cover_fetch_inflight_url = None;
        }

        let Some(now) = self.now_playing.as_ref() else {
            return;
        };
        if now.song_id != result.song_id {
            return;
        }

        if now.cover.is_some() {
            return;
        }

        let Some(bytes) = result.bytes else {
            return;
        };

        self.persist_cover_to_disk_cache(&result.url, &bytes);
        if let Some(now_mut) = self.now_playing.as_mut() {
            now_mut.cover = Some(bytes.clone());
        }

        if let Some(index) = self.playback_index {
            if let Some(slot) = self.playback_queue.get_mut(index) {
                slot.cover = Some(bytes);
            }
        }
    }

    fn maybe_schedule_now_playing_cover_fetch(&mut self) {
        let (song_id, url) = match self.now_playing.as_ref() {
            Some(now) if now.cover.is_none() => {
                let Some(url) = now.cover_url.clone() else {
                    return;
                };
                (now.song_id.clone(), url)
            }
            Some(_) => {
                self.cover_fetch_inflight_url = None;
                return;
            }
            None => {
                return;
            }
        };

        if let Some(bytes) = self.load_cover_from_disk_cache(&url) {
            if let Some(now_mut) = self.now_playing.as_mut() {
                now_mut.cover = Some(bytes.clone());
            }
            if let Some(index) = self.playback_index {
                if let Some(slot) = self.playback_queue.get_mut(index) {
                    slot.cover = Some(bytes);
                }
            }
            self.cover_fetch_inflight_url = None;
            return;
        }

        if self.cover_fetch_inflight_url.as_deref() == Some(url.as_str()) {
            return;
        }

        let now_at = Instant::now();
        if let Some(last) = self.cover_fetch_last_attempt_at {
            if now_at.duration_since(last) < Duration::from_millis(COVER_FETCH_RETRY_MS) {
                return;
            }
        }

        let req = CoverFetchRequest {
            song_id,
            url: url.trim().into(),
        };
        if self.cover_fetch_tx.start_send(req).is_ok() {
            self.cover_fetch_inflight_url = Some(url);
            self.cover_fetch_last_attempt_at = Some(now_at);
        }
    }

    fn tick_cover_fetch(&mut self) {
        let needs_schedule = self
            .now_playing
            .as_ref()
            .map(|now| now.cover.is_none() && now.cover_url.is_some())
            .unwrap_or(false);
        if self.cover_fetch_inflight_url.is_none() && !needs_schedule {
            return;
        }

        loop {
            match self.cover_fetch_rx.try_recv() {
                Ok(result) => self.apply_cover_fetch_result(result),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => break,
            }
        }

        self.maybe_schedule_now_playing_cover_fetch();
    }

    fn apply_lyric_fetch_result(&mut self, result: LyricFetchResult) {
        if self.lyric_fetch_inflight_song_id.as_deref() == Some(result.song_id.as_str()) {
            self.lyric_fetch_inflight_song_id = None;
        }

        let Some(now) = self.now_playing.as_ref() else {
            return;
        };
        if now.song_id != result.song_id {
            return;
        }
        if now.lyrics.is_some() {
            return;
        }

        let Some(lyrics) = result.lyrics else {
            return;
        };

        if let Some(now_mut) = self.now_playing.as_mut() {
            now_mut.lyrics = Some(lyrics.clone());
        }
        if let Some(index) = self.playback_index {
            if let Some(slot) = self.playback_queue.get_mut(index) {
                slot.lyrics = Some(lyrics);
            }
        }
    }

    fn maybe_schedule_now_playing_lyric_fetch(&mut self) {
        let song_id = match self.now_playing.as_ref() {
            Some(now) if now.lyrics.is_none() => now.song_id.clone(),
            Some(_) => {
                self.lyric_fetch_inflight_song_id = None;
                return;
            }
            None => {
                return;
            }
        };

        if self.lyric_fetch_inflight_song_id.as_deref() == Some(song_id.as_str()) {
            return;
        }

        let now_at = Instant::now();
        if let Some(last) = self.lyric_fetch_last_attempt_at {
            if now_at.duration_since(last) < Duration::from_millis(LYRICS_FETCH_RETRY_MS) {
                return;
            }
        }

        let req = LyricFetchRequest {
            song_id: song_id.clone(),
            cookie: self
                .api
                .session_cookie()
                .map(|value| value.to_string())
                .or_else(|| self.session_cookie.clone()),
        };
        if self.lyric_fetch_tx.start_send(req).is_ok() {
            self.lyric_fetch_inflight_song_id = Some(song_id);
            self.lyric_fetch_last_attempt_at = Some(now_at);
        }
    }

    fn tick_lyric_fetch(&mut self) {
        let needs_schedule = self
            .now_playing
            .as_ref()
            .map(|now| now.lyrics.is_none())
            .unwrap_or(false);
        if self.lyric_fetch_inflight_song_id.is_none() && !needs_schedule {
            return;
        }

        loop {
            match self.lyric_fetch_rx.try_recv() {
                Ok(result) => self.apply_lyric_fetch_result(result),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => break,
            }
        }

        self.maybe_schedule_now_playing_lyric_fetch();
    }

    fn trim_non_current_cover_memory(&mut self, current_index: usize) {
        let cover_cache_dir = self.cover_cache_dir.clone();
        for (idx, track) in self.playback_queue.iter_mut().enumerate() {
            if idx == current_index {
                continue;
            }

            if let (Some(bytes), Some(url)) = (track.cover.as_deref(), track.cover_url.as_deref()) {
                let mut hasher = DefaultHasher::new();
                url.hash(&mut hasher);
                let hash = hasher.finish();
                let path = cover_cache_dir.join(format!("{hash:016x}.img"));
                let _ = fs::create_dir_all(&cover_cache_dir);
                let _ = fs::write(path, bytes);
            }
            track.cover = None;
        }
    }

    async fn enrich_track_metadata(&mut self, track: &mut PlaybackTrack, allow_network: bool) {
        if track.cover.is_none() {
            if let Some(url) = track.cover_url.as_deref() {
                let bytes = if allow_network {
                    self.fetch_cover_with_disk_cache(url).await
                } else {
                    self.load_cover_from_disk_cache(url)
                };
                if let Some(bytes) = bytes {
                    track.cover = Some(bytes);
                }
            }
        }

        if allow_network && track.cover.is_none() {
            if let Ok(detail) = self.api.song_detail(&track.song_id).await {
                if let Some(song) = detail
                    .body
                    .get("songs")
                    .and_then(|value| value.as_array())
                    .and_then(|items| items.first())
                {
                    if let Some(cover_url) =
                        song.pointer("/al/picUrl").and_then(|value| value.as_str())
                    {
                        if let Some(bytes) = self.fetch_cover_with_disk_cache(cover_url).await {
                            track.cover = Some(bytes);
                        }
                    }
                }
            }
        }

        if allow_network && track.lyrics.is_none() {
            if let Ok(lyric) = self.api.lyric(&track.song_id).await {
                if let Some(raw_lrc) = lyric
                    .body
                    .pointer("/lrc/lyric")
                    .and_then(|value| value.as_str())
                {
                    track.lyrics =
                        crate::tmplayer::playback::metadata::parse_lrc(raw_lrc).or_else(|| {
                            crate::tmplayer::playback::metadata::parse_plain_lyrics(raw_lrc)
                        });
                }
            }
        }
    }

    async fn replace_queue_and_play(
        &mut self,
        queue: Vec<PlaybackTrack>,
        index: usize,
        source_cover_url: Option<String>,
    ) {
        if queue.is_empty() {
            self.set_runtime_status(
                self.lang_text("当前页面没有可播放歌曲", "No playable songs on this page"),
            );
            return;
        }

        self.playback_queue = queue;
        // 在换队列的此刻记下来源封面，之后浏览别的页面不会影响它。
        self.playback_queue_cover_url = source_cover_url;
        self.playback_queue_source_id = self.playlist.id.clone();
        let target = index.min(self.playback_queue.len() - 1);
        self.play_queue_index(target, true).await;
    }

    fn build_queue_from_playlist(&self) -> (Vec<PlaybackTrack>, usize) {
        let focused = self.playlist.focused_idx;
        let mut queue = Vec::new();
        let mut mapped_focus = None;

        for (idx, track) in self.playlist.tracks.iter().enumerate() {
            if let Some(item) = PlaybackTrack::from_playlist_track(track) {
                if idx == focused {
                    mapped_focus = Some(queue.len());
                }
                queue.push(item);
            }
        }

        let target = mapped_focus.unwrap_or(0);
        (queue, target)
    }

    fn build_queue_from_search(&self) -> (Vec<PlaybackTrack>, usize) {
        let focused = self.search.focused_idx;
        let mut queue = Vec::new();
        let mut mapped_focus = None;

        for (idx, item) in self.search.results.iter().enumerate() {
            if let Some(track) = PlaybackTrack::from_search_item(item) {
                if idx == focused {
                    mapped_focus = Some(queue.len());
                }
                queue.push(track);
            }
        }

        let target = mapped_focus.unwrap_or(0);
        (queue, target)
    }

    async fn play_focused_playlist_track(&mut self) {
        let Some(track) = self.playlist.tracks.get(self.playlist.focused_idx) else {
            return;
        };

        match track.kind {
            PlaylistTrackKind::Song => {
                let (queue, target) = self.build_queue_from_playlist();
                let source_cover = self.playlist.cover.url.clone();
                self.replace_queue_and_play(queue, target, source_cover)
                    .await;
            }
            PlaylistTrackKind::Album | PlaylistTrackKind::Ep | PlaylistTrackKind::Single => {
                self.open_focused_playlist_album().await;
            }
        }
    }

    async fn play_focused_search_track(&mut self) {
        let (queue, target) = self.build_queue_from_search();
        // 搜索结果没有"所属列表"，交给首歌封面兜底。
        self.replace_queue_and_play(queue, target, None).await;
    }

    async fn play_focused_author_tile(&mut self) {
        let Some(item) = self.author.tiles.get(self.author.focused_idx) else {
            return;
        };

        let (section_title, tracks, section_cover) = match item.kind {
            AuthorTileKind::HotSong => (
                self.lang_text("热门歌曲", "Hot Songs").to_string(),
                self.author.hot_songs.clone(),
                self.author
                    .hot_songs
                    .first()
                    .and_then(|track| track.cover_url.clone())
                    .or_else(|| self.author.cover.url.clone()),
            ),
            AuthorTileKind::Album => (
                self.lang_text("专辑", "Albums").to_string(),
                self.author.albums.clone(),
                self.author
                    .albums
                    .first()
                    .and_then(|track| track.cover_url.clone())
                    .or_else(|| self.author.cover.url.clone()),
            ),
            AuthorTileKind::Ep => (
                "EP".to_string(),
                self.author.eps.clone(),
                self.author
                    .eps
                    .first()
                    .and_then(|track| track.cover_url.clone())
                    .or_else(|| self.author.cover.url.clone()),
            ),
            AuthorTileKind::Single => (
                "Single".to_string(),
                self.author.singles.clone(),
                self.author
                    .singles
                    .first()
                    .and_then(|track| track.cover_url.clone())
                    .or_else(|| self.author.cover.url.clone()),
            ),
        };

        if tracks.is_empty() {
            self.set_runtime_status(self.lang_text(
                "当前分类暂无可用内容",
                "This section has no available items",
            ));
            return;
        }

        self.playlist_return_page = Page::Author;
        self.playlist_section_return_snapshot = None;
        // 这一页换成作者页分区：在途的占位拉取作废（同 `apply_playlist_fetch`）。
        self.playlist_fetch = None;
        self.playlist.id = self
            .author
            .id
            .as_ref()
            .map(|id| format!("artist:{}:{}", id, section_title));
        self.playlist.title = format!("{} · {}", self.author.title, section_title);
        self.playlist.artist = self.author.title.clone();
        self.playlist.description = self
            .lang_text(
                "按 Enter 进入专辑或播放歌曲，Esc 返回作者页",
                "Press Enter to open album or play song, Esc to return",
            )
            .to_string();
        self.playlist.set_tracks(tracks);
        section_cover.map(|x| self.playlist.cover.load(self.api.clone(), x));
        self.page = Page::Playlist;
    }

    async fn open_focused_playlist_album(&mut self) {
        let (album_id, title, fallback_cover_url, track_kind) = {
            let Some(track) = self.playlist.tracks.get(self.playlist.focused_idx) else {
                return;
            };

            let Some(album_id) = track.id.clone() else {
                self.set_runtime_status(self.lang_text(
                    "当前条目缺少专辑 ID，无法打开",
                    "The current item has no album ID",
                ));
                return;
            };

            (
                album_id,
                track.title.clone(),
                track.cover_url.clone(),
                track.kind,
            )
        };

        let is_author_section_album = self.playlist_return_page == Page::Author
            && matches!(
                track_kind,
                PlaylistTrackKind::Album | PlaylistTrackKind::Ep | PlaylistTrackKind::Single
            );
        let section_snapshot = if is_author_section_album {
            Some(self.playlist.clone())
        } else {
            None
        };

        match self.load_album_detail(&album_id).await {
            Ok(()) => {
                self.playlist_section_return_snapshot = section_snapshot;
                match (&self.playlist.cover.image, fallback_cover_url) {
                    (None, Some(url)) => self.playlist.cover.load(self.api.clone(), url),
                    _ => (),
                }
                self.set_runtime_status(format!(
                    "{} {}",
                    self.lang_text("已打开专辑", "Opened album"),
                    title
                ));
            }
            Err(err) => {
                self.set_runtime_status(format!(
                    "{}: {}",
                    self.lang_text("打开专辑失败", "Failed to open album"),
                    err
                ));
            }
        }
    }

    /// 搜索页打开作者：立即落占位作者页 + 派发后台拉取（结果由 `tick_author_fetch` 搬进来）。
    fn open_focused_search_author(&mut self) {
        let (artist_id, title, fallback_cover_url) = {
            let Some(item) = self.search.results.get(self.search.focused_idx) else {
                return;
            };

            let Some(artist_id) = item.artist_id.clone() else {
                self.search.status_line = self
                    .lang_text(
                        "当前结果缺少作者 ID，无法打开作者页",
                        "The current result has no author ID",
                    )
                    .to_string();
                return;
            };

            (artist_id, item.left_label.clone(), item.cover_url.clone())
        };

        self.author = AuthorState::placeholder(
            title,
            self.lang_text("正在加载作者…", "Loading artist…")
                .to_string(),
        );
        self.playlist_section_return_snapshot = None;
        self.author_return_page = Page::Search;
        self.page = Page::Author;

        let fut = fetch_author_page_by_id(
            self.api.clone(),
            self.config.language,
            artist_id,
            fallback_cover_url,
        );
        let fut: AuthorFetchTask = Box::pin(async move { Some(fut.await) });
        self.author_fetch = Some(shot_and_share(fut));
    }

    /// 搜索页打开专辑：立即落占位歌单页 + 派发后台拉取（结果由 `tick_playlist_fetch` 搬进来）。
    fn open_focused_search_album(&mut self) {
        let (album_id, title, fallback_cover_url) = {
            let Some(item) = self.search.results.get(self.search.focused_idx) else {
                return;
            };

            let Some(album_id) = item.album_id.clone() else {
                self.search.status_line = self
                    .lang_text(
                        "当前结果缺少专辑 ID，无法打开专辑页",
                        "The current result has no album ID",
                    )
                    .to_string();
                return;
            };

            (
                album_id,
                item.title
                    .clone()
                    .unwrap_or_else(|| item.left_label.clone()),
                item.cover_url.clone(),
            )
        };

        self.playlist = PlaylistState::placeholder(
            title,
            self.lang_text("正在加载专辑…", "Loading album…")
                .to_string(),
        );
        self.playlist_section_return_snapshot = None;
        self.playlist_return_page = Page::Search;
        self.page = Page::Playlist;

        let fut = fetch_album_page(
            self.api.clone(),
            self.config.language,
            album_id,
            fallback_cover_url,
        );
        let fut: PlaylistFetchTask = Box::pin(async move { Some(fut.await) });
        self.playlist_page_kind = PlaylistPageKind::Album;
        self.playlist_fetch = Some(PlaylistFetchSlot {
            kind: PlaylistPageKind::Album,
            future: shot_and_share(fut),
        });
    }

    /// 搜索页打开歌单：立即落占位歌单页 + 派发后台拉取（结果由 `tick_playlist_fetch` 搬进来）。
    fn open_focused_search_playlist(&mut self) {
        let (playlist_id, title, fallback_cover_url) = {
            let Some(item) = self.search.results.get(self.search.focused_idx) else {
                return;
            };

            let Some(playlist_id) = item.playlist_id.clone() else {
                self.search.status_line = self
                    .lang_text(
                        "当前结果缺少歌单 ID，无法打开歌单页",
                        "The current result has no playlist ID",
                    )
                    .to_string();
                return;
            };

            (playlist_id, item.left_label.clone(), item.cover_url.clone())
        };

        self.playlist = PlaylistState::placeholder(
            title,
            self.lang_text("正在加载歌单…", "Loading playlist…")
                .to_string(),
        );
        self.playlist_section_return_snapshot = None;
        self.playlist_return_page = Page::Search;
        self.page = Page::Playlist;

        let fut = fetch_playlist_page(
            self.api.clone(),
            self.config.language,
            playlist_id,
            fallback_cover_url,
            self.home_sidebar.liked_playlist_id.clone(),
            self.home_sidebar.user_id.clone(),
        );
        let fut: PlaylistFetchTask = Box::pin(async move { Some(fut.await) });
        self.playlist_page_kind = PlaylistPageKind::Playlist;
        self.playlist_fetch = Some(PlaylistFetchSlot {
            kind: PlaylistPageKind::Playlist,
            future: shot_and_share(fut),
        });
    }

    async fn activate_focused_search_result(&mut self) {
        let Some(kind) = self
            .search
            .results
            .get(self.search.focused_idx)
            .map(|item| item.kind)
        else {
            return;
        };

        match kind {
            SearchItemKind::Song => self.play_focused_search_track().await,
            SearchItemKind::Album => self.open_focused_search_album(),
            SearchItemKind::Artist => self.open_focused_search_author(),
            SearchItemKind::Playlist => self.open_focused_search_playlist(),
        }
    }

    pub fn is_now_playing_song(&self, song_id: Option<&str>) -> bool {
        match (self.now_playing.as_ref(), song_id) {
            (Some(now), Some(song_id)) => now.song_id == song_id,
            _ => false,
        }
    }

    fn open_settings(&mut self) {
        self.settings_selected = 0;
        self.settings_keybind_rebinding = None;
        self.overlay = Some(Overlay::Settings);
    }

    fn open_keybind_settings(&mut self) {
        self.settings_keybind_selected = 0;
        self.settings_keybind_rebinding = None;
        self.overlay = Some(Overlay::SettingsKeybinds);
    }

    async fn handle_search_box_key(&mut self, key: KeyEvent) {
        if key.modifiers.contains(KeyModifiers::CONTROL)
            && matches!(key.code, KeyCode::Char('s') | KeyCode::Char('S'))
        {
            self.close_overlay();
            return;
        }

        match key.code {
            KeyCode::Esc => self.close_overlay(),
            KeyCode::Enter => self.execute_search_from_box().await,
            KeyCode::Backspace => {
                if self.search_box_cursor > 0 {
                    self.search_box_cursor =
                        remove_char_before(&mut self.search_box_input, self.search_box_cursor);
                }
            }
            KeyCode::Delete => {
                remove_char_at(&mut self.search_box_input, self.search_box_cursor);
            }
            KeyCode::Left => {
                if self.search_box_cursor > 0 {
                    self.search_box_cursor -= 1;
                }
            }
            KeyCode::Right => {
                let len = char_count(&self.search_box_input);
                if self.search_box_cursor < len {
                    self.search_box_cursor += 1;
                }
            }
            KeyCode::Home => {
                self.search_box_cursor = 0;
            }
            KeyCode::End => {
                self.search_box_cursor = char_count(&self.search_box_input);
            }
            KeyCode::Char(ch) => {
                if key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT {
                    if char_count(&self.search_box_input) < MAX_INPUT_LEN {
                        insert_char_at(&mut self.search_box_input, self.search_box_cursor, ch);
                        self.search_box_cursor += 1;
                    }
                }
            }
            _ => {}
        }

        self.search_box_cursor = self
            .search_box_cursor
            .min(char_count(&self.search_box_input));
    }

    fn handle_search_box_click(&mut self, col: u16, row: u16) {
        let Ok((term_w, term_h)) = crossterm::terminal::size() else {
            return;
        };
        if term_w < 20 || term_h < 2 {
            return;
        }

        let visible_h = self
            .search_box_anim_height
            .min(crate::ui::search_box::TARGET_HEIGHT)
            .min(term_h);
        if visible_h < crate::ui::search_box::TARGET_HEIGHT {
            return;
        }

        let width = (term_w / 2).max(24).min(term_w.saturating_sub(2));
        let area_x = term_w.saturating_sub(width) / 2;
        let area_y = 0_u16;
        let area = HitRect {
            x: area_x,
            y: area_y,
            width,
            height: visible_h,
        };

        if !area.contains(col, row) {
            return;
        }

        let inner_x = area.x.saturating_add(1);
        let inner_y = area.y.saturating_add(1);
        let inner_w = area.width.saturating_sub(2);
        if inner_w == 0 || row != inner_y {
            return;
        }

        if col <= inner_x {
            self.search_box_cursor = 0;
            return;
        }

        let max_col = inner_x.saturating_add(inner_w).saturating_sub(1);
        if col >= max_col {
            self.search_box_cursor = char_count(&self.search_box_input);
            return;
        }

        let rel = col.saturating_sub(inner_x);
        self.search_box_cursor = char_index_for_display_column(&self.search_box_input, rel);
    }

    /// 设置弹窗内的鼠标点击：单击聚焦该行，400ms 内再点同一行等同 Enter。
    ///
    /// 弹窗盖住整页，未命中行的点击由调用方直接丢弃（不穿透到底层页面）。
    async fn handle_settings_modal_click(&mut self, overlay: Overlay, col: u16, row: u16) {
        let Some((_, index)) = self
            .settings_item_hits
            .iter()
            .find(|(rect, _)| rect.contains(col, row))
            .copied()
        else {
            self.last_settings_click = None;
            return;
        };

        match overlay {
            Overlay::Settings => {
                self.settings_selected = index;
                if self.is_double_settings_click(overlay, index) {
                    self.activate_settings_root_item().await;
                }
            }
            Overlay::SettingsPlayback => {
                self.settings_playback_selected = index;
                if self.is_double_settings_click(overlay, index) {
                    self.apply_settings_playback_delta(1);
                }
            }
            Overlay::SettingsLyrics => {
                // 这三行都是开关：左键直接改值（不用双击）。
                self.settings_lyrics_selected = index;
                self.apply_settings_lyrics_delta(1);
            }
            Overlay::SettingsDownload => {
                // 与歌词浮窗同构：单击即执行（音质改值 / 路径进编辑 / 恢复默认两段式）。
                self.settings_download_selected = index;
                self.activate_settings_download_item();
            }
            Overlay::SettingsKeybinds => {
                self.settings_keybind_selected = index;
                if self.is_double_settings_click(overlay, index) {
                    self.begin_keybind_rebind(index);
                }
            }
            _ => {}
        }
    }

    /// 设置弹窗滚轮：上下移动选中行（与 Up/Down 同效）。
    fn scroll_settings_modal(&mut self, forward: bool) {
        let step = |selected: &mut usize, count: usize| {
            if count == 0 {
                return;
            }
            *selected = if forward {
                (*selected + 1) % count
            } else if *selected == 0 {
                count - 1
            } else {
                *selected - 1
            };
        };

        // 下载设置页的行在禁用态下不连续，单独走"可选中行"列表。
        if self.overlay == Some(Overlay::SettingsDownload) {
            self.move_download_selection(if forward { 1 } else { -1 });
            return;
        }

        match self.overlay {
            Some(Overlay::Settings) => step(&mut self.settings_selected, SETTINGS_ROOT_ITEMS),
            Some(Overlay::SettingsPlayback) => step(
                &mut self.settings_playback_selected,
                SETTINGS_PLAYBACK_ITEMS,
            ),
            Some(Overlay::SettingsLyrics) => {
                step(&mut self.settings_lyrics_selected, SETTINGS_LYRICS_ITEMS)
            }
            Some(Overlay::SettingsKeybinds) => {
                step(&mut self.settings_keybind_selected, SETTINGS_KEYBIND_ITEMS)
            }
            _ => {}
        }
    }

    fn is_double_settings_click(&mut self, overlay: Overlay, index: usize) -> bool {
        let now = Instant::now();
        let is_double = self
            .last_settings_click
            .map(|(at, o, i)| {
                o == overlay
                    && i == index
                    && now.duration_since(at) <= Duration::from_millis(CONTENT_DOUBLE_CLICK_MS)
            })
            .unwrap_or(false);
        self.last_settings_click = Some((now, overlay, index));
        is_double
    }

    /// 设置根页选中项的执行（键盘 Enter 与双击共用）。
    async fn activate_settings_root_item(&mut self) {
        match self.settings_selected {
            0..=3 => self.apply_settings_root_delta(1).await,
            4 => {
                self.settings_playback_selected = 0;
                self.overlay = Some(Overlay::SettingsPlayback);
            }
            5 => self.open_keybind_settings(),
            6 => {
                self.settings_lyrics_selected = 0;
                self.overlay = Some(Overlay::SettingsLyrics);
            }
            7..=9 => self.apply_settings_root_delta(1).await,
            10 => self.open_download_settings(),
            11 => self.logout_to_login().await,
            12 => {
                self.overlay = Some(Overlay::SettingsAbout);
            }
            _ => {}
        }
    }

    /// 开始重绑某条快捷键（键盘 Enter 与双击共用）。
    fn begin_keybind_rebind(&mut self, index: usize) {
        self.settings_keybind_rebinding = Some(index);
        self.set_runtime_status(format!(
            "{} [{}]，{}",
            self.lang_text("正在重绑", "Rebinding"),
            self.keybind_name_for_index(index),
            self.lang_text(
                "请按新快捷键（Esc 取消）",
                "press a new shortcut (Esc to cancel)"
            )
        ));
    }

    /// “歌词浮窗”子页：三行开关。吸附行只在拖动开启时可改（关闭时灰置）。
    fn apply_settings_lyrics_delta(&mut self, delta: i32) {
        if delta == 0 {
            return;
        }

        match self.settings_lyrics_selected {
            0 => {
                self.config.page_lyrics = !self.config.page_lyrics;
                let _ = self.config.save();
            }
            1 => {
                self.config.page_lyrics_drag = !self.config.page_lyrics_drag;
                let _ = self.config.save();
            }
            2 => {
                // 拖动关闭时吸附无意义：灰置且不可改。
                if !self.config.page_lyrics_drag {
                    return;
                }
                self.config.page_lyrics_snap = !self.config.page_lyrics_snap;
                let _ = self.config.save();
            }
            _ => {}
        }
    }

    fn handle_settings_lyrics_key(&mut self, key: KeyEvent) {
        match key.code {
            // 三行都是开关（值行）：与播放设置页同构，Left/Right 都用来改值，
            // 返回上一级只走 Esc。
            KeyCode::Esc => self.overlay = Some(Overlay::Settings),
            KeyCode::Char('t') | KeyCode::Char('T') => {
                if key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT {
                    self.close_overlay();
                }
            }
            KeyCode::Left => self.apply_settings_lyrics_delta(-1),
            KeyCode::Right | KeyCode::Enter => self.apply_settings_lyrics_delta(1),
            KeyCode::Up | KeyCode::BackTab => {
                if self.settings_lyrics_selected == 0 {
                    self.settings_lyrics_selected = SETTINGS_LYRICS_ITEMS - 1;
                } else {
                    self.settings_lyrics_selected -= 1;
                }
            }
            KeyCode::Down | KeyCode::Tab => {
                self.settings_lyrics_selected =
                    (self.settings_lyrics_selected + 1) % SETTINGS_LYRICS_ITEMS;
            }
            _ => {}
        }
    }

    /// 打开「下载设置」子页：光标落在第一个可选中行（禁用态下就是路径行）。
    fn open_download_settings(&mut self) {
        self.download_path_edit = None;
        self.download_reset_armed = false;
        self.settings_download_selected = self
            .download_selectable_rows()
            .first()
            .copied()
            .unwrap_or(1);
        self.overlay = Some(Overlay::SettingsDownload);
    }

    /// 下载目录可用（系统里能找到可写位置）。不可用时除路径行外全部灰置。
    pub fn download_settings_enabled(&self) -> bool {
        self.download_root.is_some()
    }

    /// 某一行是否可选中：下载不可用时只有「音质」灰置——路径行是自救入口，
    /// 「恢复默认」是把显式 `Null` / 无家目录状态拉回来的出口，两者都要能选。
    pub fn download_row_selectable(&self, row: usize) -> bool {
        self.download_settings_enabled() || row != 0
    }

    fn download_selectable_rows(&self) -> Vec<usize> {
        (0..SETTINGS_DOWNLOAD_ITEMS)
            .filter(|row| self.download_row_selectable(*row))
            .collect()
    }

    fn move_download_selection(&mut self, delta: i32) {
        let rows = self.download_selectable_rows();
        if rows.is_empty() || delta == 0 {
            return;
        }
        let current = rows
            .iter()
            .position(|row| *row == self.settings_download_selected)
            .unwrap_or(0) as i32;
        let next = (current + delta).rem_euclid(rows.len() as i32) as usize;
        self.settings_download_selected = rows[next];
        // 换行即撤下待确认态：恢复默认必须连着选两次同一个地方。
        self.download_reset_armed = false;
    }

    /// 下载设置页的「执行」：Enter、双击与单击共用。
    fn activate_settings_download_item(&mut self) {
        match self.settings_download_selected {
            0 => self.apply_settings_download_delta(1),
            1 => self.begin_download_path_edit(),
            2 => self.activate_download_reset(),
            _ => {}
        }
    }

    /// 音质行：与播放设置同一套可选值（按会员放开）。
    fn apply_settings_download_delta(&mut self, delta: i32) {
        if delta == 0 || self.settings_download_selected != 0 || !self.download_settings_enabled() {
            return;
        }
        let next = self
            .config
            .download_audio_quality
            .cycle(delta, self.vip_audio_unlocked);
        if next != self.config.download_audio_quality {
            self.config.download_audio_quality = next;
            let _ = self.config.save();
        }
    }

    /// 「恢复默认」两段式：首次进入待确认态（文字换成警戒色），再选一次才写回默认。
    ///
    /// 下载不可用（显式 `Null` / 系统没有可写位置）时也允许：它就是那个出口。
    fn activate_download_reset(&mut self) {
        if !self.download_reset_armed {
            self.download_reset_armed = true;
            self.set_runtime_status(self.lang_text(
                "再按一次确认恢复下载设置",
                "Press again to restore download settings",
            ));
            return;
        }

        self.download_reset_armed = false;
        self.config.download_audio_quality = crate::data::config::default_download_audio_quality();
        self.config.download_path = None;
        let _ = self.config.save();
        self.refresh_download_root();
        self.set_runtime_status(self.lang_text(
            "下载设置已恢复默认",
            "Download settings restored to defaults",
        ));
    }

    /// 进入路径行的行内编辑：以当前生效路径为初值，光标停在末尾。
    ///
    /// 填字面量 `Null` 回车 = 显式禁用下载；填绝对路径恢复。
    fn begin_download_path_edit(&mut self) {
        let current = self
            .download_root
            .as_ref()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| self.download_display_path());
        self.download_path_edit = Some(DownloadPathEdit {
            cursor: current.chars().count(),
            buffer: current,
            window_col: 0,
        });
    }

    fn download_path_edit_insert(&mut self, ch: char) {
        let Some(edit) = self.download_path_edit.as_mut() else {
            return;
        };
        if edit.cursor >= DOWNLOAD_PATH_MAX_CHARS {
            return;
        }
        let index = byte_index_for_char(&edit.buffer, edit.cursor);
        edit.buffer.insert(index, ch);
        edit.cursor += 1;
    }

    fn download_path_edit_backspace(&mut self) {
        let Some(edit) = self.download_path_edit.as_mut() else {
            return;
        };
        if edit.cursor == 0 {
            return;
        }
        let index = byte_index_for_char(&edit.buffer, edit.cursor - 1);
        edit.buffer.remove(index);
        edit.cursor -= 1;
    }

    fn download_path_edit_delete(&mut self) {
        let Some(edit) = self.download_path_edit.as_mut() else {
            return;
        };
        if edit.cursor >= char_count(&edit.buffer) {
            return;
        }
        let index = byte_index_for_char(&edit.buffer, edit.cursor);
        edit.buffer.remove(index);
    }

    fn download_path_edit_move(&mut self, delta: i32) {
        let Some(edit) = self.download_path_edit.as_mut() else {
            return;
        };
        let last = char_count(&edit.buffer) as i32;
        edit.cursor = (edit.cursor as i32 + delta).clamp(0, last) as usize;
    }

    fn download_path_edit_home(&mut self) {
        if let Some(edit) = self.download_path_edit.as_mut() {
            edit.cursor = 0;
        }
    }

    fn download_path_edit_end(&mut self) {
        if let Some(edit) = self.download_path_edit.as_mut() {
            edit.cursor = char_count(&edit.buffer);
        }
    }

    /// 回车确认：`Null` = 显式禁用；非法（空 / 非绝对 / 不可写）保留修改前的值。
    fn commit_download_path_edit(&mut self) {
        let Some(edit) = self.download_path_edit.take() else {
            return;
        };
        let raw = edit.buffer.trim().to_string();
        match crate::app::download::parse_download_path(&raw) {
            Ok(crate::app::download::DownloadPathChoice::Disabled) => {
                self.config.download_path =
                    Some(crate::app::download::DOWNLOAD_PATH_NULL.to_string());
                let _ = self.config.save();
                self.refresh_download_root();
                self.set_runtime_status(self.lang_text(
                    "已禁用下载（路径填 Null）",
                    "Downloads disabled (path is Null)",
                ));
            }
            Ok(crate::app::download::DownloadPathChoice::Dir(path)) => {
                self.config.download_path = Some(path.display().to_string());
                let _ = self.config.save();
                self.refresh_download_root();
                self.set_runtime_status(format!(
                    "{}: {}",
                    self.lang_text("下载路径已更新", "Download path updated"),
                    path.display()
                ));
            }
            Err(err) => {
                let reason = match err {
                    crate::app::download::DownloadPathError::NotAbsolute => {
                        self.lang_text("必须使用绝对路径", "path must be absolute")
                    }
                    crate::app::download::DownloadPathError::NotWritable => {
                        self.lang_text("路径不可写", "path is not writable")
                    }
                };
                self.set_runtime_status(format!(
                    "{}（{reason}）",
                    self.lang_text(
                        "下载路径无效，保留修改前的值",
                        "Invalid download path, keeping the previous value"
                    )
                ));
            }
        }
    }

    /// 设置弹窗里显示的下载路径（`Null` = 系统里没有可用位置）。
    pub fn download_display_path(&self) -> String {
        self.download_root
            .as_ref()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| crate::app::download::DOWNLOAD_PATH_NULL.to_string())
    }

    /// 「下载设置」页：音质 / 路径 / 恢复默认三行。
    ///
    /// 编辑态下所有按键都进输入框（含 `t`）；非编辑态沿用设置弹窗的习惯（Esc 返回、t 关闭）。
    fn handle_settings_download_key(&mut self, key: KeyEvent) {
        if self.download_path_edit.is_some() {
            match key.code {
                KeyCode::Esc => {
                    self.download_path_edit = None;
                    self.set_runtime_status(
                        self.lang_text("已取消修改下载路径", "Download path edit cancelled"),
                    );
                }
                KeyCode::Enter => self.commit_download_path_edit(),
                KeyCode::Backspace => self.download_path_edit_backspace(),
                KeyCode::Delete => self.download_path_edit_delete(),
                KeyCode::Left => self.download_path_edit_move(-1),
                KeyCode::Right => self.download_path_edit_move(1),
                KeyCode::Home => self.download_path_edit_home(),
                KeyCode::End => self.download_path_edit_end(),
                KeyCode::Char(ch) => {
                    if !key
                        .modifiers
                        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
                        && !ch.is_control()
                    {
                        self.download_path_edit_insert(ch);
                    }
                }
                _ => {}
            }
            return;
        }

        match key.code {
            KeyCode::Esc => {
                self.download_reset_armed = false;
                self.overlay = Some(Overlay::Settings);
            }
            KeyCode::Char('t') | KeyCode::Char('T') => {
                if key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT {
                    self.download_reset_armed = false;
                    self.close_overlay();
                }
            }
            KeyCode::Up | KeyCode::BackTab => self.move_download_selection(-1),
            KeyCode::Down | KeyCode::Tab => self.move_download_selection(1),
            KeyCode::Left => self.apply_settings_download_delta(-1),
            KeyCode::Right | KeyCode::Enter => self.activate_settings_download_item(),
            _ => {}
        }
    }

    async fn handle_settings_root_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => self.close_overlay(),
            KeyCode::Char('t') | KeyCode::Char('T') => {
                if key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT {
                    self.close_overlay();
                }
            }
            KeyCode::Up | KeyCode::BackTab => {
                if self.settings_selected == 0 {
                    self.settings_selected = SETTINGS_ROOT_ITEMS - 1;
                } else {
                    self.settings_selected -= 1;
                }
            }
            KeyCode::Down | KeyCode::Tab => {
                self.settings_selected = (self.settings_selected + 1) % SETTINGS_ROOT_ITEMS;
            }
            KeyCode::Left => self.apply_settings_root_delta(-1).await,
            KeyCode::Right => self.apply_settings_root_delta(1).await,
            KeyCode::Enter => self.activate_settings_root_item().await,
            _ => {}
        }
    }

    fn handle_settings_playback_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => self.overlay = Some(Overlay::Settings),
            KeyCode::Char('t') | KeyCode::Char('T') => {
                if key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT {
                    self.close_overlay();
                }
            }
            KeyCode::Left => {
                self.apply_settings_playback_delta(-1);
            }
            KeyCode::Right | KeyCode::Enter => {
                self.apply_settings_playback_delta(1);
            }
            KeyCode::Up | KeyCode::BackTab => {
                if self.settings_playback_selected == 0 {
                    self.settings_playback_selected = SETTINGS_PLAYBACK_ITEMS - 1;
                } else {
                    self.settings_playback_selected -= 1;
                }
            }
            KeyCode::Down | KeyCode::Tab => {
                self.settings_playback_selected =
                    (self.settings_playback_selected + 1) % SETTINGS_PLAYBACK_ITEMS;
            }
            _ => {}
        }
    }

    fn handle_settings_keybinds_key(&mut self, key: KeyEvent) {
        if let Some(index) = self.settings_keybind_rebinding {
            match key.code {
                KeyCode::Esc => {
                    self.settings_keybind_rebinding = None;
                    self.set_runtime_status(
                        self.lang_text("已取消快捷键重绑", "Cancelled keybind rebinding"),
                    );
                }
                _ => {
                    let Some(binding) = key_event_to_keybind_text(key) else {
                        self.set_runtime_status(self.lang_text(
                            "该按键暂不支持绑定，请重试",
                            "This key is not supported for binding, please retry",
                        ));
                        return;
                    };

                    if binding == RESERVED_RESET_KEYBIND {
                        self.set_runtime_status(self.lang_text(
                            "Ctrl+Alt+R 为保留快捷键，不能重新绑定",
                            "Ctrl+Alt+R is reserved and cannot be rebound",
                        ));
                        return;
                    }

                    if let Some(conflict_index) = self.find_keybind_conflict(index, &binding) {
                        self.set_runtime_status(format!(
                            "{}: [{}] {} [{}]，{}",
                            self.lang_text("快捷键冲突", "Keybind conflict"),
                            binding,
                            self.lang_text("已用于", "is already used by"),
                            self.keybind_name_for_index(conflict_index),
                            self.lang_text("请使用其他按键", "please choose another key")
                        ));
                        return;
                    }

                    if let Some(slot) = self.keybind_value_mut_for_index(index) {
                        *slot = binding.clone();
                        let _ = self.config.save();
                        self.set_runtime_status(format!(
                            "{} [{}] {} {}",
                            self.lang_text("已将", "Bound"),
                            self.keybind_name_for_index(index),
                            self.lang_text("绑定为", "to"),
                            binding
                        ));
                    }
                    self.settings_keybind_rebinding = None;
                }
            }
            return;
        }

        if is_reserved_reset_combo(key) {
            self.reset_keybinds_to_default();
            let _ = self.config.save();
            self.set_runtime_status(
                self.lang_text("已恢复默认快捷键", "Restored default keybinds"),
            );
            return;
        }

        match key.code {
            KeyCode::Esc => {
                self.settings_keybind_rebinding = None;
                self.overlay = Some(Overlay::Settings);
            }
            KeyCode::Char('t') | KeyCode::Char('T') => {
                if key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT {
                    self.settings_keybind_rebinding = None;
                    self.close_overlay();
                }
            }
            KeyCode::Left => {
                self.settings_keybind_rebinding = None;
                self.overlay = Some(Overlay::Settings);
            }
            KeyCode::Up | KeyCode::BackTab => {
                if self.settings_keybind_selected == 0 {
                    self.settings_keybind_selected = SETTINGS_KEYBIND_ITEMS - 1;
                } else {
                    self.settings_keybind_selected -= 1;
                }
            }
            KeyCode::Down | KeyCode::Tab => {
                self.settings_keybind_selected =
                    (self.settings_keybind_selected + 1) % SETTINGS_KEYBIND_ITEMS;
            }
            KeyCode::Enter => {
                let idx = self.settings_keybind_selected;
                self.begin_keybind_rebind(idx);
            }
            _ => {}
        }
    }

    fn handle_settings_about_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc | KeyCode::Left | KeyCode::Enter => {
                // 彩蛋状态随 about 一起复位：下次进来需重新点满触发次数。
                #[cfg(feature = "easter-egg")]
                {
                    self.about_egg = AboutEasterEgg::default();
                }
                self.overlay = Some(Overlay::Settings);
            }
            KeyCode::Char('t') | KeyCode::Char('T') => {
                if key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT {
                    self.close_overlay();
                }
            }
            // 空格等价于点击一次形象：重新计时即打断当前动画重头播放。
            #[cfg(feature = "easter-egg")]
            KeyCode::Char(' ') if key.modifiers.is_empty() => {
                if self.about_egg.phase == EasterEggPhase::Active {
                    self.about_egg.jelly_started_at = Some(Instant::now());
                }
            }
            _ => {}
        }
    }

    /// about 弹窗里的点击：累计版本行点击触发彩蛋，或让形象再弹一次。
    #[cfg(feature = "easter-egg")]
    fn handle_settings_about_click(&mut self, col: u16, row: u16) {
        use crate::render::mascot;

        if self.about_egg.phase == EasterEggPhase::Active {
            if let Some(rect) = self.about_egg.mascot_hit
                && rect.contains(col, row)
            {
                // 播放中再次点击即重新计时，等价于打断当前动画重头播放。
                self.about_egg.jelly_started_at = Some(Instant::now());
            }
            return;
        }

        if self.about_egg.phase != EasterEggPhase::Idle {
            return;
        }

        let Some(rect) = self.about_egg.version_hit else {
            return;
        };
        if !rect.contains(col, row) {
            return;
        }

        self.about_egg.version_clicks = self.about_egg.version_clicks.saturating_add(1);
        if self.about_egg.version_clicks >= mascot::TRIGGER_CLICKS {
            self.about_egg.phase = EasterEggPhase::Charging;
            self.about_egg.phase_started_at = Some(Instant::now());
        }
    }

    async fn apply_settings_root_delta(&mut self, delta: i32) {
        match self.settings_selected {
            0 => {
                let themes = ["system", "latte", "frappe", "macchiato", "mocha"];
                let current = themes
                    .iter()
                    .position(|name| name.eq_ignore_ascii_case(self.config.theme.as_str()))
                    .unwrap_or(0) as i32;
                let next = (current + delta).rem_euclid(themes.len() as i32) as usize;
                let next_name = themes[next];
                if let Ok(theme) = ThemeLoader::load(next_name) {
                    self.theme = theme;
                    self.config.theme = next_name.to_string();
                    let _ = self.config.save();
                }
            }
            1 => {
                if delta != 0 {
                    self.config.transparent_background = !self.config.transparent_background;
                    let _ = self.config.save();
                }
            }
            2 => {
                if delta != 0 {
                    self.config.language = match self.config.language {
                        Language::Zh => Language::En,
                        Language::En => Language::Zh,
                    };
                    let _ = self.config.save();
                }
            }
            3 => {
                if delta != 0 {
                    let next_protocol = self.config.graphics_protocol.cycle(delta);
                    if next_protocol != self.config.graphics_protocol {
                        self.config.graphics_protocol = next_protocol;
                        let _ = self.config.save();
                    }
                }
            }
            6 => {
                // “歌词浮窗...”是可进入项：左右键不改变配置（与播放设置/按键绑定一致）
            }
            7 => {
                if delta != 0 {
                    self.config.show_hints = !self.config.show_hints;
                    let _ = self.config.save();
                }
            }
            8 => {
                if delta != 0 {
                    let was_small_context = self.is_small_window_context();
                    self.config.small_window_display = !self.config.small_window_display;
                    let _ = self.config.save();
                    if !was_small_context && self.is_small_window_context() {
                        self.close_panels_for_small_window();
                    }
                    self.sync_terminal_size();
                }
            }
            9 => {
                if delta != 0 {
                    self.config.home_more_recommend = !self.config.home_more_recommend;
                    let _ = self.config.save();
                    if self.page == Page::Home {
                        if let Err(err) = self.load_home_recommendations().await {
                            self.home.status_line = format!(
                                "{}: {}",
                                self.lang_text(
                                    "推荐歌单刷新失败",
                                    "Failed to refresh home recommendations",
                                ),
                                err
                            );
                        }
                    }
                }
            }
            _ => {}
        }
    }

    fn apply_settings_playback_delta(&mut self, delta: i32) {
        if delta == 0 {
            return;
        }

        match self.settings_playback_selected {
            0 => {
                self.config.visualize = self.config.visualize.cycle(delta);
                let _ = self.config.save();
            }
            1 => {
                self.config.super_smooth_bar = !self.config.super_smooth_bar;
                let _ = self.config.save();
            }
            2 => {
                self.config.bars_gap = !self.config.bars_gap;
                let _ = self.config.save();
            }
            3 => {
                self.config.bar_number = cycle_bar_number(self.config.bar_number, delta);
                let _ = self.config.save();
            }
            4 => {
                self.config.bar_channels = match self.config.bar_channels {
                    BarChannels::Mono => BarChannels::Stereo,
                    BarChannels::Stereo => BarChannels::Mono,
                };
                let _ = self.config.save();
            }
            5 => {
                self.config.album_border = !self.config.album_border;
                let _ = self.config.save();
            }
            6 => {
                let next = self
                    .config
                    .audio_quality
                    .cycle(delta, self.vip_audio_unlocked);
                self.set_audio_quality(next);
            }
            7 => {
                self.config.playback_memory = !self.config.playback_memory;
                let _ = self.config.save();
                if self.config.playback_memory {
                    self.persist_playback_memory();
                } else {
                    self.clear_playback_memory();
                }
            }
            _ => {}
        }
    }

    async fn handle_search_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc | KeyCode::Left => {
                self.page = self.search_return_page;
            }
            KeyCode::Tab | KeyCode::Down => {
                self.advance_search_focus().await;
            }
            KeyCode::BackTab | KeyCode::Up => {
                let _ = self.search.focus_prev();
            }
            KeyCode::Enter => self.activate_focused_search_result().await,
            _ => {}
        }
    }

    async fn handle_login_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::F(1) => {
                self.login.set_method(LoginMethod::Qr);
                self.refresh_qr_login().await;
            }
            KeyCode::F(2) => self.login.set_method(LoginMethod::Username),
            KeyCode::F(3) => self.login.set_method(LoginMethod::Phone),
            KeyCode::Tab | KeyCode::Down => self.login.next_focus(),
            KeyCode::BackTab | KeyCode::Up => self.login.prev_focus(),
            KeyCode::Enter => self.submit_login_action().await,
            KeyCode::Backspace => self.login.pop_char(),
            KeyCode::Char(ch) if matches!(ch, 'q' | 'Q') => {
                if key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT {
                    let typing_username_or_password =
                        self.login.method == LoginMethod::Username && self.login.focus_index <= 1;
                    if typing_username_or_password {
                        self.login.push_char(ch);
                    } else {
                        self.should_quit = true;
                    }
                }
            }
            KeyCode::Char(ch) => {
                if key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT {
                    self.login.push_char(ch);
                }
            }
            _ => {}
        }
    }

    async fn handle_home_key(&mut self, key: KeyEvent) {
        if self.home_sidebar.expanded {
            if key.modifiers.contains(KeyModifiers::CONTROL) {
                match key.code {
                    KeyCode::Up => {
                        self.home_sidebar.switch_section_prev();
                        return;
                    }
                    KeyCode::Down => {
                        self.home_sidebar.switch_section_next();
                        return;
                    }
                    _ => {}
                }
            }

            match key.code {
                KeyCode::Esc => {
                    self.home_sidebar.expanded = false;
                    self.animate_home_sidebar();
                }
                KeyCode::Up | KeyCode::BackTab => self.home_sidebar.focus_prev(),
                KeyCode::Down | KeyCode::Tab => self.home_sidebar.focus_next(),
                KeyCode::Enter => self.open_focused_home_sidebar_playlist().await,
                _ => {}
            }
            return;
        }

        match key.code {
            KeyCode::Tab => self.home.focus_next(),
            KeyCode::BackTab => self.home.focus_prev(),
            KeyCode::Left => self.home.focus_left(),
            KeyCode::Right => self.home.focus_right(),
            KeyCode::Up => self.home.focus_up(),
            KeyCode::Down => self.home.focus_down(),
            KeyCode::Enter => self.enter_home_tile().await,
            _ => {}
        }
    }

    async fn handle_playlist_key(&mut self, key: KeyEvent) {
        // 数据还在路上：占位页上的焦点/条目都没有意义，只留返回键。
        if self.playlist_fetch.is_some() {
            if matches!(key.code, KeyCode::Esc | KeyCode::Left) {
                self.page = self.playlist_return_page;
            }
            return;
        }

        match key.code {
            KeyCode::Up | KeyCode::BackTab => {
                let _ = self.playlist.focus_prev();
            }
            KeyCode::Down | KeyCode::Tab => {
                let _ = self.playlist.focus_next();
            }
            KeyCode::Enter => self.play_focused_playlist_track().await,
            KeyCode::Esc | KeyCode::Left => {
                if let Some(snapshot) = self.playlist_section_return_snapshot.take() {
                    self.playlist = snapshot;
                    return;
                }
                self.page = match self.playlist_return_page {
                    Page::Author => Page::Author,
                    Page::Search => Page::Search,
                    _ => Page::Home,
                };
            }
            _ => {}
        }
    }

    async fn handle_author_key(&mut self, key: KeyEvent) {
        // 数据还在路上：占位页上的焦点/条目都没有意义，只留返回键。
        if self.author_fetch.is_some() {
            if key.code == KeyCode::Esc {
                self.page = self.author_return_page;
            }
            return;
        }

        match key.code {
            KeyCode::Tab => {
                let _ = self.author.focus_next();
            }
            KeyCode::BackTab => {
                let _ = self.author.focus_prev();
            }
            KeyCode::Left => self.author.focus_left(),
            KeyCode::Right => self.author.focus_right(),
            KeyCode::Up => {
                let _ = self.author.focus_up();
            }
            KeyCode::Down => {
                let _ = self.author.focus_down();
            }
            KeyCode::Enter => self.play_focused_author_tile().await,
            KeyCode::Esc => {
                self.page = self.author_return_page;
            }
            _ => {}
        }
    }

    fn tick_search_box_animation(&mut self) {
        if matches!(self.overlay, Some(Overlay::SearchBox)) {
            // time-based：动画时长与驱动帧率解耦，与 startup_loading 同风格
            let started_at = self
                .search_box_anim_started_at
                .get_or_insert_with(Instant::now);
            let elapsed = started_at.elapsed();
            if elapsed >= SEARCH_BOX_ANIM_DURATION {
                self.search_box_anim_height = SEARCH_BOX_TARGET_HEIGHT;
                return;
            }
            let t = elapsed.as_secs_f32() / SEARCH_BOX_ANIM_DURATION.as_secs_f32();
            // ease-out：先快后慢（cubic-bezier y 曲线，p2y=0.7）
            let eased = cubic_bezier_y(t, 0.0, 0.7);
            self.search_box_anim_height =
                ((SEARCH_BOX_TARGET_HEIGHT as f32) * eased).round() as u16;
        } else {
            self.search_box_anim_height = 0;
            self.search_box_anim_started_at = None;
        }
    }

    /// 推进 about 彩蛋的蓄力/迸发阶段（time-based，与帧率解耦）。
    #[cfg(feature = "easter-egg")]
    fn tick_about_easter_egg(&mut self) {
        use crate::render::mascot;

        // 离开 about 后不该留下动画状态。
        if !matches!(self.overlay, Some(Overlay::SettingsAbout)) {
            self.about_egg = AboutEasterEgg::default();
            return;
        }

        // 果冻是一次性的：播完就回到静止形象。
        if let Some(started_at) = self.about_egg.jelly_started_at
            && started_at.elapsed() >= mascot::JELLY_DURATION
        {
            self.about_egg.jelly_started_at = None;
        }

        let Some(started_at) = self.about_egg.phase_started_at else {
            return;
        };
        let elapsed = started_at.elapsed();

        match self.about_egg.phase {
            EasterEggPhase::Charging => {
                if elapsed >= mascot::CHARGE_DURATION {
                    self.about_egg.phase = EasterEggPhase::Bursting;
                    self.about_egg.phase_started_at = Some(Instant::now());
                }
            }
            EasterEggPhase::Bursting => {
                if elapsed >= mascot::BURST_DURATION {
                    self.about_egg.phase = EasterEggPhase::Active;
                    self.about_egg.phase_started_at = None;
                    self.about_egg.mascot_activated_at = Some(Instant::now());
                }
            }
            EasterEggPhase::Idle | EasterEggPhase::Active => {}
        }
    }

    fn tick_home_sidebar_animation(&mut self) {
        let target = if self.home_sidebar.expanded { 1.0 } else { 0.0 };
        let state = &mut self.home_sidebar;
        if (state.anim_progress - target).abs() < 0.001 {
            state.anim_progress = target;
            state.anim_started_at = None;
            return;
        }
        // time-based：从 anim_from 向 target 插值（ease-out），支持中途反向
        let started_at = state.anim_started_at.get_or_insert_with(Instant::now);
        let elapsed = started_at.elapsed();
        let t = if elapsed >= SIDEBAR_ANIM_DURATION {
            1.0
        } else {
            elapsed.as_secs_f32() / SIDEBAR_ANIM_DURATION.as_secs_f32()
        };
        let eased = cubic_bezier_y(t, 0.0, 0.7);
        state.anim_progress = state.anim_from + (target - state.anim_from) * eased;
        if t >= 1.0 {
            state.anim_progress = target;
            state.anim_started_at = None;
        }
    }

    /// 启动一次侧边栏滑出/收起动画（记录当前进度作为动画起点，支持中途反向）。
    fn animate_home_sidebar(&mut self) {
        let target = if self.home_sidebar.expanded { 1.0 } else { 0.0 };
        if (self.home_sidebar.anim_progress - target).abs() < 0.001 {
            // 已在目标态：无需动画，清掉可能的残留状态
            self.home_sidebar.anim_progress = target;
            self.home_sidebar.anim_started_at = None;
            return;
        }
        self.home_sidebar.anim_from = self.home_sidebar.anim_progress;
        self.home_sidebar.anim_started_at = Some(Instant::now());
    }

    fn begin_startup_loading(&mut self, target: Page) {
        self.page = Page::Loading;
        self.overlay = None;
        self.startup_loading_progress = 0.0;
        self.startup_loading_started_at = Some(Instant::now());
        self.startup_loading_complete_started_at = None;
        self.startup_loading_complete_requested = false;
        self.startup_loading_target = target;
    }

    fn finish_startup_loading(&mut self) {
        self.startup_loading_complete_requested = true;
        if self.startup_loading_complete_started_at.is_none() {
            self.startup_loading_complete_started_at = Some(Instant::now());
        }
    }

    fn tick_startup_loading(&mut self) {
        if self.page != Page::Loading {
            return;
        }

        let Some(started_at) = self.startup_loading_started_at else {
            self.startup_loading_started_at = Some(Instant::now());
            return;
        };

        let elapsed = started_at.elapsed().as_secs_f32();
        self.startup_loading_progress = self.startup_loading_progress();

        // 让位条件：数据齐了、进度条收尾 ramp 跑满、且满足最短可见时长。
        // 少了 ramp 这一条，进度条会停在一半就消失。
        let ramp_done = self
            .startup_loading_complete_started_at
            .map(|completed_at| {
                completed_at.elapsed().as_secs_f32() >= STARTUP_LOADING_COMPLETE_RAMP_SECS
            })
            .unwrap_or(false);
        if self.startup_loading_complete_requested
            && ramp_done
            && elapsed >= STARTUP_LOADING_MIN_VISIBLE_SECS
        {
            self.page = self.startup_loading_target;
            self.startup_loading_progress = 0.0;
            self.startup_loading_started_at = None;
            self.startup_loading_complete_started_at = None;
            self.startup_loading_complete_requested = false;
        }
    }

    /// 加载页进度：后台初始化完成的步数 + 当前步的时间缓动。
    fn startup_loading_progress(&self) -> f32 {
        startup_loading_progress(
            self.startup.step_done(),
            self.startup.step_total(),
            self.startup.step_elapsed(),
            self.startup_loading_complete_started_at
                .map(|completed_at| completed_at.elapsed().as_secs_f32()),
            self.startup_loading_complete_requested,
        )
    }

    pub fn startup_loading_progress_for_width(&self, _bar_width: u16) -> f32 {
        if self.page != Page::Loading {
            return 0.0;
        }

        if self.startup_loading_started_at.is_none() {
            return 0.0;
        }

        self.startup_loading_progress()
    }

    fn is_double_content_click(&mut self, page: Page, index: usize) -> bool {
        let now = Instant::now();
        let is_double = self
            .last_content_click
            .map(|(at, p, i)| {
                p == page
                    && i == index
                    && now.duration_since(at) <= Duration::from_millis(CONTENT_DOUBLE_CLICK_MS)
            })
            .unwrap_or(false);
        self.last_content_click = Some((now, page, index));
        is_double
    }

    fn home_sidebar_double_click_index(hit: HomeSidebarHit) -> usize {
        const CREATED_BASE: usize = 10_000;
        const COLLECTED_BASE: usize = 20_000;
        match hit.section {
            HomeSidebarSection::Created => CREATED_BASE.saturating_add(hit.index),
            HomeSidebarSection::Collected => COLLECTED_BASE.saturating_add(hit.index),
        }
    }

    async fn handle_content_click(&mut self, col: u16, row: u16) -> bool {
        match self.page {
            Page::Home => {
                if self.home_sidebar.is_visible() {
                    if let Some(panel) = self.home_sidebar_panel_hit {
                        if panel.contains(col, row) {
                            let sidebar_hit = self
                                .home_sidebar_playlist_hits
                                .iter()
                                .find(|(rect, _)| rect.contains(col, row))
                                .map(|(_, hit)| *hit);
                            if let Some(hit) = sidebar_hit {
                                if self.home_sidebar.expanded {
                                    self.home_sidebar.set_focus(hit.section, hit.index);
                                    if self.is_double_content_click(
                                        Page::Home,
                                        Self::home_sidebar_double_click_index(hit),
                                    ) {
                                        self.open_focused_home_sidebar_playlist().await;
                                    }
                                }
                                return true;
                            }
                            self.last_content_click = None;
                            return true;
                        }
                    }

                    if self.home_sidebar.expanded {
                        self.last_content_click = None;
                        return true;
                    }
                }

                let hit = self
                    .home_tile_hits
                    .iter()
                    .find(|(rect, _)| rect.contains(col, row))
                    .map(|(_, idx)| *idx);
                if let Some(idx) = hit {
                    if idx < self.home.tiles.len() {
                        self.home.focused_idx = idx;
                        if self.is_double_content_click(Page::Home, idx) {
                            self.enter_home_tile().await;
                        }
                        return true;
                    }
                }
            }
            Page::Playlist => {
                // 行内下载图标优先于整行命中：命中即下载 / 取消下载，
                // 且不更新 `last_content_click`（否则 400ms 双击会顺带播放该行）。
                if let Some(idx) = self
                    .playlist_track_download_hits
                    .iter()
                    .find(|(rect, _)| rect.contains(col, row))
                    .map(|(_, idx)| *idx)
                    && let Some(candidate) = self.playlist_download_candidate(idx)
                {
                    self.toggle_download(candidate);
                    return true;
                }

                let hit = self
                    .playlist_track_hits
                    .iter()
                    .find(|(rect, _)| rect.contains(col, row))
                    .map(|(_, idx)| *idx);
                if let Some(idx) = hit {
                    if idx < self.playlist.tracks.len() {
                        self.playlist.set_focus(idx);
                        if self.is_double_content_click(Page::Playlist, idx) {
                            self.play_focused_playlist_track().await;
                        }
                        return true;
                    }
                }
            }
            Page::Author => {
                let hit = self
                    .author_tile_hits
                    .iter()
                    .find(|(rect, _)| rect.contains(col, row))
                    .map(|(_, idx)| *idx);
                if let Some(idx) = hit {
                    if idx < self.author.tiles.len() {
                        self.author.set_focus(idx);
                        if self.is_double_content_click(Page::Author, idx) {
                            self.play_focused_author_tile().await;
                        }
                        return true;
                    }
                }
            }
            Page::Search => {
                // 与歌单页同理：图标命中先于整行命中，且不动双击判定。
                if let Some(idx) = self
                    .search_item_download_hits
                    .iter()
                    .find(|(rect, _)| rect.contains(col, row))
                    .map(|(_, idx)| *idx)
                    && let Some(candidate) = self.search_download_candidate(idx)
                {
                    self.toggle_download(candidate);
                    return true;
                }

                let hit = self
                    .search_item_hits
                    .iter()
                    .find(|(rect, _)| rect.contains(col, row))
                    .map(|(_, idx)| *idx);
                if let Some(idx) = hit {
                    if idx < self.search.results.len() {
                        self.search.set_focus(idx);
                        if self.is_double_content_click(Page::Search, idx) {
                            self.activate_focused_search_result().await;
                        }
                        return true;
                    }
                }
            }
            _ => {}
        }
        false
    }

    fn open_search_box(&mut self) {
        if self.page != Page::Search {
            self.search_return_page = Page::Home;
        }
        self.search_box_input = self.search.query.clone();
        self.search_box_cursor = char_count(&self.search_box_input);
        self.search_box_anim_height = 0;
        self.search_box_anim_started_at = Some(Instant::now());
        self.overlay = Some(Overlay::SearchBox);
    }

    fn close_overlay(&mut self) {
        self.overlay = None;
        self.search_box_anim_height = 0;
        self.search_box_anim_started_at = None;
        self.last_settings_click = None;
        self.download_path_edit = None;
        self.download_reset_armed = false;
        self.clear_settings_item_hits();
    }

    async fn execute_search_from_box(&mut self) {
        let raw_query = self.search_box_input.trim().to_string();
        let (keywords, filter) = parse_search_input(&raw_query);
        if keywords.is_empty() && !is_followed_author_query(&keywords, filter) {
            self.search.status_line = self
                .lang_text("请输入搜索关键词", "Please enter search keywords")
                .to_string();
            return;
        }

        self.search.query = raw_query;
        if let Err(err) = self.execute_search().await {
            self.search.status_line = format!("搜索失败: {}", err);
            self.search.set_results(Vec::new(), 0, false);
        }
        self.playlist_section_return_snapshot = None;
        self.page = Page::Search;
        self.close_overlay();
    }

    pub fn consume_fullscreen_launch_request(&mut self) -> bool {
        std::mem::take(&mut self.launch_fullscreen_requested)
    }

    pub fn open_settings_from_fullscreen(&mut self) {
        if self.page != Page::Login {
            self.open_settings();
        }
    }

    /// 全屏页点了作者名：立即落占位作者页 + 派发后台拉取，结果由 `App::tick_author_fetch`
    /// 搬进来（宿主循环在这期间照常重绘、照常响应输入）。
    ///
    /// `index` 是显示串（`now_playing.artist`，形如 "A / B"）里的段序号：
    /// 全屏页信息区按字符位置分段命中，点谁的名字就传谁的序号。
    /// 全屏页只有显示名（ID 要靠 `song/detail` 的 `ar` 补），
    /// 本机音频 / 无播放时解析不出来，只在状态行里说明，不换页。
    pub fn open_author_page_from_fullscreen(&mut self, index: usize) {
        let Some(song_id) = self.current_song_id() else {
            self.set_runtime_status(
                self.lang_text("当前没有正在播放的歌曲", "Nothing is playing right now"),
            );
            return;
        };

        let artist_line = self
            .now_playing
            .as_ref()
            .map(|track| track.artist.clone())
            .unwrap_or_default();
        // 标题先显示点中的那段名字；作者 ID 解析失败时也只是把错误写进简介。
        let title = {
            let line = artist_line.trim();
            let clicked = artist_name_segments(&artist_line)
                .get(index)
                .map(|name| name.trim())
                .filter(|name| !name.is_empty())
                .unwrap_or(line);
            if clicked.is_empty() {
                self.lang_text("作者页", "Artist Page").to_string()
            } else {
                clicked.to_string()
            }
        };

        self.author = AuthorState::placeholder(
            title,
            self.lang_text("正在加载作者…", "Loading artist…")
                .to_string(),
        );
        // 从全屏页进来：Esc 回首页，而不是回搜索页（那里可能不是用户来时的页面）。
        self.author_return_page = Page::Home;
        self.page = Page::Author;

        let fut = fetch_author_page(
            self.api.clone(),
            self.config.language,
            song_id,
            index,
            artist_line,
        );
        let fut: AuthorFetchTask = Box::pin(async move { Some(fut.await) });
        self.author_fetch = Some(shot_and_share(fut));
    }

    /// 全屏页点了专辑名：立即落占位专辑页 + 派发后台拉取，结果由 `App::tick_playlist_fetch`
    /// 搬进来（宿主循环在这期间照常重绘、照常响应输入）。
    ///
    /// 与作者页同理：全屏页只有专辑显示名，ID 取 `song/detail` 的 `al.id`。
    pub fn open_album_page_from_fullscreen(&mut self) {
        let Some(song_id) = self.current_song_id() else {
            self.set_runtime_status(
                self.lang_text("当前没有正在播放的歌曲", "Nothing is playing right now"),
            );
            return;
        };

        // 标题先显示正在播放的专辑名；解析失败时也只是把错误写进简介。
        let title = {
            let album = self
                .now_playing
                .as_ref()
                .map(|track| track.album.trim().to_string())
                .unwrap_or_default();
            if album.is_empty() {
                self.lang_text("专辑页", "Album Page").to_string()
            } else {
                album
            }
        };

        self.playlist = PlaylistState::placeholder(
            title,
            self.lang_text("正在加载专辑…", "Loading album…")
                .to_string(),
        );
        self.playlist_section_return_snapshot = None;
        // 从全屏页进来：Esc 回首页，而不是回上一次的来源页。
        self.playlist_return_page = Page::Home;
        self.page = Page::Playlist;

        let fut = fetch_album_page_from_song(self.api.clone(), self.config.language, song_id);
        let fut: PlaylistFetchTask = Box::pin(async move { Some(fut.await) });
        self.playlist_page_kind = PlaylistPageKind::Album;
        self.playlist_fetch = Some(PlaylistFetchSlot {
            kind: PlaylistPageKind::Album,
            future: shot_and_share(fut),
        });
    }

    pub fn fullscreen_config_snapshot(&self) -> crate::tmplayer::HostConfigSync {
        crate::tmplayer::HostConfigSync {
            theme: self.config.theme.clone(),
            transparent_background: self.config.transparent_background,
            album_border: self.config.album_border,
            language: self.config.language,
            graphics_protocol: self.config.graphics_protocol,
            page_lyrics: self.config.page_lyrics,
            page_lyrics_drag: self.config.page_lyrics_drag,
            page_lyrics_snap: self.config.page_lyrics_snap,
            page_lyrics_pos_x: self.config.page_lyrics_pos_x,
            page_lyrics_pos_y: self.config.page_lyrics_pos_y,
            audio_quality: self.config.audio_quality,
            download_audio_quality: self.config.download_audio_quality,
            download_path: self.config.download_path.clone(),
            eq_bands_db: self.config.eq_bands_db,
            playback_memory: self.config.playback_memory,
            vip_audio_unlocked: self.vip_audio_unlocked,
            show_hints: self.config.show_hints,
            small_window_display: self.config.small_window_display,
            home_more_recommend: self.config.home_more_recommend,
            visualize: self.config.visualize,
            super_smooth_bar: self.config.super_smooth_bar,
            bars_gap: self.config.bars_gap,
            bar_number: self.config.bar_number,
            bar_channels: self.config.bar_channels,
            bar_channel_reverse: self.config.bar_channel_reverse,
        }
    }

    pub async fn fullscreen_apply_config_sync(&mut self, sync: crate::tmplayer::HostConfigSync) {
        let mut changed = false;
        let mut home_more_recommend_changed = false;

        if self.config.theme != sync.theme {
            if let Ok(theme) = ThemeLoader::load(&sync.theme) {
                self.theme = theme;
                self.config.theme = sync.theme;
                changed = true;
            }
        }

        if self.config.transparent_background != sync.transparent_background {
            self.config.transparent_background = sync.transparent_background;
            changed = true;
        }

        if self.config.album_border != sync.album_border {
            self.config.album_border = sync.album_border;
            changed = true;
        }

        if self.config.language != sync.language {
            self.config.language = sync.language;
            changed = true;
        }

        if self.config.graphics_protocol != sync.graphics_protocol {
            self.config.graphics_protocol = sync.graphics_protocol;
            changed = true;
        }

        if self.config.page_lyrics != sync.page_lyrics {
            self.config.page_lyrics = sync.page_lyrics;
            changed = true;
        }

        if self.config.page_lyrics_drag != sync.page_lyrics_drag {
            self.config.page_lyrics_drag = sync.page_lyrics_drag;
            changed = true;
        }

        if self.config.page_lyrics_snap != sync.page_lyrics_snap {
            self.config.page_lyrics_snap = sync.page_lyrics_snap;
            changed = true;
        }

        // 全屏页也可能改到浮窗位置（拖拽时由宿主写、这里只做兜底同步）。
        let pos_x = sync.page_lyrics_pos_x.clamp(0.0, 1.0);
        if (self.config.page_lyrics_pos_x - pos_x).abs() > f32::EPSILON {
            self.config.page_lyrics_pos_x = pos_x;
            changed = true;
        }
        let pos_y = sync.page_lyrics_pos_y.clamp(0.0, 1.0);
        if (self.config.page_lyrics_pos_y - pos_y).abs() > f32::EPSILON {
            self.config.page_lyrics_pos_y = pos_y;
            changed = true;
        }

        if self.vip_audio_unlocked != sync.vip_audio_unlocked {
            self.vip_audio_unlocked = sync.vip_audio_unlocked;
            changed = true;
        }

        let clamped_quality = sync.audio_quality.clamp_for_vip(self.vip_audio_unlocked);
        if self.config.audio_quality != clamped_quality {
            self.config.audio_quality = clamped_quality;
            changed = true;
        }

        // 下载音质与播放音质同一套可选值，同样按会员收口。
        let clamped_download_quality = sync
            .download_audio_quality
            .clamp_for_vip(self.vip_audio_unlocked);
        if self.config.download_audio_quality != clamped_download_quality {
            self.config.download_audio_quality = clamped_download_quality;
            changed = true;
        }

        // 全屏页改的下载路径：非法（空/非绝对/不可写）就保留修改前的值。
        if self.config.download_path != sync.download_path
            && sync
                .download_path
                .as_deref()
                .is_none_or(|raw| crate::app::download::parse_download_path(raw).is_ok())
        {
            self.config.download_path = sync.download_path.clone();
            changed = true;
        }

        if self.config.eq_bands_db != sync.eq_bands_db {
            self.config.eq_bands_db = sync.eq_bands_db;
            let _ = self
                .audio_player
                .set_eq(crate::tmplayer::app::state::EqSettings {
                    bands_db: sync.eq_bands_db,
                });
            changed = true;
        }

        if self.config.playback_memory != sync.playback_memory {
            self.config.playback_memory = sync.playback_memory;
            changed = true;
            if self.config.playback_memory {
                self.persist_playback_memory();
            } else {
                self.clear_playback_memory();
            }
        }

        if self.config.show_hints != sync.show_hints {
            self.config.show_hints = sync.show_hints;
            changed = true;
        }

        if self.config.small_window_display != sync.small_window_display {
            self.config.small_window_display = sync.small_window_display;
            changed = true;
        }

        if self.config.home_more_recommend != sync.home_more_recommend {
            self.config.home_more_recommend = sync.home_more_recommend;
            changed = true;
            home_more_recommend_changed = true;
        }

        if self.config.visualize != sync.visualize {
            self.config.visualize = sync.visualize;
            changed = true;
        }

        if self.config.super_smooth_bar != sync.super_smooth_bar {
            self.config.super_smooth_bar = sync.super_smooth_bar;
            changed = true;
        }

        if self.config.bars_gap != sync.bars_gap {
            self.config.bars_gap = sync.bars_gap;
            changed = true;
        }

        if self.config.bar_number != sync.bar_number {
            self.config.bar_number = sync.bar_number;
            changed = true;
        }

        if self.config.bar_channels != sync.bar_channels {
            self.config.bar_channels = sync.bar_channels;
            changed = true;
        }

        if self.config.bar_channel_reverse != sync.bar_channel_reverse {
            self.config.bar_channel_reverse = sync.bar_channel_reverse;
            changed = true;
        }

        // 下载路径可能刚被全屏页改过：重算根目录并作废"已下载"缓存。
        self.refresh_download_root();

        if changed {
            let _ = self.config.save();
        }

        if home_more_recommend_changed && self.page != Page::Login {
            if let Err(err) = self.load_home_recommendations().await {
                self.home.status_line = format!(
                    "{}: {}",
                    self.lang_text("推荐歌单刷新失败", "Failed to refresh home recommendations",),
                    err
                );
            }
        }
    }

    pub async fn build_fullscreen_bootstrap(&mut self) -> crate::tmplayer::FullscreenBootstrap {
        let mut bootstrap = crate::tmplayer::FullscreenBootstrap::default();

        if self.now_playing.is_none() {
            return bootstrap;
        }

        // peek_shared_future(&self.playlist.cover_bytes).map(|x| Cow::Borrowed(x.as_slice()));
        // Todo: fixme
        let mut playlist_cover = None;

        if playlist_cover.is_none() {
            // 用播放队列的来源封面，而非最后访问的页面封面。
            if let Some(cover_url) = self.playback_queue_cover_url.clone() {
                playlist_cover = self.fetch_cover_with_disk_cache(&cover_url).await
            }
        }

        // Prefer persistent now-playing queue so fullscreen follows actual playback state.
        if !self.playback_queue.is_empty() {
            bootstrap.playlist = self
                .playback_queue
                .iter()
                .map(|track| crate::tmplayer::FullscreenPlaylistItemSeed {
                    id: Some(track.song_id.clone()),
                    title: track.title.clone(),
                    artist: track.artist.clone(),
                    album: track.album.clone(),
                    duration: Duration::from_millis(track.duration_ms.max(0) as u64),
                })
                .collect();
            if !bootstrap.playlist.is_empty() {
                bootstrap.current_index = self
                    .playback_index
                    .map(|index| index.min(bootstrap.playlist.len() - 1));
            }
        }

        // Keep fullscreen in true idle state when nothing is actually playing.
        if bootstrap.playlist.is_empty() {
            if let Some(track) = self.now_playing.as_ref() {
                bootstrap
                    .playlist
                    .push(crate::tmplayer::FullscreenPlaylistItemSeed {
                        id: Some(track.song_id.clone()),
                        title: track.title.clone(),
                        artist: track.artist.clone(),
                        album: track.album.clone(),
                        duration: Duration::from_millis(track.duration_ms.max(0) as u64),
                    });
                bootstrap.current_index = Some(0);
            }
        }

        if !bootstrap.playlist.is_empty() {
            let mut active_idx = bootstrap
                .current_index
                .unwrap_or(0)
                .min(bootstrap.playlist.len() - 1);

            if let Some(now) = self.now_playing.as_ref() {
                if let Some(found) = bootstrap.playlist.iter().position(|item| {
                    item.id
                        .as_deref()
                        .map(|id| id == now.song_id.as_str())
                        .unwrap_or(false)
                }) {
                    active_idx = found;
                }
            }
            bootstrap.current_index = Some(active_idx);

            let active = bootstrap.playlist[active_idx].clone();
            let mut seed = crate::tmplayer::FullscreenTrackSeed {
                playlist_index: Some(active_idx),
                title: active.title,
                artist: active.artist,
                album: active.album,
                duration: active.duration,
                liked: self.now_playing_liked,
                cover: None,
                lyrics: None,
            };

            if let Some(now) = self.now_playing.as_ref() {
                seed.title = now.title.clone();
                seed.artist = now.artist.clone();
                seed.album = now.album.clone();
                seed.duration = Duration::from_millis(now.duration_ms.max(0) as u64);
                seed.cover = now.cover.clone();
                seed.lyrics = now.lyrics.clone();
            }

            let song_id = self
                .now_playing
                .as_ref()
                .map(|track| track.song_id.clone())
                .or_else(|| bootstrap.playlist[active_idx].id.clone());

            if let Some(song_id) = song_id {
                if let Ok(detail) = self.api.song_detail(&song_id).await {
                    if let Some(song) = detail
                        .body
                        .get("songs")
                        .and_then(|value| value.as_array())
                        .and_then(|items| items.first())
                    {
                        if let Some(name) = song.get("name").and_then(|value| value.as_str()) {
                            seed.title = name.to_string();
                        }
                        if let Some(artist) = parse_artists(song) {
                            seed.artist = artist;
                        }
                        if let Some(album) =
                            song.pointer("/al/name").and_then(|value| value.as_str())
                        {
                            seed.album = album.to_string();
                        }
                        if let Some(duration_ms) = song.get("dt").and_then(|value| value.as_i64()) {
                            seed.duration = Duration::from_millis(duration_ms.max(0) as u64);
                        }

                        if seed.cover.is_none() {
                            if let Some(cover_url) =
                                song.pointer("/al/picUrl").and_then(|value| value.as_str())
                            {
                                if let Some(bytes) =
                                    self.fetch_cover_with_disk_cache(cover_url).await
                                {
                                    seed.cover = Some(bytes);
                                }
                            }
                        }
                    }
                }

                if seed.cover.is_none() {
                    let fallback_cover_url = self
                        .now_playing
                        .as_ref()
                        .and_then(|track| track.cover_url.clone());
                    if let Some(cover_url) = fallback_cover_url.as_deref() {
                        if let Some(bytes) = self.fetch_cover_with_disk_cache(cover_url).await {
                            seed.cover = Some(bytes);
                        }
                    }
                }

                if seed.lyrics.is_none() {
                    if let Ok(lyric) = self.api.lyric(&song_id).await {
                        if let Some(raw_lrc) = lyric
                            .body
                            .pointer("/lrc/lyric")
                            .and_then(|value| value.as_str())
                        {
                            seed.lyrics = crate::tmplayer::playback::metadata::parse_lrc(raw_lrc)
                                .or_else(|| {
                                    crate::tmplayer::playback::metadata::parse_plain_lyrics(raw_lrc)
                                });
                        }
                    }
                }
            }

            if playlist_cover.is_none() {
                let first_track = self
                    .playback_queue
                    .first()
                    .cloned()
                    .or_else(|| self.now_playing.clone());

                if let Some(first_track) = first_track {
                    playlist_cover = first_track.cover.clone();

                    if playlist_cover.is_none() {
                        if let Some(cover_url) = first_track.cover_url.as_deref() {
                            playlist_cover = self.fetch_cover_with_disk_cache(cover_url).await
                        }
                    }

                    if playlist_cover.is_none() {
                        if let Ok(detail) = self.api.song_detail(&first_track.song_id).await {
                            if let Some(song) = detail
                                .body
                                .get("songs")
                                .and_then(|value| value.as_array())
                                .and_then(|items| items.first())
                            {
                                if let Some(cover_url) =
                                    song.pointer("/al/picUrl").and_then(|value| value.as_str())
                                {
                                    playlist_cover =
                                        self.fetch_cover_with_disk_cache(cover_url).await
                                }
                            }
                        }
                    }
                }
            }

            bootstrap.playlist_cover = playlist_cover;
            bootstrap.current_track = Some(seed);
        }

        bootstrap
    }

    pub fn set_runtime_status(&mut self, text: impl Into<String>) {
        let text = text.into();
        self.home.status_line = text.clone();
        self.search.status_line = text;
    }

    pub fn persist_playback_memory_on_exit(&self) {
        self.persist_playback_memory();
    }

    fn clear_playback_memory(&self) {
        let _ = playback_session::clear();
    }

    fn persist_playback_memory(&self) {
        if !self.config.playback_memory || self.playback_queue.is_empty() {
            return;
        }

        let queue = self
            .playback_queue
            .iter()
            .map(|track| playback_session::PlaybackSessionTrack {
                song_id: track.song_id.clone(),
                title: track.title.clone(),
                artist: track.artist.clone(),
                album: track.album.clone(),
                duration_ms: track.duration_ms,
                cover_url: track.cover_url.clone(),
            })
            .collect::<Vec<_>>();

        let record = playback_session::PlaybackSessionRecord {
            queue,
            current_index: self.playback_index,
            repeat_mode: Some(playback_repeat_mode_key(self.playback_repeat_mode).to_string()),
            source_playlist_id: self.playback_queue_source_id.clone(),
            updated_at: 0,
        };

        let _ = playback_session::save(&record);
    }

    async fn try_restore_playback_memory(&mut self) {
        if !self.config.playback_memory {
            return;
        }

        let Ok(Some(record)) = playback_session::load() else {
            return;
        };

        let queue = record
            .queue
            .into_iter()
            .filter_map(|track| {
                let song_id = track.song_id.trim().to_string();
                if song_id.is_empty() {
                    return None;
                }
                Some(PlaybackTrack {
                    song_id,
                    title: track.title,
                    artist: track.artist,
                    album: track.album,
                    duration_ms: track.duration_ms,
                    cover_url: track.cover_url,
                    cover: None,
                    lyrics: None,
                })
            })
            .collect::<Vec<_>>();

        if queue.is_empty() {
            return;
        }

        if let Some(mode) = record
            .repeat_mode
            .as_deref()
            .and_then(playback_repeat_mode_from_key)
        {
            self.playback_repeat_mode = mode;
        }

        self.playback_queue = queue;
        self.playback_queue_cover_url = None;
        // 还原队列来源，使漫游的尾部追加/续播/封面跟随在重启后依然生效。
        self.playback_queue_source_id = record.source_playlist_id.clone();
        let target = record
            .current_index
            .unwrap_or(0)
            .min(self.playback_queue.len().saturating_sub(1));
        self.play_queue_index(target, false).await;
        self.set_runtime_status(self.lang_text("已恢复播放记忆", "Playback memory restored"));
    }

    fn lang_text<'a>(&self, zh: &'a str, en: &'a str) -> &'a str {
        lang_text(self.config.language, zh, en)
    }

    async fn refresh_vip_audio_access(&mut self) {
        let unlocked = fetch_vip_unlocked(&mut self.api).await;
        self.apply_vip_audio_access(unlocked);
    }

    fn apply_vip_audio_access(&mut self, unlocked: bool) {
        self.vip_audio_unlocked = unlocked;
        self.set_audio_quality(self.config.audio_quality);
    }

    fn set_audio_quality(&mut self, quality: AudioQuality) {
        let clamped = quality.clamp_for_vip(self.vip_audio_unlocked);
        if self.config.audio_quality != clamped {
            self.config.audio_quality = clamped;
            let _ = self.config.save();
        }
    }

    pub fn current_page_lyric_lines(&self) -> (String, String) {
        let Some(track) = self.now_playing.as_ref() else {
            return (String::new(), String::new());
        };
        let Some(lines) = track.lyrics.as_ref() else {
            return (String::new(), String::new());
        };
        if lines.is_empty() {
            return (String::new(), String::new());
        }

        let pos_ms = self.playback_position().as_millis() as u64;
        let mut idx = 0usize;
        for (line_idx, line) in lines.iter().enumerate() {
            if line.start_ms <= pos_ms {
                idx = line_idx;
            } else {
                break;
            }
        }

        let current = lines
            .get(idx)
            .map(|line| line.text.clone())
            .unwrap_or_default();
        let next = lines
            .get(idx + 1)
            .map(|line| line.text.clone())
            .unwrap_or_default();
        (current, next)
    }

    async fn logout_to_login(&mut self) {
        self.close_overlay();
        self.page = Page::Login;
        self.search_return_page = Page::Home;
        self.search_box_input.clear();
        self.settings_selected = 0;
        self.settings_playback_selected = 0;
        self.settings_keybind_selected = 0;
        self.settings_keybind_rebinding = None;
        self.session_cookie = None;
        self.api.clear_cookie();
        let _ = session::clear_cookie();
        self.clear_playback_memory();
        let _ = private_roam::clear();
        self.private_roam = PrivateRoamState::default();
        self.vip_audio_unlocked = false;
        self.config.audio_quality = self.config.audio_quality.clamp_for_vip(false);

        self.login = LoginState::default();
        self.search = SearchState::default();
        // 登出同样要作废在途拉取：它们带着上一账号的 cookie 落地，会把已清空的
        // 状态写回旧账号的数据（同 `apply_playlist_fetch` 的规则）。
        self.playlist_fetch = None;
        self.author_fetch = None;
        self.playlist = PlaylistState::default();
        self.author = AuthorState::default();
        self.home = HomeState::default();
        self.home_sidebar = HomeSidebarState::default();
        self.playlist_section_return_snapshot = None;
        self.startup_loading_progress = 0.0;
        self.startup_loading_started_at = None;
        self.startup_loading_complete_started_at = None;
        self.startup_loading_complete_requested = false;
        self.last_global_hotkey_at = None;
        self.last_content_click = None;
        self.clear_content_hits();
        self.audio_player.stop();
        self.now_playing = None;
        self.now_playing_liked = false;
        self.like_machine.clear();
        self.playback_queue.clear();
        self.playback_index = None;
        self.playback_state = PlaybackRuntimeState::Stopped;
        self.playback_repeat_mode = PlaybackRepeatMode::Sequence;

        self.refresh_qr_login().await;
    }

    async fn enter_home_tile(&mut self) {
        if self.home.tiles.is_empty() {
            return;
        }

        let focused = self.home.focused_idx.min(self.home.tiles.len() - 1);
        let title = self.home.tiles[focused].title.clone();
        let Some(playlist_id) = self.home.tiles[focused].id.clone() else {
            self.home.status_line = "当前块暂无可用歌单".to_string();
            return;
        };

        self.home.status_line = format!("正在加载 {}", title);
        let result = if playlist_id == HOME_DAILY_RECOMMEND_TILE_ID {
            self.load_daily_recommend_playlist().await
        } else if playlist_id == HOME_PRIVATE_ROAM_TILE_ID {
            self.load_private_roam_playlist().await
        } else {
            self.load_playlist_detail(&playlist_id).await
        };

        match result {
            Ok(()) => {
                self.playlist_return_page = Page::Home;
                self.playlist_section_return_snapshot = None;
                self.page = Page::Playlist;
                self.home.status_line = format!("已打开 {}", title);
            }
            Err(err) => {
                self.home.status_line = format!("打开歌单失败: {}", err);
            }
        }
    }

    async fn submit_login_action(&mut self) {
        match self.login.method {
            LoginMethod::Qr => {
                if self.login.focus_index == 0 {
                    self.refresh_qr_login().await;
                } else {
                    self.check_qr_status_and_login().await;
                }
            }
            LoginMethod::Username => match self.login.focus_index {
                0 | 1 => self.login.next_focus(),
                _ => self.submit_username_login().await,
            },
            LoginMethod::Phone => match self.login.focus_index {
                0 | 1 => self.login.next_focus(),
                2 => self.send_phone_captcha().await,
                _ => self.submit_phone_login().await,
            },
        }
    }

    async fn refresh_qr_login(&mut self) {
        self.qr_last_poll_at = None;

        match fetch_qr_login_code(&mut self.api, self.config.language).await {
            Ok(code) => self.apply_qr_login_code(code),
            Err(err) => self.login.status_line = format!("{err}"),
        }
    }

    /// 应用一个刚拿到的登录二维码（手动刷新与启动初始化共用）。
    fn apply_qr_login_code(&mut self, code: QrLoginCode) {
        self.login.qr_key = code.key;
        self.login.qr_url = code.url.clone();
        self.login.status_line = if code.url.is_empty() {
            "二维码已刷新，请按 Enter 轮询状态".to_string()
        } else {
            format!("二维码已刷新: {}", truncate_text(&code.url, 48))
        };
    }

    async fn check_qr_status_and_login(&mut self) {
        if self.login.qr_key.trim().is_empty() {
            self.login.status_line = "请先按 Enter 刷新二维码".to_string();
            return;
        }

        let response = match self.api.login_qr_check(&self.login.qr_key).await {
            Ok(response) => response,
            Err(err) => {
                self.login.status_line = format!("轮询二维码失败: {}", err);
                return;
            }
        };

        let code = response_code(&response);
        match code {
            800 => {
                self.login.status_line = "二维码已过期，已自动刷新".to_string();
                self.refresh_qr_login().await;
            }
            801 => self.login.status_line = "等待扫码".to_string(),
            802 => self.login.status_line = "已扫码，等待确认".to_string(),
            803 | 200 => self.mark_login_success("二维码登录成功").await,
            _ => {
                self.login.status_line =
                    format!("二维码状态异常({}): {}", code, response_message(&response))
            }
        }
    }

    async fn submit_username_login(&mut self) {
        let username = self.login.username.trim().to_string();
        if username.is_empty() || self.login.password.trim().is_empty() {
            self.login.status_line = "请填写用户名和密码".to_string();
            return;
        }

        let response = match self.api.login_email(&username, &self.login.password).await {
            Ok(response) => response,
            Err(err) => {
                self.login.status_line = format!("登录失败: {}", err);
                return;
            }
        };

        let code = response_code(&response);
        if code == 200 {
            let nickname = response.body["profile"]["nickname"]
                .as_str()
                .unwrap_or("用户");
            self.mark_login_success(&format!("欢迎回来，{}", nickname))
                .await;
            return;
        }

        self.login.status_line = format!("登录失败({}): {}", code, response_message(&response));
    }

    async fn send_phone_captcha(&mut self) {
        let phone = self.login.phone.trim().to_string();
        if phone.is_empty() {
            self.login.status_line = "请输入手机号".to_string();
            return;
        }

        let response = match self.api.captcha_sent(&phone).await {
            Ok(response) => response,
            Err(err) => {
                self.login.status_line = format!("验证码发送失败: {}", err);
                return;
            }
        };

        let code = response_code(&response);
        if code == 200 {
            self.login.status_line = format!("验证码已发送到 {}", phone);
            return;
        }

        self.login.status_line = format!("发送失败({}): {}", code, response_message(&response));
    }

    async fn submit_phone_login(&mut self) {
        let phone = self.login.phone.trim().to_string();
        let captcha = self.login.captcha.trim().to_string();

        if phone.is_empty() || captcha.is_empty() {
            self.login.status_line = "请填写手机号和验证码".to_string();
            return;
        }

        let response = match self.api.login_phone_captcha(&phone, &captcha).await {
            Ok(response) => response,
            Err(err) => {
                self.login.status_line = format!("手机号登录失败: {}", err);
                return;
            }
        };

        let code = response_code(&response);
        if code == 200 {
            let nickname = response.body["profile"]["nickname"]
                .as_str()
                .unwrap_or("用户");
            self.mark_login_success(&format!("欢迎回来，{}", nickname))
                .await;
            return;
        }

        self.login.status_line = format!("登录失败({}): {}", code, response_message(&response));
    }

    async fn load_home_recommendations(&mut self) -> Result<()> {
        let tiles = fetch_home_tiles(&mut self.api, self.config.home_more_recommend).await;
        self.apply_home_tiles(tiles);
        Ok(())
    }

    /// 应用首页推荐 tile（首页刷新与启动初始化共用）。
    fn apply_home_tiles(&mut self, tiles: Vec<HomeTile>) {
        self.home.set_tiles(tiles);
        // 私人漫游 tile 封面：未播放过时为首歌封面，播放后为最后播放歌曲的封面
        self.sync_home_roam_tile_cover();
        self.home.status_line = self
            .lang_text(
                "方向键/Tab 切换，Enter 打开歌单",
                "Use arrows/Tab to focus, Enter to open playlist",
            )
            .to_string();
    }

    async fn resolve_current_user_id(&mut self) -> Result<String> {
        if let Some(uid) = self.home_sidebar.user_id.as_ref() {
            return Ok(uid.clone());
        }

        let profile = fetch_account_profile(&mut self.api, self.config.language).await?;
        self.apply_account_profile(profile);
        Ok(self.home_sidebar.user_id.clone().unwrap_or_default())
    }

    /// 应用账号档案（侧边栏用户名 / uid / 我喜欢歌单 id）。
    fn apply_account_profile(&mut self, profile: AccountProfile) {
        self.home_sidebar.user_id = Some(profile.uid);
        self.home_sidebar.liked_playlist_id = profile.liked_playlist_id;
        if let Some(name) = profile.name {
            self.home_sidebar.user_name = name;
        }
    }

    async fn refresh_liked_song_cache(&mut self) -> Result<()> {
        let uid = self.resolve_current_user_id().await?;
        let ids = fetch_liked_song_ids(&mut self.api, &uid, self.config.language).await?;
        self.apply_liked_song_ids(ids);
        Ok(())
    }

    /// 应用「我喜欢的音乐」全量 id 集合。
    fn apply_liked_song_ids(&mut self, ids: HashSet<String>) {
        self.like_machine.replace_confirmed(ids);
        self.refresh_now_playing_like_state();
    }

    fn is_liked_playlist(&self, playlist_id: &str, title: Option<&str>) -> bool {
        is_liked_playlist(
            self.home_sidebar.liked_playlist_id.as_deref(),
            playlist_id,
            title,
        )
    }

    async fn load_playlist_detail(&mut self, playlist_id: &str) -> Result<()> {
        let fetch = fetch_playlist_page(
            self.api.clone(),
            self.config.language,
            playlist_id.to_string(),
            None,
            self.home_sidebar.liked_playlist_id.clone(),
            self.home_sidebar.user_id.clone(),
        )
        .await
        .map_err(anyhow::Error::msg)?;
        self.apply_playlist_fetch(fetch);
        Ok(())
    }

    /// 把拉到的歌单/专辑数据落到状态上（阻塞版与 `tick_playlist_fetch` 共用）。
    ///
    /// 落状态即宣告"这一页换成了新来源"：在途的那次拉取随之作废，否则它迟到时
    /// 会把刚打开的页面覆盖成被放弃的那一份（`tick_playlist_fetch` 只看句柄）。
    fn apply_playlist_fetch(&mut self, fetch: PlaylistFetch) {
        self.playlist_fetch = None;
        self.playlist.id = Some(fetch.id);
        self.playlist.title = fetch.title;
        self.playlist.artist = fetch.artist;
        self.playlist.description = fetch.description;
        self.playlist.set_tracks(fetch.tracks);
        if let Some(url) = fetch.cover_url {
            self.playlist.cover.load(self.api.clone(), url);
        }

        if let Some(liked) = fetch.liked {
            if let Some(profile) = liked.profile {
                self.apply_account_profile(profile);
            }
            self.apply_liked_song_ids(liked.ids);
        }
    }

    async fn load_daily_recommend_playlist(&mut self) -> Result<()> {
        let response = self.api.recommend_songs().await?;
        let code = response_code(&response);
        if code != 200 {
            return Err(anyhow!(
                "请求失败({}): {}",
                code,
                response_message(&response)
            ));
        }

        let songs = home_daily_song_items(&response.body).ok_or_else(|| {
            anyhow!(self.lang_text("每日推荐数据缺失", "Daily recommendations are missing"))
        })?;

        let tracks = parse_tracks(songs);
        if tracks.is_empty() {
            return Err(anyhow!(
                self.lang_text("每日推荐为空", "Daily recommendations are empty")
            ));
        }

        let cover_url = tracks.iter().find_map(|track| track.cover_url.clone());

        // 这一页换成每日推荐：在途的占位拉取作废（同 `apply_playlist_fetch`）。
        self.playlist_fetch = None;
        self.playlist.id = Some(HOME_DAILY_RECOMMEND_TILE_ID.to_string());
        self.playlist.title = self
            .lang_text("每日推荐", "Daily Recommendations")
            .to_string();
        self.playlist.artist = self
            .lang_text("网易云音乐", "Netease Cloud Music")
            .to_string();
        self.playlist.description = self
            .lang_text(
                "来自网易云每日推荐歌曲，按 Enter 播放",
                "Daily songs from Netease. Press Enter to play",
            )
            .to_string();
        self.playlist.set_tracks(tracks);
        cover_url.map(|x| self.playlist.cover.load(self.api.clone(), x));
        Ok(())
    }

    async fn load_private_roam_playlist(&mut self) -> Result<()> {
        // 兜底：内存列表为空（首次使用且启动刷新失败过）时现场拉取
        if self.private_roam.tracks.is_empty() {
            let fetched = fetch_private_roam_songs(&mut self.api).await;
            if fetched.is_empty() {
                return Err(anyhow!(
                    self.lang_text("私人漫游为空", "Private roam is empty")
                ));
            }
            self.private_roam.tracks = fetched;
            if self.private_roam.cover_url.is_none() {
                self.private_roam.cover_url = self
                    .private_roam
                    .tracks
                    .first()
                    .and_then(|track| track.cover_url.clone());
            }
            self.persist_private_roam();
        }

        let tracks = self.private_roam.tracks.clone();
        let cover_url = self.private_roam.cover_url.clone();
        let focus_index = self.private_roam.last_played_index;

        // 这一页换成私人漫游：在途的占位拉取作废（同 `apply_playlist_fetch`）。
        self.playlist_fetch = None;
        self.playlist.id = Some(HOME_PRIVATE_ROAM_TILE_ID.to_string());
        self.playlist.title = self.lang_text("私人漫游", "Private Roam").to_string();
        self.playlist.artist = self
            .lang_text("网易云音乐", "Netease Cloud Music")
            .to_string();
        self.playlist.description = self
            .lang_text(
                "来自网易云私人漫游，按 Enter 播放",
                "Private roam songs from Netease. Press Enter to play",
            )
            .to_string();
        self.playlist.set_tracks(tracks);
        // 进入漫游后默认聚焦到最后播放的歌曲
        if let Some(index) = focus_index {
            if index < self.playlist.tracks.len() {
                self.playlist.focused_idx = index;
                self.playlist.scroll_offset = index;
            }
        }
        cover_url.map(|x| self.playlist.cover.load(self.api.clone(), x));
        Ok(())
    }

    /// 今天是否已刷新过私人漫游（刷新判据只有这一处）。
    fn private_roam_refreshed_today(&self) -> bool {
        self.private_roam.last_refresh_day == Some(today_day_number())
    }

    /// 应用一批新拉取的漫游歌曲（启动初始化与每日刷新共用）。
    fn apply_private_roam_refresh(&mut self, fetched: Vec<PlaylistTrack>) {
        let today = today_day_number();
        if fetched.is_empty() {
            // 拉取失败保留旧列表，下次启动再试
            return;
        }

        let (new_tracks, new_index) = merge_private_roam_refresh(
            &self.private_roam.tracks,
            self.private_roam.last_played_index,
            fetched,
        );

        self.private_roam.tracks = new_tracks;
        self.private_roam.last_played_index = new_index;
        if self.private_roam.cover_url.is_none() {
            self.private_roam.cover_url = self
                .private_roam
                .tracks
                .first()
                .and_then(|track| track.cover_url.clone());
        }
        self.private_roam.last_refresh_day = Some(today);
        self.persist_private_roam();
    }

    /// 播放到列表末尾时追加一批新歌
    async fn append_private_roam_songs(&mut self) {
        let fetched = fetch_private_roam_songs(&mut self.api).await;
        if fetched.is_empty() {
            return;
        }

        let mut added = false;
        let mut new_queue_items = Vec::new();
        for track in fetched {
            if !self.private_roam.tracks.iter().any(|t| t.id == track.id) {
                if let Some(item) = PlaybackTrack::from_playlist_track(&track) {
                    new_queue_items.push(item);
                }
                self.private_roam.tracks.push(track);
                added = true;
            }
        }
        if !added {
            return;
        }

        self.persist_private_roam();

        // 扩展播放队列是播放行为，只看队列来源，不看当前在哪个页面：
        // 否则重启后（页面停在主页）新歌只进列表不进队列，追加等于白做。
        if self.playback_queue_is_roam() {
            self.playback_queue.extend(new_queue_items);
        }

        // 列表页 UI 同步则确实只在该页打开时才需要。
        if self.playlist.id.as_deref() == Some(HOME_PRIVATE_ROAM_TILE_ID) {
            self.playlist.tracks = self.private_roam.tracks.clone();
        }
    }

    /// 播放队列切到某首歌时，记录漫游播放位置与封面；播放到最后一首时追加新歌
    async fn track_private_roam_playback(&mut self, track: &PlaybackTrack) {
        let Some(pos) = self
            .private_roam
            .tracks
            .iter()
            .position(|t| t.id.as_deref() == Some(track.song_id.as_str()))
        else {
            return;
        };

        let is_last = pos + 1 == self.private_roam.tracks.len();
        self.private_roam.last_played_index = Some(pos);
        if let Some(cover) = self.private_roam.tracks[pos].cover_url.clone() {
            self.private_roam.cover_url = Some(cover.clone());
            self.private_roam.last_played_cover_url = Some(cover.clone());
            // 列表页封面同步为播放到的歌曲封面
            if self.playlist.id.as_deref() == Some(HOME_PRIVATE_ROAM_TILE_ID) {
                self.playlist.cover.load(self.api.clone(), cover.clone());
            }
            // 漫游没有固定的列表封面，其语义是「跟随当前播放歌曲」。
            // 队列来源封面（全屏侧边栏用）同步更新，否则会停在换队列那一刻的旧封面。
            if self.playback_queue_is_roam() {
                self.playback_queue_cover_url = Some(cover);
            }
        }
        self.sync_home_roam_tile_cover();
        self.persist_private_roam();

        if is_last && self.playback_queue_is_roam() {
            self.append_private_roam_songs().await;
        }
    }

    /// 当前播放队列是否来自私人漫游。
    ///
    /// 判据是随播放记忆持久化的来源 id，而非 `self.playlist.id`——后者是
    /// 当前浏览页面，重启后为 None，会让漫游退化成普通歌单。
    fn playback_queue_is_roam(&self) -> bool {
        self.playback_queue_source_id.as_deref() == Some(HOME_PRIVATE_ROAM_TILE_ID)
    }

    fn persist_private_roam(&self) {
        let record = private_roam::PrivateRoamRecord {
            tracks: self
                .private_roam
                .tracks
                .iter()
                .map(|track| private_roam::PrivateRoamTrack {
                    song_id: track.id.clone().unwrap_or_default(),
                    title: track.title.clone(),
                    artist: track.artist.clone(),
                    album: track.album.clone(),
                    duration_ms: track.duration_ms,
                    cover_url: track.cover_url.clone(),
                })
                .collect(),
            last_played_index: self.private_roam.last_played_index,
            last_played_cover_url: self.private_roam.last_played_cover_url.clone(),
            last_refresh_day: self.private_roam.last_refresh_day,
            updated_at: 0,
        };
        let _ = private_roam::save(&record);
    }

    fn load_private_roam_memory(&mut self) {
        let Ok(Some(record)) = private_roam::load() else {
            return;
        };

        self.private_roam.tracks = record
            .tracks
            .into_iter()
            .filter_map(|track| {
                if track.song_id.is_empty() {
                    return None;
                }
                Some(PlaylistTrack {
                    kind: PlaylistTrackKind::Song,
                    id: Some(track.song_id),
                    title: track.title,
                    artist: track.artist,
                    album: track.album,
                    cover_url: track.cover_url,
                    duration_ms: track.duration_ms,
                    duration: format_duration(track.duration_ms),
                })
            })
            .collect();
        self.private_roam.last_played_index = record.last_played_index;
        self.private_roam.last_played_cover_url = record.last_played_cover_url.clone();
        self.private_roam.last_refresh_day = record.last_refresh_day;
        self.private_roam.cover_url = record.last_played_cover_url.or_else(|| {
            self.private_roam
                .tracks
                .first()
                .and_then(|track| track.cover_url.clone())
        });
    }

    fn sync_home_roam_tile_cover(&mut self) {
        let Some(url) = self.private_roam.cover_url.clone() else {
            return;
        };
        if let Some(tile) = self
            .home
            .tiles
            .iter_mut()
            .find(|tile| tile.title == "私人漫游")
        {
            tile.cover.load(self.api.clone(), url);
        }
    }

    async fn load_album_detail(&mut self, album_id: &str) -> Result<()> {
        let fetch = fetch_album_page(
            self.api.clone(),
            self.config.language,
            album_id.to_string(),
            None,
        )
        .await
        .map_err(anyhow::Error::msg)?;
        self.apply_playlist_fetch(fetch);
        Ok(())
    }

    /// 把拉到的作者页数据落到状态上（`tick_author_fetch` 搬运结果时用）。
    ///
    /// 同 `apply_playlist_fetch`：新数据落地即在途拉取作废，免得迟到的旧结果覆盖它。
    fn apply_author_fetch(&mut self, fetch: AuthorFetch) {
        self.author_fetch = None;
        self.author.id = Some(fetch.id);
        self.author.title = fetch.title;
        self.author.artist = fetch.artist;
        self.author.description = fetch.description;
        if let Some(url) = fetch.cover_url {
            self.author.cover.load(self.api.clone(), url);
        }
        self.author.set_tiles(fetch.tiles);
        self.author.hot_songs = fetch.hot_songs;
        self.author.albums = fetch.albums;
        self.author.eps = fetch.eps;
        self.author.singles = fetch.singles;
        self.author.focused_idx = 0;
    }

    /// 解析 `artist/*` 的回包（网络部分见 `fetch_artist_responses`）。
    ///
    /// 不借 `&mut self`：全屏页那条把整段解析连同请求一起丢给 `shot_and_share` 后台跑。
    fn build_author_page(
        api: &ApiState,
        language: Language,
        artist_id: &str,
        responses: AuthorResponses,
    ) -> Result<AuthorFetch, String> {
        let AuthorResponses {
            detail,
            desc,
            top_song,
            album,
        } = responses;

        if detail.is_none() && desc.is_none() && top_song.is_none() && album.is_none() {
            return Err(
                lang_text(language, "作者数据获取失败", "Failed to fetch artist data").to_string(),
            );
        }

        let mut title = String::new();
        let mut description = String::new();
        let mut cover_url = None;

        if let Some(response) = detail.as_ref() {
            if response_code(response) == 200 {
                title = first_non_empty(
                    &response.body,
                    &["/data/artist/name", "/artist/name", "/data/name"],
                )
                .unwrap_or_default();
                cover_url = first_non_empty(
                    &response.body,
                    &[
                        "/data/artist/avatarUrl",
                        "/data/artist/cover",
                        "/artist/picUrl",
                        "/artist/img1v1Url",
                        "/artist/avatarUrl",
                    ],
                );
                description = first_non_empty(
                    &response.body,
                    &["/data/artist/briefDesc", "/artist/briefDesc"],
                )
                .unwrap_or_default();
            }
        }

        if title.is_empty() {
            if let Some(response) = top_song.as_ref() {
                if let Some(first_song) = response
                    .body
                    .get("songs")
                    .and_then(|value| value.as_array())
                    .and_then(|songs| songs.first())
                {
                    title = parse_artists(first_song).unwrap_or_default();
                }
            }
        }

        if let Some(response) = desc.as_ref() {
            if response_code(response) == 200 {
                if let Some(text) =
                    first_non_empty(&response.body, &["/briefDesc", "/data/briefDesc"])
                {
                    if !text.trim().is_empty() {
                        description = text;
                    }
                }

                if description.trim().is_empty() {
                    if let Some(text) = first_non_empty_intro_text(&response.body) {
                        description = text;
                    }
                }
            }
        }

        if title.trim().is_empty() {
            title = lang_text(language, "未知作者", "Unknown Author").to_string();
        }

        if description.trim().is_empty() {
            description =
                lang_text(language, "暂无作者简介", "No author description yet").to_string();
        }

        let mut hot_songs = Vec::new();
        let mut albums = Vec::new();
        let mut eps = Vec::new();
        let mut singles = Vec::new();

        if let Some(response) = top_song.as_ref() {
            if response_code(response) == 200 {
                if let Some(items) = response
                    .body
                    .get("songs")
                    .and_then(|value| value.as_array())
                {
                    hot_songs = parse_tracks(items);
                }
            }
        }

        if let Some(response) = album.as_ref() {
            if response_code(response) == 200 {
                let album_items = response
                    .body
                    .get("hotAlbums")
                    .and_then(|value| value.as_array())
                    .or_else(|| {
                        response
                            .body
                            .pointer("/artist/albums")
                            .and_then(|value| value.as_array())
                    });

                if let Some(items) = album_items {
                    for item in items {
                        let Some(name) = item.get("name").and_then(|value| value.as_str()) else {
                            continue;
                        };

                        let size = item
                            .get("size")
                            .and_then(|value| value.as_i64())
                            .unwrap_or_default();

                        let kind = artist_album_kind(item);
                        let cover_url = first_non_empty(item, &["/picUrl", "/blurPicUrl"]);
                        let track = PlaylistTrack {
                            kind: match kind {
                                AuthorTileKind::HotSong => PlaylistTrackKind::Song,
                                AuthorTileKind::Album => PlaylistTrackKind::Album,
                                AuthorTileKind::Ep => PlaylistTrackKind::Ep,
                                AuthorTileKind::Single => PlaylistTrackKind::Single,
                            },
                            id: parse_value_as_string(item.get("id")),
                            title: name.to_string(),
                            artist: first_non_empty(item, &["/artist/name", "/artists/0/name"])
                                .unwrap_or_else(|| title.clone()),
                            album: name.to_string(),
                            cover_url,
                            duration_ms: 0,
                            duration: format!("{} {}", size, lang_text(language, "首", "tracks")),
                        };

                        match kind {
                            AuthorTileKind::HotSong | AuthorTileKind::Album => albums.push(track),
                            AuthorTileKind::Ep => eps.push(track),
                            AuthorTileKind::Single => singles.push(track),
                        }
                    }
                }
            }
        }

        let hot_count = hot_songs.len();
        let album_count = albums.len();
        let ep_count = eps.len();
        let single_count = singles.len();

        let tiles = vec![
            AuthorTile::from_album(
                api,
                lang_text(language, "热门歌曲", "Hot Songs").to_string(),
                format!("{} {}", hot_count, lang_text(language, "首", "tracks")),
                hot_songs
                    .first()
                    .and_then(|track| track.cover_url.clone())
                    .or_else(|| cover_url.clone()),
                AuthorTileKind::HotSong,
            ),
            AuthorTile::from_album(
                api,
                lang_text(language, "专辑", "Albums").to_string(),
                format!("{} {}", album_count, lang_text(language, "张", "items")),
                albums
                    .first()
                    .and_then(|track| track.cover_url.clone())
                    .or_else(|| cover_url.clone()),
                AuthorTileKind::Album,
            ),
            AuthorTile::from_album(
                api,
                "EP".to_string(),
                format!("{} {}", ep_count, lang_text(language, "张", "items")),
                eps.first()
                    .and_then(|track| track.cover_url.clone())
                    .or_else(|| cover_url.clone()),
                AuthorTileKind::Ep,
            ),
            AuthorTile::from_album(
                api,
                "Single".to_string(),
                format!("{} {}", single_count, lang_text(language, "张", "items")),
                singles
                    .first()
                    .and_then(|track| track.cover_url.clone())
                    .or_else(|| cover_url.clone()),
                AuthorTileKind::Single,
            ),
        ];

        let artist = match language {
            Language::Zh => format!(
                "热门 {} · 专辑 {} · EP {} · Single {}",
                hot_count, album_count, ep_count, single_count
            ),
            Language::En => format!(
                "Hot {} · Albums {} · EP {} · Singles {}",
                hot_count, album_count, ep_count, single_count
            ),
        };

        Ok(AuthorFetch {
            id: artist_id.to_string(),
            title,
            artist,
            description,
            cover_url,
            tiles,
            hot_songs,
            albums,
            eps,
            singles,
        })
    }

    async fn execute_search(&mut self) -> Result<()> {
        let (keywords, scope) = parse_search_input(&self.search.query);
        let followed_author_query = is_followed_author_query(&keywords, scope);
        if keywords.is_empty() && !followed_author_query {
            self.search.status_line = "请输入搜索关键词".to_string();
            self.search.set_results(Vec::new(), 0, false);
            return Ok(());
        }

        self.search.scope = scope;
        self.search.next_offset = 0;
        self.search.has_more = true;

        if scope == SearchScope::Mixed {
            return self.execute_mixed_search(&keywords).await;
        }

        let response = if followed_author_query {
            self.api.artist_sublist(SEARCH_RESULT_PAGE_SIZE, 0).await?
        } else {
            let search_type = scope.search_type().unwrap_or(1);
            self.api
                .search(&keywords, search_type, SEARCH_RESULT_PAGE_SIZE, 0)
                .await?
        };
        let code = response_code(&response);
        if code != 200 {
            return Err(anyhow!(
                "请求失败({}): {}",
                code,
                response_message(&response)
            ));
        }

        if followed_author_query {
            let page = parse_followed_author_page(&response);
            let count = page.items.len();
            let next_offset = page.fetched_count;
            let has_more = followed_author_has_more(&page, next_offset);
            let mut items = page.items;
            self.load_search_item_covers(&mut items);
            self.search.set_results(items, next_offset, has_more);
            self.search.status_line = format!("{} 搜索完成，共 {} 条", scope.display_name(), count);
            return Ok(());
        }

        let mut items = parse_search_items(&response, scope);
        let count = items.len();
        self.load_search_item_covers(&mut items);
        self.search
            .set_results(items, count, count >= SEARCH_RESULT_PAGE_SIZE);
        self.search.status_line = format!("{} 搜索完成，共 {} 条", scope.display_name(), count);
        Ok(())
    }

    /// 无后缀搜索：并发拉取作者 / 歌单 / 单曲，按 作者 → 歌单 → 单曲 拼接。
    /// 辅助分区（作者 / 歌单）失败只让该分区为空，不影响单曲结果；只有分页游标跟着单曲走。
    async fn execute_mixed_search(&mut self, keywords: &str) -> Result<()> {
        let mut artists_api = self.api.clone();
        let mut playlists_api = self.api.clone();
        let (songs, artists, playlists) = futures::join!(
            self.api.search(keywords, 1, SEARCH_RESULT_PAGE_SIZE, 0),
            artists_api.search(keywords, 100, MIXED_AUX_RESULT_LIMIT, 0),
            playlists_api.search(keywords, 1000, MIXED_AUX_RESULT_LIMIT, 0),
        );

        let songs = songs?;
        let code = response_code(&songs);
        if code != 200 {
            let message = response_message(&songs);
            return Err(anyhow!("请求失败({}): {}", code, message));
        }

        let mut items = parse_optional_search_section(artists, SearchScope::Author);
        items.extend(parse_optional_search_section(
            playlists,
            SearchScope::Playlist,
        ));
        let song_items = parse_search_items(&songs, SearchScope::Single);
        let song_count = song_items.len();
        items.extend(song_items);

        self.load_search_item_covers(&mut items);
        let total = items.len();
        self.search
            .set_results(items, song_count, song_count >= SEARCH_RESULT_PAGE_SIZE);
        self.search.status_line = format!("搜索完成，共 {} 条", total);
        Ok(())
    }

    /// 作者条目要渲染头像：结果落定时一次性发起封面请求（复用封面管线）。
    fn load_search_item_covers(&self, items: &mut [SearchItem]) {
        for item in items.iter_mut() {
            if item.kind != SearchItemKind::Artist || item.cover.url.is_some() {
                continue;
            }
            if let Some(url) = item.cover_url.clone() {
                item.cover.load(self.api.clone(), url);
            }
        }
    }

    async fn load_more_search_results(&mut self) -> Result<usize> {
        if !self.search.has_more {
            return Ok(0);
        }

        let (keywords, scope) = parse_search_input(&self.search.query);
        let followed_author_query = is_followed_author_query(&keywords, scope);
        if keywords.is_empty() && !followed_author_query {
            return Ok(0);
        }

        // 混合搜索只有末尾的单曲分区可继续分页；辅助分区固定取最相关若干条。
        let search_type = if scope == SearchScope::Mixed {
            1
        } else {
            scope.search_type().unwrap_or(1)
        };
        let response = if followed_author_query {
            self.api
                .artist_sublist(SEARCH_RESULT_PAGE_SIZE, self.search.next_offset)
                .await?
        } else {
            self.api
                .search(
                    &keywords,
                    search_type,
                    SEARCH_RESULT_PAGE_SIZE,
                    self.search.next_offset,
                )
                .await?
        };
        let code = response_code(&response);
        if code != 200 {
            return Err(anyhow!(
                "请求失败({}): {}",
                code,
                response_message(&response)
            ));
        }

        if followed_author_query {
            let mut page = parse_followed_author_page(&response);
            // 追加的条目同样要发起头像请求，否则第二页起的作者卡片没有头像。
            self.load_search_item_covers(&mut page.items);
            let fetched_count = page.fetched_count;
            let added = page.items.len();
            self.search.results.append(&mut page.items);
            self.search.next_offset = self.search.next_offset.saturating_add(fetched_count);
            self.search.has_more = followed_author_has_more(&page, self.search.next_offset);

            if added == 0 {
                if self.search.has_more {
                    self.search.status_line = format!(
                        "{} 已加载 {} 条",
                        scope.display_name(),
                        self.search.results.len()
                    );
                } else {
                    self.search.status_line = format!(
                        "{} 搜索结果已全部加载，共 {} 条",
                        scope.display_name(),
                        self.search.results.len()
                    );
                }
                return Ok(0);
            }

            self.search.status_line = format!(
                "{} 已加载 {} 条",
                scope.display_name(),
                self.search.results.len()
            );
            return Ok(added);
        }

        // 混合搜索的分页页就是单曲分区。
        let page_scope = match scope {
            SearchScope::Mixed => SearchScope::Single,
            other => other,
        };
        let mut items = parse_search_items(&response, page_scope);
        self.load_search_item_covers(&mut items);
        let added = self.search.append_results(items);
        self.search.next_offset = self.search.next_offset.saturating_add(added);
        self.search.has_more = added >= SEARCH_RESULT_PAGE_SIZE;

        if added == 0 {
            self.search.has_more = false;
            self.search.status_line = format!(
                "{} 搜索结果已全部加载，共 {} 条",
                scope.display_name(),
                self.search.results.len()
            );
            return Ok(0);
        }

        self.search.status_line = format!(
            "{} 已加载 {} 条",
            scope.display_name(),
            self.search.results.len()
        );
        Ok(added)
    }

    async fn mark_login_success(&mut self, text: &str) {
        self.session_cookie = self.api.session_cookie().map(|value| value.to_string());
        if let Some(cookie) = self.session_cookie.as_deref() {
            let _ = session::save_cookie(cookie);
        }
        self.refresh_vip_audio_access().await;
        let _ = self.refresh_liked_song_cache().await;
        self.home_sidebar = HomeSidebarState::default();
        self.playlist_section_return_snapshot = None;
        self.home.status_line = text.to_string();
        // 登录后这次刷新仍走同步链路：进度条只按时间缓动，不接后台步数。
        self.startup.reset_steps(0, 1);
        self.begin_startup_loading(Page::Home);
        if let Err(err) = self.load_home_recommendations().await {
            self.home.status_line = format!("{}，推荐歌单加载失败: {}", text, err);
        }
        self.finish_startup_loading();
        self.try_restore_playback_memory().await;
    }
}

fn playback_repeat_mode_key(mode: PlaybackRepeatMode) -> &'static str {
    match mode {
        PlaybackRepeatMode::Sequence => "sequence",
        PlaybackRepeatMode::Shuffle => "shuffle",
        PlaybackRepeatMode::LoopAll => "loop_all",
        PlaybackRepeatMode::LoopOne => "loop_one",
    }
}

fn playback_repeat_mode_from_key(value: &str) -> Option<PlaybackRepeatMode> {
    match value {
        "sequence" => Some(PlaybackRepeatMode::Sequence),
        "shuffle" => Some(PlaybackRepeatMode::Shuffle),
        "loop_all" => Some(PlaybackRepeatMode::LoopAll),
        "loop_one" => Some(PlaybackRepeatMode::LoopOne),
        _ => None,
    }
}

/// 加载页进度：已完成步数 + 当前步的时间缓动。
///
/// 只有后台初始化真正走完的步骤才会推进进度条，单步内的缓动只是为了在
/// 跳步之间保持呼吸感；收尾时再由 ramp 补到 1.0（超时跳过剩余步骤也走这条路）。
fn startup_loading_progress(
    step_done: usize,
    step_total: usize,
    step_elapsed: f32,
    complete_elapsed: Option<f32>,
    complete_requested: bool,
) -> f32 {
    // 单步内的非线性缓动（沿用原来的 cubic-bezier 曲线手感）。
    let t = (step_elapsed / STARTUP_LOADING_FILL_SECS).clamp(0.0, 1.0);
    let eased = cubic_bezier_y(t, 0.08, 0.98);
    // 单步最多推进到 1/total 的份额，留出到下一步的余量。
    let within = (eased / step_total.max(1) as f32).min(0.96);
    let base = ((step_done as f32 / step_total.max(1) as f32) + within).min(0.96);

    if !complete_requested {
        return base;
    }

    let complete_t =
        (complete_elapsed.unwrap_or(0.0) / STARTUP_LOADING_COMPLETE_RAMP_SECS).clamp(0.0, 1.0);
    let complete_eased = cubic_bezier_y(complete_t, 0.25, 1.0);
    (base + (1.0 - base) * complete_eased).clamp(0.0, 1.0)
}

pub(crate) fn mean_square_to_lufs(mean_square: f32) -> f32 {
    if mean_square <= 1.0e-12 {
        VU_LUFS_FLOOR
    } else {
        (-0.691 + 10.0 * mean_square.log10()).max(VU_LUFS_FLOOR)
    }
}

pub(crate) fn lufs_to_mean_square(lufs: f32) -> f32 {
    if lufs <= VU_LUFS_FLOOR + 0.5 {
        0.0
    } else {
        10.0_f32.powf((lufs + 0.691) / 10.0)
    }
}

pub(crate) fn lufs_to_bar_level(lufs: f32) -> f32 {
    ((lufs - VU_LUFS_MIN) / (VU_LUFS_MAX - VU_LUFS_MIN)).clamp(0.0, 1.0)
}

fn smooth_lufs_level(current: f32, target: f32, dt: f32) -> (f32, bool) {
    let target = target.max(VU_LUFS_FLOOR);
    let mut current = current.max(VU_LUFS_FLOOR);
    if (current - target).abs() <= VU_SETTLED_EPSILON {
        return (target, false);
    }

    let tau = if target > current {
        VU_ATTACK_SECS
    } else {
        VU_RELEASE_SECS
    };
    let amount = if dt <= 0.0 {
        0.0
    } else {
        1.0 - (-dt / tau).exp()
    };
    current += (target - current) * amount;
    if (current - target).abs() <= VU_SETTLED_EPSILON {
        current = target;
    }
    (current, (current - target).abs() > VU_SETTLED_EPSILON)
}

pub(crate) fn cubic_bezier_y(t: f32, p1y: f32, p2y: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    let inv = 1.0 - t;
    let a = 3.0 * inv * inv * t * p1y;
    let b = 3.0 * inv * t * t * p2y;
    let c = t * t * t;
    (a + b + c).clamp(0.0, 1.0)
}

fn mpris_metadata_signature(track: &PlaybackTrack) -> u64 {
    let mut hasher = DefaultHasher::new();
    track.song_id.hash(&mut hasher);
    track.duration_ms.hash(&mut hasher);
    track
        .cover
        .as_ref()
        .map(|bytes| bytes.len())
        .unwrap_or(0)
        .hash(&mut hasher);
    track
        .cover_url
        .as_deref()
        .unwrap_or_default()
        .hash(&mut hasher);
    track
        .lyrics
        .as_ref()
        .map(|lines| lines.len())
        .unwrap_or(0)
        .hash(&mut hasher);
    track
        .lyrics
        .as_ref()
        .and_then(|lines| lines.last().map(|line| line.start_ms))
        .unwrap_or(0)
        .hash(&mut hasher);
    hasher.finish()
}

fn map_audio_state(state: AudioPlayerState) -> PlaybackRuntimeState {
    match state {
        AudioPlayerState::Playing => PlaybackRuntimeState::Playing,
        AudioPlayerState::Paused => PlaybackRuntimeState::Paused,
        AudioPlayerState::Stopped => PlaybackRuntimeState::Stopped,
    }
}

fn braille_from_two_bars(left: u8, right: u8) -> char {
    const LEFT_BITS: [u8; 4] = [6, 2, 1, 0];
    const RIGHT_BITS: [u8; 4] = [7, 5, 4, 3];

    let mut dots = 0u8;
    for idx in 0..left.min(4) {
        dots |= 1 << LEFT_BITS[idx as usize];
    }
    for idx in 0..right.min(4) {
        dots |= 1 << RIGHT_BITS[idx as usize];
    }

    if dots == 0 {
        ' '
    } else {
        char::from_u32(0x2800 + dots as u32).unwrap_or(' ')
    }
}

fn pick_shuffle_index(len: usize, current: usize) -> usize {
    if len <= 1 {
        return 0;
    }

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as usize;
    let mut index = now % len;
    if index == current {
        index = (index + 1) % len;
    }
    index
}

fn response_code(response: &ApiResponse) -> i64 {
    response
        .body
        .get("code")
        .and_then(|value| value.as_i64())
        .unwrap_or(response.status)
}

fn response_message(response: &ApiResponse) -> String {
    if let Some(message) = response.body.get("msg").and_then(|value| value.as_str()) {
        return message.to_string();
    }
    if let Some(message) = response
        .body
        .get("message")
        .and_then(|value| value.as_str())
    {
        return message.to_string();
    }
    "未知错误".to_string()
}

fn truncate_text(text: &str, max_chars: usize) -> String {
    let mut out = String::new();
    for (index, ch) in text.chars().enumerate() {
        if index >= max_chars {
            out.push_str("...");
            return out;
        }
        out.push(ch);
    }
    out
}

fn extract_qr_key(response: &ApiResponse) -> String {
    for pointer in ["/data/unikey", "/data/uniKey", "/unikey", "/uniKey"] {
        if let Some(value) = response
            .body
            .pointer(pointer)
            .and_then(|value| value.as_str())
        {
            if !value.trim().is_empty() {
                return value.to_string();
            }
        }
    }
    String::new()
}

fn extract_qr_url(response: &ApiResponse) -> String {
    for pointer in ["/data/qrurl", "/data/qrUrl", "/qrurl", "/qrUrl"] {
        if let Some(value) = response
            .body
            .pointer(pointer)
            .and_then(|value| value.as_str())
        {
            if !value.trim().is_empty() {
                return value.to_string();
            }
        }
    }
    String::new()
}

fn char_count(text: &str) -> usize {
    text.chars().count()
}

fn byte_index_for_char(text: &str, char_index: usize) -> usize {
    if char_index == 0 {
        return 0;
    }

    text.char_indices()
        .nth(char_index)
        .map(|(idx, _)| idx)
        .unwrap_or(text.len())
}

fn insert_char_at(text: &mut String, char_index: usize, ch: char) {
    let byte_index = byte_index_for_char(text, char_index);
    text.insert(byte_index, ch);
}

fn remove_char_before(text: &mut String, char_index: usize) -> usize {
    if char_index == 0 {
        return 0;
    }

    let start = byte_index_for_char(text, char_index - 1);
    let end = byte_index_for_char(text, char_index);
    if start < end && end <= text.len() {
        text.drain(start..end);
    }
    char_index.saturating_sub(1)
}

fn remove_char_at(text: &mut String, char_index: usize) {
    let start = byte_index_for_char(text, char_index);
    let end = byte_index_for_char(text, char_index + 1);
    if start < end && end <= text.len() {
        text.drain(start..end);
    }
}

fn char_index_for_display_column(text: &str, column: u16) -> usize {
    let mut width = 0usize;
    let target = column as usize;
    let mut index = 0usize;

    for ch in text.chars() {
        let ch_width = ch.width().unwrap_or(1).max(1);
        if width + ch_width > target {
            break;
        }
        width += ch_width;
        index += 1;
    }

    index
}

struct RecommendCard {
    id: Option<String>,
    title: String,
    subtitle: String,
    cover_url: Option<String>,
}

fn home_daily_song_items(body: &Value) -> Option<&[Value]> {
    body.pointer("/data/dailySongs")
        .and_then(|value| value.as_array().map(Vec::as_slice))
        .or_else(|| {
            body.get("dailySongs")
                .and_then(|value| value.as_array().map(Vec::as_slice))
        })
        .or_else(|| {
            body.pointer("/recommend")
                .and_then(|value| value.as_array().map(Vec::as_slice))
        })
        .or_else(|| {
            body.pointer("/data/recommend")
                .and_then(|value| value.as_array().map(Vec::as_slice))
        })
}

fn normalize_home_pinned_title(title: &str) -> Option<&'static str> {
    let compact: String = title.chars().filter(|ch| !ch.is_whitespace()).collect();

    if compact.contains("私人漫游") {
        return Some("私人漫游");
    }
    if compact.contains("私人雷达") {
        return Some("私人雷达");
    }
    if compact.contains("每日推荐") {
        return Some("每日推荐");
    }

    None
}

/// 与 `App::lang_text` 同一套文案选择，供拿不到 `App` 的请求函数使用。
fn lang_text<'a>(lang: Language, zh: &'a str, en: &'a str) -> &'a str {
    match lang {
        Language::Zh => zh,
        Language::En => en,
    }
}

/// 账号档案：uid / 昵称 /「我喜欢的音乐」歌单 id。
#[derive(Clone)]
struct AccountProfile {
    uid: String,
    name: Option<String>,
    liked_playlist_id: Option<String>,
}

/// 当前账号档案：`user/account`，失败时回退 `login/status`。
async fn fetch_account_profile(api: &mut ApiState, lang: Language) -> Result<AccountProfile> {
    let account = match api.user_account().await {
        Ok(response) => response,
        Err(_) => api.login_status().await?,
    };
    let code = response_code(&account);
    if code != 200 {
        return Err(anyhow!(
            "{}({}): {}",
            lang_text(lang, "账号信息请求失败", "Failed to fetch account profile"),
            code,
            response_message(&account)
        ));
    }

    let uid = extract_current_user_id(&account).ok_or_else(|| {
        anyhow!(lang_text(
            lang,
            "未找到当前用户 ID",
            "Current user id not found"
        ))
    })?;

    Ok(AccountProfile {
        uid,
        name: extract_current_user_name(&account),
        liked_playlist_id: extract_liked_playlist_id(&account),
    })
}

///「我喜欢的音乐」全量 id 集合。
async fn fetch_liked_song_ids(
    api: &mut ApiState,
    uid: &str,
    lang: Language,
) -> Result<HashSet<String>> {
    let response = api.likelist(uid).await?;
    let code = response_code(&response);
    if code != 200 {
        return Err(anyhow!(
            "{}({}): {}",
            lang_text(lang, "喜爱列表请求失败", "Failed to fetch liked songs"),
            code,
            response_message(&response)
        ));
    }

    Ok(parse_likelist_song_ids(&response.body))
}

/// 会员音质权限：先查 `vip/info/v2`，未命中再回退 `vip/info`。
async fn fetch_vip_unlocked(api: &mut ApiState) -> bool {
    let mut unlocked = false;

    if let Ok(response) = api.vip_info_v2().await {
        unlocked = response_indicates_vip(&response);
    }

    if !unlocked && let Ok(response) = api.vip_info().await {
        unlocked = response_indicates_vip(&response);
    }

    unlocked
}

/// 歌单封面（私人雷达 tile 用）。
async fn fetch_playlist_cover_url(api: &mut ApiState, playlist_id: &str) -> Option<String> {
    let response = api.playlist_detail(playlist_id).await.ok()?;
    if response_code(&response) != 200 {
        return None;
    }

    if let Some(playlist) = response.body.get("playlist") {
        if let Some(cover_url) = first_non_empty(playlist, &["/coverImgUrl", "/picUrl"]) {
            return Some(cover_url);
        }
    }

    response
        .body
        .pointer("/playlist/tracks")
        .and_then(|value| value.as_array())
        .and_then(|items| items.first())
        .and_then(|track| first_non_empty(track, &["/al/picUrl", "/album/picUrl"]))
        .map(|s| s.to_string())
}

/// 首页推荐 tile：每日推荐 + 推荐歌单卡片，已按固定顺序排好。
async fn fetch_home_tiles(api: &mut ApiState, show_more: bool) -> Vec<HomeTile> {
    let mut daily_tile = HomeTile::placeholder_daily();
    if let Ok(response) = api.recommend_songs().await {
        if response_code(&response) == 200 {
            if let Some(songs) = home_daily_song_items(&response.body) {
                if let Some(cover_url) = songs
                    .iter()
                    .find_map(|item| first_non_empty(item, &["/al/picUrl", "/album/picUrl"]))
                {
                    daily_tile.cover.load(api.clone(), cover_url);
                }
            }
        }
    }

    let mut cards = Vec::new();

    if let Ok(response) = api.recommend_resource().await {
        cards = parse_recommend_cards(&response, 24);
    }

    if cards.is_empty() {
        if let Ok(response) = api.personalized(24).await {
            cards = parse_personalized_cards(&response, 24);
        }
    }

    let mut tiles = Vec::with_capacity(cards.len().saturating_add(1));
    tiles.push(daily_tile);

    for card in cards {
        let pinned_title = normalize_home_pinned_title(&card.title);
        if pinned_title == Some("每日推荐") {
            continue;
        }

        let mut tile =
            HomeTile::from_recommendation(api, card.id, card.title, card.subtitle, card.cover_url);

        if pinned_title == Some("私人雷达") {
            if let Some(playlist_id) = tile.id.clone() {
                if let Some(cover_url) = fetch_playlist_cover_url(api, &playlist_id).await {
                    tile.cover.load(api.clone(), cover_url);
                }
            }
        }

        if pinned_title == Some("私人漫游") {
            tile.id = Some(HOME_PRIVATE_ROAM_TILE_ID.to_string());
        }

        tiles.push(tile);
    }

    prioritize_home_tiles(api, tiles, show_more)
}

/// 扫码登录二维码：key 与二维码链接。
struct QrLoginCode {
    key: String,
    url: String,
}

async fn fetch_qr_login_code(api: &mut ApiState, lang: Language) -> Result<QrLoginCode> {
    let key_resp = api
        .login_qr_key()
        .await
        .with_context(|| lang_text(lang, "二维码 key 获取失败", "Failed to fetch QR login key"))?;

    let key = extract_qr_key(&key_resp);
    if key.is_empty() {
        bail!(lang_text(
            lang,
            "二维码 key 为空，请重试",
            "Empty QR login key, please retry"
        ));
    }

    let qr_resp = api
        .login_qr_create(&key)
        .await
        .with_context(|| lang_text(lang, "二维码创建失败", "Failed to create QR login code"))?;

    Ok(QrLoginCode {
        key,
        url: extract_qr_url(&qr_resp),
    })
}

fn prioritize_home_tiles(
    api: &ApiState,
    mut tiles: Vec<HomeTile>,
    show_more: bool,
) -> Vec<HomeTile> {
    let mut pinned = Vec::with_capacity(HOME_PINNED_TITLES.len());

    for target in HOME_PINNED_TITLES {
        if let Some(index) = tiles
            .iter()
            .position(|tile| normalize_home_pinned_title(&tile.title) == Some(target))
        {
            let mut tile = tiles.remove(index);
            tile.title = target.to_string();
            tile.subtitle.clear();
            pinned.push(tile);
            continue;
        }

        if target == "每日推荐" {
            pinned.push(HomeTile::placeholder_daily());
        } else {
            let tile_id = if target == "私人漫游" {
                Some(HOME_PRIVATE_ROAM_TILE_ID.to_string())
            } else {
                None
            };
            pinned.push(HomeTile::from_recommendation(
                api,
                tile_id,
                target.to_string(),
                String::new(),
                None,
            ));
        }
    }

    pinned.extend(tiles);

    if !show_more && pinned.len() > HOME_PINNED_TITLES.len() {
        pinned.truncate(HOME_PINNED_TITLES.len());
    }

    if pinned.is_empty() {
        pinned.push(HomeTile::placeholder_daily());
    }

    pinned
}

fn parse_recommend_cards(response: &ApiResponse, limit: usize) -> Vec<RecommendCard> {
    response
        .body
        .get("recommend")
        .and_then(|value| value.as_array())
        .map(|items| {
            items
                .iter()
                .filter_map(|item| {
                    let title = item
                        .get("name")
                        .and_then(|value| value.as_str())?
                        .to_string();
                    let subtitle = first_non_empty(item, &["/copywriter", "/creator/nickname"])
                        .unwrap_or_else(|| "推荐歌单".to_string());
                    Some(RecommendCard {
                        id: parse_value_as_string(item.get("id")),
                        title,
                        subtitle,
                        cover_url: first_non_empty(item, &["/picUrl", "/coverImgUrl"]),
                    })
                })
                .take(limit.max(1))
                .collect()
        })
        .unwrap_or_default()
}

fn parse_personalized_cards(response: &ApiResponse, limit: usize) -> Vec<RecommendCard> {
    response
        .body
        .get("result")
        .and_then(|value| value.as_array())
        .map(|items| {
            items
                .iter()
                .filter_map(|item| {
                    let title = item
                        .get("name")
                        .and_then(|value| value.as_str())?
                        .to_string();
                    let subtitle = first_non_empty(item, &["/copywriter", "/creator/nickname"])
                        .unwrap_or_else(|| "推荐歌单".to_string());
                    Some(RecommendCard {
                        id: parse_value_as_string(item.get("id")),
                        title,
                        subtitle,
                        cover_url: first_non_empty(item, &["/picUrl", "/coverImgUrl"]),
                    })
                })
                .take(limit.max(1))
                .collect()
        })
        .unwrap_or_default()
}

fn parse_home_sidebar_playlists(response: &ApiResponse) -> Vec<HomeSidebarPlaylist> {
    let Some(items) = response
        .body
        .get("playlist")
        .and_then(|value| value.as_array())
        .or_else(|| {
            response
                .body
                .pointer("/data/list")
                .and_then(|value| value.as_array())
        })
        .or_else(|| {
            response
                .body
                .pointer("/data/playlist")
                .and_then(|value| value.as_array())
        })
    else {
        return Vec::new();
    };

    let mut out = Vec::with_capacity(items.len());
    for item in items {
        let Some(title) = item.get("name").and_then(|value| value.as_str()) else {
            continue;
        };

        let track_count = item
            .get("trackCount")
            .and_then(|value| value.as_u64())
            .map(|value| value as usize)
            .or_else(|| {
                item.get("trackCount")
                    .and_then(|value| value.as_i64())
                    .map(|value| value.max(0) as usize)
            })
            .unwrap_or(0);

        out.push(HomeSidebarPlaylist {
            id: parse_value_as_string(item.get("id")),
            title: title.to_string(),
            creator: item
                .pointer("/creator/nickname")
                .and_then(|value| value.as_str())
                .unwrap_or("Unknown User")
                .to_string(),
            track_count,
        });
    }

    out
}

fn extract_current_user_id(response: &ApiResponse) -> Option<String> {
    for pointer in [
        "/profile/userId",
        "/data/profile/userId",
        "/account/id",
        "/data/account/id",
    ] {
        if let Some(value) = response.body.pointer(pointer) {
            if let Some(id) = parse_value_as_string(Some(value)) {
                if !id.trim().is_empty() {
                    return Some(id);
                }
            }
        }
    }

    None
}

fn extract_liked_playlist_id(response: &ApiResponse) -> Option<String> {
    for pointer in [
        "/profile/playlistId",
        "/data/profile/playlistId",
        "/profile/likesPlaylistId",
        "/data/profile/likesPlaylistId",
    ] {
        if let Some(value) = response.body.pointer(pointer) {
            if let Some(id) = parse_value_as_string(Some(value)) {
                if !id.trim().is_empty() {
                    return Some(id);
                }
            }
        }
    }

    None
}

fn extract_current_user_name(response: &ApiResponse) -> Option<String> {
    for pointer in [
        "/profile/nickname",
        "/data/profile/nickname",
        "/account/userName",
        "/data/account/userName",
    ] {
        if let Some(name) = response
            .body
            .pointer(pointer)
            .and_then(|value| value.as_str())
        {
            let name = name.trim();
            if !name.is_empty() {
                return Some(name.to_string());
            }
        }
    }

    None
}

/// 拉取一批私人漫游歌曲（接口每次固定返回 3 首，连拉几次去重）
async fn fetch_private_roam_songs(api: &mut ApiState) -> Vec<PlaylistTrack> {
    let mut tracks = Vec::new();
    let mut seen = std::collections::HashSet::new();

    for _ in 0..3 {
        let Ok(response) = api.personal_fm_mode("DEFAULT", 20).await else {
            continue;
        };
        if response_code(&response) != 200 {
            continue;
        }
        let Some(songs) = response.body.get("data").and_then(|value| value.as_array()) else {
            continue;
        };
        let normalized = normalize_fm_song_items(songs);
        for track in parse_tracks(&normalized) {
            if let Some(id) = &track.id {
                if seen.insert(id.clone()) {
                    tracks.push(track);
                }
            }
        }
    }

    tracks
}

/// 当前 UTC 日期对应的天数（每日刷新标记用）
fn today_day_number() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|value| value.as_secs() as i64 / 86400)
        .unwrap_or_default()
}

/// 每日刷新合并规则：上次最后播放的歌曲保留在首位，其后追加新歌（按 id 去重）
fn merge_private_roam_refresh(
    old_tracks: &[PlaylistTrack],
    last_played_index: Option<usize>,
    fetched: Vec<PlaylistTrack>,
) -> (Vec<PlaylistTrack>, Option<usize>) {
    let mut new_tracks = Vec::new();
    let mut kept_last_played = false;

    if let Some(index) = last_played_index {
        if let Some(last) = old_tracks.get(index) {
            new_tracks.push(last.clone());
            kept_last_played = true;
        }
    }
    for track in fetched {
        if !new_tracks.iter().any(|t| t.id == track.id) {
            new_tracks.push(track);
        }
    }

    let new_index = if kept_last_played { Some(0) } else { None };
    (new_tracks, new_index)
}

/// 私人 FM/漫游接口的歌曲对象用 `album`/`artists` 字段，规整为歌单通用的 `al`/`ar`
fn normalize_fm_song_items(items: &[Value]) -> Vec<Value> {
    items
        .iter()
        .map(|item| {
            let mut song = item.clone();
            if song.get("al").is_none() {
                if let Some(album) = song.get("album").cloned() {
                    song["al"] = album;
                }
            }
            if song.get("ar").is_none() {
                if let Some(artists) = song.get("artists").cloned() {
                    song["ar"] = artists;
                }
            }
            song
        })
        .collect()
}

fn parse_tracks(items: &[Value]) -> Vec<PlaylistTrack> {
    let mut tracks = Vec::new();

    for item in items {
        let Some(title) = item.get("name").and_then(|value| value.as_str()) else {
            continue;
        };

        let artist = parse_artists(item)
            .unwrap_or_else(|| "Unknown Artist".to_string())
            .trim()
            .to_string();
        let duration_ms = item
            .get("dt")
            .and_then(|value| value.as_i64())
            .or_else(|| item.get("duration").and_then(|value| value.as_i64()))
            .unwrap_or(0);

        tracks.push(PlaylistTrack {
            kind: PlaylistTrackKind::Song,
            id: parse_value_as_string(item.get("id")),
            title: title.to_string(),
            artist,
            album: item
                .pointer("/al/name")
                .and_then(|value| value.as_str())
                .unwrap_or("Unknown Album")
                .to_string(),
            cover_url: first_non_empty(item, &["/al/picUrl", "/album/picUrl"]),
            duration_ms,
            duration: format_duration(duration_ms),
        });
    }

    tracks
}

fn parse_song_like_check_result(body: &Value, song_id: &str) -> Option<bool> {
    if let Some(value) = body.pointer(&format!("/data/{song_id}")) {
        if let Some(liked) = parse_song_like_check_flag(value) {
            return Some(liked);
        }
    }

    for pointer in [
        "/data/0/liked",
        "/songs/0/liked",
        "/data/songs/0/liked",
        "/liked",
    ] {
        if let Some(value) = body.pointer(pointer) {
            if let Some(liked) = parse_song_like_check_flag(value) {
                return Some(liked);
            }
        }
    }

    if let Some(obj) = body.get("data").and_then(|value| value.as_object()) {
        for value in obj.values() {
            if let Some(liked) = parse_song_like_check_flag(value) {
                return Some(liked);
            }
        }
    }

    None
}

fn parse_song_like_check_flag(value: &Value) -> Option<bool> {
    if let Some(flag) = value.as_bool() {
        return Some(flag);
    }

    if let Some(flag) = value.as_i64() {
        return match flag {
            0 => Some(false),
            1 => Some(true),
            _ => None,
        };
    }

    if let Some(flag) = value.as_u64() {
        return match flag {
            0 => Some(false),
            1 => Some(true),
            _ => None,
        };
    }

    if let Some(flag) = value.as_str() {
        return match flag.trim().to_ascii_lowercase().as_str() {
            "0" | "false" => Some(false),
            "1" | "true" => Some(true),
            _ => None,
        };
    }

    None
}

fn parse_likelist_song_ids(body: &Value) -> HashSet<String> {
    let mut out = HashSet::new();
    let arrays = [
        body.pointer("/ids").and_then(|value| value.as_array()),
        body.pointer("/data/ids").and_then(|value| value.as_array()),
        body.pointer("/data").and_then(|value| value.as_array()),
    ];

    for maybe_arr in arrays {
        let Some(arr) = maybe_arr else {
            continue;
        };

        for item in arr {
            if let Some(value) = item
                .as_i64()
                .map(|value| value.to_string())
                .or_else(|| item.as_u64().map(|value| value.to_string()))
                .or_else(|| item.as_str().map(|value| value.trim().to_string()))
            {
                if !value.is_empty() {
                    out.insert(value);
                }
            }
        }
    }

    out
}

fn first_non_empty_intro_text(value: &Value) -> Option<String> {
    value
        .get("introduction")
        .and_then(|item| item.as_array())
        .and_then(|items| {
            items
                .iter()
                .find_map(|intro| first_non_empty(intro, &["/txt", "/ti"]))
        })
}

fn artist_album_kind(item: &Value) -> AuthorTileKind {
    let type_text = item
        .get("type")
        .and_then(|value| value.as_str())
        .unwrap_or_default();
    let sub_type_text = item
        .get("subType")
        .and_then(|value| value.as_str())
        .unwrap_or_default();

    let type_lower = type_text.to_ascii_lowercase();
    let sub_type_lower = sub_type_text.to_ascii_lowercase();

    let is_single = type_lower.contains("single")
        || sub_type_lower.contains("single")
        || type_text.contains("单曲")
        || sub_type_text.contains("单曲");
    if is_single {
        return AuthorTileKind::Single;
    }

    let is_ep =
        type_lower.contains("ep") || sub_type_lower.contains("ep") || type_text.contains("EP");
    if is_ep {
        return AuthorTileKind::Ep;
    }

    AuthorTileKind::Album
}

fn parse_search_input(raw: &str) -> (String, SearchScope) {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return (String::new(), SearchScope::Mixed);
    }

    let lower = trimmed.to_ascii_lowercase();

    // 后缀表来自 `SearchScope::SUFFIXED`（单一来源）；`@artist` 是同义别名。
    let suffixes = SearchScope::SUFFIXED
        .into_iter()
        .map(|scope| (scope.suffix(), scope))
        .chain([("@artist", SearchScope::Author)]);
    for (suffix, scope) in suffixes {
        if lower.ends_with(suffix) {
            let cut = trimmed.len().saturating_sub(suffix.len());
            let stripped = &trimmed[..cut];
            return (stripped.trim().to_string(), scope);
        }
    }

    (trimmed.to_string(), SearchScope::Mixed)
}

fn is_followed_author_query(keywords: &str, scope: SearchScope) -> bool {
    scope == SearchScope::Author && keywords.trim().is_empty()
}

struct FollowedAuthorPage {
    items: Vec<SearchItem>,
    fetched_count: usize,
    has_more: Option<bool>,
    total_count: Option<usize>,
}

fn parse_followed_author_page(response: &ApiResponse) -> FollowedAuthorPage {
    let items = response
        .body
        .get("data")
        .and_then(|value| value.as_array())
        .or_else(|| {
            response
                .body
                .get("artists")
                .and_then(|value| value.as_array())
        })
        .or_else(|| {
            response
                .body
                .pointer("/result/artists")
                .and_then(|value| value.as_array())
        });

    let fetched_count = items.map(|values| values.len()).unwrap_or_default();
    let parsed_items = items
        .map(|values| parse_author_items(values))
        .unwrap_or_default();

    let has_more = ["/hasMore", "/more", "/data/hasMore", "/result/hasMore"]
        .iter()
        .find_map(|pointer| {
            response
                .body
                .pointer(pointer)
                .and_then(|value| value.as_bool())
        });
    let total_count = ["/count", "/data/count", "/result/count"]
        .iter()
        .find_map(|pointer| parse_usize_value(response.body.pointer(pointer)));

    FollowedAuthorPage {
        items: parsed_items,
        fetched_count,
        has_more,
        total_count,
    }
}

fn followed_author_has_more(page: &FollowedAuthorPage, next_offset: usize) -> bool {
    if page.fetched_count == 0 {
        return false;
    }

    if let Some(has_more) = page.has_more {
        return has_more;
    }

    if let Some(total_count) = page.total_count {
        return next_offset < total_count;
    }

    page.fetched_count >= SEARCH_RESULT_PAGE_SIZE
}

fn parse_usize_value(value: Option<&Value>) -> Option<usize> {
    let value = value?;
    if let Some(number) = value.as_u64() {
        return Some(number as usize);
    }
    if let Some(number) = value.as_i64() {
        if number >= 0 {
            return Some(number as usize);
        }
    }
    if let Some(text) = value.as_str() {
        return text.trim().parse::<usize>().ok();
    }
    None
}

/// 混合搜索的辅助分区：请求失败或非 200 时退化为空分区（该分区不显示），
/// 不影响单曲分区，也不把整次搜索判为失败。
fn parse_optional_search_section(
    response: Result<ApiResponse>,
    scope: SearchScope,
) -> Vec<SearchItem> {
    match response {
        Ok(response) if response_code(&response) == 200 => parse_search_items(&response, scope),
        Ok(response) => {
            log::warn!(
                "{} 分区搜索失败({}): {}",
                scope.display_name(),
                response_code(&response),
                response_message(&response)
            );
            Vec::new()
        }
        Err(err) => {
            log::warn!("{} 分区搜索失败: {}", scope.display_name(), err);
            Vec::new()
        }
    }
}

/// 分区解析器：把 `result.<key>` 的数组转成条目。
type SearchSectionParser = fn(&[Value]) -> Vec<SearchItem>;

/// 按 scope 取对应分区并解析。`Mixed` 没有单一条目种类（由 `execute_mixed_search` 合并三段）。
fn parse_search_items(response: &ApiResponse, scope: SearchScope) -> Vec<SearchItem> {
    let Some(result) = response.body.get("result") else {
        return Vec::new();
    };

    let (key, parse): (&str, SearchSectionParser) = match scope {
        SearchScope::Mixed => return Vec::new(),
        SearchScope::Single => ("songs", parse_song_items),
        SearchScope::Album => ("albums", parse_album_items),
        SearchScope::Author => ("artists", parse_author_items),
        SearchScope::Playlist => ("playlists", parse_playlist_items),
    };

    result
        .get(key)
        .and_then(|value| value.as_array())
        .map(|items| parse(items))
        .unwrap_or_default()
}

fn parse_song_items(items: &[Value]) -> Vec<SearchItem> {
    let mut out = Vec::new();

    for item in items {
        let Some(name) = item.get("name").and_then(|value| value.as_str()) else {
            continue;
        };
        let artist = parse_artists(item).unwrap_or_else(|| "Unknown Artist".to_string());
        let duration = item
            .get("dt")
            .and_then(|value| value.as_i64())
            .or_else(|| item.get("duration").and_then(|value| value.as_i64()))
            .unwrap_or(0);

        out.push(SearchItem {
            kind: SearchItemKind::Song,
            left_label: format!("{} - {}", name, artist),
            right_label: format_duration(duration),
            song_id: parse_value_as_string(item.get("id")),
            album_id: None,
            playlist_id: None,
            artist_id: None,
            title: Some(name.to_string()),
            artist: Some(artist),
            album: item
                .pointer("/al/name")
                .and_then(|value| value.as_str())
                .map(|value| value.to_string()),
            cover_url: first_non_empty(item, &["/al/picUrl", "/album/picUrl"]),
            duration_ms: Some(duration),
            cover: CoverFetchState::default(),
        });
    }

    out
}

fn parse_album_items(items: &[Value]) -> Vec<SearchItem> {
    let mut out = Vec::new();

    for item in items {
        let Some(name) = item.get("name").and_then(|value| value.as_str()) else {
            continue;
        };

        let artist = item
            .pointer("/artist/name")
            .and_then(|value| value.as_str())
            .unwrap_or("Unknown Artist");
        let size = item
            .get("size")
            .and_then(|value| value.as_i64())
            .unwrap_or_default();

        out.push(SearchItem {
            kind: SearchItemKind::Album,
            left_label: format!("{} - {}", name, artist),
            right_label: format!("{} 首", size),
            song_id: None,
            album_id: parse_value_as_string(item.get("id")),
            playlist_id: None,
            artist_id: None,
            title: Some(name.to_string()),
            artist: Some(artist.to_string()),
            album: Some(name.to_string()),
            cover_url: first_non_empty(item, &["/picUrl", "/blurPicUrl"]),
            duration_ms: None,
            cover: CoverFetchState::default(),
        });
    }

    out
}

fn parse_author_items(items: &[Value]) -> Vec<SearchItem> {
    let mut out = Vec::new();

    for item in items {
        let Some(name) = item.get("name").and_then(|value| value.as_str()) else {
            continue;
        };

        let album_size = item
            .get("albumSize")
            .and_then(|value| value.as_i64())
            .unwrap_or_default();

        out.push(SearchItem {
            kind: SearchItemKind::Artist,
            left_label: name.to_string(),
            right_label: format!("{} 张专辑", album_size),
            song_id: None,
            album_id: None,
            playlist_id: None,
            artist_id: parse_value_as_string(item.get("id")),
            title: None,
            artist: Some(name.to_string()),
            album: None,
            cover_url: first_non_empty(item, &["/picUrl", "/img1v1Url", "/avatarUrl"]),
            duration_ms: None,
            cover: CoverFetchState::default(),
        });
    }

    out
}

fn parse_playlist_items(items: &[Value]) -> Vec<SearchItem> {
    let mut out = Vec::new();

    for item in items {
        let Some(name) = item.get("name").and_then(|value| value.as_str()) else {
            continue;
        };

        let creator = item
            .pointer("/creator/nickname")
            .and_then(|value| value.as_str())
            .unwrap_or("Unknown User");
        let count = item
            .get("trackCount")
            .and_then(|value| value.as_i64())
            .unwrap_or_default();

        out.push(SearchItem {
            kind: SearchItemKind::Playlist,
            left_label: format!("{} - {}", name, creator),
            right_label: format!("{} 首", count),
            song_id: None,
            album_id: None,
            playlist_id: parse_value_as_string(item.get("id")),
            artist_id: None,
            title: None,
            artist: None,
            album: None,
            cover_url: first_non_empty(item, &["/coverImgUrl", "/picUrl"]),
            duration_ms: None,
            cover: CoverFetchState::default(),
        });
    }

    out
}

/// 多作者显示串的连接符：`parse_artists` 用它把 `ar` 拼成一行，
/// 全屏页信息区再按它切回每段，好让"点谁的名字进谁的页面"。
pub(crate) const ARTIST_SEPARATOR: &str = " / ";

/// 把 `parse_artists` 拼出来的作者行按连接符切回每段（显示顺序 = `ar` 顺序）。
///
/// 空段保留：段序号要和显示位置一一对应，全屏页传回来的序号才对得上。
pub(crate) fn artist_name_segments(line: &str) -> Vec<&str> {
    line.split(ARTIST_SEPARATOR).collect()
}

/// 全屏页点名字进页面时，`song/detail` 里用得上的两样东西。
#[derive(Debug, Default)]
struct SongPageRefs {
    /// `ar`：显示顺序的作者（名称 + ID；缺 ID 的条目保留占位，别让后面的下标错位）。
    artists: Vec<(String, Option<String>)>,
    album_id: Option<String>,
}

/// `song/detail` 里显示串第 `index` 段对应的作者 ID。
///
/// 先按名字匹配（显示串由同一份 `ar` 拼出来，正常都能命中；`ar` 顺序或名字带后缀时也稳），
/// 再按位置兜底（`ar` 顺序 = 显示顺序）；两者都没有就返回 `None`，让调用方只报状态、不换页。
fn pick_artist_id(
    artists: &[(String, Option<String>)],
    line: &str,
    index: usize,
) -> Option<String> {
    if let Some(name) = artist_name_segments(line).get(index).copied() {
        if let Some((_, id)) = artists
            .iter()
            .find(|(candidate, _)| candidate.as_str() == name)
        {
            return id.clone();
        }
    }

    artists.get(index).and_then(|(_, id)| id.clone())
}

fn parse_artists(track: &Value) -> Option<String> {
    let artists = track
        .get("ar")
        .and_then(|value| value.as_array())
        .or_else(|| track.get("artists").and_then(|value| value.as_array()))?;

    let names: Vec<String> = artists
        .iter()
        .filter_map(|item| item.get("name").and_then(|value| value.as_str()))
        .map(|name| name.to_string())
        .collect();

    if names.is_empty() {
        None
    } else {
        Some(names.join(ARTIST_SEPARATOR))
    }
}

fn format_duration(duration_ms: i64) -> String {
    let total = (duration_ms.max(0) / 1000) as u64;
    let mm = total / 60;
    let ss = total % 60;
    format!("{:02}:{:02}", mm, ss)
}

fn parse_value_as_string(value: Option<&Value>) -> Option<String> {
    let value = value?;
    if let Some(text) = value.as_str() {
        if !text.trim().is_empty() {
            return Some(text.to_string());
        }
    }
    if let Some(number) = value.as_i64() {
        return Some(number.to_string());
    }
    None
}

fn first_non_empty(value: &Value, pointers: &[&str]) -> Option<String> {
    for pointer in pointers {
        if let Some(text) = value.pointer(pointer).and_then(|item| item.as_str()) {
            let text = text.trim();
            if !text.is_empty() {
                return Some(text.to_string());
            }
        }
    }
    None
}

fn response_indicates_vip(response: &ApiResponse) -> bool {
    let code = response
        .body
        .get("code")
        .and_then(|value| value.as_i64())
        .unwrap_or(response.status);
    if code != 200 {
        return false;
    }

    let root = response.body.get("data").unwrap_or(&response.body);
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64;

    for pointer in [
        "/redVipLevel",
        "/redplusLevel",
        "/musicPackage/vipCode",
        "/associator/vipCode",
        "/musicVipLevel",
    ] {
        if root
            .pointer(pointer)
            .and_then(|value| value.as_i64())
            .unwrap_or(0)
            > 0
        {
            return true;
        }
    }

    for pointer in [
        "/vipStatus",
        "/musicPackage/isSign",
        "/associator/isSign",
        "/isVip",
    ] {
        if root
            .pointer(pointer)
            .and_then(|value| value.as_bool())
            .unwrap_or(false)
        {
            return true;
        }
    }

    let music_expire = root
        .pointer("/musicPackage/expireTime")
        .and_then(|value| value.as_i64())
        .unwrap_or(0);
    if music_expire > now_ms {
        return true;
    }

    let associator_expire = root
        .pointer("/associator/expireTime")
        .and_then(|value| value.as_i64())
        .unwrap_or(0);
    if associator_expire > now_ms {
        return true;
    }

    false
}

fn cycle_bar_number(current: BarNumber, delta: i32) -> BarNumber {
    let options = [
        BarNumber::Auto,
        BarNumber::N16,
        BarNumber::N32,
        BarNumber::N48,
        BarNumber::N64,
        BarNumber::N80,
        BarNumber::N96,
    ];
    let current_idx = options
        .iter()
        .position(|item| *item == current)
        .unwrap_or(0) as i32;
    let next = (current_idx + delta).rem_euclid(options.len() as i32) as usize;
    options[next]
}

fn keybind_matches(binding: &str, key: KeyEvent) -> bool {
    let Some(expected) = normalize_keybind_text(binding) else {
        return false;
    };
    let Some(actual) = key_event_to_keybind_text(key) else {
        return false;
    };
    expected.eq_ignore_ascii_case(actual.as_str())
}

fn is_reserved_reset_combo(key: KeyEvent) -> bool {
    key_event_to_keybind_text(key)
        .map(|value| value.eq_ignore_ascii_case(RESERVED_RESET_KEYBIND))
        .unwrap_or(false)
}

fn key_event_to_keybind_text(key: KeyEvent) -> Option<String> {
    let mut parts: Vec<&str> = Vec::new();

    if key.modifiers.contains(KeyModifiers::CONTROL) {
        parts.push("Ctrl");
    }
    if key.modifiers.contains(KeyModifiers::ALT) {
        parts.push("Alt");
    }

    let include_shift = key.modifiers.contains(KeyModifiers::SHIFT)
        && !matches!(key.code, KeyCode::Char(ch) if ch.is_ascii_alphabetic());
    if include_shift {
        parts.push("Shift");
    }

    let key_token = key_code_to_keybind_token(key.code)?;
    let mut out = parts.join("+");
    if !out.is_empty() {
        out.push('+');
    }
    out.push_str(&key_token);
    Some(out)
}

fn normalize_keybind_text(raw: &str) -> Option<String> {
    let mut ctrl = false;
    let mut alt = false;
    let mut shift = false;
    let mut key_token: Option<String> = None;

    for token in raw.split('+') {
        let token = token.trim();
        if token.is_empty() {
            continue;
        }

        if token.eq_ignore_ascii_case("ctrl") || token.eq_ignore_ascii_case("control") {
            ctrl = true;
            continue;
        }
        if token.eq_ignore_ascii_case("alt") {
            alt = true;
            continue;
        }
        if token.eq_ignore_ascii_case("shift") {
            shift = true;
            continue;
        }
        if token.eq_ignore_ascii_case("backtab") {
            shift = true;
        }

        if key_token.is_some() {
            return None;
        }
        key_token = normalize_keybind_token(token);
        key_token.as_ref()?;
    }

    let key_token = key_token?;
    let mut parts: Vec<&str> = Vec::new();
    if ctrl {
        parts.push("Ctrl");
    }
    if alt {
        parts.push("Alt");
    }
    if shift {
        parts.push("Shift");
    }

    let mut out = parts.join("+");
    if !out.is_empty() {
        out.push('+');
    }
    out.push_str(&key_token);
    Some(out)
}

fn normalize_keybind_token(token: &str) -> Option<String> {
    let token = token.trim();
    if token.is_empty() {
        return None;
    }

    let lower = token.to_ascii_lowercase();
    match lower.as_str() {
        "esc" | "escape" => return Some("Esc".to_string()),
        "enter" | "return" => return Some("Enter".to_string()),
        "space" | "spacebar" => return Some("Space".to_string()),
        "tab" | "backtab" => return Some("Tab".to_string()),
        "left" => return Some("Left".to_string()),
        "right" => return Some("Right".to_string()),
        "up" => return Some("Up".to_string()),
        "down" => return Some("Down".to_string()),
        "home" => return Some("Home".to_string()),
        "end" => return Some("End".to_string()),
        "pageup" | "pgup" => return Some("PageUp".to_string()),
        "pagedown" | "pgdown" | "pgdn" => return Some("PageDown".to_string()),
        "insert" | "ins" => return Some("Insert".to_string()),
        "delete" | "del" => return Some("Delete".to_string()),
        "backspace" | "bs" => return Some("Backspace".to_string()),
        "plus" => return Some("Plus".to_string()),
        _ => {}
    }

    if let Some(rest) = lower.strip_prefix('f') {
        if let Ok(num) = rest.parse::<u8>() {
            if num > 0 {
                return Some(format!("F{}", num));
            }
        }
    }

    let mut chars = token.chars();
    let ch = chars.next()?;
    if chars.next().is_some() {
        return None;
    }

    if ch == ' ' {
        return Some("Space".to_string());
    }
    if ch == '+' {
        return Some("Plus".to_string());
    }
    if ch.is_control() {
        return None;
    }
    if ch.is_ascii_alphabetic() {
        return Some(ch.to_ascii_uppercase().to_string());
    }
    Some(ch.to_string())
}

fn key_code_to_keybind_token(code: KeyCode) -> Option<String> {
    match code {
        KeyCode::Backspace => Some("Backspace".to_string()),
        KeyCode::Enter => Some("Enter".to_string()),
        KeyCode::Left => Some("Left".to_string()),
        KeyCode::Right => Some("Right".to_string()),
        KeyCode::Up => Some("Up".to_string()),
        KeyCode::Down => Some("Down".to_string()),
        KeyCode::Home => Some("Home".to_string()),
        KeyCode::End => Some("End".to_string()),
        KeyCode::PageUp => Some("PageUp".to_string()),
        KeyCode::PageDown => Some("PageDown".to_string()),
        KeyCode::Tab => Some("Tab".to_string()),
        KeyCode::BackTab => Some("Tab".to_string()),
        KeyCode::Delete => Some("Delete".to_string()),
        KeyCode::Insert => Some("Insert".to_string()),
        KeyCode::F(n) if n > 0 => Some(format!("F{}", n)),
        KeyCode::Char(' ') => Some("Space".to_string()),
        KeyCode::Char('+') => Some("Plus".to_string()),
        KeyCode::Char(ch) => {
            if ch.is_control() {
                return None;
            }
            if ch.is_ascii_alphabetic() {
                return Some(ch.to_ascii_uppercase().to_string());
            }
            Some(ch.to_string())
        }
        KeyCode::Esc => Some("Esc".to_string()),
        _ => None,
    }
}

fn placeholder_cover_ascii(width: u16, height: u16, ch: char) -> String {
    if width == 0 || height == 0 {
        return String::new();
    }

    let row = ch.to_string().repeat(width as usize);
    let mut out = String::new();
    for _ in 0..height {
        out.push_str(&row);
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

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

    /// 可见行按比例映射到源图行：整体可见取整段，只露下半就只取下半，绝不压缩。
    #[test]
    fn source_rows_follow_visible_proportion() {
        // 全部可见：整段 viewport。
        assert_eq!(source_rows_for_visible(8, 4, 0, 4), (0, 8));
        // 只露第 2 行（占四分之一）：取第二段四分之一。
        assert_eq!(source_rows_for_visible(8, 4, 1, 1), (2, 2));
        // 头像形状（2 行）：只露下半 → 取源图下半。
        assert_eq!(source_rows_for_visible(8, 2, 1, 1), (4, 4));
        // 最后一个四分之一，且不越界。
        let (start, len) = source_rows_for_visible(8, 4, 3, 1);
        assert_eq!((start, len), (6, 2));
        // 源图比行数还小（退化）：至少 1 行且不越界。
        assert_eq!(source_rows_for_visible(1, 4, 2, 1), (0, 1));
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
        let mut state = SearchState {
            results: vec![
                search_item(SearchItemKind::Artist, "artist-1"), // 行 0..4（卡片）
                search_item(SearchItemKind::Artist, "artist-2"), // 行 4..8
                search_item(SearchItemKind::Song, "song-1"),     // 行 8..10（分区线 1 + 单曲 1）
                search_item(SearchItemKind::Song, "song-2"),     // 行 10..11
                search_item(SearchItemKind::Song, "song-3"),     // 行 11..12
            ],
            ..SearchState::default()
        };
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
        let mut state = SearchState {
            results: vec![
                search_item(SearchItemKind::Artist, "artist-1"),
                search_item(SearchItemKind::Artist, "artist-2"),
                search_item(SearchItemKind::Artist, "artist-3"),
            ],
            ..SearchState::default()
        };
        state.set_viewport(6, true);

        state.set_focus(1);
        assert_eq!(state.effective_scroll_rows(), 2);
        assert!(state.focus_next());
        assert_eq!(state.effective_scroll_rows(), 6);
    }

    /// 视口行数变化后聚焦条目仍然完整可见（窗口缩放 / 小窗模式）。
    #[test]
    fn search_viewport_resize_keeps_focus_visible() {
        let mut state = SearchState {
            results: (0..20)
                .map(|i| search_item(SearchItemKind::Song, &format!("song-{i}")))
                .collect(),
            ..SearchState::default()
        };
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
        let fut: Pin<Box<dyn Future<Output = Option<Result<(), String>>>>> =
            Box::pin(async { None });
        fut.shared()
    }

    fn pending_verify_future() -> LikeVerifyFuture {
        let fut: Pin<Box<dyn Future<Output = Option<Result<bool, ()>>>>> = Box::pin(async { None });
        fut.shared()
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
        assert!(!machine.displayed("s1"));
    }

    /// 旧回包不得覆盖更新的意图：确认值照写，显示与补发按新意图走。
    #[test]
    fn like_stale_response_keeps_newer_intent() {
        let mut machine = LikeMachine::default();
        machine.set_intent("s1".to_string(), true);
        machine.begin_toggle("s1".to_string(), true, pending_toggle_future());
        machine.set_intent("s1".to_string(), false);
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
        let mut state = HomeSidebarState::default();
        state.created_playlists = (0..created)
            .map(|i| sidebar_item(&format!("created-{i}")))
            .collect();
        state.collected_playlists = (0..collected)
            .map(|i| sidebar_item(&format!("collected-{i}")))
            .collect();
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
