use super::super::layout::{ModalRows, centered_rect};
use super::super::text::lang_text;
use crate::tmplayer::app::state::AppState;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};

pub(in crate::tmplayer::ui::tui) fn render_download_settings_modal(
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
        .title(lang_text(app, " 下载设置 ", " Download Settings "))
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

    let enabled = app.download_enabled();
    let text_color = app.theme.color_text();
    let subtext = app.theme.color_subtext();
    let accent2 = app.theme.color_accent2();
    let warning = app.theme.color_accent3();
    let buff = app.theme.color_buff();
    let surface = app.theme.color_surface();

    let quality_label = match app.config.download_audio_quality {
        crate::data::config::AudioQuality::Standard => lang_text(app, "标准", "Standard"),
        crate::data::config::AudioQuality::Higher => lang_text(app, "较高", "Higher"),
        crate::data::config::AudioQuality::Exhigh => lang_text(app, "极高", "Exhigh"),
        crate::data::config::AudioQuality::Lossless => lang_text(app, "无损", "Lossless"),
        crate::data::config::AudioQuality::Hires => "Hi-Res",
        crate::data::config::AudioQuality::Jyeffect => lang_text(app, "高清环绕声", "JYEffect"),
        crate::data::config::AudioQuality::Sky => lang_text(app, "沉浸环绕声", "Sky"),
        crate::data::config::AudioQuality::Dolby => lang_text(app, "杜比全景声", "Dolby"),
        crate::data::config::AudioQuality::Jymaster => lang_text(app, "超清母带", "JYMaster"),
    };
    let path_prefix = format!("{}: ", lang_text(app, "下载路径", "Download Path"));
    let path_display = app.download_display_path();
    let reset_label = if app.download_reset_armed {
        lang_text(app, "确认恢复", "Confirm Restore")
    } else {
        lang_text(app, "恢复默认", "Restore Defaults")
    };

    let editing = app.download_path_edit.is_some();
    for idx in 0..3 {
        // 不可用时只灰置「音质」：路径行与「恢复默认」都留着当出口。
        let disabled = !enabled && idx == 0;
        let selected = idx == app.download_settings_selected;
        let base_style = if selected {
            if disabled {
                Style::default().fg(subtext)
            } else {
                Style::default().fg(accent2).add_modifier(Modifier::BOLD)
            }
        } else if disabled {
            Style::default().fg(subtext)
        } else {
            Style::default().fg(text_color)
        };

        let (text, style) = match idx {
            0 => (
                format!(
                    "  {}: {}",
                    lang_text(app, "音质", "Audio Quality"),
                    quality_label
                ),
                base_style,
            ),
            2 => (
                format!("  {reset_label}"),
                if app.download_reset_armed {
                    Style::default().fg(warning).add_modifier(Modifier::BOLD)
                } else {
                    base_style
                },
            ),
            _ => {
                let avail = usize::from(rows[1].width)
                    .saturating_sub(crate::ui::settings::display_width(&path_prefix) + 2)
                    .max(1);
                match app.download_path_edit.as_mut() {
                    Some(edit) if editing => {
                        // 同主应用：只有路径值段落变底色；窗口只在光标撞边界时才滚。
                        let caret_col =
                            crate::ui::settings::caret_display_col(&edit.buffer, edit.cursor);
                        edit.window_col = crate::ui::settings::adjust_path_window(
                            edit.window_col,
                            caret_col,
                            avail,
                        );
                        let (visible, caret) = crate::ui::settings::path_window(
                            &edit.buffer,
                            edit.cursor,
                            avail,
                            edit.window_col,
                        );
                        let value_style = Style::default().fg(text_color).bg(buff);
                        let mut spans: Vec<Span> = vec![Span::styled(
                            format!("  {path_prefix}"),
                            base_style.bg(surface),
                        )];
                        let head: String = visible.chars().take(caret).collect();
                        let caret_char = visible
                            .chars()
                            .nth(caret)
                            .map(|ch| ch.to_string())
                            .unwrap_or_else(|| " ".to_string());
                        let tail: String = visible.chars().skip(caret + 1).collect();
                        spans.push(Span::styled(head, value_style));
                        spans.push(Span::styled(
                            caret_char,
                            value_style.add_modifier(Modifier::REVERSED),
                        ));
                        spans.push(Span::styled(tail, value_style));
                        f.render_widget(
                            Paragraph::new(Line::from(spans)),
                            Rect {
                                x: rows[1].x,
                                y: rows[1].y + idx as u16,
                                width: rows[1].width,
                                height: 1,
                            },
                        );
                        modal_rows.push(
                            Rect {
                                x: rows[1].x,
                                y: rows[1].y + idx as u16,
                                width: rows[1].width,
                                height: 1,
                            },
                            idx,
                        );
                        continue;
                    }
                    _ => (
                        format!(
                            "  {path_prefix}{}",
                            crate::ui::settings::clip_to_display_width(&path_display, avail)
                        ),
                        base_style,
                    ),
                }
            }
        };

        let rect = Rect {
            x: rows[1].x,
            y: rows[1].y + idx as u16,
            width: rows[1].width,
            height: 1,
        };
        f.render_widget(Paragraph::new(Line::styled(text, style)), rect);
        modal_rows.push(rect, idx);
    }
}
