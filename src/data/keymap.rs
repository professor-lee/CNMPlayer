//! 快捷键注册表：可重绑槽位与固定键的唯一来源。
//!
//! 主程序「按键绑定」页与全屏页按键提示弹窗都从这里取名称、分组与作用域；
//! 两侧的按键匹配也共用这里的规范化规则，固定键的分派直接引用这里的常量。

use crate::data::config::{Config, Language};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::sync::LazyLock;

/// 快捷键在哪一侧生效。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum KeyScope {
    Both,
    Host,
    Fullscreen,
}

impl KeyScope {
    pub(crate) fn host(self) -> bool {
        !matches!(self, Self::Fullscreen)
    }

    pub(crate) fn fullscreen(self) -> bool {
        !matches!(self, Self::Host)
    }

    /// 两个键是否在同一侧生效（冲突检测只比较有交集的槽位）。
    pub(crate) fn overlaps(self, other: Self) -> bool {
        self.host() && other.host() || self.fullscreen() && other.fullscreen()
    }
}

/// 一个可重绑槽位。槽位序号 = 在 [`SLOTS`] 中的下标。
pub(crate) struct KeySlot {
    zh: &'static str,
    en: &'static str,
    pub(crate) scope: KeyScope,
    pub(crate) get: fn(&Config) -> &String,
    pub(crate) get_mut: fn(&mut Config) -> &mut String,
}

macro_rules! slot {
    ($field:ident, $scope:ident, $zh:literal, $en:literal) => {
        KeySlot {
            zh: $zh,
            en: $en,
            scope: KeyScope::$scope,
            get: |config| &config.$field,
            get_mut: |config| &mut config.$field,
        }
    };
}

pub(crate) const SLOTS: [KeySlot; 25] = [
    slot!(keybind_search_box, Host, "搜索框", "Search Box"),
    slot!(keybind_fullscreen, Both, "全屏播放页", "Fullscreen"),
    slot!(keybind_settings, Both, "设置弹窗", "Settings"),
    slot!(
        keybind_sidebar,
        Both,
        "侧边栏 / 歌单浮层",
        "Sidebar / Playlist Overlay"
    ),
    slot!(keybind_quit, Host, "退出应用", "Quit"),
    slot!(keybind_page_up, Host, "快速上翻页", "Page Up"),
    slot!(keybind_page_down, Host, "快速下翻页", "Page Down"),
    slot!(keybind_prev, Host, "上一首", "Previous"),
    slot!(keybind_next, Host, "下一首", "Next"),
    slot!(keybind_toggle_play_pause, Host, "播放/暂停", "Play/Pause"),
    slot!(keybind_fullscreen_prev, Fullscreen, "上一首", "Previous"),
    slot!(keybind_fullscreen_next, Fullscreen, "下一首", "Next"),
    slot!(
        keybind_fullscreen_toggle_play_pause,
        Fullscreen,
        "播放/暂停",
        "Play/Pause"
    ),
    slot!(
        keybind_fullscreen_toggle_mode,
        Fullscreen,
        "循环模式切换",
        "Repeat Mode"
    ),
    slot!(keybind_fullscreen_eq, Fullscreen, "均衡器", "Equalizer"),
    slot!(
        keybind_fullscreen_eq_reset,
        Fullscreen,
        "均衡器重置",
        "Equalizer Reset"
    ),
    slot!(
        keybind_toggle_like_fullscreen,
        Fullscreen,
        "收藏/取消收藏歌曲",
        "Like/Unlike Song"
    ),
    slot!(keybind_toggle_mode, Host, "循环模式切换", "Repeat Mode"),
    slot!(
        keybind_toggle_like_collapsed,
        Host,
        "收藏/取消收藏歌曲",
        "Like/Unlike Song"
    ),
    slot!(
        keybind_small_window_toggle,
        Host,
        "小窗口面板切换",
        "Small Window Switch"
    ),
    slot!(keybind_download, Host, "下载歌曲", "Download Song"),
    slot!(
        keybind_download_fullscreen,
        Fullscreen,
        "下载歌曲",
        "Download Song"
    ),
    slot!(
        keybind_toggle_subscribe,
        Host,
        "收藏歌手/歌单",
        "Follow Artist/Collect Playlist"
    ),
    slot!(
        keybind_fullscreen_open_artist,
        Fullscreen,
        "进入歌手页",
        "Open Artist"
    ),
    slot!(
        keybind_fullscreen_open_album,
        Fullscreen,
        "进入专辑页",
        "Open Album"
    ),
];

pub(crate) const KEY_FORCE_QUIT: &str = "Ctrl+C";
pub(crate) const KEY_TOGGLE_KEYBINDS: &str = "Ctrl+K";
pub(crate) const KEY_RESET_KEYBINDS: &str = "Ctrl+Alt+R";
pub(crate) const KEY_SIDEBAR_SECTION_PREV: &str = "Ctrl+Up";
pub(crate) const KEY_SIDEBAR_SECTION_NEXT: &str = "Ctrl+Down";
pub(crate) const KEY_LOGIN_QR: &str = "F1";
pub(crate) const KEY_LOGIN_ACCOUNT: &str = "F2";
pub(crate) const KEY_LOGIN_PHONE: &str = "F3";

/// 不可重绑的固定键（只读展示）。
pub(crate) struct FixedKey {
    zh: &'static str,
    en: &'static str,
    scope: KeyScope,
    keys: &'static [&'static str],
}

pub(crate) const FIXED: [FixedKey; 9] = [
    FixedKey {
        zh: "强制退出",
        en: "Force Quit",
        scope: KeyScope::Host,
        keys: &[KEY_FORCE_QUIT],
    },
    FixedKey {
        zh: "按键绑定开关",
        en: "Toggle Keybinds",
        scope: KeyScope::Both,
        keys: &[KEY_TOGGLE_KEYBINDS],
    },
    FixedKey {
        zh: "恢复默认快捷键（按键绑定页）",
        en: "Reset Keybinds (Keybind Page)",
        scope: KeyScope::Host,
        keys: &[KEY_RESET_KEYBINDS],
    },
    FixedKey {
        zh: "侧边栏歌单分区切换",
        en: "Sidebar Section Switch",
        scope: KeyScope::Host,
        keys: &[KEY_SIDEBAR_SECTION_PREV, KEY_SIDEBAR_SECTION_NEXT],
    },
    FixedKey {
        zh: "登录方式（二维码/账号/手机号）",
        en: "Login Method (QR/Account/Phone)",
        scope: KeyScope::Host,
        keys: &[KEY_LOGIN_QR, KEY_LOGIN_ACCOUNT, KEY_LOGIN_PHONE],
    },
    FixedKey {
        zh: "返回上级 / 关闭浮层",
        en: "Back / Close Overlay",
        scope: KeyScope::Both,
        keys: &["Esc"],
    },
    FixedKey {
        zh: "确认 / 打开",
        en: "Confirm / Open",
        scope: KeyScope::Both,
        keys: &["Enter"],
    },
    FixedKey {
        zh: "移动焦点",
        en: "Move Focus",
        scope: KeyScope::Host,
        keys: &["Up", "Down", "Tab", "Shift+Tab"],
    },
    FixedKey {
        zh: "音量 / 弹窗内移动焦点",
        en: "Volume / Move Focus in Modals",
        scope: KeyScope::Fullscreen,
        keys: &["Up", "Down"],
    },
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum KeyGroup {
    General,
    Playback,
    Library,
    Jump,
    Equalizer,
    Window,
    Fixed,
}

impl KeyGroup {
    fn name(self, lang: Language) -> &'static str {
        let (zh, en) = match self {
            Self::General => ("常规", "General"),
            Self::Playback => ("播放控制", "Playback"),
            Self::Library => ("收藏与下载", "Like & Download"),
            Self::Jump => ("跳转", "Jump"),
            Self::Equalizer => ("均衡器", "Equalizer"),
            Self::Window => ("窗口", "Window"),
            Self::Fixed => ("固定键（不可修改）", "Fixed Keys (Read-only)"),
        };
        pick(lang, zh, en)
    }
}

/// 列表中的一行：分组标题、可重绑槽位（槽位序号）或固定键（[`FIXED`] 下标）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum KeyRow {
    Header(KeyGroup),
    Slot(usize),
    Fixed(usize),
}

impl KeyRow {
    pub(crate) fn is_header(self) -> bool {
        matches!(self, Self::Header(_))
    }

    fn scope(self) -> Option<KeyScope> {
        match self {
            Self::Header(_) => None,
            Self::Slot(index) => Some(SLOTS[index].scope),
            Self::Fixed(index) => Some(FIXED[index].scope),
        }
    }
}

/// 按功能分组的展示顺序（与槽位序号无关；槽位序号不落盘，配置按字段名存）。
const LAYOUT: &[(KeyGroup, &[KeyRow])] = &[
    (
        KeyGroup::General,
        &[
            KeyRow::Slot(0),
            KeyRow::Slot(1),
            KeyRow::Slot(2),
            KeyRow::Slot(3),
            KeyRow::Slot(4),
            KeyRow::Slot(5),
            KeyRow::Slot(6),
        ],
    ),
    (
        KeyGroup::Playback,
        &[
            KeyRow::Slot(7),
            KeyRow::Slot(8),
            KeyRow::Slot(9),
            KeyRow::Slot(17),
            KeyRow::Slot(10),
            KeyRow::Slot(11),
            KeyRow::Slot(12),
            KeyRow::Slot(13),
        ],
    ),
    (
        KeyGroup::Library,
        &[
            KeyRow::Slot(18),
            KeyRow::Slot(16),
            KeyRow::Slot(22),
            KeyRow::Slot(20),
            KeyRow::Slot(21),
        ],
    ),
    (KeyGroup::Jump, &[KeyRow::Slot(23), KeyRow::Slot(24)]),
    (KeyGroup::Equalizer, &[KeyRow::Slot(14), KeyRow::Slot(15)]),
    (KeyGroup::Window, &[KeyRow::Slot(19)]),
    (
        KeyGroup::Fixed,
        &[
            KeyRow::Fixed(0),
            KeyRow::Fixed(1),
            KeyRow::Fixed(2),
            KeyRow::Fixed(3),
            KeyRow::Fixed(4),
            KeyRow::Fixed(5),
            KeyRow::Fixed(6),
            KeyRow::Fixed(7),
            KeyRow::Fixed(8),
        ],
    ),
];

/// 列表展示在哪一侧：主程序列出全部键，全屏页只列全屏页可用的键。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum KeyView {
    Host,
    Fullscreen,
}

fn build_rows(keep: fn(KeyScope) -> bool) -> Vec<KeyRow> {
    let mut rows = Vec::new();
    for (group, entries) in LAYOUT {
        let mut kept = entries
            .iter()
            .copied()
            .filter(|row| row.scope().is_some_and(keep))
            .peekable();
        // 整组都不在这一侧生效时连标题一起省掉。
        if kept.peek().is_some() {
            rows.push(KeyRow::Header(*group));
            rows.extend(kept);
        }
    }
    rows
}

static HOST_ROWS: LazyLock<Vec<KeyRow>> = LazyLock::new(|| build_rows(|_| true));
static FULLSCREEN_ROWS: LazyLock<Vec<KeyRow>> = LazyLock::new(|| build_rows(KeyScope::fullscreen));

pub(crate) fn key_rows(view: KeyView) -> &'static [KeyRow] {
    match view {
        KeyView::Host => &HOST_ROWS,
        KeyView::Fullscreen => &FULLSCREEN_ROWS,
    }
}

/// 第一条可聚焦的行（跳过分组标题）。
pub(crate) fn first_focusable(rows: &[KeyRow]) -> usize {
    rows.iter().position(|row| !row.is_header()).unwrap_or(0)
}

/// 聚焦移动一步，跳过分组标题，首尾循环。
pub(crate) fn step_focus(rows: &[KeyRow], current: usize, forward: bool) -> usize {
    let len = rows.len();
    if len == 0 {
        return 0;
    }
    let mut index = current.min(len - 1);
    for _ in 0..len {
        index = if forward {
            (index + 1) % len
        } else {
            (index + len - 1) % len
        };
        if !rows[index].is_header() {
            return index;
        }
    }
    current
}

/// 列表视口：焦点行越界才滚；向上滚时把紧挨着的分组标题一起露出来，
/// 否则聚焦某组首条时它的标题会停在视口外。
pub(crate) fn scroll_for_row(
    scroll: usize,
    rows: &[KeyRow],
    visible: usize,
    focus: usize,
) -> usize {
    let top = if focus > 0 && rows.get(focus - 1).is_some_and(|row| row.is_header()) {
        focus - 1
    } else {
        focus
    };
    crate::ui::settings::scroll_for_focus(scroll.min(top), rows.len(), visible, focus)
}

fn pick(lang: Language, zh: &'static str, en: &'static str) -> &'static str {
    match lang {
        Language::Zh => zh,
        Language::En => en,
    }
}

/// 槽位名称。主程序列表同时列出两侧的键，名字后面标注仅在哪一侧生效。
pub(crate) fn slot_name(index: usize, lang: Language, view: KeyView) -> String {
    let slot = &SLOTS[index];
    qualified(pick(lang, slot.zh, slot.en), slot.scope, lang, view)
}

fn qualified(name: &str, scope: KeyScope, lang: Language, view: KeyView) -> String {
    let suffix = match (view, scope) {
        (KeyView::Fullscreen, _) | (_, KeyScope::Both) => "",
        (KeyView::Host, KeyScope::Host) => pick(lang, "（主程序）", " (Host)"),
        (KeyView::Host, KeyScope::Fullscreen) => pick(lang, "（全屏）", " (Fullscreen)"),
    };
    format!("{name}{suffix}")
}

/// 一行的展示文本：标题是分组名，其余是 `名称: 按键`。
pub(crate) fn row_text(row: KeyRow, config: &Config, lang: Language, view: KeyView) -> String {
    match row {
        KeyRow::Header(group) => group.name(lang).to_string(),
        KeyRow::Slot(index) => format!(
            "{}: {}",
            slot_name(index, lang, view),
            (SLOTS[index].get)(config)
        ),
        KeyRow::Fixed(index) => {
            let fixed = &FIXED[index];
            format!(
                "{}: {}",
                qualified(pick(lang, fixed.zh, fixed.en), fixed.scope, lang, view),
                fixed.keys.join(" / ")
            )
        }
    }
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

#[cfg(test)]
mod tests;
