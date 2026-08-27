use crate::app::App;
use crate::data::config::Language;
use crate::ui::page_lyrics;
use crate::ui::player_bar;
use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

pub fn draw_playlist(frame: &mut Frame, app: &mut App) {
    app.clear_player_bar_hits();
    app.clear_content_hits();

    let size = frame.area();
    frame.render_widget(Block::default().style(base_bg_style(app)), size);

    if size.width < 40 || size.height < 14 {
        frame.render_widget(
            Paragraph::new(match app.config.language {
                Language::Zh => "终端窗口过小",
                Language::En => "Terminal too small",
            })
            .style(Style::default().fg(app.theme.color_subtext())),
            size,
        );
        return;
    }

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(1),
            Constraint::Length(player_bar::PLAYER_BAR_HEIGHT),
        ])
        .split(size);

    let (content_area, hint_area) = if app.config.show_hints {
        let split = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(1), Constraint::Length(1)])
            .split(rows[0]);
        (split[0], split[1])
    } else {
        (rows[0], Rect::default())
    };

    // 封面宽度上限 26 格（见 draw_*_header 的 cols[0] 约束），折算方形边长上限 13 行；
    // header 再上下内缩 1 行，故顶部区域超过 15 行只会产生空白。此处封顶，
    // 多出的高度全部让给下方列表。
    const HEADER_MAX_HEIGHT: u16 = 15;
    let header_height = (content_area.height * 34 / 100).min(HEADER_MAX_HEIGHT);
    let main = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(header_height), Constraint::Min(1)])
        .split(content_area);

    draw_playlist_header(frame, app, main[0]);
    draw_playlist_tracks(frame, app, main[1]);
    if app.config.page_lyrics {
        let panel_area = page_lyrics::overlay_panel_area(content_area);
        page_lyrics::draw_page_lyrics_panel(frame, app, panel_area);
    }
    if app.config.show_hints {
        draw_playlist_hint(frame, app, hint_area);
    }
    player_bar::draw_collapsed_player_bar(frame, app, rows[1]);
}

fn draw_playlist_header(frame: &mut Frame, app: &mut App, area: Rect) {
    let inner = area.inner(ratatui::layout::Margin {
        horizontal: 1,
        vertical: 1,
    });
    if inner.width < 6 || inner.height < 3 {
        return;
    }

    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Length((inner.height * 2).min(26)),
            Constraint::Min(1),
        ])
        .split(inner);

    let mut cover_line_limit = inner.height;
    let cover_area = centered_visual_square_block(cols[0]);
    if !cover_area.is_empty() {
        cover_line_limit = cover_area.height;
        let bg_style = surface_bg_style(app);
        let draw_ascii = app.draw_ascii();
        let text_style = Style::default().fg(app.theme.color_text());
        app.playlist.cover.render(
            frame,
            &mut app.graphics_picker,
            cover_area,
            text_style,
            Some(bg_style),
            draw_ascii,
        );
    }

    let info_area = cols[1].inner(ratatui::layout::Margin {
        horizontal: 1,
        vertical: 0,
    });
    // 文字内容与封面同高、顶端对齐。
    let info_area = if cover_area.is_empty() {
        info_area
    } else {
        Rect {
            x: info_area.x,
            y: cover_area.y,
            width: info_area.width,
            height: cover_area.height,
        }
    };
    if info_area.width == 0 || info_area.height == 0 {
        return;
    }

    let description_line_limit =
        intro_line_limit(&app.playlist.description, info_area.width, cover_line_limit);
    let available_extra = info_area.height.saturating_sub(3);
    let spacer_height = u16::from(description_line_limit > 0 && available_extra >= 2);
    let description_height = available_extra
        .saturating_sub(spacer_height)
        .min(description_line_limit);

    let mut cursor_y = info_area.y;

    frame.render_widget(
        Paragraph::new(app.playlist.title.as_str()).style(
            Style::default()
                .fg(app.theme.color_text())
                .add_modifier(Modifier::BOLD),
        ),
        Rect {
            x: info_area.x,
            y: cursor_y,
            width: info_area.width,
            height: 1,
        },
    );
    cursor_y = cursor_y.saturating_add(1);

    if cursor_y < info_area.y + info_area.height {
        frame.render_widget(
            Paragraph::new(app.playlist.artist.as_str())
                .style(Style::default().fg(app.theme.color_subtext())),
            Rect {
                x: info_area.x,
                y: cursor_y,
                width: info_area.width,
                height: 1,
            },
        );
        cursor_y = cursor_y.saturating_add(1);
    }

    if spacer_height > 0 && cursor_y < info_area.y + info_area.height {
        cursor_y = cursor_y.saturating_add(1);
    }

    if description_height > 0 && cursor_y < info_area.y + info_area.height {
        frame.render_widget(
            Paragraph::new(app.playlist.description.as_str())
                .style(Style::default().fg(app.theme.color_text()))
                .wrap(Wrap { trim: true }),
            Rect {
                x: info_area.x,
                y: cursor_y,
                width: info_area.width,
                height: description_height,
            },
        );
        cursor_y = cursor_y.saturating_add(description_height);
    }

    if cursor_y < info_area.y + info_area.height {
        frame.render_widget(
            Paragraph::new(format!(
                "{} {} {}",
                match app.config.language {
                    Language::Zh => "共",
                    Language::En => "Total",
                },
                app.playlist.tracks.len(),
                match app.config.language {
                    Language::Zh => "首",
                    Language::En => "tracks",
                }
            ))
            .style(Style::default().fg(app.theme.color_subtext())),
            Rect {
                x: info_area.x,
                y: cursor_y,
                width: info_area.width,
                height: 1,
            },
        );
    }
}

fn draw_playlist_tracks(frame: &mut Frame, app: &mut App, area: Rect) {
    let inner = Rect {
        x: area.x.saturating_add(1),
        y: area.y.saturating_add(1),
        width: area.width.saturating_sub(2),
        height: area.height.saturating_sub(1),
    };
    if inner.width < 8 || inner.height < 3 {
        return;
    }

    frame.render_widget(
        Block::default()
            .borders(Borders::TOP)
            .border_style(Style::default().fg(app.theme.color_surface())),
        area,
    );

    let visible = inner.height as usize;
    app.playlist.set_visible_rows(visible);
    let offset = app.playlist.effective_scroll_offset();

    for (line_idx, track_idx) in (offset..app.playlist.tracks.len())
        .take(visible)
        .enumerate()
    {
        let y = inner.y + line_idx as u16;
        let row = Rect {
            x: inner.x,
            y,
            width: inner.width,
            height: 1,
        };

        app.push_playlist_track_hit(
            crate::app::HitRect {
                x: row.x,
                y: row.y,
                width: row.width,
                height: row.height,
            },
            track_idx,
        );

        let track = &app.playlist.tracks[track_idx];
        let focused = track_idx == app.playlist.focused_idx;
        let is_now_playing = app.is_now_playing_song(track.id.as_deref());
        let zebra_bg = if app.config.transparent_background {
            None
        } else if track_idx % 2 == 0 {
            Some(app.theme.color_base())
        } else {
            Some(app.theme.color_surface())
        };

        let style = if focused {
            Style::default()
                .fg(app.theme.color_base())
                .bg(app.theme.color_accent())
                .add_modifier(Modifier::BOLD)
        } else {
            let mut style = Style::default().fg(if is_now_playing {
                app.theme.color_accent3()
            } else {
                app.theme.color_text()
            });
            if is_now_playing {
                style = style.add_modifier(Modifier::BOLD);
            }
            if let Some(bg) = zebra_bg {
                style = style.bg(bg);
            }
            style
        };

        let index_label = format!("{:>2}.", track_idx + 1);
        let left = format!("{} - {}", track.title, track.artist);
        let duration = track.duration.clone();
        let reserved = display_width(&index_label) + 1 + display_width(&duration) + 1;
        let max_left = usize::from(row.width).saturating_sub(reserved);
        let clipped_left = clip_to_display_width(&left, max_left);
        let used = display_width(&index_label)
            + 1
            + display_width(&clipped_left)
            + display_width(&duration);
        let space = usize::from(row.width).saturating_sub(used).max(1);

        let index_style = if focused {
            style
        } else if is_now_playing {
            style.fg(app.theme.color_accent3())
        } else {
            style.fg(app.theme.color_subtext())
        };
        let duration_style = if focused {
            style
        } else if is_now_playing {
            style.fg(app.theme.color_accent3())
        } else {
            style.fg(app.theme.color_subtext())
        };

        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(index_label, index_style),
                Span::styled(" ", style),
                Span::styled(clipped_left, style),
                Span::styled(" ".repeat(space), style),
                Span::styled(duration, duration_style),
            ])),
            row,
        );
    }
}

fn centered_visual_square_block(area: Rect) -> Rect {
    if area.width < 2 || area.height < 1 {
        return Rect::default();
    }

    let side = area.height.min(area.width / 2);
    if side == 0 {
        return Rect::default();
    }

    let width = side.saturating_mul(2);
    let height = side;
    Rect {
        x: area.x + area.width.saturating_sub(width) / 2,
        y: area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    }
}

fn base_bg_style(app: &App) -> Style {
    if app.config.transparent_background {
        Style::default()
    } else {
        Style::default().bg(app.theme.color_base())
    }
}

fn surface_bg_style(app: &App) -> Style {
    if app.config.transparent_background {
        Style::default()
    } else {
        Style::default().bg(app.theme.color_surface())
    }
}

fn display_width(text: &str) -> usize {
    UnicodeWidthStr::width(text)
}

fn intro_line_limit(text: &str, width: u16, cover_line_limit: u16) -> u16 {
    if width == 0 || cover_line_limit == 0 || text.trim().is_empty() {
        return 0;
    }

    wrapped_line_count(text, width)
        .min(20)
        .min(usize::from(cover_line_limit)) as u16
}

fn wrapped_line_count(text: &str, width: u16) -> usize {
    if width == 0 || text.trim().is_empty() {
        return 0;
    }

    let max_width = usize::from(width);
    text.split('\n')
        .map(|line| display_width(line).max(1).div_ceil(max_width))
        .sum()
}

fn clip_to_display_width(text: &str, max_width: usize) -> String {
    if max_width == 0 {
        return String::new();
    }

    let mut out = String::new();
    let mut used = 0;
    for ch in text.chars() {
        let w = ch.width().unwrap_or(0);
        if used + w > max_width {
            break;
        }
        out.push(ch);
        used += w;
    }
    out
}

fn draw_playlist_hint(frame: &mut Frame, app: &App, area: Rect) {
    if area.width == 0 || area.height == 0 {
        return;
    }

    let text = match app.config.language {
        Language::Zh => format!(
            "Enter 播放/打开专辑  Esc 返回  {} 搜索  {} 全屏",
            app.config.keybind_search_box, app.config.keybind_fullscreen
        ),
        Language::En => format!(
            "Enter play/open album  Esc back  {} Search  {} Fullscreen",
            app.config.keybind_search_box, app.config.keybind_fullscreen
        ),
    };

    frame.render_widget(
        Paragraph::new(text)
            .style(Style::default().fg(app.theme.color_subtext()))
            .alignment(Alignment::Left),
        area,
    );
}
