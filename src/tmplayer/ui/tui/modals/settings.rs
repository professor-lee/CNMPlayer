use super::super::layout::{ModalRows, centered_rect};
use super::super::text::{lang_on_off, lang_text};
use crate::tmplayer::app::state::AppState;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Paragraph};

pub(in crate::tmplayer::ui::tui) fn render_settings_modal(
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
        .title(lang_text(app, " 设置 ", " Settings "))
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

    let language_label = match app.language {
        crate::data::config::Language::Zh => "中文",
        crate::data::config::Language::En => "English",
    };

    let items = vec![
        format!("{}: {}", lang_text(app, "主题", "Theme"), app.config.theme),
        format!(
            "{}: {}",
            lang_text(app, "背景透明", "Transparent Background"),
            lang_on_off(app, app.config.transparent_background)
        ),
        format!("{}: {}", lang_text(app, "语言", "Language"), language_label),
        format!(
            "{}: {}",
            lang_text(app, "图形协议", "Graphics"),
            app.config.graphics_protocol.display_name()
        ),
        format!("{}...", lang_text(app, "播放设置", "Playback Settings")),
        format!("{}...", lang_text(app, "按键绑定", "Keybinds")),
        format!("{}...", lang_text(app, "歌词设置", "Lyrics Settings")),
        format!(
            "{}: {}",
            lang_text(app, "显示提示", "Show Hints"),
            lang_on_off(app, app.config.show_hints)
        ),
        format!(
            "{}: {}",
            lang_text(app, "小窗口显示", "Small Window Display"),
            lang_on_off(app, app.config.small_window_display)
        ),
        format!(
            "{}: {}",
            lang_text(app, "主页更多推荐", "More Home Recommendations"),
            lang_on_off(app, app.config.home_more_recommend)
        ),
        format!("{}...", lang_text(app, "下载设置", "Download Settings")),
        lang_text(app, "退出登录", "Logout").to_string(),
        "about".to_string(),
    ];

    let item_style = |idx: usize| {
        if idx == app.settings_selected {
            Style::default()
                .fg(app.theme.color_accent2())
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(app.theme.color_text())
        }
    };

    // "about" is pinned to the bottom row of the modal; the rest stack from the top.
    let about_idx = items.len().saturating_sub(1);
    for (idx, text) in items.iter().take(about_idx).enumerate() {
        if idx as u16 >= rows[1].height {
            break;
        }
        let rect = Rect {
            x: rows[1].x,
            y: rows[1].y + idx as u16,
            width: rows[1].width,
            height: 1,
        };
        f.render_widget(
            Paragraph::new(Line::styled(format!("  {}", text), item_style(idx))),
            rect,
        );
        modal_rows.push(rect, idx);
    }

    let bottom_cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(55), Constraint::Percentage(45)])
        .split(rows[2]);

    f.render_widget(
        Paragraph::new(Line::styled(
            format!("  {}", items[about_idx]),
            item_style(about_idx),
        )),
        bottom_cols[0],
    );
    modal_rows.push(bottom_cols[0], about_idx);
    f.render_widget(Paragraph::new(""), bottom_cols[1]);
}
