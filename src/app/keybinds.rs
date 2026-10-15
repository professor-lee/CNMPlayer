use super::*;

pub(crate) const RESERVED_RESET_KEYBIND: &str = "Ctrl+Alt+R";

pub(crate) const DEFAULT_KEYBIND_SEARCH_BOX: &str = "Ctrl+S";

pub(crate) const DEFAULT_KEYBIND_FULLSCREEN: &str = "Ctrl+F";

pub(crate) const DEFAULT_KEYBIND_SETTINGS: &str = "T";

pub(crate) const DEFAULT_KEYBIND_SIDEBAR: &str = "P";

pub(crate) const DEFAULT_KEYBIND_QUIT: &str = "Q";

pub(crate) const DEFAULT_KEYBIND_PAGE_UP: &str = "pageUP";

pub(crate) const DEFAULT_KEYBIND_PAGE_DOWN: &str = "pageDown";

pub(crate) const DEFAULT_KEYBIND_PREV: &str = "Alt+Left";

pub(crate) const DEFAULT_KEYBIND_NEXT: &str = "Alt+Right";

pub(crate) const DEFAULT_KEYBIND_TOGGLE_PLAY_PAUSE: &str = "Alt+Space";

pub(crate) const DEFAULT_KEYBIND_TOGGLE_MODE: &str = "Alt+M";

pub(crate) const DEFAULT_KEYBIND_FULLSCREEN_PREV: &str = "Left";

pub(crate) const DEFAULT_KEYBIND_FULLSCREEN_NEXT: &str = "Right";

pub(crate) const DEFAULT_KEYBIND_FULLSCREEN_TOGGLE_PLAY_PAUSE: &str = "Space";

pub(crate) const DEFAULT_KEYBIND_FULLSCREEN_TOGGLE_MODE: &str = "M";

pub(crate) const DEFAULT_KEYBIND_FULLSCREEN_EQ: &str = "E";

pub(crate) const DEFAULT_KEYBIND_FULLSCREEN_EQ_RESET: &str = "Alt+R";

pub(crate) const DEFAULT_KEYBIND_TOGGLE_LIKE_FULLSCREEN: &str = "L";

pub(crate) const DEFAULT_KEYBIND_TOGGLE_LIKE_COLLAPSED: &str = "Alt+L";

pub(crate) const DEFAULT_KEYBIND_SMALL_WINDOW_TOGGLE: &str = "Alt+X";

pub(crate) const DEFAULT_KEYBIND_DOWNLOAD: &str = "Ctrl+Alt+D";

pub(crate) const DEFAULT_KEYBIND_DOWNLOAD_FULLSCREEN: &str = "Ctrl+D";

#[derive(Debug, Clone, Copy)]
pub(crate) enum KeybindAction {
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

pub(crate) fn keybind_matches(binding: &str, key: KeyEvent) -> bool {
    let Some(expected) = normalize_keybind_text(binding) else {
        return false;
    };
    let Some(actual) = key_event_to_keybind_text(key) else {
        return false;
    };
    expected.eq_ignore_ascii_case(actual.as_str())
}

pub(crate) fn is_reserved_reset_combo(key: KeyEvent) -> bool {
    key_event_to_keybind_text(key)
        .map(|value| value.eq_ignore_ascii_case(RESERVED_RESET_KEYBIND))
        .unwrap_or(false)
}

pub(crate) fn key_event_to_keybind_text(key: KeyEvent) -> Option<String> {
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

pub(crate) fn normalize_keybind_text(raw: &str) -> Option<String> {
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

pub(crate) fn normalize_keybind_token(token: &str) -> Option<String> {
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

pub(crate) fn key_code_to_keybind_token(code: KeyCode) -> Option<String> {
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

impl App {
    pub(crate) fn keybind_action_from_event(&self, key: KeyEvent) -> Option<KeybindAction> {
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

    pub(crate) fn keybind_value_for_action(&self, action: KeybindAction) -> &str {
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

    pub(crate) fn keybind_value_mut_for_index(&mut self, index: usize) -> Option<&mut String> {
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

    pub(crate) fn keybind_value_for_index(&self, index: usize) -> Option<&str> {
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

    pub(crate) fn find_keybind_conflict(
        &self,
        current_index: usize,
        binding: &str,
    ) -> Option<usize> {
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

    pub(crate) fn keybind_name_for_index(&self, index: usize) -> &'static str {
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
            20 => self.lang_text("主应用下载歌曲", "Host Download Song"),
            21 => self.lang_text("全屏页下载歌曲", "Fullscreen Download Song"),
            _ => self.lang_text("未知", "Unknown"),
        }
    }

    pub(crate) fn reset_keybinds_to_default(&mut self) {
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
}
