mod about;
mod auth;
mod author;
mod downloads;
mod fullscreen;
mod home;
mod input;
mod keybinds;
mod likes;
mod metadata;
mod mpris;
mod playback;
mod playlist;
mod presentation;
mod roam;
mod search;
mod settings;
mod startup_view;
mod api;
pub(crate) mod controllers;
mod cover_presentation;
pub(crate) mod download;
mod latest_fetch;
mod mpris_bridge;
mod owned_task;
pub(crate) mod player;
mod playlist_pagination;
mod startup;
pub(crate) mod streaming;

pub(crate) mod browse_controller;
pub(crate) mod download_controller;
pub(crate) mod input_controller;
pub(crate) mod playback_controller;
pub(crate) mod settings_controller;
pub(crate) mod startup_controller;
use crate::app::player::{
    AudioPlayer, AudioPlayerState, cleanup_cache_dir, is_nonempty_file, resolve_cache_root,
};
use crate::data::config::{AudioQuality, BarChannels, BarNumber, Language, VisualizeMode};
use crate::data::config::{Config, GraphicsProtocol};
use crate::data::playback_session;
use crate::data::private_roam;
use crate::data::session;
use crate::data::theme_loader::ThemeLoader;
use crate::launch;
use crate::render::cover_pipeline::CoverKey;
use crate::render::cover_renderer::render_cover_ascii;
use crate::render::motion::{Curve, Toggle, Trail, Transition};
use crate::render::wake::WakeSignal;
use crate::tmplayer::app::state::LyricLine;
use crate::tmplayer::audio::pcm_tap::PcmRing;
use crate::tmplayer::audio::spectrum::{MINI_BARS, Spectrum};
use crate::tmplayer::playback::metadata::{parse_lrc, parse_plain_lyrics};
use crate::ui::page_lyrics;
use crate::ui::theme::Theme;
use anyhow::{Context, Result, anyhow, bail};
use crossterm::event::{
    KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use cyper::Client;
use futures::channel::mpsc as async_mpsc;
use http::header;
use image::DynamicImage;
use ncm_api::ApiResponse;
use ratatui::Frame;
use ratatui::layout::{Rect, Size};
use ratatui::style::Style;
use ratatui::widgets::{Block, Paragraph};
use see::unsync as watch;
use serde_json::Value;
use std::collections::HashSet;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use unicode_width::UnicodeWidthChar;

use crate::data::atomic_file::write_atomic;
use crate::data::persistence::{PersistenceHandle, PersistenceKey, PersistenceWorker};
use api::ApiState;
use browse_controller::BrowseController;
use controllers::SearchController;
use cover_presentation::CoverPresentation;
use download::{
    DownloadEvent, DownloadManager, DownloadRequest, DownloadRow, DownloadRowCache, DownloadState,
    DownloadTarget,
};
use download_controller::DownloadController;
use input_controller::InputController;
use mpris_bridge::{MprisBridge, MprisControlEvent, MprisSyncPayload};
use owned_task::{SharedTask, spawn_shared};
use playback_controller::PlaybackController;
use playlist_pagination::PlaylistPagination;
use settings_controller::SettingsController;
use startup::StartupInit;
use startup_controller::StartupController;
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
const SEARCH_RESULT_PAGE_SIZE: usize = 100;
const ARTIST_ALBUM_PAGE_SIZE: usize = 60;
/// 无后缀（混合）搜索里作者 / 歌单分区只取最相关的少量条目，不参与分页。
const MIXED_AUX_RESULT_LIMIT: usize = 5;
/// 搜索框滑出动画时长（time-based，与帧率解耦）
const SEARCH_BOX_ANIM_DURATION: Duration = Duration::from_millis(180);
/// 侧边栏滑出动画时长（time-based，与帧率解耦）。主页与全屏播放页共用。
pub(crate) const SIDEBAR_ANIM_DURATION: Duration = Duration::from_millis(200);
const HOME_SIDEBAR_PLAYLIST_LIMIT: usize = 100;
const SETTINGS_ROOT_ITEMS: usize = 13;
const SETTINGS_PLAYBACK_ITEMS: usize = 7;
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
        if let Some(value) = self.active_input_mut()
            && value.chars().count() < MAX_INPUT_LEN
        {
            value.push(ch);
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

type HomeSidebarTask = Pin<Box<dyn Future<Output = Option<Result<HomeSidebarFetch, String>>>>>;

/// 侧边栏歌单的一次拉取结果。异步任务不持有 `&mut App`，
/// 取完由 `tick_home_sidebar_fetch` 搬进状态。
#[derive(Clone)]
struct HomeSidebarFetch {
    user_id: String,
    liked_playlist_id: Option<String>,
    user_name: String,
    created: Vec<HomeSidebarPlaylist>,
    collected: Vec<HomeSidebarPlaylist>,
    created_more: bool,
    collected_more: bool,
}

#[derive(Clone)]
struct HomeSidebarPageFetch {
    section: HomeSidebarSection,
    items: Vec<HomeSidebarPlaylist>,
    next_offset: usize,
    has_more: bool,
}

type HomeSidebarPageTask =
    Pin<Box<dyn Future<Output = Option<Result<HomeSidebarPageFetch, String>>>>>;

type HomeSidebarFetchFuture = SharedTask<Result<HomeSidebarFetch, String>>;
type HomeSidebarPageFetchFuture = SharedTask<Result<HomeSidebarPageFetch, String>>;

fn response_page_has_more(
    response: &ApiResponse,
    offset: usize,
    fetched: usize,
    limit: usize,
) -> bool {
    response
        .body
        .pointer("/data/more")
        .or_else(|| response.body.pointer("/more"))
        .and_then(Value::as_bool)
        .or_else(|| {
            response
                .body
                .pointer("/data/count")
                .or_else(|| response.body.pointer("/count"))
                .and_then(|value| parse_usize_value(Some(value)))
                .map(|count| offset.saturating_add(fetched) < count)
        })
        .unwrap_or(fetched >= limit)
}

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

    let created = parse_home_sidebar_playlists(&created_response);
    let collected = parse_home_sidebar_playlists(&collected_response);
    Ok(HomeSidebarFetch {
        user_id,
        liked_playlist_id: extract_liked_playlist_id(&account),
        user_name,
        created_more: response_page_has_more(
            &created_response,
            0,
            created.len(),
            HOME_SIDEBAR_PLAYLIST_LIMIT,
        ),
        collected_more: response_page_has_more(
            &collected_response,
            0,
            collected.len(),
            HOME_SIDEBAR_PLAYLIST_LIMIT,
        ),
        created,
        collected,
    })
}

async fn fetch_home_sidebar_page(
    mut api: ApiState,
    user_id: String,
    section: HomeSidebarSection,
    offset: usize,
    language: Language,
) -> Result<HomeSidebarPageFetch, String> {
    let response = match section {
        HomeSidebarSection::Created => {
            api.user_playlist_create(&user_id, HOME_SIDEBAR_PLAYLIST_LIMIT, offset)
                .await
        }
        HomeSidebarSection::Collected => {
            api.user_playlist_collect(&user_id, HOME_SIDEBAR_PLAYLIST_LIMIT, offset)
                .await
        }
    }
    .map_err(|err| err.to_string())?;
    let code = response_code(&response);
    if code != 200 {
        return Err(format!("请求失败({code}): {}", response_message(&response)));
    }

    let items = parse_home_sidebar_playlists(&response);
    if items.is_empty() {
        return Err(match language {
            Language::Zh => "侧边栏歌单分页为空".to_string(),
            Language::En => "Sidebar playlist page is empty".to_string(),
        });
    }
    let item_count = items.len();
    let next_offset = offset.saturating_add(item_count);
    Ok(HomeSidebarPageFetch {
        section,
        items,
        next_offset,
        has_more: response_page_has_more(
            &response,
            offset,
            item_count,
            HOME_SIDEBAR_PLAYLIST_LIMIT,
        ),
    })
}
type CoverFuture = SharedTask<Arc<DynamicImage>>;
type AsciiFuture = SharedTask<String>;

type AuthorFetchFuture = SharedTask<Result<AuthorFetch, String>>;
/// 装箱后的任务体（`spawn_shared` 的入参类型）。
type AuthorFetchTask = Pin<Box<dyn Future<Output = Option<Result<AuthorFetch, String>>>>>;

/// 作者页四个接口的原始回包（`None` = 该请求失败）；发行物请求会先完成全部分页。
///
/// 解析见 `App::build_author_page`。
struct AuthorResponses {
    detail: Option<ApiResponse>,
    desc: Option<ApiResponse>,
    top_song: Option<ApiResponse>,
    album: Option<ApiResponse>,
}

/// 作者页的一次拉取结果：`AuthorState` 里除封面句柄与视口字段外的全部字段。
///
/// 结果要经 `spawn_shared` 搬运，故实现 `Clone`（封面句柄共享输出与取消所有权）。
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

/// 四个 `artist/*` 接口一次并发拉取；发行物接口在自己的后台 future 内顺序遍历分页。
///
/// 它们彼此独立，按仓库既有做法用 `futures::join!`：`cyper::Client` 是 `!Send`，
/// 只能同一个 runtime 里并发，不能各自 spawn。
async fn fetch_artist_responses(
    api: &ApiState,
    artist_id: &str,
) -> Result<AuthorResponses, String> {
    let mut detail_api = api.clone();
    let mut desc_api = api.clone();
    let mut top_song_api = api.clone();
    let album_api = api.clone();
    let album_fetch = fetch_all_artist_albums(album_api, artist_id);
    let (detail, desc, top_song, album) = futures::join!(
        detail_api.artist_detail(artist_id),
        desc_api.artist_desc(artist_id),
        top_song_api.artist_top_song(artist_id),
        album_fetch,
    );
    let album = album.map_err(|error| format!("artist album pagination failed: {error}"))?;

    Ok(AuthorResponses {
        detail: detail.ok(),
        desc: desc.ok(),
        top_song: top_song.ok(),
        album: Some(album),
    })
}

/// 后台顺序遍历歌手发行物的所有分页。
///
/// 分页中途失败直接返回错误，不把已取得的首批数据冒充成完整列表。
async fn fetch_all_artist_albums(
    mut api: ApiState,
    artist_id: &str,
) -> Result<ApiResponse, String> {
    let mut pages = Vec::new();
    let mut offset = 0usize;

    loop {
        let page = api
            .artist_album(artist_id, ARTIST_ALBUM_PAGE_SIZE, offset)
            .await
            .map_err(|error| format!("offset {offset}: {error:#}"))?;
        let item_count = artist_album_items(&page.body)
            .map(|items| items.len())
            .ok_or_else(|| format!("offset {offset}: response has no album list"))?;
        let more = page
            .body
            .get("more")
            .and_then(Value::as_bool)
            .unwrap_or(false);

        pages.push(page);
        if !more {
            break;
        }
        if item_count == 0 {
            return Err(format!("offset {offset}: empty page marked as more"));
        }
        offset = offset
            .checked_add(item_count)
            .ok_or_else(|| "artist album pagination offset overflowed".to_string())?;
    }

    merge_artist_album_pages(pages)
}

fn merge_artist_album_pages(pages: Vec<ApiResponse>) -> Result<ApiResponse, String> {
    let mut pages = pages.into_iter();
    let Some(mut merged) = pages.next() else {
        return Err("artist album pagination returned no pages".to_string());
    };

    for mut page in pages {
        let items = take_artist_album_items(&mut page.body)
            .ok_or_else(|| "artist album page has no album list".to_string())?;
        let target = artist_album_items_mut(&mut merged.body)
            .ok_or_else(|| "merged artist album response has no album list".to_string())?;
        target.extend(items);
    }

    Ok(merged)
}

fn artist_album_items(body: &Value) -> Option<&[Value]> {
    if let Some(items) = body.get("hotAlbums").and_then(Value::as_array) {
        return Some(items);
    }
    body.pointer("/artist/albums")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
}

fn artist_album_items_mut(body: &mut Value) -> Option<&mut Vec<Value>> {
    if body.get("hotAlbums").and_then(Value::as_array).is_some() {
        return body.get_mut("hotAlbums").and_then(Value::as_array_mut);
    }
    body.pointer_mut("/artist/albums")
        .and_then(Value::as_array_mut)
}

fn take_artist_album_items(body: &mut Value) -> Option<Vec<Value>> {
    if body.get("hotAlbums").and_then(Value::as_array).is_some() {
        return body
            .get_mut("hotAlbums")
            .and_then(Value::as_array_mut)
            .map(std::mem::take);
    }
    body.pointer_mut("/artist/albums")
        .and_then(Value::as_array_mut)
        .map(std::mem::take)
}

/// 全屏页点作者名要拉的东西：先 `song/detail` 解析出段对应的作者 ID，再拉作者页数据。
///
/// 整段不借 `&mut App`，交给 `spawn_shared` 后台跑，宿主循环照常重绘。
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
    let responses = fetch_artist_responses(&api, &artist_id).await?;
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

const PLAYLIST_PAGE_SIZE: usize = 100;
type PlaylistFetchFuture = SharedTask<Result<PlaylistFetch, String>>;
type PlaylistPageFetchFuture = SharedTask<Result<PlaylistPageFetch, String>>;
/// 装箱后的任务体（`spawn_shared` 的入参类型）。
type PlaylistFetchTask = Pin<Box<dyn Future<Output = Option<Result<PlaylistFetch, String>>>>>;

type PlaylistPageTask = Pin<Box<dyn Future<Output = Option<Result<PlaylistPageFetch, String>>>>>;

#[derive(Clone)]
struct PlaylistPageFetch {
    source_id: String,
    offset: usize,
    tracks: Vec<PlaylistTrack>,
    next_offset: usize,
    has_more: bool,
}

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
/// 结果要经 `spawn_shared` 搬运，故实现 `Clone`（封面句柄共享输出与取消所有权）。
#[derive(Clone)]
struct PlaylistFetch {
    id: String,
    title: String,
    artist: String,
    description: String,
    cover_url: Option<String>,
    tracks: Vec<PlaylistTrack>,
    total_tracks: Option<usize>,
    next_offset: usize,
    has_more: bool,
    paginated: bool,
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

/// 后台获取歌单指定分页的歌曲详情，不持有 `&mut App`。
///
/// 使用 `trackIds` 的分页接口，避免 `playlist.detail` 返回的 `tracks` 截断。
async fn fetch_playlist_tracks_page(
    mut api: ApiState,
    playlist_id: String,
    offset: usize,
    total_tracks: Option<usize>,
) -> Result<PlaylistPageFetch, String> {
    let response = api
        .playlist_track_all(&playlist_id, PLAYLIST_PAGE_SIZE, offset)
        .await
        .map_err(|err| err.to_string())?;
    let code = response_code(&response);
    if code != 200 {
        return Err(format!("请求失败({code}): {}", response_message(&response)));
    }

    let tracks = response
        .body
        .get("songs")
        .or_else(|| response.body.pointer("/data/songs"))
        .or_else(|| response.body.pointer("/playlist/tracks"))
        .and_then(Value::as_array)
        .map(|items| parse_tracks(items))
        .unwrap_or_default();
    if tracks.is_empty() && total_tracks.is_some_and(|total| offset < total) {
        return Err("歌单分页返回空数据".to_string());
    }

    let track_count = tracks.len();
    let next_offset = total_tracks
        .map(|total| offset.saturating_add(PLAYLIST_PAGE_SIZE).min(total))
        .unwrap_or_else(|| offset.saturating_add(track_count));
    Ok(PlaylistPageFetch {
        source_id: playlist_id,
        offset,
        tracks,
        next_offset,
        has_more: total_tracks
            .map(|total| next_offset < total)
            .unwrap_or(track_count >= PLAYLIST_PAGE_SIZE),
    })
}

/// 拉一次歌单页数据（不借 `&mut App`，可交给 `spawn_shared` 后台跑）。
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

    let total_tracks = parse_usize_value(playlist.get("trackCount"));
    let first_page =
        fetch_playlist_tracks_page(api.clone(), playlist_id.clone(), 0, total_tracks).await?;

    Ok(PlaylistFetch {
        id: playlist_id,
        title,
        artist,
        description,
        cover_url,
        tracks: first_page.tracks,
        total_tracks,
        next_offset: first_page.next_offset,
        has_more: first_page.has_more,
        paginated: true,
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
        total_tracks: Some(tracks.len()),
        next_offset: tracks.len(),
        has_more: false,
        paginated: false,
        tracks,
        liked: None,
    })
}

/// 全屏页点专辑名要拉的东西：先 `song/detail` 解析出 `al.id`，再拉专辑页数据。
///
/// 与作者页同理：整段不借 `&mut App`，交给 `spawn_shared` 后台跑；
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

pub fn peek_shared_future<T>(cover_bytes: &Option<SharedTask<T>>) -> Option<&T> {
    cover_bytes.as_ref()?.peek()?.as_ref()
}

/// 句柄不在 `Option` 里时的取值变体。
fn peek_shared<T>(fut: &SharedTask<T>) -> Option<&T> {
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

type LikeToggleFuture = SharedTask<Result<(), String>>;
type LikeVerifyFuture = SharedTask<Result<bool, ()>>;
/// 装箱后的任务体（`spawn_shared` 的入参类型）。
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
        if self.toggle.is_some() {
            return None;
        }
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
    identity: u64,
    wake: WakeSignal,
}

impl CoverFetchState {
    pub fn load(&mut self, api: ApiState, url: String) {
        self.wake = api.wake_signal();
        self.identity = next_list_generation();
        let cover_url = url.clone();
        let fut = Box::pin(async move { api.fetch_cover_image(&cover_url).await.ok() });
        self.image = Some(spawn_shared(fut, self.wake.clone()));
        self.url = Some(url);
        self.size = Size::ZERO;
        self.ascii = None;
    }

    pub fn render(
        &mut self,
        frame: &mut Frame,
        covers: &mut CoverPresentation,
        area: Rect,
        text_style: Style,
        bg_style: Option<Style>,
        draw_ascii: bool,
    ) {
        self.render_rows(
            frame,
            covers,
            area,
            area.height,
            0..area.height,
            text_style,
            bg_style,
            draw_ascii,
        );
    }

    /// Partial views copy rows of the full-size prepared surface. Scrolling does
    /// not create additional image geometry, codecs, or cropped cache entries.
    #[allow(clippy::too_many_arguments)]
    pub fn render_rows(
        &mut self,
        frame: &mut Frame,
        covers: &mut CoverPresentation,
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
                self.ascii = Some(make_ascii_future(
                    bytes.clone(),
                    area.width,
                    full_rows,
                    self.wake.clone(),
                ));
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

        if let Some(image) = peek_shared_future(&self.image) {
            covers.show(
                frame,
                CoverKey {
                    hash: self.identity,
                    width: area.width,
                    height: full_rows,
                },
                image,
                area,
                visible.start,
            );
        }
    }
}

fn make_ascii_future(
    image: Arc<DynamicImage>,
    width: u16,
    height: u16,
    wake: WakeSignal,
) -> AsciiFuture {
    let fut = Box::pin(async move {
        compio::runtime::spawn_blocking(move || render_cover_ascii(image, width, height))
            .await
            .ok()
            .flatten()
    });
    spawn_shared(fut, wake)
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
        if let Some(x) = cover_url {
            cover.load(api.clone(), x)
        }
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
    pub loading_more: bool,
    pub user_id: Option<String>,
    pub liked_playlist_id: Option<String>,
    pub user_name: String,
    pub created_playlists: Vec<HomeSidebarPlaylist>,
    pub collected_playlists: Vec<HomeSidebarPlaylist>,
    pub created_has_more: bool,
    pub collected_has_more: bool,
    pub created_next_offset: usize,
    pub collected_next_offset: usize,
    pub focused_section: HomeSidebarSection,
    pub focused_index: usize,
    pub created_focused_index: usize,
    pub collected_focused_index: usize,
    pub created_scroll_offset: usize,
    pub collected_scroll_offset: usize,
    pub(crate) motion: Toggle,
    pub(crate) trail: Trail,
    pub status_line: String,
}

impl Default for HomeSidebarState {
    fn default() -> Self {
        Self {
            expanded: false,
            loading: false,
            loading_more: false,
            user_id: None,
            liked_playlist_id: None,
            user_name: String::new(),
            created_playlists: Vec::new(),
            collected_playlists: Vec::new(),
            created_has_more: false,
            collected_has_more: false,
            created_next_offset: 0,
            collected_next_offset: 0,
            focused_section: HomeSidebarSection::Created,
            focused_index: 0,
            created_focused_index: 0,
            collected_focused_index: 0,
            created_scroll_offset: 0,
            collected_scroll_offset: 0,
            motion: Toggle::new(false),
            trail: Trail::new(0.0),
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
    fn section_has_more(&self, section: HomeSidebarSection) -> bool {
        match section {
            HomeSidebarSection::Created => self.created_has_more,
            HomeSidebarSection::Collected => self.collected_has_more,
        }
    }

    fn section_next_offset(&self, section: HomeSidebarSection) -> usize {
        match section {
            HomeSidebarSection::Created => self.created_next_offset,
            HomeSidebarSection::Collected => self.collected_next_offset,
        }
    }

    fn append_section_page(&mut self, page: HomeSidebarPageFetch) {
        match page.section {
            HomeSidebarSection::Created => {
                self.created_playlists.extend(page.items);
                self.created_next_offset = page.next_offset;
                self.created_has_more = page.has_more;
            }
            HomeSidebarSection::Collected => {
                self.collected_playlists.extend(page.items);
                self.collected_next_offset = page.next_offset;
                self.collected_has_more = page.has_more;
            }
        }
        self.clamp_focus();
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
        self.expanded || self.motion.on() || self.trail.value() > 0.0
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
    pub total_tracks: Option<usize>,
    pagination: Option<PlaylistPagination>,
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
            total_tracks: None,
            pagination: None,
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
        self.cancel_pagination();
        self.total_tracks = None;
        self.tracks = tracks;
        self.focused_idx = 0;
        self.scroll_offset = 0;
        self.generation = next_list_generation();
        self.ensure_focus_visible();
    }
    pub fn append_tracks(&mut self, tracks: Vec<PlaylistTrack>) {
        self.tracks.extend(tracks);
        self.generation = next_list_generation();
        self.ensure_focus_visible();
    }

    fn cancel_pagination(&mut self) {
        if let Some(pagination) = self.pagination.take() {
            pagination.cancel();
        }
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
        if let Some(x) = cover_url {
            cover.load(api.clone(), x)
        }
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
    fn as_playlist_track(&self) -> PlaylistTrack {
        PlaylistTrack {
            kind: PlaylistTrackKind::Song,
            id: Some(self.song_id.clone()),
            title: self.title.clone(),
            artist: self.artist.clone(),
            album: self.album.clone(),
            cover_url: self.cover_url.clone(),
            duration_ms: self.duration_ms,
            duration: format_duration(self.duration_ms),
        }
    }

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
    pub playlist_cover: Option<Vec<u8>>,
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
    generation: u64,
}

#[derive(Debug, Clone)]
struct CoverFetchResult {
    song_id: String,
    url: String,
    generation: u64,
    bytes: Option<Vec<u8>>,
}

fn cover_cache_path_for_dir(cache_dir: &Path, url: &str) -> Option<PathBuf> {
    let key = url.trim();
    if key.is_empty() {
        return None;
    }

    let mut hasher = DefaultHasher::new();
    key.hash(&mut hasher);
    Some(cache_dir.join(format!("{:016x}.img", hasher.finish())))
}

fn validate_cached_cover(bytes: Vec<u8>) -> Option<Vec<u8>> {
    if bytes.is_empty() {
        return None;
    }
    let format = image::guess_format(&bytes).ok()?;
    match format {
        image::ImageFormat::Png
            if !bytes.ends_with(&[0, 0, 0, 0, 73, 69, 78, 68, 174, 66, 96, 130]) =>
        {
            return None;
        }
        image::ImageFormat::Jpeg if !bytes.ends_with(&[0xff, 0xd9]) => return None,
        image::ImageFormat::Png | image::ImageFormat::Jpeg => {}
        _ => return None,
    }
    image::load_from_memory(&bytes).ok()?;
    Some(bytes)
}

/// Only validated API bytes enter this path. Keep the buffer shared with the
/// FIFO job until completion, then recover ownership without copying its data.
async fn persist_fetched_cover(
    persistence: &PersistenceHandle,
    path: PathBuf,
    bytes: Vec<u8>,
) -> Vec<u8> {
    let bytes = Arc::new(bytes);
    let job_bytes = bytes.clone();
    let (done_tx, done_rx) = futures::channel::oneshot::channel();
    let queued = persistence.enqueue(move || {
        let result = write_atomic(&path, &job_bytes);
        drop(job_bytes);
        let _ = done_tx.send(());
        result
    });
    if queued.is_ok() {
        let _ = done_rx.await;
    } else if let Err(error) = queued {
        log::warn!("cover persistence queue unavailable: {error:#}");
    }
    Arc::unwrap_or_clone(bytes)
}

#[derive(Debug, Clone)]
struct LyricFetchRequest {
    song_id: String,
    cookie: Option<String>,
    generation: u64,
}

#[derive(Debug, Clone)]
struct LyricFetchResult {
    song_id: String,
    generation: u64,
    lyrics: Option<Vec<LyricLine>>,
}

async fn loop_cover_fetch(
    rx: watch::Receiver<Option<CoverFetchRequest>>,
    tx: async_mpsc::Sender<CoverFetchResult>,
    client: Client,
    cache_dir: PathBuf,
    persistence: PersistenceHandle,
    wake: WakeSignal,
) {
    let Ok(api) = ApiState::new(None, client) else {
        return;
    };
    let process_fn = async move |req: &CoverFetchRequest| {
        if req.url.is_empty() {
            return None;
        }
        if let Some(path) = cover_cache_path_for_dir(&cache_dir, &req.url) {
            let cached = compio::runtime::spawn_blocking(move || {
                std::fs::read(path).ok().and_then(validate_cached_cover)
            })
            .await
            .ok()
            .flatten();
            if cached.is_some() {
                return cached;
            }
        }
        let bytes = api.fetch_cover_bytes(&req.url).await.ok()?;
        let path = cover_cache_path_for_dir(&cache_dir, &req.url)?;
        Some(persist_fetched_cover(&persistence, path, bytes).await)
    };
    latest_fetch::run_latest(
        rx,
        tx,
        async move |req: CoverFetchRequest| {
            let bytes = process_fn(&req).await;
            CoverFetchResult {
                song_id: req.song_id,
                url: req.url,
                generation: req.generation,
                bytes,
            }
        },
        wake,
    )
    .await;
}

async fn loop_lyric_fetch(
    rx: watch::Receiver<Option<LyricFetchRequest>>,
    tx: async_mpsc::Sender<LyricFetchResult>,
    mut api: ApiState,
) {
    let wake = api.wake_signal();
    let mut process_fn = async move |req: &LyricFetchRequest| {
        if let Some(cookie) = &req.cookie {
            api.set_cookie(cookie.to_string());
        }

        let lyric = api.lyric(&req.song_id).await.ok()?;
        let lrc = lyric.body.pointer("/lrc/lyric")?.as_str()?;
        parse_lrc(lrc).or_else(|| parse_plain_lyrics(lrc))
    };
    latest_fetch::run_latest(
        rx,
        tx,
        async move |req: LyricFetchRequest| {
            let lyrics = process_fn(&req).await;
            LyricFetchResult {
                song_id: req.song_id,
                generation: req.generation,
                lyrics,
            }
        },
        wake,
    )
    .await;
}

pub struct App {
    pub(crate) wake: WakeSignal,
    pub config: Config,
    pub theme: Theme,
    pub page: Page,
    pub overlay: Option<Overlay>,
    pub login: LoginState,
    pub browse: BrowseController,
    /// 上次检查 stderr 日志体积的时刻。
    stderr_trim_checked_at: Option<Instant>,
    stderr_trim_inflight: Option<Receiver<()>>,
    home_sidebar_anim_span_cells: u16,
    pub search: SearchController,
    pub playback: PlaybackController,
    pub startup: StartupController,
    pub player_bar_hits: PlayerBarHitTargets,
    /// 最近一次同步到的终端尺寸（单元格）。
    pub term_width: u16,
    pub term_height: u16,
    /// 当前生效的小窗口模式；None 表示普通内容页或未启用小窗口显示。
    pub small_window_mode: Option<SmallWindowMode>,
    /// 扁窗高度恰为播放栏高度时，Alt+X 切换的当前目标面板。
    pub flat_panel: FlatPanel,
    /// Visual transition, separate from the logical target in `flat_panel`.
    pub(crate) flat_switch_anim: Option<Transition>,
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
    pub input: InputController,
    pub settings: SettingsController,
    /// about 弹窗里的形象彩蛋状态。
    #[cfg(feature = "easter-egg")]
    pub about_egg: AboutEasterEgg,
    pub session_cookie: Option<String>,
    pub should_quit: bool,
    pub launch_fullscreen_requested: bool,
    pub vip_audio_unlocked: bool,
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
    pub downloads: DownloadController,
    pub(crate) covers: CoverPresentation,
}

impl App {
    pub fn draw_ascii(&self) -> bool {
        self.config.graphics_protocol == GraphicsProtocol::Off
    }

    /// 构造 App 并启动后台初始化：本地设置同步完成，网络初始化交给
    /// [`StartupInit`]，加载页随即可以显示真实进度。
    pub async fn new(config: Config, theme: Theme) -> Result<Self> {
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
    pub fn flush_persistence(&self) -> Result<()> {
        self.persistence.flush().map_err(anyhow::Error::msg)
    }
    pub async fn tick(&mut self) {
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

    pub fn set_runtime_status(&mut self, text: impl Into<String>) {
        let text = text.into();
        self.browse.home.status_line = text.clone();
        self.search.status_line = text;
    }

    fn lang_text<'a>(&self, zh: &'a str, en: &'a str) -> &'a str {
        lang_text(self.config.language, zh, en)
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
            && !value.trim().is_empty()
        {
            return value.to_string();
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
            && !value.trim().is_empty()
        {
            return value.to_string();
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

    if let Some(playlist) = response.body.get("playlist")
        && let Some(cover_url) = first_non_empty(playlist, &["/coverImgUrl", "/picUrl"])
    {
        return Some(cover_url);
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
    if let Ok(response) = api.recommend_songs().await
        && response_code(&response) == 200
        && let Some(songs) = home_daily_song_items(&response.body)
        && let Some(cover_url) = songs
            .iter()
            .find_map(|item| first_non_empty(item, &["/al/picUrl", "/album/picUrl"]))
    {
        daily_tile.cover.load(api.clone(), cover_url);
    }

    let mut cards = Vec::new();

    if let Ok(response) = api.recommend_resource().await {
        cards = parse_recommend_cards(&response, 24);
    }

    if cards.is_empty()
        && let Ok(response) = api.personalized(24).await
    {
        cards = parse_personalized_cards(&response, 24);
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

        if pinned_title == Some("私人雷达")
            && let Some(playlist_id) = tile.id.clone()
            && let Some(cover_url) = fetch_playlist_cover_url(api, &playlist_id).await
        {
            tile.cover.load(api.clone(), cover_url);
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
        if let Some(value) = response.body.pointer(pointer)
            && let Some(id) = parse_value_as_string(Some(value))
            && !id.trim().is_empty()
        {
            return Some(id);
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
        if let Some(value) = response.body.pointer(pointer)
            && let Some(id) = parse_value_as_string(Some(value))
            && !id.trim().is_empty()
        {
            return Some(id);
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
            if let Some(id) = &track.id
                && seen.insert(id.clone())
            {
                tracks.push(track);
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

    if let Some(index) = last_played_index
        && let Some(last) = old_tracks.get(index)
    {
        new_tracks.push(last.clone());
        kept_last_played = true;
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
            if song.get("al").is_none()
                && let Some(album) = song.get("album").cloned()
            {
                song["al"] = album;
            }
            if song.get("ar").is_none()
                && let Some(artists) = song.get("artists").cloned()
            {
                song["ar"] = artists;
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
    if let Some(value) = body.pointer(&format!("/data/{song_id}"))
        && let Some(liked) = parse_song_like_check_flag(value)
    {
        return Some(liked);
    }

    for pointer in [
        "/data/0/liked",
        "/songs/0/liked",
        "/data/songs/0/liked",
        "/liked",
    ] {
        if let Some(value) = body.pointer(pointer)
            && let Some(liked) = parse_song_like_check_flag(value)
        {
            return Some(liked);
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
                && !value.is_empty()
            {
                out.insert(value);
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
    if let Some(number) = value.as_i64()
        && number >= 0
    {
        return Some(number as usize);
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
    if let Some(name) = artist_name_segments(line).get(index).copied()
        && let Some((_, id)) = artists
            .iter()
            .find(|(candidate, _)| candidate.as_str() == name)
    {
        return id.clone();
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
    if let Some(text) = value.as_str()
        && !text.trim().is_empty()
    {
        return Some(text.to_string());
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

    if let Some(rest) = lower.strip_prefix('f')
        && let Ok(num) = rest.parse::<u8>()
        && num > 0
    {
        return Some(format!("F{}", num));
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
