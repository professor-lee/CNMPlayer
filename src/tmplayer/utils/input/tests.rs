use super::*;
use crossterm::event::{MouseEvent, MouseEventKind};

fn mouse(kind: MouseEventKind, column: u16, row: u16) -> MouseEvent {
    MouseEvent {
        kind,
        column,
        row,
        modifiers: KeyModifiers::NONE,
    }
}

/// 鼠标事件映射：滚轮与拖动都要带坐标传给事件循环（此前只认左键按下）。
#[test]
fn mouse_mapping_covers_wheel_drag_and_release() {
    assert_eq!(
        map_mouse(mouse(MouseEventKind::ScrollDown, 3, 4)),
        Action::MouseScroll {
            col: 3,
            row: 4,
            forward: true
        }
    );
    assert_eq!(
        map_mouse(mouse(MouseEventKind::ScrollUp, 3, 4)),
        Action::MouseScroll {
            col: 3,
            row: 4,
            forward: false
        }
    );
    assert_eq!(
        map_mouse(mouse(MouseEventKind::Drag(MouseButton::Left), 5, 6)),
        Action::MouseDrag { col: 5, row: 6 }
    );
    assert_eq!(
        map_mouse(mouse(MouseEventKind::Up(MouseButton::Left), 5, 6)),
        Action::MouseUp
    );
    assert_eq!(
        map_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 1, 2)),
        Action::MouseClick { col: 1, row: 2 }
    );
    assert_eq!(map_mouse(mouse(MouseEventKind::Moved, 1, 2)), Action::None);
}

fn esc() -> KeyEvent {
    KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)
}

/// 全屏页 Ctrl+D 必须映射成下载动作（且不能被下面"与修饰键无关"的
/// `match ev.code` 分支吃掉）。
#[test]
fn ctrl_d_maps_to_toggle_download() {
    let config = crate::data::config::Config::default();
    assert_eq!(
        config.keybind_download_fullscreen, "Ctrl+D",
        "默认键位变了？"
    );

    let ev = KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL);
    assert_eq!(map_key(ev, Overlay::None, &config), Action::ToggleDownload);
}

/// 设置类弹窗里的 Esc 必须走"关闭弹窗"而不是退出全屏页。
///
/// 曾经漏了 LyricsSettingsModal 的分支，Esc 于是落到默认的 Action::Quit，
/// 在歌词浮窗子页按 Esc 会直接把全屏页关掉。
#[test]
fn esc_in_settings_submodals_closes_the_modal() {
    let config = crate::data::config::Config::default();

    for overlay in [
        Overlay::SettingsModal,
        Overlay::BarSettingsModal,
        Overlay::LyricsSettingsModal,
        Overlay::DownloadSettingsModal,
        Overlay::DownloadPathEditModal,
        Overlay::HelpModal,
        Overlay::AboutModal,
        Overlay::EqModal,
    ] {
        assert_eq!(
            map_key(esc(), overlay, &config),
            Action::CloseOverlay,
            "{overlay:?} 里 Esc 应该是返回上一级"
        );
    }

    // 没有弹窗时 Esc 才是退出全屏页。
    assert_eq!(map_key(esc(), Overlay::None, &config), Action::Quit);
}
