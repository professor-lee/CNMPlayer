use crate::app::{ARTIST_CARD_ROWS, App, SearchItemKind, SearchState};
use crate::data::config::Language;
use crate::ui::page_lyrics;
use crate::ui::player_bar;
use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::symbols::border::PLAIN;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use std::ops::Range;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// 卡片内布局：行 0 上边框、行 1 名字 + 头像上半、行 2 标签 + 头像下半、最后一行下边框。
const ARTIST_CARD_NAME_ROW: u16 = 1;
const ARTIST_CARD_TAG_ROW: u16 = 2;
/// 卡片头像区：2 行高、4 列宽，约等于方形。
const ARTIST_CARD_AVATAR_HEIGHT: u16 = 2;
const ARTIST_CARD_AVATAR_WIDTH: u16 = 4;
/// 卡片内头像起始列（边框 + 2 空格）与文字起始列（头像后再 3 空格）。
const ARTIST_CARD_AVATAR_X: u16 = 3;
const ARTIST_CARD_TEXT_X: u16 = 10;
/// 面板比这还窄 / 矮就用单行样式，避免卡片被压扁。
const ARTIST_CARD_MIN_WIDTH: u16 = 24;

pub fn draw_search(frame: &mut Frame, app: &mut App) {
    app.clear_player_bar_hits();
    app.clear_content_hits();

    let size = frame.area();
    frame.render_widget(Block::default().style(base_bg_style(app)), size);

    if !app.config.small_window_display && (size.width < 42 || size.height < 14) {
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

    draw_result_panel(frame, app, rows[0]);
    if app.config.page_lyrics {
        page_lyrics::draw_page_lyrics_overlay(frame, app, rows[0]);
    }

    player_bar::draw_collapsed_player_bar(frame, app, rows[1]);
}

fn draw_result_panel(frame: &mut Frame, app: &mut App, area: Rect) {
    let inner = area.inner(ratatui::layout::Margin {
        horizontal: 1,
        vertical: 1,
    });
    if inner.width < 10 || inner.height < 2 {
        return;
    }

    let list_height = if app.config.show_hints {
        inner.height.saturating_sub(1)
    } else {
        inner.height
    };
    if list_height == 0 {
        return;
    }

    let list_area = Rect {
        x: inner.x,
        y: inner.y,
        width: inner.width,
        height: list_height,
    };
    let hint_rect = Rect {
        x: inner.x,
        y: inner.y + list_height,
        width: inner.width,
        height: 1,
    };

    let card = list_area.width >= ARTIST_CARD_MIN_WIDTH
        && usize::from(list_area.height) >= ARTIST_CARD_ROWS;
    app.search.set_viewport(usize::from(list_area.height), card);

    // 行内图标：每帧刷新一次 memo（列表代/任务版本不变时只做一次 u64 比较）。
    app.refresh_search_downloads();

    // 视口按行定位：条目高度不一，行滚动才能让顶部与底部同步移动。
    let top_row = app.search.effective_scroll_rows();
    let bottom_row = top_row.saturating_add(usize::from(list_area.height));

    for item_idx in 0..app.search.results.len() {
        let start_row = app.search.item_start_row(item_idx);
        if start_row >= bottom_row {
            break;
        }
        let end_row = app.search.item_end_row(item_idx);
        if end_row <= top_row {
            continue;
        }

        let kind = app.search.results[item_idx].kind;
        let divider = app.search.divider_rows(item_idx);
        let item_top_row = start_row + divider;

        // 分区线：单行，整行落在视口内才画（它不产生命中区）。
        if divider == 1 && start_row >= top_row {
            draw_search_divider(
                frame,
                app,
                Rect {
                    x: list_area.x,
                    y: list_area.y + (start_row - top_row) as u16,
                    width: list_area.width,
                    height: 1,
                },
            );
        }

        let visible_top = item_top_row.max(top_row);
        let visible_bottom = end_row.min(bottom_row);
        if visible_top >= visible_bottom {
            continue;
        }

        let focused = item_idx == app.search.focused_idx;
        let visible_rect = Rect {
            x: list_area.x,
            y: list_area.y + (visible_top - top_row) as u16,
            width: list_area.width,
            height: (visible_bottom - visible_top) as u16,
        };
        // 命中区覆盖条目可见部分：被裁切的卡片仍能点到露出来的那几行。
        app.push_search_item_hit(
            crate::app::HitRect {
                x: visible_rect.x,
                y: visible_rect.y,
                width: visible_rect.width,
                height: visible_rect.height,
            },
            item_idx,
        );

        if card && kind == SearchItemKind::Artist {
            // 只画与视口相交的那几行：卡内行号 = 列表行 − 该条目首行。
            render_artist_card(
                frame,
                app,
                visible_rect,
                (visible_top - item_top_row) as u16..(visible_bottom - item_top_row) as u16,
                item_idx,
                focused,
            );
        } else {
            let ordinal = search_item_ordinal(&app.search, item_idx);
            // 单曲行才有图标；状态从 memo 表里取（每帧只刷新一次）。
            let download_state = app.search_download_state_at(item_idx);
            render_search_row(
                frame,
                app,
                visible_rect,
                item_idx,
                ordinal,
                focused,
                download_state,
            );
        }
    }

    if app.config.show_hints && list_height < inner.height {
        let hint = match app.config.language {
            Language::Zh => {
                "Enter 打开/播放  Esc 返回  无后缀=作者/歌单/单曲  @single/@album/@author/@list 限定类型  @author 空关键词=关注作者"
            }
            Language::En => {
                "Enter open/play  Esc back  no suffix = artists/playlists/songs  @single/@album/@author/@list narrows  bare @author = followed"
            }
        };
        frame.render_widget(
            Paragraph::new(hint).style(Style::default().fg(app.theme.color_subtext())),
            hint_rect,
        );
    }
}

/// 绘制作者卡片：只写卡内 `visible` 行（`row` 是可见部分在屏幕上的矩形，
/// `row.y` 对应卡内行 `visible.start`），被裁掉的行完全不写入——
/// 于是顶部/底部各滚 1 行就只裁 1 行，且不需要事后擦除。
fn render_artist_card(
    frame: &mut Frame,
    app: &mut App,
    row: Rect,
    visible: Range<u16>,
    item_idx: usize,
    focused: bool,
) {
    if row.is_empty() || visible.is_empty() {
        return;
    }

    let name = app.search.results[item_idx].left_label.clone();
    let tag = app.search.results[item_idx]
        .kind
        .tag()
        .unwrap_or_default()
        .to_string();
    let border_style = if focused {
        Style::default()
            .fg(app.theme.color_accent())
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(app.theme.color_surface())
    };
    let card_style = base_bg_style(app);
    let name_style = if focused {
        Style::default()
            .fg(app.theme.color_accent2())
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(app.theme.color_text())
    };
    let tag_style = if focused {
        Style::default().fg(app.theme.color_accent())
    } else {
        Style::default().fg(app.theme.color_subtext())
    };

    frame.render_widget(Block::default().style(card_style), row);

    // 边框按行画：上/下边框只在真上/真下那一行，内容行只画左右竖边。
    // Block 仅在相邻两条边同时设置时才画角，所以角另外补。
    let last_row = ARTIST_CARD_ROWS as u16 - 1;
    for (offset, card_row) in visible.clone().enumerate() {
        let line = Rect {
            x: row.x,
            y: row.y + offset as u16,
            width: row.width,
            height: 1,
        };
        let (borders, corners) = match card_row {
            0 => (Borders::TOP, Some((PLAIN.top_left, PLAIN.top_right))),
            r if r == last_row => (
                Borders::BOTTOM,
                Some((PLAIN.bottom_left, PLAIN.bottom_right)),
            ),
            _ => (Borders::LEFT | Borders::RIGHT, None),
        };
        frame.render_widget(
            Block::default()
                .borders(borders)
                .border_style(border_style)
                .style(card_style),
            line,
        );
        if let Some((left, right)) = corners {
            let buf = frame.buffer_mut();
            if let Some(cell) = buf.cell_mut((line.x, line.y)) {
                cell.set_symbol(left);
                cell.set_style(border_style);
            }
            if line.width > 1
                && let Some(cell) = buf.cell_mut((line.x + line.width - 1, line.y))
            {
                cell.set_symbol(right);
                cell.set_style(border_style);
            }
        }
    }

    let text_width = row
        .width
        .saturating_sub(ARTIST_CARD_TEXT_X.saturating_add(2));
    if text_width > 0 {
        let text_x = row.x.saturating_add(ARTIST_CARD_TEXT_X);
        let text_line = |card_row: u16| Rect {
            x: text_x,
            y: row.y + (card_row - visible.start),
            width: text_width,
            height: 1,
        };
        if visible.contains(&ARTIST_CARD_NAME_ROW) {
            frame.render_widget(
                Paragraph::new(clip_to_display_width(&name, usize::from(text_width)))
                    .style(name_style),
                text_line(ARTIST_CARD_NAME_ROW),
            );
        }
        if visible.contains(&ARTIST_CARD_TAG_ROW) {
            frame.render_widget(
                Paragraph::new(tag)
                    .style(tag_style)
                    .alignment(Alignment::Right),
                text_line(ARTIST_CARD_TAG_ROW),
            );
        }
    }

    // 头像：只渲染可见那几行，源图按可见比例裁（不是把整图压进子矩形）。
    let avatar_first = ARTIST_CARD_NAME_ROW;
    let avatar_last = avatar_first + ARTIST_CARD_AVATAR_HEIGHT;
    let visible_start = visible.start.max(avatar_first);
    let visible_end = visible.end.min(avatar_last);
    if visible_start < visible_end {
        let avatar_area = Rect {
            x: row.x.saturating_add(ARTIST_CARD_AVATAR_X),
            y: row.y + (visible_start - visible.start),
            width: ARTIST_CARD_AVATAR_WIDTH.min(row.width),
            height: visible_end - visible_start,
        };
        let draw_ascii = app.draw_ascii();
        let text_style = Style::default().fg(app.theme.color_text());
        app.search.results[item_idx].cover.render_rows(
            frame,
            &mut app.graphics_picker,
            avatar_area,
            ARTIST_CARD_AVATAR_HEIGHT,
            (visible_start - avatar_first)..(visible_end - avatar_first),
            text_style,
            None,
            draw_ascii,
        );
    }
}

/// 分区内序号（同一 kind 内的第几条）。带后缀搜索只有一种 kind，等价于旧版的行号。
fn search_item_ordinal(state: &SearchState, index: usize) -> usize {
    let kind = state.results[index].kind;
    state.results[..index]
        .iter()
        .filter(|item| item.kind == kind)
        .count()
        + 1
}

fn draw_search_divider(frame: &mut Frame, app: &App, row: Rect) {
    if row.width == 0 || row.height == 0 {
        return;
    }

    frame.render_widget(
        Paragraph::new("─".repeat(usize::from(row.width)))
            .style(Style::default().fg(app.theme.color_subtext())),
        row,
    );
}

fn render_search_row(
    frame: &mut Frame,
    app: &mut App,
    row: Rect,
    item_idx: usize,
    ordinal: usize,
    focused: bool,
    download_state: Option<crate::app::download::DownloadState>,
) {
    let song_id = app.search.results[item_idx].song_id.clone();
    let is_now_playing = app.is_now_playing_song(song_id.as_deref());
    let zebra_bg = if app.config.transparent_background {
        None
    } else if item_idx.is_multiple_of(2) {
        Some(app.theme.color_base())
    } else {
        Some(app.theme.color_surface())
    };

    let row_style = if focused {
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

    let right = app.search.results[item_idx]
        .kind
        .tag()
        .map(str::to_string)
        .unwrap_or_else(|| app.search.results[item_idx].right_label.clone());
    let left = format!(
        "{:02}. {}",
        ordinal, app.search.results[item_idx].left_label
    );

    // 下载图标落在右侧标签（单曲行就是时长）左边：图标 + 一列分隔空格。
    let icon_width = usize::from(download_state.is_some()) * 2;
    let reserved = display_width(&right) + 1 + icon_width;
    let left_max = usize::from(row.width).saturating_sub(reserved);
    let clipped_left = clip_to_display_width(&left, left_max);
    let used = display_width(&clipped_left) + icon_width + display_width(&right);
    let space = usize::from(row.width).saturating_sub(used).max(1);

    let download_style = if focused {
        row_style
    } else {
        let download_style = match download_state {
            Some(crate::app::download::DownloadState::Done) => {
                Style::default().fg(app.theme.color_accent3())
            }
            Some(crate::app::download::DownloadState::Downloading) => Style::default()
                .fg(app.theme.color_accent2())
                .add_modifier(Modifier::BOLD),
            _ => Style::default().fg(app.theme.color_subtext()),
        };
        // 图标格与所在行同底色：斑马底在行样式上，这里补齐，
        // 避免图标格露出与行不同的背景。
        match zebra_bg {
            Some(bg) => download_style.bg(bg),
            None => download_style,
        }
    };

    let mut spans: Vec<Span> = vec![
        Span::styled(clipped_left.clone(), row_style),
        Span::styled(" ".repeat(space), row_style),
    ];

    if let Some(state) = download_state {
        let icon_x = row
            .x
            .saturating_add((display_width(&clipped_left) + space) as u16);
        spans.push(Span::styled(
            crate::app::download::state_glyph(state, app.download_spinner_phase()).to_string(),
            download_style,
        ));
        spans.push(Span::styled(" ", row_style));
        app.push_search_item_download_hit(
            crate::app::HitRect {
                x: icon_x,
                y: row.y,
                width: 1,
                height: 1,
            },
            item_idx,
        );
    }

    spans.push(Span::styled(right, row_style));

    frame.render_widget(Paragraph::new(Line::from(spans)), row);
}

fn base_bg_style(app: &App) -> Style {
    if app.config.transparent_background {
        Style::default()
    } else {
        Style::default().bg(app.theme.color_base())
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::CoverFetchState;
    use crate::app::SearchItem;

    fn item(kind: SearchItemKind, label: &str) -> SearchItem {
        SearchItem {
            kind,
            left_label: label.to_string(),
            right_label: String::new(),
            song_id: None,
            album_id: None,
            playlist_id: None,
            artist_id: None,
            title: None,
            artist: None,
            album: None,
            cover_url: None,
            duration_ms: None,
            cover: CoverFetchState::default(),
        }
    }

    /// 序号按分区重新开始（同 kind 计数），跨窗口滚动时也不受影响。
    #[test]
    fn ordinal_restarts_per_section() {
        let mut state = SearchState::default();
        state.set_results(
            vec![
                item(SearchItemKind::Artist, "artist-1"),
                item(SearchItemKind::Artist, "artist-2"),
                item(SearchItemKind::Playlist, "playlist-1"),
                item(SearchItemKind::Song, "song-1"),
                item(SearchItemKind::Song, "song-2"),
            ],
            0,
            false,
        );

        assert_eq!(search_item_ordinal(&state, 1), 2);
        assert_eq!(search_item_ordinal(&state, 2), 1);
        assert_eq!(search_item_ordinal(&state, 4), 2);
    }
}
