use crate::app::App;
use crate::data::config::Language;
use crate::ui::lyric_view;
use ratatui::Frame;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear};

pub(crate) const PAGE_LYRICS_PANEL_HEIGHT: u16 = 4;

/// 歌词浮窗尺寸：宽 = 内容区 33%（夹在 18..=min(内容宽, 72)），高固定。
/// 内容区放不下时返回 `None`。
fn panel_size(content_area: Rect) -> Option<(u16, u16)> {
    if content_area.width < 18 || content_area.height < PAGE_LYRICS_PANEL_HEIGHT {
        return None;
    }

    let width = ((content_area.width as f32) * 0.33).round() as u16;
    let width = width.clamp(18, content_area.width.min(72));
    Some((width, PAGE_LYRICS_PANEL_HEIGHT))
}

fn clamp_pos(pos: (f32, f32)) -> (f32, f32) {
    (pos.0.clamp(0.0, 1.0), pos.1.clamp(0.0, 1.0))
}

/// 归一化位置（内容区内左上角，0..=1）→ 屏幕矩形。
fn panel_rect_at(content_area: Rect, size: (u16, u16), pos: (f32, f32)) -> Rect {
    let (width, height) = size;
    let (pos_x, pos_y) = clamp_pos(pos);
    let max_x = content_area.width.saturating_sub(width);
    let max_y = content_area.height.saturating_sub(height);

    Rect {
        x: content_area.x + (pos_x * max_x as f32).round() as u16,
        y: content_area.y + (pos_y * max_y as f32).round() as u16,
        width,
        height,
    }
}

/// 歌词浮窗矩形。`pos` 是归一化位置，默认 (1,1) = 右下角（历史行为）。
pub fn overlay_panel_area(content_area: Rect, pos: (f32, f32)) -> Rect {
    match panel_size(content_area) {
        Some(size) => panel_rect_at(content_area, size, pos),
        None => Rect::default(),
    }
}

/// 拖拽中的新位置：`grab` 是按下时鼠标相对浮窗左上角的偏移，
/// 这样拖动时浮窗不会跳到光标下。
pub fn pos_after_drag(
    content_area: Rect,
    panel: Rect,
    col: u16,
    row: u16,
    grab: (u16, u16),
) -> (f32, f32) {
    let max_x = content_area.width.saturating_sub(panel.width);
    let max_y = content_area.height.saturating_sub(panel.height);
    let x = col
        .saturating_sub(content_area.x)
        .saturating_sub(grab.0)
        .min(max_x);
    let y = row
        .saturating_sub(content_area.y)
        .saturating_sub(grab.1)
        .min(max_y);

    (
        if max_x == 0 {
            0.0
        } else {
            (x as f32 / max_x as f32).clamp(0.0, 1.0)
        },
        if max_y == 0 {
            0.0
        } else {
            (y as f32 / max_y as f32).clamp(0.0, 1.0)
        },
    )
}

/// 边缘吸附：把浮窗吸到最近的那条边（左/右/上/下），**另一轴保持自由**。
///
/// 不吸到角：贴着上边横向居中拖动时只吸 y，x 留在原位。到两条边等距
/// （例如本来就在角上）时两轴都吸。
pub fn snap_pos(content_area: Rect, panel: Rect, pos: (f32, f32)) -> (f32, f32) {
    let (pos_x, pos_y) = clamp_pos(pos);
    let d_left = panel.x.saturating_sub(content_area.x);
    let d_right = content_area.right().saturating_sub(panel.right());
    let d_top = panel.y.saturating_sub(content_area.y);
    let d_bottom = content_area.bottom().saturating_sub(panel.bottom());
    let nearest = d_left.min(d_right).min(d_top).min(d_bottom);

    (
        if nearest == d_left {
            0.0
        } else if nearest == d_right {
            1.0
        } else {
            pos_x
        },
        if nearest == d_top {
            0.0
        } else if nearest == d_bottom {
            1.0
        } else {
            pos_y
        },
    )
}

/// 画歌词浮窗（按配置位置）并登记这一帧的几何：拖拽与点击拦截都以这份为准，
/// 免得各处各算一遍位置算错。
pub fn draw_page_lyrics_overlay(frame: &mut Frame, app: &mut App, content_area: Rect) {
    let panel = overlay_panel_area(content_area, app.page_lyrics_pos());
    app.set_page_lyrics_layout(content_area, panel);
    draw_page_lyrics_panel(frame, app, panel);
}

pub fn draw_page_lyrics_panel(frame: &mut Frame, app: &App, area: Rect) {
    if area.width < 8 || area.height < PAGE_LYRICS_PANEL_HEIGHT {
        return;
    }

    frame.render_widget(Clear, area);
    frame.render_widget(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(app.theme.color_subtext()))
            .style(base_bg_style(app)),
        area,
    );

    let inner = area.inner(ratatui::layout::Margin {
        horizontal: 1,
        vertical: 1,
    });
    if inner.width == 0 || inner.height < 2 {
        return;
    }

    let mut cache = app.lyric_render_cache.borrow_mut();
    let fallback;
    let lyric_rows = match app
        .playback
        .now_playing
        .as_ref()
        .and_then(|track| track.lyrics.as_ref())
        .filter(|lines| !lines.is_empty())
    {
        Some(lines) => cache.pair(
            lyric_view::LyricSurface::Overlay,
            lines,
            app.lyric_position_ms(),
            app.is_seeking(),
            inner.width,
            &app.theme,
            app.lyric_options(),
        ),
        None => {
            let label = match app.config.language {
                Language::Zh => "暂无歌词",
                Language::En => "No lyrics",
            };
            fallback = ratatui::text::Text::from(vec![
                Line::from(Span::styled(
                    label,
                    Style::default()
                        .fg(app.theme.color_accent2())
                        .add_modifier(Modifier::BOLD),
                )),
                Line::default(),
            ])
            .alignment(Alignment::Center);
            &fallback
        }
    };

    frame.render_widget(lyric_rows, inner);
}

fn base_bg_style(app: &App) -> Style {
    if app.config.transparent_background {
        Style::default()
    } else {
        Style::default().bg(app.theme.color_base())
    }
}

#[cfg(test)]
mod tests;
