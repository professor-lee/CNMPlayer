use crate::data::config::Config;
use crate::data::keymap::{KEY_TOGGLE_KEYBINDS, keybind_matches};
use crate::tmplayer::app::state::Overlay;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Action {
    Quit,
    TogglePlayPause,
    Prev,
    Next,
    VolumeUp,
    VolumeDown,
    SetVolume(f32),
    ToggleRepeatMode,
    ToggleFavorite,
    /// 下载当前播放的歌曲（再按一次取消在途下载）。
    ToggleDownload,
    /// 信息区里点了作者名（多作者显示串里的**段序号**）：退出全屏页，由宿主打开该作者页。
    OpenAuthorPage(usize),
    /// 信息区里点了专辑名：退出全屏页，由宿主打开该专辑页。
    OpenAlbumPage,
    /// 全屏进入歌手键：多作者时在各段间轮询选中，停按后由 tick 提交进入。
    CycleOpenArtist,
    /// 弹窗条目行被点击（序号）；单击聚焦，双击等同 Enter。
    ModalSelect(usize),
    TogglePlaylist,
    Confirm,
    CloseOverlay,
    CloseSettings,

    OpenSettingsModal,
    ToggleHelpModal,

    OpenEqModal,

    EqResetDefault,

    EqSetBandDb {
        band: usize,
        db: f32,
    },

    ModalUp,
    ModalDown,

    ModalLeft,
    ModalRight,

    PlaylistUp,
    PlaylistDown,
    PlaylistSelect(usize),

    SeekToFraction(f32),

    TextChar(char),
    TextBackspace,
    TextDelete,
    TextHome,
    TextEnd,

    MouseClick {
        col: u16,
        row: u16,
    },

    /// 滚轮：`col/row` 用于判断落在哪个面板上，`forward` 为向下滚。
    MouseScroll {
        col: u16,
        row: u16,
        forward: bool,
    },

    /// 按住左键拖动（音量条这类需要按住拖的控件）。
    MouseDrag {
        col: u16,
        row: u16,
    },

    /// 松开左键。
    MouseUp,

    None,
}

pub fn map_key(ev: KeyEvent, overlay: Overlay, config: &Config) -> Action {
    // modal-specific handling first
    if overlay == Overlay::SettingsModal {
        if keybind_matches(&config.keybind_settings, ev) {
            return Action::CloseOverlay;
        }
        return match ev.code {
            KeyCode::Esc => Action::CloseOverlay,
            KeyCode::Enter => Action::Confirm,
            KeyCode::Up => Action::ModalUp,
            KeyCode::Down => Action::ModalDown,
            KeyCode::Left => Action::ModalLeft,
            KeyCode::Right => Action::ModalRight,
            _ => Action::None,
        };
    }

    if overlay == Overlay::GeneralSettingsModal {
        if keybind_matches(&config.keybind_settings, ev) {
            return Action::CloseSettings;
        }
        return match ev.code {
            KeyCode::Esc => Action::CloseOverlay,
            KeyCode::Enter => Action::Confirm,
            KeyCode::Up | KeyCode::BackTab => Action::ModalUp,
            KeyCode::Down | KeyCode::Tab => Action::ModalDown,
            KeyCode::Left => Action::ModalLeft,
            KeyCode::Right => Action::ModalRight,
            _ => Action::None,
        };
    }

    if overlay == Overlay::BarSettingsModal {
        return match ev.code {
            KeyCode::Esc => Action::CloseOverlay,
            KeyCode::Enter => Action::Confirm,
            KeyCode::Up => Action::ModalUp,
            KeyCode::Down => Action::ModalDown,
            KeyCode::Left => Action::ModalLeft,
            KeyCode::Right => Action::ModalRight,
            _ => Action::None,
        };
    }

    if overlay == Overlay::LyricsSettingsModal {
        return match ev.code {
            KeyCode::Esc => Action::CloseOverlay,
            KeyCode::Enter => Action::Confirm,
            KeyCode::Up => Action::ModalUp,
            KeyCode::Down => Action::ModalDown,
            KeyCode::Left => Action::ModalLeft,
            KeyCode::Right => Action::ModalRight,
            _ => Action::None,
        };
    }

    if overlay == Overlay::DownloadSettingsModal {
        return match ev.code {
            KeyCode::Esc => Action::CloseOverlay,
            KeyCode::Enter => Action::Confirm,
            KeyCode::Up => Action::ModalUp,
            KeyCode::Down => Action::ModalDown,
            KeyCode::Left => Action::ModalLeft,
            KeyCode::Right => Action::ModalRight,
            _ => Action::None,
        };
    }

    if matches!(
        overlay,
        Overlay::DownloadPathEditModal | Overlay::MprisNameEditModal
    ) {
        return match ev.code {
            KeyCode::Esc => Action::CloseOverlay,
            KeyCode::Enter => Action::Confirm,
            KeyCode::Backspace => Action::TextBackspace,
            KeyCode::Delete => Action::TextDelete,
            KeyCode::Home => Action::TextHome,
            KeyCode::End => Action::TextEnd,
            KeyCode::Left => Action::ModalLeft,
            KeyCode::Right => Action::ModalRight,
            KeyCode::Char(ch) => {
                if ev
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
                    || ch.is_control()
                {
                    Action::None
                } else {
                    Action::TextChar(ch)
                }
            }
            _ => Action::None,
        };
    }

    if overlay == Overlay::EqModal {
        if keybind_matches(&config.keybind_fullscreen_eq_reset, ev) {
            return Action::EqResetDefault;
        }

        if keybind_matches(&config.keybind_fullscreen_eq, ev) {
            return Action::CloseOverlay;
        }

        return match ev.code {
            KeyCode::Esc => Action::CloseOverlay,
            KeyCode::Enter => Action::Confirm,
            KeyCode::Up => Action::ModalUp,
            KeyCode::Down => Action::ModalDown,
            KeyCode::Left => Action::ModalLeft,
            KeyCode::Right => Action::ModalRight,
            _ => Action::None,
        };
    }

    if overlay == Overlay::HelpModal {
        if keybind_matches(KEY_TOGGLE_KEYBINDS, ev) {
            return Action::ToggleHelpModal;
        }
        return match ev.code {
            KeyCode::Esc => Action::CloseOverlay,
            KeyCode::Up | KeyCode::BackTab => Action::ModalUp,
            KeyCode::Down | KeyCode::Tab => Action::ModalDown,
            _ => Action::None,
        };
    }

    if overlay == Overlay::AboutModal {
        return match ev.code {
            KeyCode::Esc => Action::CloseOverlay,
            _ => Action::None,
        };
    }

    // global shortcuts use the host Config bindings.
    if keybind_matches(&config.keybind_settings, ev) {
        return Action::OpenSettingsModal;
    }

    if keybind_matches(&config.keybind_fullscreen_eq, ev) {
        return Action::OpenEqModal;
    }

    // 下载（默认 Ctrl+D）：必须排在下面"与修饰键无关"的 match 之前，
    // 否则 Ctrl+D 会落进 Char('d') 之类的分支。
    if keybind_matches(&config.keybind_download_fullscreen, ev) {
        return Action::ToggleDownload;
    }

    if keybind_matches(&config.keybind_fullscreen_open_artist, ev) {
        return Action::CycleOpenArtist;
    }

    if keybind_matches(&config.keybind_fullscreen_open_album, ev) {
        return Action::OpenAlbumPage;
    }

    if keybind_matches(&config.keybind_fullscreen, ev) {
        return Action::Quit;
    }
    if keybind_matches(KEY_TOGGLE_KEYBINDS, ev) {
        return Action::ToggleHelpModal;
    }

    if overlay == Overlay::Playlist {
        if keybind_matches(&config.keybind_sidebar, ev) {
            return Action::TogglePlaylist;
        }
        return match ev.code {
            KeyCode::Esc => Action::CloseOverlay,
            KeyCode::Enter => Action::Confirm,
            KeyCode::Up => Action::PlaylistUp,
            KeyCode::Down => Action::PlaylistDown,
            _ => Action::None,
        };
    }

    if keybind_matches(&config.keybind_sidebar, ev) {
        return Action::TogglePlaylist;
    }

    if keybind_matches(&config.keybind_fullscreen_toggle_mode, ev) {
        return Action::ToggleRepeatMode;
    }

    if keybind_matches(&config.keybind_toggle_like_fullscreen, ev) {
        return Action::ToggleFavorite;
    }

    if keybind_matches(&config.keybind_fullscreen_prev, ev) {
        return Action::Prev;
    }

    if keybind_matches(&config.keybind_fullscreen_next, ev) {
        return Action::Next;
    }

    if keybind_matches(&config.keybind_fullscreen_toggle_play_pause, ev) {
        return Action::TogglePlayPause;
    }

    // 其余只剩不可重绑的固定键（见 `data::keymap::FIXED`）。
    match ev.code {
        KeyCode::Esc => Action::Quit,
        KeyCode::Enter => Action::Confirm,
        KeyCode::Up => Action::VolumeUp,
        KeyCode::Down => Action::VolumeDown,
        _ => Action::None,
    }
}

pub fn map_mouse(ev: MouseEvent) -> Action {
    match ev.kind {
        MouseEventKind::Down(MouseButton::Left) => Action::MouseClick {
            col: ev.column,
            row: ev.row,
        },
        MouseEventKind::ScrollUp => Action::MouseScroll {
            col: ev.column,
            row: ev.row,
            forward: false,
        },
        MouseEventKind::ScrollDown => Action::MouseScroll {
            col: ev.column,
            row: ev.row,
            forward: true,
        },
        MouseEventKind::Drag(MouseButton::Left) => Action::MouseDrag {
            col: ev.column,
            row: ev.row,
        },
        MouseEventKind::Up(MouseButton::Left) => Action::MouseUp,
        _ => Action::None,
    }
}

#[cfg(test)]
mod tests;
