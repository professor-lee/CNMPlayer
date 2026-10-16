use super::App;
use super::Overlay;
use super::Page;
use super::keybinds::KeybindAction;
use super::keybinds::keybind_matches;
use super::presentation::SmallWindowMode;
use super::search_controller::SEARCH_BOX_ANIM_DURATION;
use super::sidebar::HomeSidebarHit;
use super::sidebar::HomeSidebarSection;
use crate::render::motion::Curve;
use crate::render::motion::Toggle;
use crate::ui::page_lyrics;
use crossterm::event::KeyCode;
use crossterm::event::KeyEvent;
use crossterm::event::KeyEventKind;
use crossterm::event::KeyModifiers;
use crossterm::event::MouseButton;
use crossterm::event::MouseEvent;
use crossterm::event::MouseEventKind;
use ratatui::layout::Rect;
use std::time::Duration;
use std::time::Instant;

/// 全屏页普通布局的最小宽度；主程序小于该宽度时不响应打开全屏。
const FULLSCREEN_MIN_WIDTH: u16 = 50;

pub(super) const CONTENT_DOUBLE_CLICK_MS: u64 = 400;

const GLOBAL_HOTKEY_COOLDOWN_MS: u64 = 120;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct HitRect {
    pub(crate) x: u16,
    pub(crate) y: u16,
    pub(crate) width: u16,
    pub(crate) height: u16,
}

impl HitRect {
    pub(crate) fn contains(self, col: u16, row: u16) -> bool {
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
pub(super) struct PageLyricsLayout {
    pub(super) content: HitRect,
    pub(super) panel: HitRect,
}

impl PageLyricsLayout {
    fn content_rect(self) -> Rect {
        Rect {
            x: self.content.x,
            y: self.content.y,
            width: self.content.width,
            height: self.content.height,
        }
    }

    fn panel_rect(self) -> Rect {
        Rect {
            x: self.panel.x,
            y: self.panel.y,
            width: self.panel.width,
            height: self.panel.height,
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct PlayerBarHitTargets {
    pub(crate) prev: Option<HitRect>,
    pub(crate) play_pause: Option<HitRect>,
    pub(crate) next: Option<HitRect>,
    pub(crate) progress: Option<HitRect>,
    /// 收藏爱心（左列右端）。
    pub(crate) like: Option<HitRect>,
    /// 下载按钮（爱心左侧隔一格；下载不可用时不登记）。
    pub(crate) download: Option<HitRect>,
    /// 播放模式符号（控制串最后一个字符）。
    pub(crate) mode: Option<HitRect>,
}

impl App {
    pub(crate) async fn handle_key(&mut self, key: KeyEvent) {
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
        {
            if self.overlay == Some(Overlay::SettingsKeybinds) {
                self.close_overlay();
            } else {
                self.open_keybind_settings();
            }
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
            | KeybindAction::ToggleLikeCollapsed
                if self.can_execute_global_hotkey() =>
            {
                self.trigger_keybind_action(action).await;
            }
            _ => {}
        }
    }

    pub(crate) async fn handle_mouse(&mut self, mouse: MouseEvent) {
        // 松开左键总是收尾拖拽：即使中途切到小窗口/弹窗也不会卡住拖动状态
        // （歌词拖拽卡住会让 should_continuous_redraw 一直按高帧率重绘）。
        if matches!(mouse.kind, MouseEventKind::Up(MouseButton::Left)) {
            self.input.player_bar_progress_drag = None;
            self.page_lyrics_release();
            return;
        }

        self.cancel_invalid_player_bar_progress_drag();
        if matches!(mouse.kind, MouseEventKind::Down(_)) {
            self.input.player_bar_progress_drag = None;
        }

        if matches!(mouse.kind, MouseEventKind::Drag(MouseButton::Left))
            && let Some(rect) = self.input.player_bar_progress_drag
        {
            // 抓取后只看列：离开原行仍拖动，超出左右端点则钳制。
            self.seek_to_ratio(player_bar_progress_ratio(rect, mouse.column));
            return;
        }

        if self.page == Page::Login || self.page == Page::Loading {
            return;
        }

        if self.is_small_window_context() {
            if self.overlay.is_none() && matches!(mouse.kind, MouseEventKind::Down(_)) {
                self.dispatch_player_bar_click(
                    mouse.column,
                    mouse.row,
                    matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left)),
                )
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

                self.dispatch_player_bar_click(col, row, true).await;
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
    async fn dispatch_player_bar_click(&mut self, col: u16, row: u16, start_drag: bool) {
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
        if let Some(rect) = hits.progress
            && rect.contains(col, row)
        {
            if start_drag {
                self.input.player_bar_progress_drag = Some(rect);
            }
            self.seek_to_ratio(player_bar_progress_ratio(rect, col));
        }
    }

    /// 在下一帧清命中区前以及每次鼠标事件前验证抓取，隐藏/遮挡后不得恢复旧拖拽。
    pub(super) fn cancel_invalid_player_bar_progress_drag(&mut self) {
        if self.overlay.is_some()
            || matches!(self.page, Page::Login | Page::Loading)
            || self.input.player_bar_progress_drag != self.player_bar_hits.progress
            || (self.is_small_window_context()
                && (self.small_window_mode.is_none()
                    || (self.small_window_mode == Some(SmallWindowMode::Flat)
                        && self.flat_exact_height
                        && (self.flat_switch_animating()
                            || self.flat_panel != super::presentation::FlatPanel::Player))))
        {
            self.input.player_bar_progress_drag = None;
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
                    let _ = self.browse.playlist.focus_next();
                    self.maybe_fetch_playlist_page();
                } else {
                    let _ = self.browse.playlist.focus_prev();
                }
            }
            Page::Home => self.scroll_home_sidebar(col, row, forward),
            _ => {}
        };
    }

    pub(crate) fn clear_player_bar_hits(&mut self) {
        self.player_bar_hits = PlayerBarHitTargets::default();
    }

    pub(crate) fn set_player_bar_hits(&mut self, hits: PlayerBarHitTargets) {
        // clear_player_bar_hits 是每帧临时清表；只有重新登记后的几何变化才取消抓取。
        if self.input.player_bar_progress_drag != hits.progress {
            self.input.player_bar_progress_drag = None;
        }
        self.player_bar_hits = hits;
    }

    pub(crate) fn clear_content_hits(&mut self) {
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
    pub(crate) fn page_lyrics_pos(&self) -> (f32, f32) {
        (self.config.page_lyrics_pos_x, self.config.page_lyrics_pos_y)
    }

    /// 登记本帧歌词浮窗几何（由 `page_lyrics::draw_page_lyrics_overlay` 调用）。
    pub(crate) fn set_page_lyrics_layout(&mut self, content: Rect, panel: Rect) {
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
    pub(super) fn page_lyrics_drag(&mut self, col: u16, row: u16) {
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

        if self.config.page_lyrics_snap
            && let Some(layout) = self.page_lyrics_layout
        {
            let (pos_x, pos_y) = page_lyrics::snap_pos(
                layout.content_rect(),
                layout.panel_rect(),
                self.page_lyrics_pos(),
            );
            self.config.page_lyrics_pos_x = pos_x;
            self.config.page_lyrics_pos_y = pos_y;
        }

        self.persist_config();
    }

    pub(crate) fn clear_settings_item_hits(&mut self) {
        self.settings.item_hits.clear();
    }

    pub(crate) fn push_settings_item_hit(&mut self, rect: HitRect, index: usize) {
        self.settings.item_hits.push((rect, index));
    }

    pub(crate) fn set_home_sidebar_panel_hit(&mut self, rect: Option<HitRect>) {
        self.home_sidebar_panel_hit = rect;
    }

    pub(crate) fn set_home_sidebar_anim_span_cells(&mut self, span_cells: u16) {
        self.home_sidebar_anim_span_cells = span_cells.max(1);
    }

    pub(crate) fn push_home_sidebar_playlist_hit(&mut self, rect: HitRect, hit: HomeSidebarHit) {
        self.home_sidebar_playlist_hits.push((rect, hit));
    }

    pub(crate) fn push_home_sidebar_section_hit(
        &mut self,
        rect: HitRect,
        section: HomeSidebarSection,
    ) {
        self.home_sidebar_section_hits.push((rect, section));
    }

    pub(crate) fn push_home_tile_hit(&mut self, rect: HitRect, index: usize) {
        self.home_tile_hits.push((rect, index));
    }

    pub(crate) fn push_playlist_track_hit(&mut self, rect: HitRect, index: usize) {
        self.playlist_track_hits.push((rect, index));
    }

    pub(crate) fn push_playlist_track_download_hit(&mut self, rect: HitRect, index: usize) {
        self.playlist_track_download_hits.push((rect, index));
    }

    pub(crate) fn push_search_item_download_hit(&mut self, rect: HitRect, index: usize) {
        self.search_item_download_hits.push((rect, index));
    }

    pub(crate) fn push_author_tile_hit(&mut self, rect: HitRect, index: usize) {
        self.author_tile_hits.push((rect, index));
    }

    pub(crate) fn push_search_item_hit(&mut self, rect: HitRect, index: usize) {
        self.search_item_hits.push((rect, index));
    }

    async fn handle_overlay_key(&mut self, overlay: Overlay, key: KeyEvent) {
        match overlay {
            Overlay::Settings => self.handle_settings_root_key(key).await,
            Overlay::SettingsPlayback => self.handle_settings_playback_key(key),
            Overlay::SettingsKeybinds => self.handle_settings_keybinds_key(key),
            Overlay::SettingsLyrics => self.handle_settings_lyrics_key(key),
            Overlay::SettingsDownload => self.handle_settings_download_key(key).await,
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
        if let Some(last_at) = self.last_global_hotkey_at
            && now.duration_since(last_at) < Duration::from_millis(GLOBAL_HOTKEY_COOLDOWN_MS)
        {
            return false;
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
                let step = self.browse.playlist.visible_rows.max(1);
                for _ in 0..step {
                    if !self.browse.playlist.focus_prev() {
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
                    let before_len = self.search.results().len();
                    self.advance_search_focus().await;
                    if self.search.focused_idx == before_idx
                        && self.search.results().len() == before_len
                    {
                        break;
                    }
                }
            }
            Page::Playlist => {
                let step = self.browse.playlist.visible_rows.max(1);
                for _ in 0..step {
                    if !self.browse.playlist.focus_next() {
                        break;
                    }
                    self.maybe_fetch_playlist_page();
                }
            }
            _ => {}
        }
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
                if self.browse.home_sidebar.is_visible() {
                    if let Some(panel) = self.home_sidebar_panel_hit
                        && panel.contains(col, row)
                    {
                        let sidebar_hit = self
                            .home_sidebar_playlist_hits
                            .iter()
                            .find(|(rect, _)| rect.contains(col, row))
                            .map(|(_, hit)| *hit);
                        if let Some(hit) = sidebar_hit {
                            if self.browse.home_sidebar.expanded {
                                self.browse.home_sidebar.set_focus(hit.section, hit.index);
                                self.maybe_fetch_home_sidebar_page();
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

                    if self.browse.home_sidebar.expanded {
                        self.last_content_click = None;
                        return true;
                    }
                }

                let hit = self
                    .home_tile_hits
                    .iter()
                    .find(|(rect, _)| rect.contains(col, row))
                    .map(|(_, idx)| *idx);
                if let Some(idx) = hit
                    && idx < self.browse.home.tiles.len()
                {
                    self.browse.home.focused_idx = idx;
                    if self.is_double_content_click(Page::Home, idx) {
                        self.enter_home_tile().await;
                    }
                    return true;
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
                if let Some(idx) = hit
                    && idx < self.browse.playlist.tracks.len()
                {
                    self.browse.playlist.set_focus(idx);
                    self.maybe_fetch_playlist_page();
                    if self.is_double_content_click(Page::Playlist, idx) {
                        self.play_focused_playlist_track().await;
                    }
                    return true;
                }
            }
            Page::Author => {
                let hit = self
                    .author_tile_hits
                    .iter()
                    .find(|(rect, _)| rect.contains(col, row))
                    .map(|(_, idx)| *idx);
                if let Some(idx) = hit
                    && idx < self.browse.author.tiles.len()
                {
                    self.browse.author.set_focus(idx);
                    if self.is_double_content_click(Page::Author, idx) {
                        self.play_focused_author_tile().await;
                    }
                    return true;
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
                if let Some(idx) = hit
                    && idx < self.search.results().len()
                {
                    self.search.set_focus(idx);
                    if self.is_double_content_click(Page::Search, idx) {
                        self.activate_focused_search_result().await;
                    }
                    return true;
                }
            }
            _ => {}
        }
        false
    }

    pub(crate) fn close_overlay(&mut self) {
        self.overlay = None;
        self.settings.keybind_rebinding = None;
        self.input.search_motion.set(
            false,
            Instant::now(),
            SEARCH_BOX_ANIM_DURATION,
            Curve::EaseOut,
        );
        self.settings.last_click = None;
        self.settings.cancel_download_path_edit();
        self.clear_settings_item_hits();
    }
}

/// 与点击映射共用端点；宽度一格时保持原来的零进度语义。
fn player_bar_progress_ratio(rect: HitRect, col: u16) -> f32 {
    if rect.width <= 1 {
        0.0
    } else {
        (col.saturating_sub(rect.x) as f32 / (rect.width - 1) as f32).clamp(0.0, 1.0)
    }
}

/// Search-box input/animation and host mouse capture state.
pub(crate) struct InputController {
    pub(crate) search_box_input: String,
    pub(crate) search_box_cursor: usize,
    pub(crate) search_motion: Toggle,
    pub(super) player_bar_progress_drag: Option<HitRect>,
}

impl Default for InputController {
    fn default() -> Self {
        Self {
            search_box_input: String::new(),
            search_box_cursor: 0,
            search_motion: Toggle::new(false),
            player_bar_progress_drag: None,
        }
    }
}

impl InputController {
    pub(super) fn set_text(&mut self, text: String) {
        self.search_box_input = text;
        self.search_box_cursor = self.search_box_input.chars().count();
    }

    pub(crate) fn insert(&mut self, ch: char) -> bool {
        if self.search_box_input.chars().count() >= super::text::MAX_INPUT_LEN {
            return false;
        }
        super::text::insert_char_at(&mut self.search_box_input, self.search_box_cursor, ch);
        self.search_box_cursor += 1;
        true
    }

    pub(super) fn backspace(&mut self) {
        if self.search_box_cursor > 0 {
            self.search_box_cursor =
                super::text::remove_char_before(&mut self.search_box_input, self.search_box_cursor);
        }
    }

    pub(super) fn delete(&mut self) {
        super::text::remove_char_at(&mut self.search_box_input, self.search_box_cursor);
    }

    pub(super) fn move_cursor(&mut self, delta: isize) {
        let len = self.search_box_input.chars().count() as isize;
        self.search_box_cursor = (self.search_box_cursor as isize + delta).clamp(0, len) as usize;
    }
}

#[cfg(test)]
mod tests {
    use super::HitRect;
    use super::player_bar_progress_ratio;

    #[test]
    fn progress_drag_clamps_both_endpoints() {
        let rect = HitRect {
            x: 10,
            y: 4,
            width: 9,
            height: 1,
        };
        assert_eq!(player_bar_progress_ratio(rect, 0), 0.0);
        assert_eq!(player_bar_progress_ratio(rect, 10), 0.0);
        assert_eq!(player_bar_progress_ratio(rect, 14), 0.5);
        assert_eq!(player_bar_progress_ratio(rect, 18), 1.0);
        assert_eq!(player_bar_progress_ratio(rect, u16::MAX), 1.0);
    }

    #[test]
    fn single_cell_progress_keeps_click_mapping_during_drag() {
        let rect = HitRect {
            x: 10,
            y: 4,
            width: 1,
            height: 1,
        };
        assert_eq!(player_bar_progress_ratio(rect, 0), 0.0);
        assert_eq!(player_bar_progress_ratio(rect, 10), 0.0);
        assert_eq!(player_bar_progress_ratio(rect, u16::MAX), 0.0);
    }
}
