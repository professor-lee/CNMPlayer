use super::super::layout::{ModalRows, centered_rect};
use super::super::text::{lang_on_off, lang_text};
use crate::tmplayer::app::state::AppState;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
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

    let items = vec![
        format!("{}: {}", lang_text(app, "主题", "Theme"), app.config.theme),
        format!(
            "{}: {}",
            lang_text(app, "背景透明", "Transparent Background"),
            lang_on_off(app, app.config.transparent_background)
        ),
        format!("{}...", lang_text(app, "通用设置", "General Settings")),
        format!("{}...", lang_text(app, "播放设置", "Playback Settings")),
        format!("{}...", lang_text(app, "按键绑定", "Keybinds")),
        format!("{}...", lang_text(app, "歌词设置", "Lyrics Settings")),
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

pub(in crate::tmplayer::ui::tui) fn render_general_settings_modal(
    f: &mut ratatui::Frame,
    size: Rect,
    app: &mut AppState,
    modal_rows: &mut ModalRows,
) {
    let area = centered_rect(size, 70, 20);
    f.render_widget(ratatui::widgets::Clear, area);
    f.render_widget(
        Block::default()
            .borders(Borders::ALL)
            .border_set(crate::tmplayer::ui::borders::SOLID_BORDER)
            .title(lang_text(app, " 通用设置 ", " General Settings "))
            .style(
                Style::default()
                    .fg(app.theme.color_subtext())
                    .bg(app.theme.color_surface()),
            ),
        area,
    );
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
    let language = match app.language {
        crate::data::config::Language::Zh => "中文",
        crate::data::config::Language::En => "English",
    };
    let items = [
        format!("{}: {language}", lang_text(app, "语言", "Language")),
        format!(
            "{}: {}",
            lang_text(app, "图像协议", "Image Protocol"),
            app.config.graphics_protocol.display_name()
        ),
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
    ];
    let name_prefix = format!(
        "  {}: ",
        lang_text(app, "MPRIS服务名称", "MPRIS Service Name")
    );
    f.render_widget(
        Paragraph::new("").style(Style::default().fg(app.theme.color_subtext())),
        rows[2],
    );

    let visible = usize::from(rows[1].height);
    app.general_settings_scroll = crate::ui::settings::scroll_for_focus(
        app.general_settings_scroll,
        6,
        visible,
        app.general_settings_selected,
    );
    for (idx, item) in items
        .iter()
        .map(Some)
        .chain(std::iter::once(None))
        .enumerate()
        .skip(app.general_settings_scroll)
        .take(visible)
    {
        let rect = Rect {
            x: rows[1].x,
            y: rows[1].y + (idx - app.general_settings_scroll) as u16,
            width: rows[1].width,
            height: 1,
        };
        let style = if idx == app.general_settings_selected {
            Style::default()
                .fg(app.theme.color_accent2())
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(app.theme.color_text())
        };
        if let Some(item) = item {
            f.render_widget(
                Paragraph::new(Line::styled(format!("  {}", item), style)),
                rect,
            );
        } else {
            let avail = usize::from(rect.width)
                .saturating_sub(crate::ui::settings::display_width(&name_prefix));
            if let Some(edit) = app.mpris_name_edit.as_mut() {
                let caret_col = crate::ui::settings::caret_display_col(&edit.buffer, edit.cursor);
                edit.window_col =
                    crate::ui::settings::adjust_path_window(edit.window_col, caret_col, avail);
                let (value, caret) = crate::ui::settings::path_window(
                    &edit.buffer,
                    edit.cursor,
                    avail,
                    edit.window_col,
                );
                let value_style = Style::default()
                    .fg(app.theme.color_text())
                    .bg(app.theme.color_buff());
                let head: String = value.chars().take(caret).collect();
                let caret_char = value
                    .chars()
                    .nth(caret)
                    .map(|ch| ch.to_string())
                    .unwrap_or_else(|| " ".to_string());
                let tail: String = value.chars().skip(caret + 1).collect();
                f.render_widget(
                    Paragraph::new(Line::from(vec![
                        Span::styled(name_prefix.as_str(), style.bg(app.theme.color_surface())),
                        Span::styled(head, value_style),
                        Span::styled(caret_char, value_style.add_modifier(Modifier::REVERSED)),
                        Span::styled(tail, value_style),
                    ])),
                    rect,
                );
            } else {
                let value = if app.config.mpris_name.trim().is_empty() {
                    "CNMPlayer"
                } else {
                    app.config.mpris_name.as_str()
                };
                f.render_widget(
                    Paragraph::new(Line::styled(
                        format!(
                            "{name_prefix}{}",
                            crate::ui::settings::clip_to_display_width(value, avail)
                        ),
                        style,
                    )),
                    rect,
                );
            }
        }
        modal_rows.push(rect, idx);
    }
}
