use crate::app::App;
use crate::data::config::Language;
use crate::ui::page_lyrics;
use crate::ui::player_bar;
use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use unicode_width::UnicodeWidthStr;

pub fn draw_author(frame: &mut Frame, app: &mut App) {
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

    draw_author_header(frame, app, main[0]);
    draw_author_tiles(frame, app, main[1]);

    if app.config.page_lyrics {
        let panel_area = page_lyrics::overlay_panel_area(content_area);
        page_lyrics::draw_page_lyrics_panel(frame, app, panel_area);
    }
    if app.config.show_hints {
        draw_author_hint(frame, app, hint_area);
    }

    player_bar::draw_collapsed_player_bar(frame, app, rows[1]);
}

fn draw_author_header(frame: &mut Frame, app: &mut App, area: Rect) {
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
        app.author.cover.render(
            frame,
            &mut app.graphics_picker,
            cover_area,
            text_style,
            Some(bg_style),
            draw_ascii,
        );
    }

    let hot_count = app.author.hot_songs.len();
    let album_count = app.author.albums.len();
    let ep_count = app.author.eps.len();
    let single_count = app.author.singles.len();

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
        intro_line_limit(&app.author.description, info_area.width, cover_line_limit);
    let available_extra = info_area.height.saturating_sub(3);
    let spacer_height = u16::from(description_line_limit > 0 && available_extra >= 2);
    let description_height = available_extra
        .saturating_sub(spacer_height)
        .min(description_line_limit);

    let mut cursor_y = info_area.y;

    frame.render_widget(
        Paragraph::new(app.author.title.as_str()).style(
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
            Paragraph::new(app.author.artist.as_str())
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
            Paragraph::new(app.author.description.as_str())
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
                "{} {}  |  {} {}  |  EP {}  |  Single {}",
                match app.config.language {
                    Language::Zh => "热门歌曲",
                    Language::En => "Hot Songs",
                },
                hot_count,
                match app.config.language {
                    Language::Zh => "专辑",
                    Language::En => "Albums",
                },
                album_count,
                ep_count,
                single_count
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

fn draw_author_tiles(frame: &mut Frame, app: &mut App, area: Rect) {
    let margin = ratatui::layout::Margin {
        horizontal: 1,
        vertical: 1,
    };
    let inner = area.inner(margin);
    if inner.width < 14 || inner.height < 8 {
        return;
    }

    let tile_h = 12_u16.min(inner.height.saturating_sub(1)).max(6);
    let tile_w = tile_h.saturating_mul(2).saturating_add(4);
    let col_step = tile_w.saturating_add(2);
    let row_step = tile_h.saturating_add(1);
    let columns = usize::from((inner.width / col_step).max(1));
    app.author.set_columns(columns);

    let visible_rows = usize::from((inner.height / row_step).max(1));
    app.author.set_visible_rows(visible_rows);
    let row_offset = app.author.effective_scroll_row_offset();

    for index in 0..app.author.tiles.len() {
        let row = index / columns;
        if row < row_offset {
            continue;
        }
        let visual_row = row - row_offset;
        if visual_row >= visible_rows {
            break;
        }
        let col = index % columns;
        let x = inner.x + (col as u16) * col_step;
        let y = inner.y + (visual_row as u16) * row_step;
        if x >= inner.x + inner.width || y >= inner.y + inner.height {
            continue;
        }

        let rect = Rect {
            x,
            y,
            width: tile_w.min(inner.x + inner.width - x),
            height: tile_h.min(inner.y + inner.height - y),
        };

        app.push_author_tile_hit(
            crate::app::HitRect {
                x: rect.x,
                y: rect.y,
                width: rect.width,
                height: rect.height,
            },
            index,
        );

        let focused = index == app.author.focused_idx;
        let tile_bg = if focused {
            app.theme.color_surface()
        } else {
            app.theme.color_base()
        };
        let tile_style = if app.config.transparent_background {
            Style::default()
        } else {
            Style::default().bg(tile_bg)
        };
        let border_style = if focused {
            Style::default()
                .fg(app.theme.color_accent())
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(app.theme.color_surface())
        };

        frame.render_widget(
            Block::default()
                .borders(Borders::ALL)
                .border_style(border_style)
                .style(tile_style),
            rect,
        );

        let inner_rect = rect.inner(ratatui::layout::Margin {
            horizontal: 1,
            vertical: 1,
        });
        if inner_rect.width < 2 || inner_rect.height < 2 {
            continue;
        }

        let text_rows = if inner_rect.height >= 4 { 2 } else { 1 };
        let cover_height = inner_rect.height.saturating_sub(text_rows);
        let cover_rect = Rect {
            x: inner_rect.x,
            y: inner_rect.y,
            width: inner_rect.width,
            height: cover_height,
        };
        let text_rect = Rect {
            x: inner_rect.x,
            y: inner_rect.y + cover_height,
            width: inner_rect.width,
            height: text_rows,
        };

        if !cover_rect.is_empty() {
            let draw_ascii = app.draw_ascii();
            let text_style = if focused {
                Style::default().fg(app.theme.color_accent2())
            } else {
                Style::default().fg(app.theme.color_text())
            };
            app.author.tiles[index].cover.render(
                frame,
                &mut app.graphics_picker,
                cover_rect,
                text_style,
                None,
                draw_ascii,
            );
        }

        let (title, subtitle) = {
            let tile = &app.author.tiles[index];
            (tile.title.clone(), tile.subtitle.clone())
        };

        let title_style = if focused {
            Style::default()
                .fg(app.theme.color_accent2())
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(app.theme.color_text())
        };
        let subtitle_style = Style::default().fg(app.theme.color_subtext());

        let mut lines = vec![Line::from(Span::styled(title, title_style))];
        if text_rows > 1 {
            lines.push(Line::from(Span::styled(subtitle, subtitle_style)));
        }

        frame.render_widget(
            Paragraph::new(lines)
                .wrap(Wrap { trim: true })
                .alignment(Alignment::Center),
            text_rect,
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

fn draw_author_hint(frame: &mut Frame, app: &App, area: Rect) {
    if area.width == 0 || area.height == 0 {
        return;
    }

    let text = match app.config.language {
        Language::Zh => format!(
            "{} 搜索  Enter 进入歌单页  Esc 返回搜索  {} 全屏",
            app.config.keybind_search_box, app.config.keybind_fullscreen
        ),
        Language::En => format!(
            "{} Search  Enter open playlist page  Esc back  {} Fullscreen",
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
