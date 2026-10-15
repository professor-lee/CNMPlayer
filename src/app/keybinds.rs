use super::*;

impl App {
    pub(super) fn keybind_action_from_event(&self, key: KeyEvent) -> Option<KeybindAction> {
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

    pub(super) fn keybind_value_for_action(&self, action: KeybindAction) -> &str {
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

    pub(super) fn keybind_value_mut_for_index(&mut self, index: usize) -> Option<&mut String> {
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

    pub(super) fn keybind_value_for_index(&self, index: usize) -> Option<&str> {
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

    pub(super) fn find_keybind_conflict(&self, current_index: usize, binding: &str) -> Option<usize> {
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

    pub(super) fn keybind_name_for_index(&self, index: usize) -> &'static str {
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

    pub(super) fn reset_keybinds_to_default(&mut self) {
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
