use super::App;
use crate::data::config::Config;
use crate::data::keymap::{self, KeyView, SLOTS};
use crossterm::event::KeyEvent;

pub(super) use crate::data::keymap::KEY_RESET_KEYBINDS as RESERVED_RESET_KEYBIND;
pub(super) use crate::data::keymap::{key_event_to_keybind_text, keybind_matches};

#[derive(Debug, Clone, Copy)]
pub(super) enum KeybindAction {
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
    /// 主应用：收藏 / 取消收藏当前歌手页的歌手或歌单页的歌单（专辑、EP、Single）。
    ToggleSubscribe,
    /// 全屏页：进入当前歌曲的歌手页（多歌手时轮询选择，宿主侧不处理）。
    FullscreenOpenArtist,
    /// 全屏页：进入当前歌曲的专辑页（宿主侧不处理）。
    FullscreenOpenAlbum,
}

impl KeybindAction {
    /// 该动作在注册表 [`SLOTS`] 里的槽位序号。
    fn slot(self) -> usize {
        match self {
            Self::SearchBox => 0,
            Self::Fullscreen => 1,
            Self::Settings => 2,
            Self::Sidebar => 3,
            Self::Quit => 4,
            Self::PageUp => 5,
            Self::PageDown => 6,
            Self::Prev => 7,
            Self::Next => 8,
            Self::TogglePlayPause => 9,
            Self::FullscreenPrev => 10,
            Self::FullscreenNext => 11,
            Self::FullscreenTogglePlayPause => 12,
            Self::FullscreenToggleMode => 13,
            Self::FullscreenEq => 14,
            Self::FullscreenEqReset => 15,
            Self::ToggleLikeFullscreen => 16,
            Self::ToggleMode => 17,
            Self::ToggleLikeCollapsed => 18,
            Self::SmallWindowToggle => 19,
            Self::Download => 20,
            Self::DownloadFullscreen => 21,
            Self::ToggleSubscribe => 22,
            Self::FullscreenOpenArtist => 23,
            Self::FullscreenOpenAlbum => 24,
        }
    }
}

pub(super) fn is_reserved_reset_combo(key: KeyEvent) -> bool {
    keybind_matches(RESERVED_RESET_KEYBIND, key)
}

impl App {
    pub(super) fn keybind_action_from_event(&self, key: KeyEvent) -> Option<KeybindAction> {
        let actions = [
            // 与全屏收藏同默认键（L）：主应用侧必须先匹配到它，全屏页另走 `map_key`。
            KeybindAction::ToggleSubscribe,
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
            KeybindAction::FullscreenOpenArtist,
            KeybindAction::FullscreenOpenAlbum,
        ];

        actions
            .into_iter()
            .find(|&action| keybind_matches((SLOTS[action.slot()].get)(&self.config), key))
    }

    pub(super) fn find_keybind_conflict(
        &self,
        current_index: usize,
        binding: &str,
    ) -> Option<usize> {
        let normalized = keymap::normalize_keybind_text(binding)?;
        let current_scope = SLOTS[current_index].scope;
        SLOTS.iter().enumerate().find_map(|(other_index, other)| {
            // 只在同一处生效的两个键才算冲突：主应用收藏与全屏收藏可以同为 L。
            if other_index == current_index || !current_scope.overlaps(other.scope) {
                return None;
            }
            keymap::normalize_keybind_text((other.get)(&self.config))
                .filter(|other| other.eq_ignore_ascii_case(&normalized))
                .map(|_| other_index)
        })
    }

    pub(super) fn keybind_name_for_index(&self, index: usize) -> String {
        keymap::slot_name(index, self.config.language, KeyView::Host)
    }

    pub(super) fn reset_keybinds_to_default(&mut self) {
        let defaults = Config::default();
        for slot in &SLOTS {
            (slot.get_mut)(&mut self.config).clone_from((slot.get)(&defaults));
        }
    }
}
