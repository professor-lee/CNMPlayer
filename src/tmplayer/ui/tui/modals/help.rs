use super::super::layout::{ModalRows, centered_rect};
use super::super::text::lang_text;
use crate::tmplayer::app::state::AppState;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Paragraph};

/// 按键提示弹窗的条目 `(说明, 按键)`。
///
/// 渲染与键盘/滚轮翻页共用同一份，避免"条目数写死"与列表实际长度不一致
/// （此前末尾几行因此无法用键盘选中）。
pub fn help_items(app: &AppState) -> Vec<(String, String)> {
    let item = |zh: &'static str, en: &'static str, key: &str| {
        (lang_text(app, zh, en).to_string(), key.to_string())
    };

    vec![
        item("搜索框", "Search Box", &app.config.keybind_search_box),
        item("全屏播放页", "Fullscreen", &app.config.keybind_fullscreen),
        item("设置弹窗", "Settings Modal", &app.config.keybind_settings),
        item("侧边栏", "Sidebar", &app.config.keybind_sidebar),
        item("退出应用", "Quit", &app.config.keybind_quit),
        item(
            "快速上翻页（主程序）",
            "Quick Page Up (Host)",
            &app.config.keybind_page_up,
        ),
        item(
            "快速下翻页（主程序）",
            "Quick Page Down (Host)",
            &app.config.keybind_page_down,
        ),
        item("上一首", "Previous", &app.config.keybind_fullscreen_prev),
        item("下一首", "Next", &app.config.keybind_fullscreen_next),
        item(
            "播放/暂停",
            "Play/Pause",
            &app.config.keybind_fullscreen_toggle_play_pause,
        ),
        item(
            "全屏模式切换",
            "Fullscreen Mode Switch",
            &app.config.keybind_fullscreen_toggle_mode,
        ),
        item(
            "EQ均衡器",
            "EQ Equalizer",
            &app.config.keybind_fullscreen_eq,
        ),
        item(
            "EQ重置",
            "EQ Reset",
            &app.config.keybind_fullscreen_eq_reset,
        ),
        item(
            "收藏/取消收藏",
            "Like/Unlike",
            &app.config.keybind_toggle_like_fullscreen,
        ),
        item(
            "小窗口切换显示",
            "Small Window Switch",
            &app.config.keybind_small_window_toggle,
        ),
        item(
            "主应用下载歌曲",
            "Host Download Song",
            &app.config.keybind_download,
        ),
        item(
            "全屏页下载歌曲",
            "Fullscreen Download Song",
            &app.config.keybind_download_fullscreen,
        ),
        item(
            "侧边栏歌单区切换",
            "Sidebar Playlist Section Switch",
            "Ctrl+Up/Down",
        ),
        item("按键绑定开关", "Toggle Keybinds", "Ctrl+K"),
    ]
}

/// 按键提示弹窗的条目数（键盘/滚轮翻页用）。
pub fn help_item_count(app: &AppState) -> usize {
    help_items(app).len()
}

pub(in crate::tmplayer::ui::tui) fn render_help_modal(
    f: &mut ratatui::Frame,
    size: Rect,
    app: &mut AppState,
    modal_rows: &mut ModalRows,
) {
    let area = centered_rect(size, 70, 20);
    f.render_widget(ratatui::widgets::Clear, area);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_set(crate::tmplayer::ui::borders::SOLID_BORDER)
        .title(lang_text(app, " 按键绑定 ", " Keybinds "))
        .style(
            Style::default()
                .fg(app.theme.color_subtext())
                .bg(app.theme.color_surface()),
        );
    f.render_widget(block, area);

    let inner = area.inner(ratatui::layout::Margin {
        horizontal: 2,
        vertical: 1,
    });

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(1),
        ])
        .split(inner);
    f.render_widget(Paragraph::new(""), rows[0]);

    let items = help_items(app);

    let visible_rows = rows[1].height as usize;
    let total_rows = items.len();
    let selected = app.help_keybind_selected.min(total_rows.saturating_sub(1));
    // 与主应用按键绑定弹窗共用的越界聚焦滚动：焦点在窗口内视口不动。
    let scroll = crate::ui::settings::scroll_for_focus(
        app.help_keybind_scroll,
        total_rows,
        visible_rows,
        selected,
    );
    app.help_keybind_scroll = scroll;

    for (idx, (label, key)) in items.iter().enumerate().skip(scroll).take(visible_rows) {
        let style = if idx == selected {
            Style::default()
                .fg(app.theme.color_accent2())
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(app.theme.color_text())
        };
        let rect = Rect {
            x: rows[1].x,
            y: rows[1].y + (idx - scroll) as u16,
            width: rows[1].width,
            height: 1,
        };
        f.render_widget(
            Paragraph::new(Line::styled(format!("  {}: {}", label, key), style)),
            rect,
        );
        modal_rows.push(rect, idx);
    }

    f.render_widget(
        Paragraph::new(lang_text(
            app,
            "Up/Down 浏览  Ctrl+K 关闭  Esc 返回（仅查看）",
            "Up/Down browse  Ctrl+K close  Esc back (view only)",
        ))
        .style(Style::default().fg(app.theme.color_subtext())),
        rows[2],
    );
}
