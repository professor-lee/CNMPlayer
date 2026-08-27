use crate::app::{App, HomeSidebarHit, HomeSidebarSection};
use crate::data::config::Language;
use crate::ui::page_lyrics;
use crate::ui::player_bar;
use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

pub fn draw_home(frame: &mut Frame, app: &mut App) {
    app.clear_player_bar_hits();
    app.clear_content_hits();

    let size = frame.area();
    frame.render_widget(Block::default().style(base_bg_style(app)), size);

    if size.width < 32 || size.height < 12 {
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

    draw_tiles(frame, app, content_area);
    if app.config.page_lyrics {
        let panel_area = page_lyrics::overlay_panel_area(content_area);
        page_lyrics::draw_page_lyrics_panel(frame, app, panel_area);
    }
    if app.config.show_hints {
        draw_home_hint(frame, app, hint_area);
    }
    if app.home_sidebar.is_visible() {
        draw_home_sidebar(frame, app, rows[0]);
    }

    player_bar::draw_collapsed_player_bar(frame, app, rows[1]);
}

fn draw_tiles(frame: &mut Frame, app: &mut App, area: Rect) {
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
    app.home.set_columns(columns);

    let visible_rows = usize::from((inner.height / row_step).max(1));
    app.home.set_visible_rows(visible_rows);
    let row_offset = app.home.effective_scroll_row_offset();

    for index in 0..app.home.tiles.len() {
        let virtual_index = home_real_to_virtual_index(index, columns);
        let row = virtual_index / columns;
        if row < row_offset {
            continue;
        }
        let visual_row = row - row_offset;
        if visual_row >= visible_rows {
            continue;
        }
        let col = virtual_index % columns;
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

        app.push_home_tile_hit(
            crate::app::HitRect {
                x: rect.x,
                y: rect.y,
                width: rect.width,
                height: rect.height,
            },
            index,
        );

        let focused = index == app.home.focused_idx;
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
            app.home.tiles[index].cover.render(
                frame,
                &mut app.graphics_picker,
                cover_rect,
                text_style,
                None,
                draw_ascii,
            );
        }

        let (title, subtitle) = {
            let tile = &app.home.tiles[index];
            (tile.title.as_str(), tile.subtitle.as_str())
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

        let content = Paragraph::new(lines)
            .wrap(Wrap { trim: true })
            .alignment(Alignment::Center);

        frame.render_widget(content, text_rect);
    }
}

fn home_real_to_virtual_index(index: usize, columns: usize) -> usize {
    let cols = columns.max(1);
    if cols <= 3 || index < 3 {
        index
    } else {
        index.saturating_add(cols - 3)
    }
}

fn draw_home_hint(frame: &mut Frame, app: &App, area: Rect) {
    if area.width == 0 || area.height == 0 {
        return;
    }

    let text = match app.config.language {
        Language::Zh => format!(
            "{} 搜索  {} 设置  {} 侧边栏  {} 全屏  {} 退出",
            app.config.keybind_search_box,
            app.config.keybind_settings,
            app.config.keybind_sidebar,
            app.config.keybind_fullscreen,
            app.config.keybind_quit
        ),
        Language::En => format!(
            "{} Search  {} Settings  {} Sidebar  {} Fullscreen  {} Quit",
            app.config.keybind_search_box,
            app.config.keybind_settings,
            app.config.keybind_sidebar,
            app.config.keybind_fullscreen,
            app.config.keybind_quit
        ),
    };

    frame.render_widget(
        Paragraph::new(text)
            .style(Style::default().fg(app.theme.color_subtext()))
            .alignment(Alignment::Left),
        area,
    );
}

fn draw_home_sidebar(frame: &mut Frame, app: &mut App, area: Rect) {
    if area.width < 20 || area.height < 8 {
        return;
    }

    let max_width = (area.width / 3).max(24).min(area.width);
    app.set_home_sidebar_anim_span_cells(max_width);
    let progress = app.home_sidebar.anim_progress.clamp(0.0, 1.0);
    let width = ((max_width as f32) * progress).round() as u16;
    if width < 12 {
        return;
    }

    let sidebar = Rect {
        x: area.x,
        y: area.y,
        width,
        height: area.height,
    };

    frame.render_widget(Clear, sidebar);

    app.set_home_sidebar_panel_hit(Some(crate::app::HitRect {
        x: sidebar.x,
        y: sidebar.y,
        width: sidebar.width,
        height: sidebar.height,
    }));

    let title = match app.config.language {
        Language::Zh => "主页侧边栏",
        Language::En => "Home Sidebar",
    };

    let panel_style = Style::default()
        .fg(app.theme.color_subtext())
        .bg(app.theme.color_surface());

    frame.render_widget(
        Block::default()
            .borders(Borders::ALL)
            .title(Line::from(Span::styled(
                title,
                Style::default().fg(app.theme.color_subtext()),
            )))
            .border_style(Style::default().fg(app.theme.color_subtext()))
            .style(panel_style),
        sidebar,
    );

    let inner = sidebar.inner(ratatui::layout::Margin {
        horizontal: 1,
        vertical: 1,
    });
    if inner.width < 8 || inner.height < 4 {
        return;
    }

    let header_height = if inner.height >= 5 { 2 } else { 1 };
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(header_height), Constraint::Min(1)])
        .split(inner);

    let user_name = if app.home_sidebar.user_name.trim().is_empty() {
        match app.config.language {
            Language::Zh => "未识别用户".to_string(),
            Language::En => "Unknown User".to_string(),
        }
    } else {
        app.home_sidebar.user_name.clone()
    };

    let status = if app.home_sidebar.loading {
        match app.config.language {
            Language::Zh => "正在同步歌单...".to_string(),
            Language::En => "Syncing playlists...".to_string(),
        }
    } else if app.home_sidebar.status_line.trim().is_empty() {
        match app.config.language {
            Language::Zh => "Ctrl+上下切换分区 上下切换歌单 Enter进入 Esc收起".to_string(),
            Language::En => {
                "Ctrl+Up/Down switch section, Up/Down switch playlist, Enter open, Esc collapse"
                    .to_string()
            }
        }
    } else {
        app.home_sidebar.status_line.clone()
    };

    let mut header_lines = vec![Line::from(Span::styled(
        user_name,
        Style::default()
            .fg(app.theme.color_text())
            .add_modifier(Modifier::BOLD),
    ))];
    if header_height > 1 {
        header_lines.push(Line::from(Span::styled(
            status,
            Style::default().fg(app.theme.color_subtext()),
        )));
    }
    frame.render_widget(
        Paragraph::new(header_lines).wrap(Wrap { trim: true }),
        chunks[0],
    );

    let sections = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(chunks[1]);

    draw_home_sidebar_section(
        frame,
        app,
        sections[0],
        match app.config.language {
            Language::Zh => "用户创建的歌单",
            Language::En => "Created Playlists",
        },
        HomeSidebarSection::Created,
    );

    draw_home_sidebar_section(
        frame,
        app,
        sections[1],
        match app.config.language {
            Language::Zh => "用户收藏的歌单",
            Language::En => "Collected Playlists",
        },
        HomeSidebarSection::Collected,
    );
}

fn draw_home_sidebar_section(
    frame: &mut Frame,
    app: &mut App,
    area: Rect,
    title: &str,
    section: HomeSidebarSection,
) {
    if area.width < 6 || area.height < 3 {
        return;
    }

    let section_focused = app.home_sidebar.expanded && app.home_sidebar.focused_section == section;
    let section_title_style = if section_focused {
        Style::default()
            .fg(app.theme.color_accent2())
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(app.theme.color_subtext())
    };

    frame.render_widget(
        Block::default().style(Style::default().bg(app.theme.color_surface())),
        area,
    );

    let title_line_area = Rect {
        x: area.x.saturating_sub(1),
        y: area.y,
        width: area.width.saturating_add(2),
        height: 1,
    };
    if title_line_area.width > 2 {
        let line_width = usize::from(title_line_area.width);
        let title_max = line_width.saturating_sub(2);
        let clipped_title = clip_to_display_width(title, title_max.max(1));
        let used = 2 + display_width(&clipped_title);
        let dash_count = line_width.saturating_sub(used);
        let connector_style = Style::default().fg(app.theme.color_subtext());

        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled("├", connector_style),
                Span::styled(clipped_title, section_title_style),
                Span::styled("─".repeat(dash_count), connector_style),
                Span::styled("┤", connector_style),
            ]))
            .style(Style::default().bg(app.theme.color_surface())),
            title_line_area,
        );
    }

    let inner = Rect {
        x: area.x.saturating_add(1),
        y: area.y.saturating_add(1),
        width: area.width.saturating_sub(2),
        height: area.height.saturating_sub(1),
    };
    if inner.width == 0 || inner.height == 0 {
        return;
    }

    // Only the list length is needed for the mutations below, so they run
    // before we borrow the playlist items - keeping the item borrow disjoint
    // from the `&mut app` used for hit collection and styles.
    let total = match section {
        HomeSidebarSection::Created => app.home_sidebar.created_playlists.len(),
        HomeSidebarSection::Collected => app.home_sidebar.collected_playlists.len(),
    };
    let max_rows = inner.height as usize;

    let mut start = 0usize;
    if total > 0 {
        let focus_idx = if section_focused {
            app.home_sidebar.focused_index.min(total.saturating_sub(1))
        } else {
            0
        };
        start = if total <= max_rows {
            0
        } else {
            app.home_sidebar
                .section_scroll_offset(section)
                .min(total.saturating_sub(max_rows))
        };

        if total > max_rows {
            if focus_idx < start {
                start = focus_idx;
            } else if focus_idx >= start.saturating_add(max_rows) {
                start = focus_idx + 1 - max_rows;
            }
            start = start.min(total.saturating_sub(max_rows));
        }
        app.home_sidebar.set_section_scroll_offset(section, start);
    }

    let mut lines = Vec::new();
    let mut hits = Vec::new();
    if total == 0 {
        lines.push(Line::from(Span::styled(
            match app.config.language {
                Language::Zh => "暂无歌单",
                Language::En => "No playlists",
            },
            Style::default().fg(app.theme.color_subtext()),
        )));
    } else {
        let items = match section {
            HomeSidebarSection::Created => &app.home_sidebar.created_playlists,
            HomeSidebarSection::Collected => &app.home_sidebar.collected_playlists,
        };

        for (visual_idx, item) in items.iter().skip(start).take(max_rows).enumerate() {
            let idx = start + visual_idx;
            let left = if item.creator.trim().is_empty() {
                format!("{:02}. {}", idx + 1, item.title)
            } else {
                format!("{:02}. {} - {}", idx + 1, item.title, item.creator)
            };
            let right = match app.config.language {
                Language::Zh => format!("{}首", item.track_count),
                Language::En => format!("{}", item.track_count),
            };

            let reserved = display_width(&right) + 1;
            let left_max = usize::from(inner.width).saturating_sub(reserved);
            let clipped_left = clip_to_display_width(&left, left_max);
            let used = display_width(&clipped_left) + display_width(&right);
            let spaces = usize::from(inner.width).saturating_sub(used).max(1);
            let is_focused = section_focused && idx == app.home_sidebar.focused_index;

            hits.push((
                crate::app::HitRect {
                    x: inner.x,
                    y: inner.y + visual_idx as u16,
                    width: inner.width,
                    height: 1,
                },
                HomeSidebarHit {
                    section,
                    index: idx,
                },
            ));

            let text_style = if is_focused {
                Style::default()
                    .fg(app.theme.color_base())
                    .bg(app.theme.color_accent())
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(app.theme.color_text())
            };
            let right_style = if is_focused {
                Style::default()
                    .fg(app.theme.color_base())
                    .bg(app.theme.color_accent())
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(app.theme.color_subtext())
            };

            lines.push(Line::from(vec![
                Span::styled(clipped_left, text_style),
                Span::styled(" ".repeat(spaces), text_style),
                Span::styled(right, right_style),
            ]));
        }
    }

    // Apply hit rects after the item borrow ends.
    for (rect, hit) in hits {
        app.push_home_sidebar_playlist_hit(rect, hit);
    }

    frame.render_widget(
        Paragraph::new(lines).style(Style::default().bg(app.theme.color_surface())),
        inner,
    );
}

fn display_width(text: &str) -> usize {
    UnicodeWidthStr::width(text)
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

fn base_bg_style(app: &App) -> Style {
    if app.config.transparent_background {
        Style::default()
    } else {
        Style::default().bg(app.theme.color_base())
    }
}
