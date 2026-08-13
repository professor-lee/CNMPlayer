use crate::tmplayer::app::state::Overlay;
use crate::tmplayer::data::config::Config;
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
    TogglePlaylist,
    Confirm,
    CloseOverlay,

    OpenSettingsModal,
    OpenHelpModal,

    OpenEqModal,

    EqResetDefault,

    EqSetBandDb { band: usize, db: f32 },

    ModalUp,
    ModalDown,

    ModalLeft,
    ModalRight,

    PlaylistUp,
    PlaylistDown,
    PlaylistMoveItemUp,
    PlaylistMoveItemDown,
    PlaylistSelect(usize),

    PrevAlbum,
    NextAlbum,

    SeekToFraction(f32),

    FolderChar(char),
    FolderBackspace,

    MouseClick { col: u16, row: u16 },

    None,
}

pub fn map_key(ev: KeyEvent, overlay: Overlay, config: &Config) -> Action {
    if overlay == Overlay::AcoustIdModal {
        match ev.code {
            KeyCode::Esc => return Action::CloseOverlay,
            KeyCode::Enter => return Action::Confirm,
            KeyCode::Backspace => return Action::FolderBackspace,
            KeyCode::Char(c) => return Action::FolderChar(c),
            KeyCode::Left => return Action::None,
            KeyCode::Right => return Action::None,
            KeyCode::Up => return Action::None,
            KeyCode::Down => return Action::None,
            _ => {}
        }
        return Action::None;
    }

    // modal-specific handling first
    if overlay == Overlay::SettingsModal {
        return match ev.code {
            KeyCode::Esc => Action::CloseOverlay,
            KeyCode::Char('t') | KeyCode::Char('T') => Action::CloseOverlay,
            KeyCode::Enter => Action::Confirm,
            KeyCode::Up => Action::ModalUp,
            KeyCode::Down => Action::ModalDown,
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

    if overlay == Overlay::LocalAudioSettingsModal {
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
        if ev.modifiers.contains(KeyModifiers::CONTROL)
            && matches!(ev.code, KeyCode::Char('k') | KeyCode::Char('K'))
        {
            return Action::CloseOverlay;
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

    // global shortcuts (except folder input)
    match ev.code {
        KeyCode::Char('t') | KeyCode::Char('T') => return Action::OpenSettingsModal,
        _ => {}
    }

    if keybind_matches(&config.keybind_fullscreen_eq, ev) {
        return Action::OpenEqModal;
    }

    if ev.modifiers.contains(KeyModifiers::CONTROL) {
        match ev.code {
            // In CNMPlayer embedded mode, Ctrl+F folds fullscreen back to the host UI.
            KeyCode::Char('f') | KeyCode::Char('F') => return Action::Quit,
            KeyCode::Char('k') | KeyCode::Char('K') => return Action::OpenHelpModal,
            _ => {}
        }
    }

    if overlay == Overlay::Playlist {
        if keybind_matches(&config.keybind_sidebar, ev) {
            return Action::TogglePlaylist;
        }
        return match ev.code {
            KeyCode::Esc => Action::CloseOverlay,
            KeyCode::Enter => Action::Confirm,
            KeyCode::Left => {
                if ev.modifiers.contains(KeyModifiers::CONTROL) {
                    Action::PrevAlbum
                } else {
                    Action::None
                }
            }
            KeyCode::Right => {
                if ev.modifiers.contains(KeyModifiers::CONTROL) {
                    Action::NextAlbum
                } else {
                    Action::None
                }
            }
            KeyCode::Up => {
                if ev.modifiers.contains(KeyModifiers::CONTROL) {
                    Action::PlaylistMoveItemUp
                } else {
                    Action::PlaylistUp
                }
            }
            KeyCode::Down => {
                if ev.modifiers.contains(KeyModifiers::CONTROL) {
                    Action::PlaylistMoveItemDown
                } else {
                    Action::PlaylistDown
                }
            }
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

    match ev.code {
        KeyCode::Char('m') | KeyCode::Char('M') => Action::ToggleRepeatMode,
        KeyCode::Char('l') | KeyCode::Char('L') => Action::ToggleFavorite,
        KeyCode::Esc => Action::Quit,
        KeyCode::Enter => Action::Confirm,
        KeyCode::Left => Action::Prev,
        KeyCode::Right => Action::Next,
        KeyCode::Up => Action::VolumeUp,
        KeyCode::Down => Action::VolumeDown,
        KeyCode::Char(' ') => Action::TogglePlayPause,
        _ => Action::None,
    }
}

pub fn map_mouse(ev: MouseEvent) -> Action {
    if let MouseEventKind::Down(MouseButton::Left) = ev.kind {
        return Action::MouseClick {
            col: ev.column,
            row: ev.row,
        };
    }
    Action::None
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
