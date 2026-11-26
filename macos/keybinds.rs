//! macOS 默认快捷键。
//!
//! macOS 终端里 Option 默认用来输入特殊字符（Option+M 得到 µ、Option+Space 得到不换行空格），
//! Terminal.app、iTerm2、Ghostty 都要手动打开「Option 作为 Meta/Alt」后才会发出 Alt 组合键；
//! Ctrl+↑/↓ 又默认被调度中心占用，Ctrl+Space 是切换输入法。所以 macOS 版把依赖这些键的默认值
//! 换成 Ctrl+字母或 Shift+←/→（Terminal.app 默认会发出 Shift+←/→，但不发 Shift+↑/↓）。
//!
//! 只改默认值：首次生成的配置文件、缺失字段的补全与「恢复默认快捷键」都取这里改写后的模板，
//! 用户配置文件里已有的键位原样保留。

use std::sync::LazyLock;

/// 可重绑槽位：`(配置字段, config/default.toml 中的默认值, macOS 默认值)`。
const SLOT_DEFAULTS: [(&str, &str, &str); 8] = [
    ("keybind_prev", "Alt+Left", "Shift+Left"),
    ("keybind_next", "Alt+Right", "Shift+Right"),
    ("keybind_toggle_play_pause", "Alt+Space", "Ctrl+P"),
    ("keybind_toggle_mode", "Alt+M", "Ctrl+O"),
    ("keybind_fullscreen_eq_reset", "Alt+R", "Ctrl+E"),
    ("keybind_toggle_like_collapsed", "Alt+L", "Ctrl+L"),
    ("keybind_small_window_toggle", "Alt+X", "Ctrl+X"),
    ("keybind_download", "Ctrl+Alt+D", "Ctrl+D"),
];

/// 按键绑定页里恢复默认快捷键（其他平台为 Ctrl+Alt+R）。
pub(crate) const KEY_RESET_KEYBINDS: &str = "Ctrl+R";
/// 侧边栏展开时切换歌单分区（其他平台为 Ctrl+↑/↓，macOS 上被调度中心占用）。
pub(crate) const KEY_SIDEBAR_SECTION_PREV: &str = "Left";
pub(crate) const KEY_SIDEBAR_SECTION_NEXT: &str = "Right";

/// 按键绑定页底部提示 `[中文, 英文]`。
pub(crate) const KEYBIND_PAGE_HINT: [&str; 2] = [
    "Enter 重绑  Ctrl+R 重置  Ctrl+K 关闭  Esc 返回",
    "Enter rebind  Ctrl+R reset  Ctrl+K close  Esc back",
];
/// 试图把某个槽位绑到恢复默认键时的提示 `[中文, 英文]`。
pub(crate) const RESERVED_RESET_NOTICE: [&str; 2] = [
    "Ctrl+R 为保留快捷键，不能重新绑定",
    "Ctrl+R is reserved and cannot be rebound",
];
/// 侧边栏空闲时的操作提示 `[中文, 英文]`。
pub(crate) const SIDEBAR_HINT: [&str; 2] = [
    "左右切换分区 上下切换歌单 Enter进入 Esc收起",
    "Left/Right switch section, Up/Down switch playlist, Enter open, Esc collapse",
];

/// 换掉默认键位后的内嵌配置模板，其余内容（含注释）与 `config/default.toml` 逐字节相同。
pub(crate) fn default_config_toml() -> &'static str {
    static TEMPLATE: LazyLock<String> =
        LazyLock::new(|| rewrite_template(crate::data::assets::DEFAULT_CONFIG_TOML));
    &TEMPLATE
}

/// 逐行替换 `字段 = "默认值"`。模板里找不到或出现多次都是开发期错误，直接 panic，
/// 与 `config.rs` 对内嵌模板的严格校验一致，避免模板改了默认值而这里悄悄失配。
fn rewrite_template(template: &str) -> String {
    let mut out = String::with_capacity(template.len());
    let mut hits = [0usize; SLOT_DEFAULTS.len()];
    for line in template.split_inclusive('\n') {
        let body = line.trim_end_matches(['\r', '\n']);
        let slot = SLOT_DEFAULTS
            .iter()
            .position(|(key, default, _)| body == format!("{key} = \"{default}\""));
        match slot {
            Some(index) => {
                let (key, _, mac) = SLOT_DEFAULTS[index];
                hits[index] += 1;
                out.push_str(&format!("{key} = \"{mac}\""));
                out.push_str(&line[body.len()..]);
            }
            None => out.push_str(line),
        }
    }
    for ((key, default, _), count) in SLOT_DEFAULTS.iter().zip(hits) {
        assert!(
            count == 1,
            "config/default.toml must contain `{key} = \"{default}\"` exactly once (found {count})"
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::config::Config;
    use crate::data::keymap::{SLOTS, keybind_matches, normalize_keybind_text};
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    /// 默认值里不能再有 Option 组合，也不能和恢复默认键或同侧的其他默认键撞车。
    #[test]
    fn defaults_avoid_option_and_do_not_collide() {
        let config = Config::default();
        let reset = normalize_keybind_text(KEY_RESET_KEYBINDS).unwrap();
        let sidebar = [KEY_SIDEBAR_SECTION_PREV, KEY_SIDEBAR_SECTION_NEXT]
            .map(|key| normalize_keybind_text(key).unwrap());
        let bindings: Vec<String> = SLOTS
            .iter()
            .map(|slot| normalize_keybind_text((slot.get)(&config)).unwrap())
            .collect();
        for (index, binding) in bindings.iter().enumerate() {
            assert!(!binding.contains("Alt"), "{binding}");
            assert_ne!(*binding, reset);
            // 主程序的快捷键先于侧边栏处理，撞上就切不了分区。
            if SLOTS[index].scope.host() {
                assert!(!sidebar.contains(binding), "{binding}");
            }
            for (other, other_binding) in bindings.iter().enumerate().skip(index + 1) {
                assert!(
                    binding != other_binding || !SLOTS[index].scope.overlaps(SLOTS[other].scope),
                    "{binding} is the default of two slots on the same side"
                );
            }
        }
        for key in [
            KEY_RESET_KEYBINDS,
            KEY_SIDEBAR_SECTION_PREV,
            KEY_SIDEBAR_SECTION_NEXT,
        ] {
            assert!(!key.contains("Alt"), "{key}");
        }
    }

    /// 终端发来的 Shift+← 要能命中默认的上一首。
    #[test]
    fn shift_arrow_default_matches_terminal_event() {
        let config = Config::default();
        let event = KeyEvent::new(KeyCode::Left, KeyModifiers::SHIFT);
        assert!(keybind_matches(&config.keybind_prev, event));
    }
}
