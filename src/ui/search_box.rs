use crate::app::App;
use crate::data::config::Language;
use crate::tmplayer::ui::borders::SOLID_BORDER;
use ratatui::Frame;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::Style;
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use unicode_width::UnicodeWidthChar;

pub const TARGET_HEIGHT: u16 = 3;

pub fn draw_search_box_overlay(frame: &mut Frame, app: &App) {
    let size = frame.area();
    if size.width < 20 || size.height < 2 {
        return;
    }

    let visible_h = ((app.input.search_motion.value() * f32::from(TARGET_HEIGHT)).round() as u16)
        .min(size.height);
    if visible_h == 0 {
        return;
    }

    let width = (size.width / 2).max(24).min(size.width.saturating_sub(2));
    let area = Rect {
        x: size.x + size.width.saturating_sub(width) / 2,
        y: size.y,
        width,
        height: visible_h,
    };

    frame.render_widget(Clear, area);
    frame.render_widget(
        Block::default()
            .borders(Borders::ALL)
            .border_set(SOLID_BORDER)
            .border_style(Style::default().fg(app.theme.color_subtext()))
            .style(base_bg_style(app)),
        area,
    );

    if visible_h < TARGET_HEIGHT {
        return;
    }

    let inner = area.inner(ratatui::layout::Margin {
        horizontal: 1,
        vertical: 1,
    });
    if inner.width == 0 || inner.height == 0 {
        return;
    }

    let input = app.input.search_box_input.clone();
    let content = if input.trim().is_empty() {
        match app.config.language {
            Language::Zh => {
                "输入关键词搜索（作者/歌单/单曲）；后缀 @single/@album/@author/@list 限定类型"
                    .to_string()
            }
            Language::En => {
                "Search artists/playlists/songs; @single/@album/@author/@list to narrow".to_string()
            }
        }
    } else {
        input.clone()
    };

    let style = if input.trim().is_empty() {
        Style::default()
            .fg(app.theme.color_subtext())
            .bg(app.theme.color_surface())
    } else {
        Style::default()
            .fg(app.theme.color_text())
            .bg(app.theme.color_surface())
    };

    frame.render_widget(
        Paragraph::new(content)
            .style(style)
            .alignment(Alignment::Left),
        inner,
    );

    // Use terminal-native block cursor without injecting extra glyphs into the text.
    let mut cursor_offset = 0u16;
    for (idx, ch) in input.chars().enumerate() {
        if idx >= app.input.search_box_cursor {
            break;
        }
        cursor_offset = cursor_offset.saturating_add(ch.width().unwrap_or(1).max(1) as u16);
    }
    let cursor_x = inner
        .x
        .saturating_add(cursor_offset.min(inner.width.saturating_sub(1)));
    frame.set_cursor_position((cursor_x, inner.y));
}

fn base_bg_style(app: &App) -> Style {
    Style::default()
        .fg(app.theme.color_subtext())
        .bg(app.theme.color_surface())
}
