use super::App;
use super::Overlay;
use super::Page;
use super::input_controller::CONTENT_DOUBLE_CLICK_MS;
use super::input_controller::HitRect;
use super::keybinds::RESERVED_RESET_KEYBIND;
use super::keybinds::is_reserved_reset_combo;
use super::keybinds::key_event_to_keybind_text;
use super::text::byte_index_for_char;
use super::text::char_count;
use crate::data::config::BarChannels;
use crate::data::config::BarNumber;
use crate::data::config::Config;
use crate::data::config::Language;
use crate::data::theme_loader::ThemeLoader;
use crossterm::event::KeyCode;
use crossterm::event::KeyEvent;
use crossterm::event::KeyModifiers;
use std::time::Duration;
use std::time::Instant;

/// 下载路径输入框的长度上限（字符）。
const DOWNLOAD_PATH_MAX_CHARS: usize = 4096;

const SETTINGS_ROOT_ITEMS: usize = 9;
pub(crate) const SETTINGS_GENERAL_ITEMS: usize = 6;
const MPRIS_NAME_MAX_CHARS: usize = 256;

const SETTINGS_PLAYBACK_ITEMS: usize = 7;

const SETTINGS_LYRICS_ITEMS: usize = 5;

const LYRICS_SETTINGS_ROWS: [usize; SETTINGS_LYRICS_ITEMS] = [0, 1, 2, 3, 4];
const LYRICS_SETTINGS_ROWS_NO_DRAG: [usize; SETTINGS_LYRICS_ITEMS - 1] = [0, 1, 3, 4];

/// 歌词子页的可选中行。拖动关闭时「边缘吸附」灰置，导航直接跳过。
fn lyrics_settings_rows(drag_enabled: bool) -> &'static [usize] {
    if drag_enabled {
        &LYRICS_SETTINGS_ROWS
    } else {
        &LYRICS_SETTINGS_ROWS_NO_DRAG
    }
}

pub(crate) fn lyrics_settings_row_enabled(row: usize, drag_enabled: bool) -> bool {
    row < SETTINGS_LYRICS_ITEMS && (row != 2 || drag_enabled)
}

pub(crate) fn step_lyrics_settings(current: usize, forward: bool, drag_enabled: bool) -> usize {
    let rows = lyrics_settings_rows(drag_enabled);
    let position = rows.iter().position(|row| *row == current).unwrap_or(0);
    let next = if forward {
        (position + 1) % rows.len()
    } else if position == 0 {
        rows.len() - 1
    } else {
        position - 1
    };
    rows[next]
}

pub(crate) const SETTINGS_DOWNLOAD_ITEMS: usize = 3;

pub(crate) const SETTINGS_KEYBIND_ITEMS: usize = 22;

/// 设置行的文本编辑状态（光标按字符计数，横向窗口按显示列算）。
#[derive(Debug, Clone, Default)]
pub(crate) struct SettingsTextEdit {
    pub(crate) buffer: String,
    pub(crate) cursor: usize,
    /// 可见窗口左边界所在的显示列：只在光标撞到窗口边界时才挪动。
    pub(crate) window_col: usize,
}

impl SettingsTextEdit {
    pub(crate) fn new(buffer: String) -> Self {
        let cursor = char_count(&buffer);
        Self {
            buffer,
            cursor,
            window_col: 0,
        }
    }

    pub(crate) fn insert(&mut self, ch: char, max_chars: usize) {
        if ch.is_control() || char_count(&self.buffer) >= max_chars {
            return;
        }
        let index = byte_index_for_char(&self.buffer, self.cursor);
        self.buffer.insert(index, ch);
        self.cursor += 1;
    }

    pub(crate) fn backspace(&mut self) {
        if self.cursor > 0 {
            let index = byte_index_for_char(&self.buffer, self.cursor - 1);
            self.buffer.remove(index);
            self.cursor -= 1;
        }
    }

    pub(crate) fn delete(&mut self) {
        if self.cursor < char_count(&self.buffer) {
            let index = byte_index_for_char(&self.buffer, self.cursor);
            self.buffer.remove(index);
        }
    }

    pub(crate) fn move_cursor(&mut self, delta: i32) {
        self.cursor = (self.cursor as i64 + i64::from(delta))
            .clamp(0, char_count(&self.buffer) as i64) as usize;
    }

    pub(crate) fn home(&mut self) {
        self.cursor = 0;
    }

    pub(crate) fn end(&mut self) {
        self.cursor = char_count(&self.buffer);
    }
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

impl App {
    pub(super) fn open_settings(&mut self) {
        self.settings.selected = 0;
        self.settings.keybind_rebinding = None;
        self.overlay = Some(Overlay::Settings);
    }

    pub(super) fn open_keybind_settings(&mut self) {
        self.settings.keybind_selected = 0;
        self.settings.keybind_rebinding = None;
        self.settings.keybind_scroll = 0;
        self.overlay = Some(Overlay::SettingsKeybinds);
    }

    /// 设置弹窗内的鼠标点击：单击聚焦该行，400ms 内再点同一行等同 Enter。
    ///
    /// 弹窗盖住整页，未命中行的点击由调用方直接丢弃（不穿透到底层页面）。
    pub(super) async fn handle_settings_modal_click(
        &mut self,
        overlay: Overlay,
        col: u16,
        row: u16,
    ) {
        if self.settings.text_editing() {
            return;
        }
        let Some((_, index)) = self
            .settings
            .item_hits
            .iter()
            .find(|(rect, _)| rect.contains(col, row))
            .copied()
        else {
            self.settings.last_click = None;
            return;
        };

        match overlay {
            Overlay::Settings => {
                self.settings.select(index, SETTINGS_ROOT_ITEMS);
                if self.is_double_settings_click(overlay, index) {
                    self.activate_settings_root_item().await;
                }
            }
            Overlay::SettingsGeneral => {
                self.settings.general_selected = index;
                self.activate_settings_general_item(1).await;
            }
            Overlay::SettingsPlayback => {
                self.settings.playback_selected = index;
                if self.is_double_settings_click(overlay, index) {
                    self.apply_settings_playback_delta(1);
                }
            }
            Overlay::SettingsLyrics => {
                // 开关行左键直接改值；灰置的吸附行不可选中。
                if !lyrics_settings_row_enabled(index, self.config.page_lyrics_drag) {
                    return;
                }
                self.settings.lyrics_selected = index;
                self.apply_settings_lyrics_delta(1);
            }
            Overlay::SettingsDownload => {
                // 与歌词浮窗同构：单击即执行（音质改值 / 路径进编辑 / 恢复默认两段式）。
                self.settings.download_selected = index;
                self.activate_settings_download_item();
            }
            Overlay::SettingsKeybinds => {
                self.settings.keybind_selected = index;
                if self.is_double_settings_click(overlay, index) {
                    self.begin_keybind_rebind(index);
                }
            }
            _ => {}
        }
    }

    /// 设置弹窗滚轮：上下移动选中行（与 Up/Down 同效）。
    pub(super) fn scroll_settings_modal(&mut self, forward: bool) {
        if self.settings.text_editing() {
            return;
        }
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
            Some(Overlay::Settings) => step(&mut self.settings.selected, SETTINGS_ROOT_ITEMS),
            Some(Overlay::SettingsGeneral) => {
                step(&mut self.settings.general_selected, SETTINGS_GENERAL_ITEMS)
            }
            Some(Overlay::SettingsPlayback) => step(
                &mut self.settings.playback_selected,
                SETTINGS_PLAYBACK_ITEMS,
            ),
            Some(Overlay::SettingsLyrics) => {
                self.settings.lyrics_selected = step_lyrics_settings(
                    self.settings.lyrics_selected,
                    forward,
                    self.config.page_lyrics_drag,
                );
            }
            Some(Overlay::SettingsKeybinds) => {
                step(&mut self.settings.keybind_selected, SETTINGS_KEYBIND_ITEMS)
            }
            _ => {}
        }
    }

    fn is_double_settings_click(&mut self, overlay: Overlay, index: usize) -> bool {
        let now = Instant::now();
        let is_double = self
            .settings
            .last_click
            .map(|(at, o, i)| {
                o == overlay
                    && i == index
                    && now.duration_since(at) <= Duration::from_millis(CONTENT_DOUBLE_CLICK_MS)
            })
            .unwrap_or(false);
        self.settings.last_click = Some((now, overlay, index));
        is_double
    }

    /// 设置根页选中项的执行（键盘 Enter 与双击共用）。
    async fn activate_settings_root_item(&mut self) {
        match self.settings.selected {
            0..=1 => self.apply_settings_root_delta(1).await,
            2 => {
                self.settings.general_selected = 0;
                self.settings.general_scroll = 0;
                self.settings.mpris_name_edit = None;
                self.overlay = Some(Overlay::SettingsGeneral);
            }
            3 => {
                self.settings.playback_selected = 0;
                self.overlay = Some(Overlay::SettingsPlayback);
            }
            4 => self.open_keybind_settings(),
            5 => {
                self.settings.lyrics_selected = 0;
                self.overlay = Some(Overlay::SettingsLyrics);
            }
            6 => self.open_download_settings(),
            7 => self.logout_to_login().await,
            8 => self.overlay = Some(Overlay::SettingsAbout),
            _ => {}
        }
    }

    async fn activate_settings_general_item(&mut self, delta: i32) {
        if self.settings.general_selected == 5 {
            self.settings.mpris_name_edit =
                Some(SettingsTextEdit::new(self.config.mpris_name.clone()));
        } else {
            self.apply_settings_general_delta(delta).await;
        }
    }

    pub(super) async fn handle_settings_general_key(&mut self, key: KeyEvent) {
        if let Some(edit) = self.settings.mpris_name_edit.as_mut() {
            match key.code {
                KeyCode::Esc => self.settings.mpris_name_edit = None,
                KeyCode::Enter => {
                    let edit = self.settings.mpris_name_edit.take().unwrap();
                    let value = if edit.buffer.trim().is_empty() {
                        String::new()
                    } else {
                        edit.buffer
                    };
                    if value != self.config.mpris_name {
                        self.config.mpris_name = value;
                        self.persist_config();
                        self.sync_mpris_exposure();
                    }
                }
                KeyCode::Backspace => edit.backspace(),
                KeyCode::Delete => edit.delete(),
                KeyCode::Left => edit.move_cursor(-1),
                KeyCode::Right => edit.move_cursor(1),
                KeyCode::Home => edit.home(),
                KeyCode::End => edit.end(),
                KeyCode::Char(ch)
                    if !key
                        .modifiers
                        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
                {
                    edit.insert(ch, MPRIS_NAME_MAX_CHARS);
                }
                _ => {}
            }
            return;
        }

        match key.code {
            KeyCode::Esc => self.overlay = Some(Overlay::Settings),
            KeyCode::Char('t') | KeyCode::Char('T')
                if key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT =>
            {
                self.close_overlay();
            }
            KeyCode::Up | KeyCode::BackTab => {
                self.settings.general_selected =
                    (self.settings.general_selected + SETTINGS_GENERAL_ITEMS - 1)
                        % SETTINGS_GENERAL_ITEMS;
            }
            KeyCode::Down | KeyCode::Tab => {
                self.settings.general_selected =
                    (self.settings.general_selected + 1) % SETTINGS_GENERAL_ITEMS;
            }
            KeyCode::Left => self.apply_settings_general_delta(-1).await,
            KeyCode::Right | KeyCode::Enter => self.activate_settings_general_item(1).await,
            _ => {}
        }
    }

    /// 开始重绑某条快捷键（键盘 Enter 与双击共用）。
    fn begin_keybind_rebind(&mut self, index: usize) {
        self.settings.keybind_rebinding = Some(index);
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

    /// “歌词”子页：浮窗开关、拖动/吸附，以及逐字与翻译显示。
    fn apply_settings_lyrics_delta(&mut self, delta: i32) {
        if delta == 0 {
            return;
        }

        match self.settings.lyrics_selected {
            0 => {
                self.config.page_lyrics = !self.config.page_lyrics;
                self.persist_config();
            }
            1 => {
                self.config.page_lyrics_drag = !self.config.page_lyrics_drag;
                self.persist_config();
            }
            2 => {
                // 拖动关闭时吸附无意义：灰置且不可改。
                if !self.config.page_lyrics_drag {
                    return;
                }
                self.config.page_lyrics_snap = !self.config.page_lyrics_snap;
                self.persist_config();
            }
            3 => {
                self.config.word_lyrics = !self.config.word_lyrics;
                self.persist_config();
            }
            4 => {
                self.config.lyric_translation = !self.config.lyric_translation;
                self.persist_config();
            }
            _ => {}
        }
    }

    pub(super) fn handle_settings_lyrics_key(&mut self, key: KeyEvent) {
        match key.code {
            // 五行都是开关（值行）：与播放设置页同构，Left/Right 都用来改值，
            // 返回上一级只走 Esc。
            KeyCode::Esc => self.overlay = Some(Overlay::Settings),
            KeyCode::Char('t') | KeyCode::Char('T')
                if (key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT) =>
            {
                self.close_overlay();
            }
            KeyCode::Left => self.apply_settings_lyrics_delta(-1),
            KeyCode::Right | KeyCode::Enter => self.apply_settings_lyrics_delta(1),
            KeyCode::Up | KeyCode::BackTab => {
                self.settings.lyrics_selected = step_lyrics_settings(
                    self.settings.lyrics_selected,
                    false,
                    self.config.page_lyrics_drag,
                );
            }
            KeyCode::Down | KeyCode::Tab => {
                self.settings.lyrics_selected = step_lyrics_settings(
                    self.settings.lyrics_selected,
                    true,
                    self.config.page_lyrics_drag,
                );
            }
            _ => {}
        }
    }

    /// 打开「下载设置」子页：光标落在第一个可选中行（禁用态下就是路径行）。
    fn open_download_settings(&mut self) {
        self.settings.download_path_edit = None;
        self.settings.download_reset_armed = false;
        self.settings.download_selected = self
            .download_selectable_rows()
            .first()
            .copied()
            .unwrap_or(1);
        self.overlay = Some(Overlay::SettingsDownload);
    }

    /// 下载目录可用（系统里能找到可写位置）。不可用时除路径行外全部灰置。
    pub(crate) fn download_settings_enabled(&self) -> bool {
        self.downloads.root.is_some()
    }

    /// 某一行是否可选中：下载不可用时只有「音质」灰置——路径行是自救入口，
    /// 「恢复默认」是把显式 `Null` / 无家目录状态拉回来的出口，两者都要能选。
    fn download_row_selectable(&self, row: usize) -> bool {
        self.download_settings_enabled() || row != 0
    }

    pub(super) fn download_selectable_rows(&self) -> Vec<usize> {
        (0..SETTINGS_DOWNLOAD_ITEMS)
            .filter(|row| self.download_row_selectable(*row))
            .collect()
    }

    pub(super) fn move_download_selection(&mut self, delta: i32) {
        let rows = self.download_selectable_rows();
        if rows.is_empty() || delta == 0 {
            return;
        }
        let current = rows
            .iter()
            .position(|row| *row == self.settings.download_selected)
            .unwrap_or(0) as i32;
        let next = (current + delta).rem_euclid(rows.len() as i32) as usize;
        self.settings.download_selected = rows[next];
        // 换行即撤下待确认态：恢复默认必须连着选两次同一个地方。
        self.settings.download_reset_armed = false;
    }

    /// 下载设置页的「执行」：Enter、双击与单击共用。
    fn activate_settings_download_item(&mut self) {
        match self.settings.download_selected {
            0 => self.apply_settings_download_delta(1),
            1 => self.begin_download_path_edit(),
            2 => self.activate_download_reset(),
            _ => {}
        }
    }

    /// 音质行：与播放设置同一套可选值（按会员放开）。
    fn apply_settings_download_delta(&mut self, delta: i32) {
        if delta == 0 || self.settings.download_selected != 0 || !self.download_settings_enabled() {
            return;
        }
        let next = self
            .config
            .download_audio_quality
            .cycle(delta, self.vip_audio_unlocked);
        if next != self.config.download_audio_quality {
            self.config.download_audio_quality = next;
            self.persist_config();
        }
    }

    /// 「恢复默认」两段式：首次进入待确认态（文字换成警戒色），再选一次才写回默认。
    ///
    /// 下载不可用（显式 `Null` / 系统没有可写位置）时也允许：它就是那个出口。
    pub(super) fn activate_download_reset(&mut self) {
        if !self.settings.download_reset_armed {
            self.settings.download_reset_armed = true;
            self.set_runtime_status(self.lang_text(
                "再按一次确认恢复下载设置",
                "Press again to restore download settings",
            ));
            return;
        }

        self.settings.download_reset_armed = false;
        let defaults = Config::default();
        self.config.download_audio_quality = defaults.download_audio_quality;
        self.config.download_path = defaults.download_path;
        self.persist_config();
        self.refresh_download_root();
        self.set_runtime_status(self.lang_text(
            "下载设置已恢复默认",
            "Download settings restored to defaults",
        ));
    }

    /// 进入路径行的行内编辑：以当前生效路径为初值，光标停在末尾。
    ///
    /// 填字面量 `Null` 回车 = 显式禁用下载；填绝对路径恢复。
    pub(super) fn begin_download_path_edit(&mut self) {
        let current = self
            .downloads
            .root
            .as_ref()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| self.download_display_path());
        self.settings.begin_download_path_edit(current);
    }

    pub(super) fn download_path_edit_insert(&mut self, ch: char) {
        if let Some(edit) = self.settings.download_path_edit.as_mut() {
            edit.insert(ch, DOWNLOAD_PATH_MAX_CHARS);
        }
    }

    pub(super) fn download_path_edit_backspace(&mut self) {
        if let Some(edit) = self.settings.download_path_edit.as_mut() {
            edit.backspace();
        }
    }

    fn download_path_edit_delete(&mut self) {
        if let Some(edit) = self.settings.download_path_edit.as_mut() {
            edit.delete();
        }
    }

    pub(super) fn download_path_edit_move(&mut self, delta: i32) {
        if let Some(edit) = self.settings.download_path_edit.as_mut() {
            edit.move_cursor(delta);
        }
    }

    fn download_path_edit_home(&mut self) {
        if let Some(edit) = self.settings.download_path_edit.as_mut() {
            edit.home();
        }
    }

    fn download_path_edit_end(&mut self) {
        if let Some(edit) = self.settings.download_path_edit.as_mut() {
            edit.end();
        }
    }

    /// 回车确认：`Null` = 显式禁用；非法（空 / 非绝对 / 不可写）保留修改前的值。
    pub(super) async fn commit_download_path_edit(&mut self) {
        let Some(edit) = self.settings.download_path_edit.take() else {
            return;
        };
        let raw = edit.buffer.trim().to_string();
        match crate::app::download::validate_download_path(&raw).await {
            Ok(crate::app::download::DownloadPathChoice::Disabled) => {
                self.config.download_path =
                    Some(crate::app::download::DOWNLOAD_PATH_NULL.to_string());
                self.persist_config();
                self.refresh_download_root();
                self.set_runtime_status(self.lang_text(
                    "已禁用下载（路径填 Null）",
                    "Downloads disabled (path is Null)",
                ));
            }
            Ok(crate::app::download::DownloadPathChoice::Dir(path)) => {
                self.config.download_path = Some(path.display().to_string());
                self.persist_config();
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
    pub(crate) fn download_display_path(&self) -> String {
        self.downloads
            .root
            .as_ref()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| crate::app::download::DOWNLOAD_PATH_NULL.to_string())
    }

    /// 「下载设置」页：音质 / 路径 / 恢复默认三行。
    ///
    /// 编辑态下所有按键都进输入框（含 `t`）；非编辑态沿用设置弹窗的习惯（Esc 返回、t 关闭）。
    pub(super) async fn handle_settings_download_key(&mut self, key: KeyEvent) {
        if self.settings.download_path_edit.is_some() {
            match key.code {
                KeyCode::Esc => {
                    self.settings.download_path_edit = None;
                    self.set_runtime_status(
                        self.lang_text("已取消修改下载路径", "Download path edit cancelled"),
                    );
                }
                KeyCode::Enter => self.commit_download_path_edit().await,
                KeyCode::Backspace => self.download_path_edit_backspace(),
                KeyCode::Delete => self.download_path_edit_delete(),
                KeyCode::Left => self.download_path_edit_move(-1),
                KeyCode::Right => self.download_path_edit_move(1),
                KeyCode::Home => self.download_path_edit_home(),
                KeyCode::End => self.download_path_edit_end(),
                KeyCode::Char(ch)
                    if !key
                        .modifiers
                        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
                        && !ch.is_control() =>
                {
                    self.download_path_edit_insert(ch);
                }
                _ => {}
            }
            return;
        }

        match key.code {
            KeyCode::Esc => {
                self.settings.download_reset_armed = false;
                self.overlay = Some(Overlay::Settings);
            }
            KeyCode::Char('t') | KeyCode::Char('T')
                if (key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT) =>
            {
                self.settings.download_reset_armed = false;
                self.close_overlay();
            }
            KeyCode::Up | KeyCode::BackTab => self.move_download_selection(-1),
            KeyCode::Down | KeyCode::Tab => self.move_download_selection(1),
            KeyCode::Left => self.apply_settings_download_delta(-1),
            KeyCode::Right | KeyCode::Enter => self.activate_settings_download_item(),
            _ => {}
        }
    }

    pub(super) async fn handle_settings_root_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => self.close_overlay(),
            KeyCode::Char('t') | KeyCode::Char('T')
                if (key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT) =>
            {
                self.close_overlay();
            }
            KeyCode::Up | KeyCode::BackTab => {
                if self.settings.selected == 0 {
                    self.settings.selected = SETTINGS_ROOT_ITEMS - 1;
                } else {
                    self.settings.selected -= 1;
                }
            }
            KeyCode::Down | KeyCode::Tab => {
                self.settings.selected = (self.settings.selected + 1) % SETTINGS_ROOT_ITEMS;
            }
            KeyCode::Left => self.apply_settings_root_delta(-1).await,
            KeyCode::Right => self.apply_settings_root_delta(1).await,
            KeyCode::Enter => self.activate_settings_root_item().await,
            _ => {}
        }
    }

    pub(super) fn handle_settings_playback_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => self.overlay = Some(Overlay::Settings),
            KeyCode::Char('t') | KeyCode::Char('T')
                if (key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT) =>
            {
                self.close_overlay();
            }
            KeyCode::Left => {
                self.apply_settings_playback_delta(-1);
            }
            KeyCode::Right | KeyCode::Enter => {
                self.apply_settings_playback_delta(1);
            }
            KeyCode::Up | KeyCode::BackTab => {
                if self.settings.playback_selected == 0 {
                    self.settings.playback_selected = SETTINGS_PLAYBACK_ITEMS - 1;
                } else {
                    self.settings.playback_selected -= 1;
                }
            }
            KeyCode::Down | KeyCode::Tab => {
                self.settings.playback_selected =
                    (self.settings.playback_selected + 1) % SETTINGS_PLAYBACK_ITEMS;
            }
            _ => {}
        }
    }

    pub(super) fn handle_settings_keybinds_key(&mut self, key: KeyEvent) {
        if let Some(index) = self.settings.keybind_rebinding {
            match key.code {
                KeyCode::Esc => {
                    self.settings.keybind_rebinding = None;
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
                        self.persist_config();
                        self.set_runtime_status(format!(
                            "{} [{}] {} {}",
                            self.lang_text("已将", "Bound"),
                            self.keybind_name_for_index(index),
                            self.lang_text("绑定为", "to"),
                            binding
                        ));
                    }
                    self.settings.keybind_rebinding = None;
                }
            }
            return;
        }

        if is_reserved_reset_combo(key) {
            self.reset_keybinds_to_default();
            self.persist_config();
            self.set_runtime_status(
                self.lang_text("已恢复默认快捷键", "Restored default keybinds"),
            );
            return;
        }

        match key.code {
            KeyCode::Esc => {
                self.settings.keybind_rebinding = None;
                self.overlay = Some(Overlay::Settings);
            }
            KeyCode::Char('t') | KeyCode::Char('T')
                if (key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT) =>
            {
                self.settings.keybind_rebinding = None;
                self.close_overlay();
            }
            KeyCode::Left => {
                self.settings.keybind_rebinding = None;
                self.overlay = Some(Overlay::Settings);
            }
            KeyCode::Up | KeyCode::BackTab => {
                if self.settings.keybind_selected == 0 {
                    self.settings.keybind_selected = SETTINGS_KEYBIND_ITEMS - 1;
                } else {
                    self.settings.keybind_selected -= 1;
                }
            }
            KeyCode::Down | KeyCode::Tab => {
                self.settings.keybind_selected =
                    (self.settings.keybind_selected + 1) % SETTINGS_KEYBIND_ITEMS;
            }
            KeyCode::Enter => {
                let idx = self.settings.keybind_selected;
                self.begin_keybind_rebind(idx);
            }
            _ => {}
        }
    }

    async fn apply_settings_root_delta(&mut self, delta: i32) {
        match self.settings.selected {
            0 => {
                let themes = ThemeLoader::list_themes_async().await;
                let current = themes
                    .iter()
                    .position(|name| name.eq_ignore_ascii_case(self.config.theme.as_str()))
                    .unwrap_or(0);
                let next = (current as i32 + delta).rem_euclid(themes.len() as i32) as usize;
                let next_name = &themes[next];
                if let Ok(theme) = ThemeLoader::load_async(next_name).await {
                    self.apply_theme(theme);
                    self.config.theme = next_name.clone();
                    self.persist_config();
                }
            }
            1 if delta != 0 => {
                self.config.transparent_background = !self.config.transparent_background;
                self.persist_config();
            }
            _ => {}
        }
    }

    async fn apply_settings_general_delta(&mut self, delta: i32) {
        if delta == 0 {
            return;
        }
        match self.settings.general_selected {
            0 => {
                self.config.language = match self.config.language {
                    Language::Zh => Language::En,
                    Language::En => Language::Zh,
                };
                self.persist_config();
            }
            1 => {
                let next_protocol = self.config.graphics_protocol.cycle(delta);
                if next_protocol != self.config.graphics_protocol {
                    self.config.graphics_protocol = next_protocol;
                    self.persist_config();
                }
            }
            2 => {
                self.config.show_hints = !self.config.show_hints;
                self.persist_config();
            }
            3 => {
                let was_small_context = self.is_small_window_context();
                self.config.small_window_display = !self.config.small_window_display;
                self.persist_config();
                if !was_small_context && self.is_small_window_context() {
                    self.close_panels_for_small_window();
                }
                self.sync_terminal_size();
            }
            4 => {
                self.config.home_more_recommend = !self.config.home_more_recommend;
                self.persist_config();
                if self.page == Page::Home
                    && let Err(err) = self.load_home_recommendations().await
                {
                    self.browse.home.status_line = format!(
                        "{}: {}",
                        self.lang_text(
                            "推荐歌单刷新失败",
                            "Failed to refresh home recommendations",
                        ),
                        err
                    );
                }
            }
            _ => {}
        }
    }

    fn apply_settings_playback_delta(&mut self, delta: i32) {
        if delta == 0 {
            return;
        }

        match self.settings.playback_selected {
            0 => {
                self.config.visualize = self.config.visualize.cycle(delta);
                self.persist_config();
            }
            1 => {
                self.config.bars_gap = !self.config.bars_gap;
                self.persist_config();
            }
            2 => {
                self.config.bar_number = cycle_bar_number(self.config.bar_number, delta);
                self.persist_config();
            }
            3 => {
                self.config.bar_channels = match self.config.bar_channels {
                    BarChannels::Mono => BarChannels::Stereo,
                    BarChannels::Stereo => BarChannels::Mono,
                };
                self.persist_config();
            }
            4 => {
                self.config.album_border = !self.config.album_border;
                self.persist_config();
            }
            5 => {
                let next = self
                    .config
                    .audio_quality
                    .cycle(delta, self.vip_audio_unlocked);
                self.set_audio_quality(next);
            }
            6 => {
                self.config.playback_memory = !self.config.playback_memory;
                self.persist_config();
                if self.config.playback_memory {
                    self.persist_playback_memory();
                } else {
                    self.clear_playback_memory();
                }
            }
            _ => {}
        }
    }
}

#[derive(Default)]
pub(crate) struct SettingsController {
    pub(crate) selected: usize,
    pub(crate) general_selected: usize,
    pub(crate) general_scroll: usize,
    pub(crate) mpris_name_edit: Option<SettingsTextEdit>,
    pub(crate) playback_selected: usize,
    pub(crate) lyrics_selected: usize,
    pub(crate) keybind_selected: usize,
    pub(crate) keybind_rebinding: Option<usize>,
    pub(crate) keybind_scroll: usize,
    pub(crate) download_selected: usize,
    pub(crate) download_path_edit: Option<SettingsTextEdit>,
    pub(crate) download_reset_armed: bool,
    pub(super) item_hits: Vec<(HitRect, usize)>,
    pub(super) last_click: Option<(Instant, Overlay, usize)>,
}

impl SettingsController {
    pub(crate) fn text_editing(&self) -> bool {
        self.download_path_edit.is_some() || self.mpris_name_edit.is_some()
    }

    pub(super) fn reset_navigation(&mut self) {
        self.selected = 0;
        self.general_selected = 0;
        self.general_scroll = 0;
        self.mpris_name_edit = None;
        self.playback_selected = 0;
        self.lyrics_selected = 0;
        self.keybind_selected = 0;
        self.keybind_rebinding = None;
        self.keybind_scroll = 0;
        self.download_selected = 0;
        self.download_path_edit = None;
        self.download_reset_armed = false;
        self.last_click = None;
    }

    fn select(&mut self, index: usize, count: usize) {
        self.selected = index.min(count.saturating_sub(1));
    }

    pub(super) fn begin_download_path_edit(&mut self, value: String) {
        self.download_path_edit = Some(SettingsTextEdit::new(value));
    }

    pub(super) fn cancel_download_path_edit(&mut self) {
        self.download_path_edit = None;
        self.download_reset_armed = false;
    }
}

#[cfg(test)]
mod tests;
