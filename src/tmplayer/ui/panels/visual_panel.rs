use crate::data::config::VisualizeMode;
use crate::tmplayer::app::state::AppState;
use crate::tmplayer::render::{oscilloscope_renderer, spectrum_renderer, vector_renderer};
use crate::tmplayer::ui::borders::SOLID_BORDER;
use crate::ui::lyric_view;
use ratatui::Frame;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Paragraph};
use std::time::Instant;

pub fn render(f: &mut Frame, lyric_area: Rect, spectrum_area: Rect, app: &mut AppState) {
    let outer = Rect {
        x: lyric_area.x,
        y: lyric_area.y,
        width: lyric_area.width,
        height: lyric_area.height.saturating_add(spectrum_area.height),
    };
    let outer_block = Block::default()
        .borders(Borders::ALL)
        .border_set(SOLID_BORDER)
        .style(Style::default().fg(app.theme.color_subtext()));
    f.render_widget(outer_block, outer);

    let inner = outer.inner(ratatui::layout::Margin {
        horizontal: 1,
        vertical: 1,
    });
    if inner.width == 0 || inner.height == 0 {
        return;
    }

    if app.config.visualize == VisualizeMode::Lyrics {
        render_full_lyrics(f, inner, app);
        return;
    }

    let lyric_h = lyric_area.height.saturating_sub(2).min(inner.height);
    let lyric_inner = Rect {
        x: inner.x,
        y: inner.y,
        width: inner.width,
        height: lyric_h,
    };
    let spectrum_inner = Rect {
        x: inner.x,
        y: inner.y + lyric_h,
        width: inner.width,
        height: inner.height.saturating_sub(lyric_h),
    };

    let position = app.lyric_position_ms(Instant::now());
    let options = app.lyric_options();
    let lyric_rows = app
        .player
        .track
        .lyrics
        .as_ref()
        .filter(|lines| !lines.is_empty())
        .map(|lines| {
            lyric_view::render_pair(lines, position, lyric_inner.width, &app.theme, options)
        });
    if lyric_inner.height >= 1
        && let Some(rows) = lyric_rows
    {
        f.render_widget(
            Paragraph::new(rows).alignment(Alignment::Center),
            lyric_inner,
        );
    }

    match app.config.visualize {
        // 「关闭」档位在布局层就没有右栏这块区域（`tui::draw` 不再调用本函数）。
        VisualizeMode::Hidden => {}
        // 已在上面提前返回。
        VisualizeMode::Lyrics => {}
        VisualizeMode::Bars => spectrum_renderer::render(f, spectrum_inner, app),
        VisualizeMode::Oscilloscope => oscilloscope_renderer::render(f, spectrum_inner, app),
        VisualizeMode::Vector => vector_renderer::render(f, spectrum_inner, app),
    }
}

fn render_full_lyrics(f: &mut Frame, area: Rect, app: &AppState) {
    let rows = area.height as usize;
    let lyrics = app
        .player
        .track
        .lyrics
        .as_ref()
        .filter(|lines| !lines.is_empty());

    let lines = if let Some(lyrics) = lyrics {
        lyric_view::render_window(
            lyrics,
            app.lyric_position_ms(Instant::now()),
            rows,
            area.width,
            &app.theme,
            app.lyric_options(),
        )
    } else {
        let count = rows.max(1);
        let mut rows = vec![Line::default(); count];
        let center = count / 2;
        rows[center] =
            Line::from(no_lyrics_label(app)).style(Style::default().fg(app.theme.color_subtext()));
        rows
    };

    f.render_widget(Paragraph::new(lines).alignment(Alignment::Center), area);
}

fn no_lyrics_label(app: &AppState) -> &'static str {
    match app.language {
        crate::data::config::Language::Zh => "暂无歌词",
        crate::data::config::Language::En => "No lyrics",
    }
}
