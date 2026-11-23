//! Windows 终端的颜色能力探测。
//!
//! Windows Terminal、PowerShell、CMD 通常都不设 `TERM`/`COLORTERM`，沿用 Unix 的探测会退化成
//! `NoColor`，主题色全部变成 `Color::Reset`。这里改读 Windows 侧的信号，没有反例时默认真彩色。

use crate::ui::theme::ColorCapability;

pub(crate) fn color_capability() -> ColorCapability {
    classify(
        |key| std::env::var(key).ok(),
        crossterm::ansi_support::supports_ansi,
    )
}

fn classify(
    env: impl Fn(&str) -> Option<String>,
    vt_enabled: impl FnOnce() -> bool,
) -> ColorCapability {
    let present = |key: &str| env(key).is_some_and(|value| !value.trim().is_empty());

    let colorterm = env("COLORTERM").unwrap_or_default().to_ascii_lowercase();
    if colorterm.contains("truecolor") || colorterm.contains("24bit") {
        return ColorCapability::TrueColor;
    }

    // Windows Terminal 给每个会话注入 WT_SESSION；VS Code、WezTerm 等内嵌终端设 TERM_PROGRAM；
    // ConEmu/Cmder 开启 ANSI 处理时设 ConEmuANSI=ON。这些宿主都支持 24 位色。
    let conemu_ansi =
        env("ConEmuANSI").is_some_and(|value| value.trim().eq_ignore_ascii_case("on"));
    if present("WT_SESSION") || present("TERM_PROGRAM") || conemu_ansi {
        return ColorCapability::TrueColor;
    }

    // PowerShell/CMD 直接跑在 conhost 里：Windows 10 1703 起控制台支持 VT 序列与 24 位色，
    // 能开启 ENABLE_VIRTUAL_TERMINAL_PROCESSING 就按真彩色。开不了 VT 的旧控制台里 crossterm
    // 改走 WinAPI 文本属性，Rgb/索引色都会被画成黑色（属性值 0），只能不上色。
    if !vt_enabled() {
        return ColorCapability::NoColor;
    }

    ColorCapability::TrueColor
}

#[cfg(test)]
mod tests {
    use super::*;

    fn classify_with(vars: &[(&str, &str)], vt: bool) -> ColorCapability {
        classify(
            |key| {
                vars.iter()
                    .find(|(name, _)| *name == key)
                    .map(|(_, value)| (*value).to_string())
            },
            || vt,
        )
    }

    #[test]
    fn windows_terminal_is_truecolor_even_without_colorterm() {
        assert_eq!(
            classify_with(&[("WT_SESSION", "1b2c")], false),
            ColorCapability::TrueColor
        );
    }

    #[test]
    fn powershell_on_vt_capable_console_defaults_to_truecolor() {
        assert_eq!(classify_with(&[], true), ColorCapability::TrueColor);
    }

    #[test]
    fn legacy_console_without_vt_does_not_paint_black_rgb() {
        assert_eq!(classify_with(&[], false), ColorCapability::NoColor);
    }

    #[test]
    fn explicit_colorterm_and_embedded_terminals_win() {
        for vars in [
            &[("COLORTERM", "TrueColor")][..],
            &[("TERM_PROGRAM", "vscode")][..],
            &[("ConEmuANSI", "ON")][..],
        ] {
            assert_eq!(classify_with(vars, false), ColorCapability::TrueColor);
        }
        assert_eq!(
            classify_with(&[("ConEmuANSI", "OFF"), ("WT_SESSION", " ")], false),
            ColorCapability::NoColor
        );
    }
}
