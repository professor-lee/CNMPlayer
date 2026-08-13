mod api;
mod mpris_bridge;
pub(crate) mod player;
pub(crate) mod streaming;

use crate::app::api::error_for_status;
use crate::app::player::is_nonempty_file;
use crate::data::config::{AudioQuality, BarChannels, BarNumber, Language, VisualizeMode};
use crate::data::config::{Config, GraphicsProtocol};
use crate::data::playback_session;
use crate::data::session;
use crate::data::theme_loader::ThemeLoader;
use crate::launch;
use crate::render::cover_renderer::render_cover_ascii;
use crate::render::graphics_overlay::cover_viewport;
use crate::tmplayer::app::state::LyricLine;
use crate::tmplayer::audio::cava::{CavaChannels, CavaConfig, MiniCavaState};
use crate::tmplayer::playback::metadata::{parse_lrc, parse_plain_lyrics};
use crate::ui::theme::Theme;
use anyhow::{Result, anyhow};
use crossterm::event::{
    KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use cyper::Client;
use futures::channel::mpsc::{UnboundedReceiver, UnboundedSender, unbounded};
use futures::{FutureExt, future::Shared};
use http::header;
use image::{DynamicImage, GenericImageView};
use ncm_api::ApiResponse;
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
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use unicode_width::UnicodeWidthChar;

use api::ApiState;
use mpris_bridge::{MprisBridge, MprisControlEvent, MprisSyncPayload};
use player::{AudioPlayer, AudioPlayerState, cleanup_cache_dir, resolve_cache_root};
use streaming::StreamingReader;

const MAX_INPUT_LEN: usize = 64;
const SEARCH_RESULT_PAGE_SIZE: usize = 50;
const SEARCH_BOX_TARGET_HEIGHT: u16 = 3;
const HOME_SIDEBAR_PLAYLIST_LIMIT: usize = 100;
const SETTINGS_ROOT_ITEMS: usize = 10;
const SETTINGS_PLAYBACK_ITEMS: usize = 9;
pub(crate) const SETTINGS_KEYBIND_ITEMS: usize = 19;
const CONTENT_DOUBLE_CLICK_MS: u64 = 400;
const GLOBAL_HOTKEY_COOLDOWN_MS: u64 = 120;
const STARTUP_LOADING_MIN_VISIBLE_SECS: f32 = 0.75;
const STARTUP_LOADING_FILL_SECS: f32 = 0.62;
const STARTUP_LOADING_COMPLETE_RAMP_SECS: f32 = 0.26;
const RESERVED_RESET_KEYBIND: &str = "Ctrl+Alt+R";
const COVER_CACHE_SUBDIR: &str = "cover";
const COVER_FETCH_RETRY_MS: u64 = 1500;
const LYRICS_FETCH_RETRY_MS: u64 = 1500;

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
    SettingsAbout,
    SearchBox,
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
type CoverFuture = SharedFuture<Arc<DynamicImage>>;
type AsciiFuture = SharedFuture<String>;

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

#[derive(Clone, Default)]
pub struct CoverFetchState {
    pub url: Option<String>,
    pub image: Option<CoverFuture>,
    ascii: Option<AsciiFuture>,
    size: Size,
    protocol: Option<Arc<Mutex<StatefulProtocol>>>,
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
        if let Some(bg) = bg_style {
            frame.render_widget(Block::default().style(bg), area);
        }

        let (w, h) = (area.width, area.height);
        if draw_ascii {
            let placeholder = move || placeholder_cover_ascii(w, h, '░');
            if self.ascii.is_none() || self.size != area.as_size() {
                if let Some(bytes) = peek_shared_future(&self.image) {
                    self.ascii = Some(make_ascii_future(bytes.clone(), w, h));
                    self.size = area.as_size();
                }
            }
            let ascii = match peek_shared_future(&self.ascii) {
                Some(x) => x.clone(),
                None => placeholder(),
            };
            frame.render_widget(Paragraph::new(ascii).style(text_style), area);
        } else {
            let Some(img) = peek_shared_future(&self.image) else {
                return;
            };
            if self.protocol.is_none() || self.size != area.as_size() {
                let (img_w, img_h) = img.dimensions();
                let (x, y, w, h) = cover_viewport(img_w, img_h, w, h);
                let img = img.crop_imm(x, y, w, h);
                self.protocol = Some(Arc::new(Mutex::new(picker.new_resize_protocol(img))));
                self.size = area.as_size();
            }
            if let Some(proto) = &self.protocol {
                let mut proto = proto.lock().unwrap();
                let widget = StatefulImage::<StatefulProtocol>::default();
                frame.render_stateful_widget(widget, area, &mut proto);
            }
        }
    }
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
const HOME_PINNED_TITLES: [&str; 3] = ["每日推荐", "私人雷达", "欧美私人雷达"];

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

pub struct SearchItem {
    pub left_label: String,
    pub right_label: String,
    pub type_tag: Option<String>,
    pub song_id: Option<String>,
    pub album_id: Option<String>,
    pub playlist_id: Option<String>,
    pub artist_id: Option<String>,
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub cover_url: Option<String>,
    pub duration_ms: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchFilter {
    Single,
    Album,
    Author,
    Playlist,
}

impl SearchFilter {
    fn search_type(self) -> i32 {
        match self {
            Self::Single => 1,
            Self::Album => 10,
            Self::Author => 100,
            Self::Playlist => 1000,
        }
    }

    fn display_name(self) -> &'static str {
        match self {
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
    pub filter: SearchFilter,
    pub next_offset: usize,
    pub has_more: bool,
    pub scroll_offset: usize,
    pub visible_rows: usize,
}

impl Default for SearchState {
    fn default() -> Self {
        Self {
            query: String::new(),
            focused_idx: 0,
            results: Vec::new(),
            status_line: "输入关键词后按 Enter 搜索".to_string(),
            filter: SearchFilter::Single,
            next_offset: 0,
            has_more: false,
            scroll_offset: 0,
            visible_rows: 1,
        }
    }
}

impl SearchState {
    fn max_scroll_offset(&self) -> usize {
        self.results.len().saturating_sub(self.visible_rows.max(1))
    }

    fn clamp_scroll_offset(&mut self) {
        self.scroll_offset = self.scroll_offset.min(self.max_scroll_offset());
    }

    fn ensure_focus_visible(&mut self) {
        if self.results.is_empty() {
            self.focused_idx = 0;
            self.scroll_offset = 0;
            return;
        }

        self.focused_idx = self.focused_idx.min(self.results.len() - 1);
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
        if self.results.is_empty() {
            self.focused_idx = 0;
            self.scroll_offset = 0;
            return;
        }

        self.focused_idx = index.min(self.results.len() - 1);
        self.ensure_focus_visible();
    }

    pub fn focus_next(&mut self) -> bool {
        if self.results.is_empty() || self.focused_idx + 1 >= self.results.len() {
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
        if self.results.is_empty() || self.focused_idx == 0 {
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

    pub fn set_results(&mut self, results: Vec<SearchItem>) {
        self.results = results;
        self.focused_idx = 0;
        self.next_offset = self.results.len();
        self.has_more = self.results.len() >= SEARCH_RESULT_PAGE_SIZE;
        self.scroll_offset = 0;
        self.ensure_focus_visible();
    }

    pub fn append_results(&mut self, mut results: Vec<SearchItem>) -> usize {
        let added = results.len();
        self.results.append(&mut results);
        self.next_offset = self.results.len();
        self.has_more = added >= SEARCH_RESULT_PAGE_SIZE;
        self.clamp_scroll_offset();
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
        self.ensure_focus_visible();
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

#[derive(Debug, Clone, Copy, Default)]
pub struct PlayerBarHitTargets {
    pub prev: Option<HitRect>,
    pub play_pause: Option<HitRect>,
    pub next: Option<HitRect>,
    pub progress: Option<HitRect>,
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
    home_sidebar_anim_span_cells: u16,
    pub playlist: PlaylistState,
    pub author: AuthorState,
    pub search: SearchState,
    pub now_playing: Option<PlaybackTrack>,
    pub now_playing_liked: bool,
    pub liked_song_ids: HashSet<String>,
    pub playback_queue: Vec<PlaybackTrack>,
    pub playback_index: Option<usize>,
    pub playback_repeat_mode: PlaybackRepeatMode,
    pub playback_state: PlaybackRuntimeState,
    pub startup_loading_progress: f32,
    pub player_bar_hits: PlayerBarHitTargets,
    pub home_sidebar_panel_hit: Option<HitRect>,
    pub home_sidebar_playlist_hits: Vec<(HitRect, HomeSidebarHit)>,
    pub home_tile_hits: Vec<(HitRect, usize)>,
    pub playlist_track_hits: Vec<(HitRect, usize)>,
    pub author_tile_hits: Vec<(HitRect, usize)>,
    pub search_item_hits: Vec<(HitRect, usize)>,
    pub search_box_input: String,
    pub search_box_cursor: usize,
    pub search_box_anim_height: u16,
    pub settings_selected: usize,
    pub settings_playback_selected: usize,
    pub settings_keybind_selected: usize,
    pub settings_keybind_rebinding: Option<usize>,
    pub session_cookie: Option<String>,
    pub should_quit: bool,
    pub launch_fullscreen_requested: bool,
    pub vip_audio_unlocked: bool,
    search_return_page: Page,
    playlist_return_page: Page,
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
    pub graphics_picker: Picker,
}

impl App {
    pub fn draw_ascii(&self) -> bool {
        self.config.graphics_protocol == GraphicsProtocol::Off
    }

    pub async fn new(config: Config, theme: Theme) -> Result<Self> {
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
            home_sidebar_anim_span_cells: 24,
            playlist: PlaylistState::default(),
            author: AuthorState::default(),
            search: SearchState::default(),
            now_playing: None,
            now_playing_liked: false,
            liked_song_ids: HashSet::new(),
            playback_queue: Vec::new(),
            playback_index: None,
            playback_repeat_mode: PlaybackRepeatMode::Sequence,
            playback_state: PlaybackRuntimeState::Stopped,
            startup_loading_progress: 0.0,
            player_bar_hits: PlayerBarHitTargets::default(),
            home_sidebar_panel_hit: None,
            home_sidebar_playlist_hits: Vec::new(),
            home_tile_hits: Vec::new(),
            playlist_track_hits: Vec::new(),
            author_tile_hits: Vec::new(),
            search_item_hits: Vec::new(),
            search_box_input: String::new(),
            search_box_cursor: 0,
            search_box_anim_height: 0,
            settings_selected: 0,
            settings_playback_selected: 0,
            settings_keybind_selected: 0,
            settings_keybind_rebinding: None,
            session_cookie: None,
            should_quit: false,
            launch_fullscreen_requested: false,
            vip_audio_unlocked: false,
            search_return_page: Page::Home,
            playlist_return_page: Page::Home,
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
            graphics_picker: Picker::halfblocks(),
        };

        if let Ok(_) = Picker::from_query_stdio() {
            // Don't use queried picker, this cause image layouted improperly on konsole.
            // It's ok to not set this if we just use Halfblocks.

            // app.graphics_picker = picker;
        }
        if let Some(protocol) = app.config.graphics_protocol.to_ratatui_protocol() {
            app.graphics_picker.set_protocol_type(protocol);
        }

        app.sync_cava();

        if let Some(cookie) = saved_cookie {
            match app.api.validate_cookie(&cookie).await {
                Ok(true) => {
                    app.session_cookie = app.api.session_cookie().map(|value| value.to_string());
                    app.refresh_vip_audio_access().await;
                    let _ = app.refresh_liked_song_cache().await;
                    app.home.status_line = "已恢复上次登录，正在加载推荐歌单".to_string();
                    app.begin_startup_loading();
                    if let Err(err) = app.load_home_recommendations().await {
                        app.home.status_line = format!("已恢复登录，但推荐加载失败: {}", err);
                    }
                    app.finish_startup_loading();
                    app.try_restore_playback_memory().await;
                    return Ok(app);
                }
                Ok(false) => {
                    let _ = session::clear_cookie();
                }
                Err(_) => {}
            }
        }

        app.refresh_qr_login().await;
        Ok(app)
    }

    pub async fn tick(&mut self) {
        self.tick_audio().await;
        self.tick_cover_fetch();
        self.tick_lyric_fetch();
        self.apply_mpris_control_events().await;
        self.sync_mpris_exposure();
        self.tick_search_box_animation();
        self.tick_startup_loading();

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

    pub async fn handle_mouse(&mut self, mouse: MouseEvent) {
        if self.page == Page::Login || self.page == Page::Loading {
            return;
        }

        let col = mouse.column;
        let row = mouse.row;

        match mouse.kind {
            MouseEventKind::ScrollUp => {
                self.handle_content_scroll(col, row, false).await;
            }
            MouseEventKind::ScrollDown => {
                self.handle_content_scroll(col, row, true).await;
            }
            MouseEventKind::Down(MouseButton::Left) => {
                if matches!(self.overlay, Some(Overlay::SearchBox)) {
                    self.handle_search_box_click(col, row);
                    return;
                }

                if self.overlay.is_some() {
                    return;
                }

                if self.handle_content_click(col, row).await {
                    return;
                }

                if let Some(rect) = self.player_bar_hits.prev {
                    if rect.contains(col, row) {
                        self.play_previous_hotkey().await;
                        return;
                    }
                }
                if let Some(rect) = self.player_bar_hits.play_pause {
                    if rect.contains(col, row) {
                        self.toggle_play_pause_hotkey().await;
                        return;
                    }
                }
                if let Some(rect) = self.player_bar_hits.next {
                    if rect.contains(col, row) {
                        self.play_next_hotkey().await;
                        return;
                    }
                }
                if let Some(rect) = self.player_bar_hits.progress {
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
            _ => {}
        }
    }

    fn player_bar_contains(&self, col: u16, row: u16) -> bool {
        self.player_bar_hits
            .prev
            .map(|rect| rect.contains(col, row))
            .unwrap_or(false)
            || self
                .player_bar_hits
                .play_pause
                .map(|rect| rect.contains(col, row))
                .unwrap_or(false)
            || self
                .player_bar_hits
                .next
                .map(|rect| rect.contains(col, row))
                .unwrap_or(false)
            || self
                .player_bar_hits
                .progress
                .map(|rect| rect.contains(col, row))
                .unwrap_or(false)
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
            _ => {}
        };
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
        self.home_tile_hits.clear();
        self.playlist_track_hits.clear();
        self.author_tile_hits.clear();
        self.search_item_hits.clear();
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

    pub fn push_home_tile_hit(&mut self, rect: HitRect, index: usize) {
        self.home_tile_hits.push((rect, index));
    }

    pub fn push_playlist_track_hit(&mut self, rect: HitRect, index: usize) {
        self.playlist_track_hits.push((rect, index));
    }

    pub fn push_author_tile_hit(&mut self, rect: HitRect, index: usize) {
        self.author_tile_hits.push((rect, index));
    }

    pub fn push_search_item_hit(&mut self, rect: HitRect, index: usize) {
        self.search_item_hits.push((rect, index));
    }

    pub fn playback_position(&self) -> Duration {
        self.audio_player.position()
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
    }

    fn sync_cava(&mut self) {
        let available = crate::tmplayer::audio::cava::is_available();
        let enable = self.config.visualize != VisualizeMode::Off;
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
            position: self.audio_player.position(),
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
            position: self.audio_player.position(),
        }
    }

    pub fn fullscreen_runtime_snapshot(&self) -> FullscreenRuntimeSnapshot {
        FullscreenRuntimeSnapshot {
            current_index: self.playback_index,
            now_playing_liked: self.now_playing_liked,
            state: self.playback_state,
            repeat_mode: self.playback_repeat_mode,
            position: self.audio_player.position(),
            volume: self.audio_player.volume(),
        }
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
        self.toggle_like_hotkey().await;
    }

    async fn handle_overlay_key(&mut self, overlay: Overlay, key: KeyEvent) {
        match overlay {
            Overlay::Settings => self.handle_settings_root_key(key).await,
            Overlay::SettingsPlayback => self.handle_settings_playback_key(key),
            Overlay::SettingsKeybinds => self.handle_settings_keybinds_key(key),
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
                self.launch_fullscreen_requested = true;
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
            KeybindAction::ToggleLikeCollapsed => self.toggle_like_hotkey().await,
        }
    }

    async fn toggle_home_sidebar(&mut self) {
        if self.page != Page::Home || self.overlay.is_some() {
            return;
        }

        if self.home_sidebar.expanded {
            self.home_sidebar.expanded = false;
            let target = if self.home_sidebar.expanded { 1.0 } else { 0.0 };
            self.home_sidebar.anim_progress = target;
            return;
        }

        self.home_sidebar.expanded = true;
        let target = if self.home_sidebar.expanded { 1.0 } else { 0.0 };
        self.home_sidebar.anim_progress = target;

        if !self.home_sidebar.created_playlists.is_empty()
            || !self.home_sidebar.collected_playlists.is_empty()
        {
            self.home_sidebar.reset_focus();
            return;
        }

        match self.load_home_sidebar_playlists().await {
            Ok(()) => {
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
            self.refresh_now_playing_like_state().await;
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
                let step = self.search.visible_rows.max(1);
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
                let step = self.search.visible_rows.max(1);
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

    async fn refresh_now_playing_like_state(&mut self) {
        let Some(song_id) = self.now_playing.as_ref().map(|track| track.song_id.clone()) else {
            self.now_playing_liked = false;
            return;
        };

        self.now_playing_liked = self.liked_song_ids.contains(&song_id);

        let Ok(song_id_num) = song_id.parse::<u64>() else {
            return;
        };

        let ids_json = format!("[{song_id_num}]");
        let Ok(response) = self.api.song_like_check(&ids_json).await else {
            return;
        };

        if response_code(&response) != 200 {
            return;
        }

        let Some(liked) = parse_song_like_check_result(&response.body, &song_id) else {
            return;
        };

        self.now_playing_liked = liked;
        if liked {
            self.liked_song_ids.insert(song_id);
        } else {
            self.liked_song_ids.remove(&song_id);
        }
    }

    async fn toggle_like_hotkey(&mut self) {
        let Some(song_id) = self.now_playing.as_ref().map(|track| track.song_id.clone()) else {
            self.set_runtime_status(self.lang_text(
                "当前没有可收藏的歌曲",
                "No song is available for like/unlike",
            ));
            return;
        };

        let target = !self.now_playing_liked;
        match self.api.like_song(&song_id, target).await {
            Ok(response) => {
                let code = response
                    .body
                    .get("code")
                    .and_then(|value| value.as_i64())
                    .unwrap_or(response.status);
                if code == 200 {
                    if target {
                        self.liked_song_ids.insert(song_id.clone());
                    } else {
                        self.liked_song_ids.remove(&song_id);
                    }
                    self.now_playing_liked = self.liked_song_ids.contains(&song_id);
                    self.set_runtime_status(if target {
                        self.lang_text("已收藏当前歌曲", "Liked current song")
                            .to_string()
                    } else {
                        self.lang_text("已取消收藏当前歌曲", "Unliked current song")
                            .to_string()
                    });
                } else {
                    self.set_runtime_status(format!(
                        "{}: {}",
                        self.lang_text("收藏操作失败", "Like operation failed"),
                        code
                    ));
                }
            }
            Err(err) => {
                self.set_runtime_status(format!(
                    "{}: {}",
                    self.lang_text("收藏操作失败", "Like operation failed"),
                    err
                ));
            }
        }
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

        let mut enriched = track.clone();
        // Switch UI state immediately and avoid blocking network fetches here.
        self.enrich_track_metadata(&mut enriched, false).await;
        if let Some(slot) = self.playback_queue.get_mut(index) {
            slot.cover = enriched.cover.clone();
        }
        self.trim_non_current_cover_memory(index);
        self.now_playing = Some(enriched.clone());
        self.refresh_now_playing_like_state().await;
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
                    &self.api.http_client(),
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

    async fn replace_queue_and_play(&mut self, queue: Vec<PlaybackTrack>, index: usize) {
        if queue.is_empty() {
            self.set_runtime_status(
                self.lang_text("当前页面没有可播放歌曲", "No playable songs on this page"),
            );
            return;
        }

        self.playback_queue = queue;
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
                self.replace_queue_and_play(queue, target).await;
            }
            PlaylistTrackKind::Album | PlaylistTrackKind::Ep | PlaylistTrackKind::Single => {
                self.open_focused_playlist_album().await;
            }
        }
    }

    async fn play_focused_search_track(&mut self) {
        if self.search.filter != SearchFilter::Single {
            self.set_runtime_status(self.lang_text(
                "仅“单曲”搜索结果支持直接播放",
                "Only 'Single' search results support direct playback",
            ));
            return;
        }

        let (queue, target) = self.build_queue_from_search();
        self.replace_queue_and_play(queue, target).await;
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

    async fn open_focused_search_author(&mut self) {
        if self.search.filter != SearchFilter::Author {
            return;
        }

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

        self.search.status_line = format!("正在加载作者 {}", title);

        match self.load_author_detail(&artist_id).await {
            Ok(()) => {
                match (&self.author.cover.image, fallback_cover_url) {
                    (None, Some(url)) => self.author.cover.load(self.api.clone(), url),
                    _ => (),
                }
                self.playlist_section_return_snapshot = None;
                self.page = Page::Author;
                self.search.status_line = format!("已打开作者 {}", self.author.title);
            }
            Err(err) => {
                self.search.status_line = format!("打开作者页失败: {}", err);
            }
        }
    }

    async fn open_focused_search_album(&mut self) {
        if self.search.filter != SearchFilter::Album {
            return;
        }

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

        self.search.status_line = format!(
            "{} {}",
            self.lang_text("正在加载专辑", "Loading album"),
            title
        );

        match self.load_album_detail(&album_id).await {
            Ok(()) => {
                self.playlist_section_return_snapshot = None;
                match (&self.playlist.cover.image, fallback_cover_url) {
                    (None, Some(url)) => self.playlist.cover.load(self.api.clone(), url),
                    _ => (),
                }
                self.playlist_return_page = Page::Search;
                self.page = Page::Playlist;
                self.search.status_line =
                    format!("{} {}", self.lang_text("已打开专辑", "Opened album"), title);
            }
            Err(err) => {
                self.search.status_line = format!(
                    "{}: {}",
                    self.lang_text("打开专辑失败", "Failed to open album"),
                    err
                );
            }
        }
    }

    async fn open_focused_search_playlist(&mut self) {
        if self.search.filter != SearchFilter::Playlist {
            return;
        }

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

        self.search.status_line = format!(
            "{} {}",
            self.lang_text("正在加载歌单", "Loading playlist"),
            title
        );

        match self.load_playlist_detail(&playlist_id).await {
            Ok(()) => {
                self.playlist_section_return_snapshot = None;
                match (&self.playlist.cover.image, fallback_cover_url) {
                    (None, Some(url)) => self.playlist.cover.load(self.api.clone(), url),
                    _ => (),
                }
                self.playlist_return_page = Page::Search;
                self.page = Page::Playlist;
                self.search.status_line = format!(
                    "{} {}",
                    self.lang_text("已打开歌单", "Opened playlist"),
                    title
                );
            }
            Err(err) => {
                self.search.status_line = format!(
                    "{}: {}",
                    self.lang_text("打开歌单失败", "Failed to open playlist"),
                    err
                );
            }
        }
    }

    async fn activate_focused_search_result(&mut self) {
        match self.search.filter {
            SearchFilter::Single => self.play_focused_search_track().await,
            SearchFilter::Album => self.open_focused_search_album().await,
            SearchFilter::Author => self.open_focused_search_author().await,
            SearchFilter::Playlist => self.open_focused_search_playlist().await,
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
            KeyCode::Enter => match self.settings_selected {
                0..=3 => self.apply_settings_root_delta(1).await,
                4 => {
                    self.settings_playback_selected = 0;
                    self.overlay = Some(Overlay::SettingsPlayback);
                }
                5 => {
                    self.open_keybind_settings();
                }
                6 => self.apply_settings_root_delta(1).await,
                7 => self.apply_settings_root_delta(1).await,
                8 => self.logout_to_login().await,
                9 => {
                    self.overlay = Some(Overlay::SettingsAbout);
                }
                _ => {}
            },
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
                self.settings_keybind_rebinding = Some(idx);
                self.set_runtime_status(format!(
                    "{} [{}]，{}",
                    self.lang_text("正在重绑", "Rebinding"),
                    self.keybind_name_for_index(idx),
                    self.lang_text(
                        "请按新快捷键（Esc 取消）",
                        "press a new shortcut (Esc to cancel)"
                    )
                ));
            }
            _ => {}
        }
    }

    fn handle_settings_about_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc | KeyCode::Left | KeyCode::Enter => {
                self.overlay = Some(Overlay::Settings);
            }
            KeyCode::Char('t') | KeyCode::Char('T') => {
                if key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT {
                    self.close_overlay();
                }
            }
            _ => {}
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
                if delta != 0 {
                    self.config.show_hints = !self.config.show_hints;
                    let _ = self.config.save();
                }
            }
            7 => {
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
                if crate::tmplayer::audio::cava::is_available() {
                    self.config.visualize = self.config.visualize.cycle(delta);
                    let _ = self.config.save();
                } else if self.config.visualize != crate::data::config::VisualizeMode::Off {
                    self.config.visualize = crate::data::config::VisualizeMode::Off;
                    let _ = self.config.save();
                }
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
                self.config.page_lyrics = !self.config.page_lyrics;
                let _ = self.config.save();
            }
            7 => {
                let next = self
                    .config
                    .audio_quality
                    .cycle(delta, self.vip_audio_unlocked);
                self.set_audio_quality(next);
            }
            8 => {
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
                    let target = if self.home_sidebar.expanded { 1.0 } else { 0.0 };
                    self.home_sidebar.anim_progress = target;
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
                self.page = Page::Search;
            }
            _ => {}
        }
    }

    fn tick_search_box_animation(&mut self) {
        if matches!(self.overlay, Some(Overlay::SearchBox)) {
            self.search_box_anim_height =
                (self.search_box_anim_height + 1).min(SEARCH_BOX_TARGET_HEIGHT);
        } else {
            self.search_box_anim_height = 0;
        }
    }

    fn begin_startup_loading(&mut self) {
        self.page = Page::Loading;
        self.overlay = None;
        self.startup_loading_progress = 0.0;
        self.startup_loading_started_at = Some(Instant::now());
        self.startup_loading_complete_started_at = None;
        self.startup_loading_complete_requested = false;
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
        self.startup_loading_progress = startup_loading_progress_at(
            elapsed,
            self.startup_loading_complete_started_at
                .map(|completed_at| completed_at.elapsed().as_secs_f32()),
            self.startup_loading_complete_requested,
        );

        if self.startup_loading_complete_requested && elapsed >= STARTUP_LOADING_MIN_VISIBLE_SECS {
            self.page = Page::Home;
            self.startup_loading_progress = 0.0;
            self.startup_loading_started_at = None;
            self.startup_loading_complete_started_at = None;
            self.startup_loading_complete_requested = false;
        }
    }

    pub fn startup_loading_progress_for_width(&self, _bar_width: u16) -> f32 {
        if self.page != Page::Loading {
            return 0.0;
        }

        let Some(started_at) = self.startup_loading_started_at else {
            return 0.0;
        };

        startup_loading_progress_at(
            started_at.elapsed().as_secs_f32(),
            self.startup_loading_complete_started_at
                .map(|completed_at| completed_at.elapsed().as_secs_f32()),
            self.startup_loading_complete_requested,
        )
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
        self.overlay = Some(Overlay::SearchBox);
    }

    fn close_overlay(&mut self) {
        self.overlay = None;
        self.search_box_anim_height = 0;
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
            self.search.set_results(Vec::new());
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

    pub fn fullscreen_config_snapshot(&self) -> crate::tmplayer::HostConfigSync {
        crate::tmplayer::HostConfigSync {
            theme: self.config.theme.clone(),
            transparent_background: self.config.transparent_background,
            album_border: self.config.album_border,
            language: self.config.language,
            graphics_protocol: self.config.graphics_protocol,
            page_lyrics: self.config.page_lyrics,
            audio_quality: self.config.audio_quality,
            eq_bands_db: self.config.eq_bands_db,
            playback_memory: self.config.playback_memory,
            vip_audio_unlocked: self.vip_audio_unlocked,
            show_hints: self.config.show_hints,
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

        if self.vip_audio_unlocked != sync.vip_audio_unlocked {
            self.vip_audio_unlocked = sync.vip_audio_unlocked;
            changed = true;
        }

        let clamped_quality = sync.audio_quality.clamp_for_vip(self.vip_audio_unlocked);
        if self.config.audio_quality != clamped_quality {
            self.config.audio_quality = clamped_quality;
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
            if let Some(cover_url) = self.playlist.cover.url.clone() {
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
        let target = record
            .current_index
            .unwrap_or(0)
            .min(self.playback_queue.len().saturating_sub(1));
        self.play_queue_index(target, false).await;
        self.set_runtime_status(self.lang_text("已恢复播放记忆", "Playback memory restored"));
    }

    fn lang_text<'a>(&self, zh: &'a str, en: &'a str) -> &'a str {
        match self.config.language {
            Language::Zh => zh,
            Language::En => en,
        }
    }

    async fn refresh_vip_audio_access(&mut self) {
        let mut unlocked = false;

        if let Ok(response) = self.api.vip_info_v2().await {
            unlocked = response_indicates_vip(&response);
        }

        if !unlocked && let Ok(response) = self.api.vip_info().await {
            unlocked = response_indicates_vip(&response);
        }

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
        self.vip_audio_unlocked = false;
        self.config.audio_quality = self.config.audio_quality.clamp_for_vip(false);

        self.login = LoginState::default();
        self.search = SearchState::default();
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
        self.liked_song_ids.clear();
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
        let key_resp = match self.api.login_qr_key().await {
            Ok(response) => response,
            Err(err) => {
                self.login.status_line = format!("二维码 key 获取失败: {}", err);
                return;
            }
        };

        let key = extract_qr_key(&key_resp);

        if key.is_empty() {
            self.login.status_line = "二维码 key 为空，请重试".to_string();
            return;
        }

        let qr_resp = match self.api.login_qr_create(&key).await {
            Ok(response) => response,
            Err(err) => {
                self.login.status_line = format!("二维码创建失败: {}", err);
                return;
            }
        };

        let qr_url = extract_qr_url(&qr_resp);

        self.login.qr_key = key;
        self.login.qr_url = qr_url.clone();
        self.login.status_line = if qr_url.is_empty() {
            "二维码已刷新，请按 Enter 轮询状态".to_string()
        } else {
            format!("二维码已刷新: {}", truncate_text(&qr_url, 48))
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

    async fn fetch_home_private_radar_cover(&mut self, playlist_id: &str) -> Option<String> {
        let response = self.api.playlist_detail(playlist_id).await.ok()?;
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

    async fn load_home_recommendations(&mut self) -> Result<()> {
        let mut daily_tile = HomeTile::placeholder_daily();
        if let Ok(response) = self.api.recommend_songs().await {
            if response_code(&response) == 200 {
                if let Some(songs) = home_daily_song_items(&response.body) {
                    if let Some(cover_url) = songs
                        .iter()
                        .find_map(|item| first_non_empty(item, &["/al/picUrl", "/album/picUrl"]))
                    {
                        daily_tile.cover.load(self.api.clone(), cover_url);
                    }
                }
            }
        }

        let mut cards = Vec::new();

        if let Ok(response) = self.api.recommend_resource().await {
            cards = parse_recommend_cards(&response, 24);
        }

        if cards.is_empty() {
            if let Ok(response) = self.api.personalized(24).await {
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

            let mut tile = HomeTile::from_recommendation(
                &self.api,
                card.id,
                card.title,
                card.subtitle,
                card.cover_url,
            );

            if pinned_title == Some("私人雷达") {
                if let Some(playlist_id) = tile.id.clone() {
                    if let Some(cover_url) = self.fetch_home_private_radar_cover(&playlist_id).await
                    {
                        tile.cover.load(self.api.clone(), cover_url);
                    }
                }
            }

            tiles.push(tile);
        }

        self.home.set_tiles(prioritize_home_tiles(
            &self.api,
            tiles,
            self.config.home_more_recommend,
        ));
        self.home.status_line = self
            .lang_text(
                "方向键/Tab 切换，Enter 打开歌单",
                "Use arrows/Tab to focus, Enter to open playlist",
            )
            .to_string();
        Ok(())
    }

    async fn load_home_sidebar_playlists(&mut self) -> Result<()> {
        self.home_sidebar.loading = true;

        let result = (async || -> Result<()> {
            let account = match self.api.user_account().await {
                Ok(v) => v,
                Err(_) => self.api.login_status().await?,
            };
            let account_code = response_code(&account);
            if account_code != 200 {
                return Err(anyhow!(
                    "{}({}): {}",
                    self.lang_text("账号信息请求失败", "Failed to fetch account profile"),
                    account_code,
                    response_message(&account)
                ));
            }

            let uid = extract_current_user_id(&account).ok_or_else(|| {
                anyhow!(self.lang_text("未找到当前用户 ID", "Current user id not found"))
            })?;
            let user_name = extract_current_user_name(&account)
                .unwrap_or_else(|| self.lang_text("当前用户", "Current User").to_string());

            let created_response = self
                .api
                .user_playlist_create(&uid, HOME_SIDEBAR_PLAYLIST_LIMIT, 0)
                .await?;
            let created_code = response_code(&created_response);
            if created_code != 200 {
                return Err(anyhow!(
                    "{}({}): {}",
                    self.lang_text("创建歌单请求失败", "Created playlists request failed"),
                    created_code,
                    response_message(&created_response)
                ));
            }

            let collected_response = self
                .api
                .user_playlist_collect(&uid, HOME_SIDEBAR_PLAYLIST_LIMIT, 0)
                .await?;
            let collected_code = response_code(&collected_response);
            if collected_code != 200 {
                return Err(anyhow!(
                    "{}({}): {}",
                    self.lang_text("收藏歌单请求失败", "Collected playlists request failed"),
                    collected_code,
                    response_message(&collected_response)
                ));
            }

            let created_playlists = parse_home_sidebar_playlists(&created_response);
            let collected_playlists = parse_home_sidebar_playlists(&collected_response);

            self.home_sidebar.user_id = Some(uid);
            self.home_sidebar.liked_playlist_id = extract_liked_playlist_id(&account);
            self.home_sidebar.user_name = user_name;
            self.home_sidebar.created_playlists = created_playlists;
            self.home_sidebar.collected_playlists = collected_playlists;
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

            Ok(())
        })()
        .await;

        self.home_sidebar.loading = false;
        result
    }

    async fn resolve_current_user_id(&mut self) -> Result<String> {
        if let Some(uid) = self.home_sidebar.user_id.as_ref() {
            return Ok(uid.clone());
        }

        let account = match self.api.user_account().await {
            Ok(v) => v,
            Err(_) => self.api.login_status().await?,
        };
        let code = response_code(&account);
        if code != 200 {
            return Err(anyhow!(
                "{}({}): {}",
                self.lang_text("账号信息请求失败", "Failed to fetch account profile"),
                code,
                response_message(&account)
            ));
        }

        let uid = extract_current_user_id(&account).ok_or_else(|| {
            anyhow!(self.lang_text("未找到当前用户 ID", "Current user id not found"))
        })?;

        self.home_sidebar.user_id = Some(uid.clone());
        self.home_sidebar.liked_playlist_id = extract_liked_playlist_id(&account);
        if let Some(name) = extract_current_user_name(&account) {
            self.home_sidebar.user_name = name;
        }

        Ok(uid)
    }

    async fn refresh_liked_song_cache(&mut self) -> Result<()> {
        let uid = self.resolve_current_user_id().await?;
        let response = self.api.likelist(&uid).await?;
        let code = response_code(&response);
        if code != 200 {
            return Err(anyhow!(
                "{}({}): {}",
                self.lang_text("喜爱列表请求失败", "Failed to fetch liked songs"),
                code,
                response_message(&response)
            ));
        }

        self.liked_song_ids = parse_likelist_song_ids(&response.body);
        self.refresh_now_playing_like_state().await;
        Ok(())
    }

    fn is_liked_playlist(&self, playlist_id: &str, title: Option<&str>) -> bool {
        if self
            .home_sidebar
            .liked_playlist_id
            .as_deref()
            .map(|id| id == playlist_id)
            .unwrap_or(false)
        {
            return true;
        }

        let title = title.unwrap_or_default().trim();
        !title.is_empty()
            && (title.contains("我喜欢的音乐")
                || title.to_ascii_lowercase().contains("liked songs"))
    }

    async fn load_playlist_detail(&mut self, playlist_id: &str) -> Result<()> {
        let response = self.api.playlist_detail(playlist_id).await?;
        let code = response_code(&response);
        if code != 200 {
            return Err(anyhow!(
                "请求失败({}): {}",
                code,
                response_message(&response)
            ));
        }

        let playlist = response
            .body
            .get("playlist")
            .ok_or_else(|| anyhow!("歌单数据缺失"))?;

        let title = playlist
            .get("name")
            .and_then(|value| value.as_str())
            .unwrap_or("未命名歌单")
            .to_string();

        if self.is_liked_playlist(playlist_id, Some(&title)) {
            let _ = self.refresh_liked_song_cache().await;
        }

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

        let cover_url = first_non_empty(playlist, &["/coverImgUrl", "/picUrl"]);

        let tracks = playlist
            .get("tracks")
            .and_then(|value| value.as_array())
            .map(|items| parse_tracks(items))
            .unwrap_or_default();

        self.playlist.id = Some(playlist_id.to_string());
        self.playlist.title = title;
        self.playlist.artist = artist;
        self.playlist.description = description;
        self.playlist.set_tracks(tracks);
        cover_url.map(|x| self.playlist.cover.load(self.api.clone(), x));
        Ok(())
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

    async fn load_album_detail(&mut self, album_id: &str) -> Result<()> {
        let response = self.api.album(album_id).await?;
        let code = response_code(&response);
        if code != 200 {
            return Err(anyhow!(
                "请求失败({}): {}",
                code,
                response_message(&response)
            ));
        }

        let album = response
            .body
            .get("album")
            .or_else(|| response.body.pointer("/data/album"))
            .ok_or_else(|| anyhow!("专辑数据缺失"))?;

        let title = album
            .get("name")
            .and_then(|value| value.as_str())
            .unwrap_or("未命名专辑")
            .to_string();

        let artist = first_non_empty(album, &["/artist/name", "/artists/0/name"])
            .unwrap_or_else(|| "网易云音乐".to_string());

        let description =
            first_non_empty(album, &["/description", "/company", "/type", "/subType"])
                .unwrap_or_else(|| self.lang_text("暂无简介", "No description").to_string());

        let cover_url = first_non_empty(album, &["/picUrl", "/blurPicUrl"]);

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

        self.playlist.id = Some(album_id.to_string());
        self.playlist.title = title;
        self.playlist.artist = artist;
        self.playlist.description = description;
        self.playlist.set_tracks(tracks);
        cover_url.map(|x| self.playlist.cover.load(self.api.clone(), x));
        Ok(())
    }

    async fn load_author_detail(&mut self, artist_id: &str) -> Result<()> {
        let detail = self.api.artist_detail(artist_id).await.ok();
        let desc = self.api.artist_desc(artist_id).await.ok();
        let top_song = self.api.artist_top_song(artist_id).await.ok();
        let album = self.api.artist_album(artist_id, 60, 0).await.ok();

        if detail.is_none() && desc.is_none() && top_song.is_none() && album.is_none() {
            return Err(anyhow!("作者数据获取失败"));
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
            title = self.lang_text("未知作者", "Unknown Author").to_string();
        }

        if description.trim().is_empty() {
            description = self
                .lang_text("暂无作者简介", "No author description yet")
                .to_string();
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
                            duration: format!("{} {}", size, self.lang_text("首", "tracks")),
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

        let mut tiles = vec![
            AuthorTile::from_album(
                &self.api,
                self.lang_text("热门歌曲", "Hot Songs").to_string(),
                format!("{} {}", hot_count, self.lang_text("首", "tracks")),
                hot_songs
                    .first()
                    .and_then(|track| track.cover_url.clone())
                    .or_else(|| cover_url.clone()),
                AuthorTileKind::HotSong,
            ),
            AuthorTile::from_album(
                &self.api,
                self.lang_text("专辑", "Albums").to_string(),
                format!("{} {}", album_count, self.lang_text("张", "items")),
                albums
                    .first()
                    .and_then(|track| track.cover_url.clone())
                    .or_else(|| cover_url.clone()),
                AuthorTileKind::Album,
            ),
            AuthorTile::from_album(
                &self.api,
                "EP".to_string(),
                format!("{} {}", ep_count, self.lang_text("张", "items")),
                eps.first()
                    .and_then(|track| track.cover_url.clone())
                    .or_else(|| cover_url.clone()),
                AuthorTileKind::Ep,
            ),
            AuthorTile::from_album(
                &self.api,
                "Single".to_string(),
                format!("{} {}", single_count, self.lang_text("张", "items")),
                singles
                    .first()
                    .and_then(|track| track.cover_url.clone())
                    .or_else(|| cover_url.clone()),
                AuthorTileKind::Single,
            ),
        ];

        if tiles.is_empty() {
            tiles.push(AuthorTile::placeholder());
        }

        self.author.id = Some(artist_id.to_string());
        self.author.title = title;
        self.author.artist = match self.config.language {
            Language::Zh => format!(
                "热门 {} · 专辑 {} · EP {} · Single {}",
                hot_count, album_count, ep_count, single_count
            ),
            Language::En => format!(
                "Hot {} · Albums {} · EP {} · Singles {}",
                hot_count, album_count, ep_count, single_count
            ),
        };
        self.author.description = description;
        cover_url.map(|x| self.author.cover.load(self.api.clone(), x));
        self.author.set_tiles(tiles);
        self.author.hot_songs = hot_songs;
        self.author.albums = albums;
        self.author.eps = eps;
        self.author.singles = singles;
        self.author.focused_idx = 0;

        Ok(())
    }

    async fn execute_search(&mut self) -> Result<()> {
        let (keywords, filter) = parse_search_input(&self.search.query);
        let followed_author_query = is_followed_author_query(&keywords, filter);
        if keywords.is_empty() && !followed_author_query {
            self.search.status_line = "请输入搜索关键词".to_string();
            self.search.set_results(Vec::new());
            return Ok(());
        }

        self.search.filter = filter;
        self.search.next_offset = 0;
        self.search.has_more = true;

        let response = if followed_author_query {
            self.api.artist_sublist(SEARCH_RESULT_PAGE_SIZE, 0).await?
        } else {
            self.api
                .search(&keywords, filter.search_type(), SEARCH_RESULT_PAGE_SIZE, 0)
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

        let items = if followed_author_query {
            let page = parse_followed_author_page(&response);
            let count = page.items.len();
            let next_offset = page.fetched_count;
            let has_more = followed_author_has_more(&page, next_offset);
            self.search.set_results(page.items);
            self.search.next_offset = next_offset;
            self.search.has_more = has_more;
            self.search.status_line =
                format!("{} 搜索完成，共 {} 条", filter.display_name(), count);
            return Ok(());
        } else {
            parse_search_items(&response, filter)
        };
        let count = items.len();
        self.search.set_results(items);
        self.search.status_line = format!("{} 搜索完成，共 {} 条", filter.display_name(), count);
        Ok(())
    }

    async fn load_more_search_results(&mut self) -> Result<usize> {
        if !self.search.has_more {
            return Ok(0);
        }

        let (keywords, filter) = parse_search_input(&self.search.query);
        let followed_author_query = is_followed_author_query(&keywords, filter);
        if keywords.is_empty() && !followed_author_query {
            return Ok(0);
        }

        let response = if followed_author_query {
            self.api
                .artist_sublist(SEARCH_RESULT_PAGE_SIZE, self.search.next_offset)
                .await?
        } else {
            self.api
                .search(
                    &keywords,
                    filter.search_type(),
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
            let fetched_count = page.fetched_count;
            let added = page.items.len();
            self.search.results.append(&mut page.items);
            self.search.next_offset = self.search.next_offset.saturating_add(fetched_count);
            self.search.has_more = followed_author_has_more(&page, self.search.next_offset);

            if added == 0 {
                if self.search.has_more {
                    self.search.status_line = format!(
                        "{} 已加载 {} 条",
                        filter.display_name(),
                        self.search.results.len()
                    );
                } else {
                    self.search.status_line = format!(
                        "{} 搜索结果已全部加载，共 {} 条",
                        filter.display_name(),
                        self.search.results.len()
                    );
                }
                return Ok(0);
            }

            self.search.status_line = format!(
                "{} 已加载 {} 条",
                filter.display_name(),
                self.search.results.len()
            );
            return Ok(added);
        }

        let items = parse_search_items(&response, filter);
        let added = self.search.append_results(items);

        if added == 0 {
            self.search.has_more = false;
            self.search.status_line = format!(
                "{} 搜索结果已全部加载，共 {} 条",
                filter.display_name(),
                self.search.results.len()
            );
            return Ok(0);
        }

        self.search.status_line = format!(
            "{} 已加载 {} 条",
            filter.display_name(),
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
        self.begin_startup_loading();
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

fn startup_loading_progress_at(
    elapsed: f32,
    complete_elapsed: Option<f32>,
    complete_requested: bool,
) -> f32 {
    if elapsed <= 0.0 {
        return 0.0;
    }

    // Monotonic non-linear loading using a cubic-bezier-like y curve.
    let t = (elapsed / STARTUP_LOADING_FILL_SECS).clamp(0.0, 1.0);
    let eased = cubic_bezier_y(t, 0.08, 0.98);
    let base = eased.min(0.96);

    if !complete_requested {
        return base;
    }

    let complete_t =
        (complete_elapsed.unwrap_or(0.0) / STARTUP_LOADING_COMPLETE_RAMP_SECS).clamp(0.0, 1.0);
    let complete_eased = cubic_bezier_y(complete_t, 0.25, 1.0);
    (base + (1.0 - base) * complete_eased).clamp(0.0, 1.0)
}

fn cubic_bezier_y(t: f32, p1y: f32, p2y: f32) -> f32 {
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

    if compact.contains("欧美私人雷达") {
        return Some("欧美私人雷达");
    }
    if compact.contains("私人雷达") {
        return Some("私人雷达");
    }
    if compact.contains("每日推荐") {
        return Some("每日推荐");
    }

    None
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
            pinned.push(HomeTile::from_recommendation(
                api,
                None,
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

fn parse_search_input(raw: &str) -> (String, SearchFilter) {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return (String::new(), SearchFilter::Single);
    }

    let lower = trimmed.to_ascii_lowercase();

    for (suffix, filter) in [
        ("@single", SearchFilter::Single),
        ("@album", SearchFilter::Album),
        ("@author", SearchFilter::Author),
        ("@artist", SearchFilter::Author),
        ("@list", SearchFilter::Playlist),
    ] {
        if lower.ends_with(suffix) {
            let cut = trimmed.len().saturating_sub(suffix.len());
            let stripped = &trimmed[..cut];
            return (stripped.trim().to_string(), filter);
        }
    }

    (trimmed.to_string(), SearchFilter::Single)
}

fn is_followed_author_query(keywords: &str, filter: SearchFilter) -> bool {
    filter == SearchFilter::Author && keywords.trim().is_empty()
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

fn parse_search_items(response: &ApiResponse, filter: SearchFilter) -> Vec<SearchItem> {
    let Some(result) = response.body.get("result") else {
        return Vec::new();
    };

    match filter {
        SearchFilter::Single => result
            .get("songs")
            .and_then(|value| value.as_array())
            .map(|items| parse_song_items(items))
            .unwrap_or_default(),
        SearchFilter::Album => result
            .get("albums")
            .and_then(|value| value.as_array())
            .map(|items| parse_album_items(items))
            .unwrap_or_default(),
        SearchFilter::Author => result
            .get("artists")
            .and_then(|value| value.as_array())
            .map(|items| parse_author_items(items))
            .unwrap_or_default(),
        SearchFilter::Playlist => result
            .get("playlists")
            .and_then(|value| value.as_array())
            .map(|items| parse_playlist_items(items))
            .unwrap_or_default(),
    }
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
            left_label: format!("{} - {}", name, artist),
            right_label: format_duration(duration),
            type_tag: None,
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
            left_label: format!("{} - {}", name, artist),
            right_label: format!("{} 首", size),
            type_tag: Some("@album".to_string()),
            song_id: None,
            album_id: parse_value_as_string(item.get("id")),
            playlist_id: None,
            artist_id: None,
            title: Some(name.to_string()),
            artist: Some(artist.to_string()),
            album: Some(name.to_string()),
            cover_url: first_non_empty(item, &["/picUrl", "/blurPicUrl"]),
            duration_ms: None,
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
            left_label: name.to_string(),
            right_label: format!("{} 张专辑", album_size),
            type_tag: Some("@author".to_string()),
            song_id: None,
            album_id: None,
            playlist_id: None,
            artist_id: parse_value_as_string(item.get("id")),
            title: None,
            artist: Some(name.to_string()),
            album: None,
            cover_url: first_non_empty(item, &["/picUrl", "/img1v1Url", "/avatarUrl"]),
            duration_ms: None,
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
            left_label: format!("{} - {}", name, creator),
            right_label: format!("{} 首", count),
            type_tag: Some("@list".to_string()),
            song_id: None,
            album_id: None,
            playlist_id: parse_value_as_string(item.get("id")),
            artist_id: None,
            title: None,
            artist: None,
            album: None,
            cover_url: first_non_empty(item, &["/coverImgUrl", "/picUrl"]),
            duration_ms: None,
        });
    }

    out
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
        Some(names.join(" / "))
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
