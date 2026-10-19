use super::layout::UiLayout;
use super::modals::about::render_about_modal;
use super::modals::bars::render_bar_settings_modal;
use super::modals::download::render_download_settings_modal;
use super::modals::equalizer::render_eq_modal;
use super::modals::help::render_help_modal;
use super::modals::lyrics::render_lyrics_settings_modal;
use super::modals::settings::{render_general_settings_modal, render_settings_modal};
use super::text::lang_text;
use crate::render::cover_pipeline::{CoverKey, CoverPipeline, ImagePhase};
use crate::render::drawer::{self, translated_clip};
use crate::tmplayer::app::state::{AppState, Overlay};
use crate::tmplayer::ui::panels::{info_panel, playlist_panel, visual_panel};
use ratatui::Terminal;
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::Style;

#[derive(Default)]
pub(super) struct PlaylistDrawer {
    buffer: Buffer,
}

/// 把提示以 `┤文字├` 的形式嵌进左侧面板的底边框。
///
/// 该行本身就是面板的 `horizontal_bottom`（`─`）。只重绘左面板那一段，
/// 两板接缝处的 `┘└` 角保持原样——整行重绘会把接缝抹平，看起来像
/// 两个面板在底部连通了。与边框同色，视觉上像是边框自带的标签。
///
/// 只在开启提示时调用。
///
/// 关闭提示时**不要**重绘这一段：`Block` 自己画的底边框是完整干净的，
/// 而额外用 `─` 覆盖会与上一帧宽字符（中文）留下的占位 cell 交互——
/// ratatui 在这些占位处跳过写入，于是底边框残留成 `─ ─ ─` 的断连样子。
/// 这一点已用 TestBackend 逐 cell 验证：仅 Block 时该行完整，叠加重绘
/// 后才出现空格。
fn render_hint_in_border(f: &mut ratatui::Frame, app: &AppState, area: Rect, left_panel: Rect) {
    // 只覆盖左面板横向范围，且要留出它自己的左右下角
    if left_panel.width < 4 || area.height == 0 {
        return;
    }
    let seg = Rect {
        x: left_panel.x + 1,
        y: area.y,
        width: left_panel.width - 2,
        height: 1,
    };
    let inner_w = usize::from(seg.width);
    if inner_w == 0 {
        return;
    }

    let text = lang_text(app, "Ctrl+K 打开按键绑定", "Ctrl+K open keybinds");
    let label_w = unicode_width::UnicodeWidthStr::width(text) + 2; // 含 ┤ ├
    if label_w > inner_w {
        // 放不下就不画，保持 Block 自己的完整底边框
        return;
    }

    // 标签左对齐，紧贴面板左下角。
    //
    // 必须用 `Buffer::set_string` 而非 `Paragraph` widget。中文是双宽字符，
    // 占两列：主 cell 放字，紧邻的占位 cell 应为空。`Block` 已先把这一行
    // 写满 `─`，而 `Paragraph` 只写主 cell、不清理占位 cell，于是占位处
    // 残留着 `─`。
    //
    // 这一帧看不出问题（终端写 `打` 时本就覆盖两列），坏在关闭提示的下一帧：
    // 差分比较发现占位处「上一帧是 `─`、这一帧也是 `─`」，判定无变化而不下发。
    // 但终端那一侧，往双宽字符的首列写 `─` 会把整个字清掉、次列变空白，
    // 且因为没有下发更新，这个空白再也不会被补回，底边框就成了 `─── ─ ─ ─`。
    //
    // `set_string` 会把占位 cell 置为空格，差分因此能察觉变化并下发。
    // 已在差分层验证：关闭提示时 Paragraph 漏发 6 列，set_string 漏发 0 列。
    let border_style = Style::default().fg(app.theme.color_subtext());
    let content = format!("┤{}├{}", text, "─".repeat(inner_w - label_w));
    f.buffer_mut()
        .set_string(seg.x, seg.y, &content, border_style);
}

pub(super) fn draw_page<B: ratatui::backend::Backend>(
    terminal: &mut Terminal<B>,
    app: &mut AppState,
    covers: &mut CoverPipeline,
    sidebar: &mut PlaylistDrawer,
) -> std::result::Result<UiLayout, B::Error> {
    draw_page_reveal_with_snapshot(terminal, app, covers, sidebar, None, 1.0)
}

pub(super) fn draw_page_reveal_with_snapshot<B: ratatui::backend::Backend>(
    terminal: &mut Terminal<B>,
    app: &mut AppState,
    covers: &mut CoverPipeline,
    sidebar: &mut PlaylistDrawer,
    host_snapshot: Option<&Buffer>,
    progress: f32,
) -> std::result::Result<UiLayout, B::Error> {
    let mut layout_out = UiLayout::default();
    terminal.autoresize()?;
    // Fixed target preparation is outside the terminal drawing closure.
    let size = terminal.get_frame().area();
    let show_right = app.config.visualize != crate::data::config::VisualizeMode::Hidden;
    let left = if show_right {
        Layout::horizontal([Constraint::Percentage(33), Constraint::Percentage(67)]).split(size)[0]
    } else {
        size
    };
    covers.begin_frame();
    if app.config.graphics_protocol == crate::data::config::GraphicsProtocol::Halfblocks {
        let content = info_panel::cover_content_rect(info_panel::layout(left, size.width).cover);
        if let Some(anim) = &app.cover_anim {
            for snapshot in [&anim.from, &anim.to] {
                if let (Some(bytes), Some(hash)) = (snapshot.cover.as_deref(), snapshot.cover_hash)
                {
                    covers.request_bytes(cover_key(content, hash), bytes, ImagePhase::Moving);
                }
            }
        } else if let (Some(bytes), Some(hash)) = (
            app.player.track.cover.as_deref(),
            app.player.track.cover_hash,
        ) {
            covers.request_bytes(cover_key(content, hash), bytes, ImagePhase::Stable);
        }
        let sidebar_cover = playlist_panel::compute_layout(left, app).cover_rect;
        if let (Some(bytes), Some(hash)) = (app.playlist_cover.as_deref(), app.playlist_cover_hash)
        {
            covers.request_bytes(cover_key(sidebar_cover, hash), bytes, ImagePhase::Stable);
        }
    }
    covers.end_frame();
    terminal.draw(|f| {
        let size = f.area();
        layout_out.full = size;

        // small terminal: keep stable, hide secondary panels
        if size.width < 50 || size.height < 12 {
            f.render_widget(ratatui::widgets::Clear, size);

            let mut base_style = Style::default().fg(app.theme.color_text());
            if !app.config.transparent_background {
                base_style = base_style.bg(app.theme.color_base());
            }
            f.render_widget(ratatui::widgets::Block::default().style(base_style), size);
            f.render_widget(
                ratatui::widgets::Paragraph::new(lang_text(
                    app,
                    "终端窗口过小",
                    "Terminal too small",
                ))
                .style(Style::default().fg(app.theme.color_subtext())),
                size,
            );
            slide_page_from_bottom(f.buffer_mut(), progress, host_snapshot);
            return;
        }

        // 提示不再独占一行：否则开关"显示提示"会把整页挤上去一行。
        // 内容区占满，提示以 `┤…├` 嵌进左面板底边框（那一行本就是 `─`）。
        let content_area = size;
        let bottom_row = if content_area.height > 0 {
            Rect {
                x: content_area.x,
                y: content_area.y + content_area.height - 1,
                width: content_area.width,
                height: 1,
            }
        } else {
            Rect::default()
        };

        // 「关闭」档位把右侧区（可视化 + 歌词）整块收起，歌曲信息区独占整宽。
        let show_right = app.config.visualize != crate::data::config::VisualizeMode::Hidden;
        let (left, right) = if show_right {
            let cols = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Percentage(33), Constraint::Percentage(67)])
                .split(content_area);
            (cols[0], cols[1])
        } else {
            (content_area, Rect::default())
        };
        layout_out.left = left;
        layout_out.right = right;
        layout_out.left_width = left.width;

        // 右栏的两行（歌词 / 可视化）；收起时保持零矩形。
        let mut lyric_row = Rect::default();
        let mut spectrum_row = Rect::default();
        if show_right {
            // right: lyrics (10%) + spectrum (rest)
            let lyric_h = ((right.height as f32) * 0.10).round() as u16;
            let lyric_h = lyric_h.clamp(3, right.height.saturating_sub(6));
            let rows = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Length(lyric_h), Constraint::Min(1)])
                .split(right);
            lyric_row = rows[0];
            spectrum_row = rows[1];

            // Mirror visual panel inner layout for auto bar count.
            let outer = Rect {
                x: rows[0].x,
                y: rows[0].y,
                width: rows[0].width,
                height: rows[0].height.saturating_add(rows[1].height),
            };
            let inner = outer.inner(ratatui::layout::Margin {
                horizontal: 1,
                vertical: 1,
            });
            let lyric_h_inner = rows[0].height.saturating_sub(2).min(inner.height);
            layout_out.spectrum_rect = Rect {
                x: inner.x,
                y: inner.y + lyric_h_inner,
                width: inner.width,
                height: inner.height.saturating_sub(lyric_h_inner),
            };
        }

        let info_l = info_panel::layout(left, size.width);
        if info_panel::core_rows_visible(&info_l) {
            layout_out.info_progress = info_l.progress;
            layout_out.info_volume = info_l.volume;
            layout_out.info_controls = info_l.controls;
            layout_out.info_meta = info_l.meta;
        }

        // base styling
        f.render_widget(ratatui::widgets::Clear, size);

        let mut base_style = Style::default().fg(app.theme.color_text());
        if !app.config.transparent_background {
            base_style = base_style.bg(app.theme.color_base());
        }
        f.render_widget(ratatui::widgets::Block::default().style(base_style), size);

        let halfblocks =
            app.config.graphics_protocol == crate::data::config::GraphicsProtocol::Halfblocks;
        info_panel::render(f, left, size.width, app);
        if show_right {
            visual_panel::render(f, lyric_row, spectrum_row, app);
        }
        if halfblocks {
            // Song cells remain behind the drawer; translation only clips cells.
            paint_halfblock_cover(f.buffer_mut(), covers, info_l.cover, app);
        }

        let pl_layout = playlist_panel::compute_layout(left, app);

        if app.overlay == Overlay::Playlist || app.playlist_slide_x != app.playlist_slide_target_x {
            let dx = app.playlist_slide_x.clamp(-(left.width as i16), 0);
            let visible = translated_clip(left, i32::from(dx), left);
            layout_out.playlist_rect = visible;
            if !visible.is_empty() {
                sidebar.buffer.resize(left);
                sidebar.buffer.reset();
                let surface = Style::default().bg(app.theme.color_surface());
                sidebar.buffer.set_style(left, surface);
                playlist_panel::render(&mut sidebar.buffer, left, app, !halfblocks);
                if halfblocks && let Some(hash) = app.playlist_cover_hash {
                    covers.paint(
                        &mut sidebar.buffer,
                        pl_layout.cover_rect,
                        pl_layout.cover_rect,
                        0,
                        0,
                        cover_key(pl_layout.cover_rect, hash),
                    );
                }
                // Keep the page's left frame above the drawer, but let the
                // drawer's background cover the same area as before.
                let content =
                    Rect::new(visible.x + 1, visible.y, visible.width - 1, visible.height);
                drawer::paint(f.buffer_mut(), &sidebar.buffer, i32::from(dx), content);
                let source_x = (i32::from(visible.x) - i32::from(dx)) as u16;
                for y in visible.top()..visible.bottom() {
                    f.buffer_mut()[(visible.x, y)].bg = sidebar.buffer[(source_x, y)].bg;
                }
                layout_out.playlist_inner =
                    translated_clip(pl_layout.inner, i32::from(dx), visible);
                layout_out.playlist_list_inner =
                    translated_clip(pl_layout.list_inner, i32::from(dx), visible);
            }
        }

        // toast
        if let Some((msg, _)) = &app.toast {
            let area = Rect {
                x: size.x,
                y: size.y,
                width: size.width,
                height: 1,
            };
            f.render_widget(
                ratatui::widgets::Paragraph::new(msg.as_str())
                    .style(Style::default().fg(app.theme.color_accent3())),
                area,
            );
        }

        if app.config.show_hints {
            render_hint_in_border(f, app, bottom_row, layout_out.left);
        }

        // modals (top-most)
        match app.overlay {
            Overlay::SettingsModal => {
                render_settings_modal(f, size, app, &mut layout_out.modal_rows)
            }
            Overlay::GeneralSettingsModal | Overlay::MprisNameEditModal => {
                render_general_settings_modal(f, size, app, &mut layout_out.modal_rows)
            }
            Overlay::BarSettingsModal => {
                render_bar_settings_modal(f, size, app, &mut layout_out.modal_rows)
            }
            Overlay::LyricsSettingsModal => {
                render_lyrics_settings_modal(f, size, app, &mut layout_out.modal_rows)
            }
            Overlay::DownloadSettingsModal | Overlay::DownloadPathEditModal => {
                render_download_settings_modal(f, size, app, &mut layout_out.modal_rows)
            }
            Overlay::AboutModal => render_about_modal(f, size, app),
            Overlay::HelpModal => render_help_modal(f, size, app, &mut layout_out.modal_rows),
            Overlay::EqModal => render_eq_modal(f, size, app),
            _ => {}
        }
        // Draw at the final geometry, then move the whole page as one drawer.
        // Cover preparation and cache keys are independent of this screen offset.
        slide_page_from_bottom(f.buffer_mut(), progress, host_snapshot);
    })?;
    Ok(layout_out)
}

pub(super) fn slide_page_from_bottom(
    buffer: &mut Buffer,
    progress: f32,
    host_snapshot: Option<&Buffer>,
) {
    let width = usize::from(buffer.area.width);
    let height = usize::from(buffer.area.height);
    if width == 0 || height == 0 {
        return;
    }
    let visible = (f32::from(buffer.area.height) * progress.clamp(0.0, 1.0)).round() as usize;
    let offset = height.saturating_sub(visible);
    if offset == 0 {
        return;
    }
    for source_row in (0..visible).rev() {
        let destination = (source_row + offset) * width;
        let (source, target) = buffer.content.split_at_mut(destination);
        let start = source_row * width;
        target[..width].clone_from_slice(&source[start..start + width]);
    }
    for row in 0..offset {
        let start = row * width;
        if let Some(snapshot) = host_snapshot.filter(|snapshot| snapshot.area == buffer.area) {
            buffer.content[start..start + width]
                .clone_from_slice(&snapshot.content[start..start + width]);
        } else {
            for cell in &mut buffer.content[start..start + width] {
                cell.reset();
            }
        }
    }
}

fn paint_halfblock_cover(
    target: &mut ratatui::buffer::Buffer,
    covers: &CoverPipeline,
    cover: Rect,
    app: &AppState,
) {
    let paint_border = |target: &mut Buffer, dx: i16, hash: u64| {
        if !app.config.album_border
            || covers
                .frame(cover_key(info_panel::cover_content_rect(cover), hash))
                .is_none()
        {
            return;
        }
        let content = info_panel::cover_content_rect(cover);
        if content == cover {
            return;
        }
        let border = crate::tmplayer::ui::borders::SOLID_BORDER;
        let clip = translated_clip(cover, i32::from(dx), cover.intersection(target.area));
        for y in clip.top()..clip.bottom() {
            for x in clip.left()..clip.right() {
                let sx = (i32::from(x) - i32::from(dx)) as u16;
                let symbol = match (
                    y == cover.top(),
                    y + 1 == cover.bottom(),
                    sx == cover.left(),
                    sx + 1 == cover.right(),
                ) {
                    (true, _, true, _) => Some(border.top_left),
                    (true, _, _, true) => Some(border.top_right),
                    (_, true, true, _) => Some(border.bottom_left),
                    (_, true, _, true) => Some(border.bottom_right),
                    (true, _, _, _) => Some(border.horizontal_top),
                    (_, true, _, _) => Some(border.horizontal_bottom),
                    (_, _, true, _) => Some(border.vertical_left),
                    (_, _, _, true) => Some(border.vertical_right),
                    _ => None,
                };
                if let Some(symbol) = symbol {
                    target[(x, y)]
                        .set_symbol(symbol)
                        .set_fg(app.theme.color_subtext());
                }
            }
        }
    };
    let content = info_panel::cover_content_rect(cover);
    if let Some(anim) = &app.cover_anim {
        let (from_dx, to_dx) = anim.slide_offsets(cover.width, app.last_frame);
        for (snapshot, dx) in [(&anim.from, from_dx), (&anim.to, to_dx)] {
            if let (Some(bytes), Some(hash)) = (snapshot.cover.as_deref(), snapshot.cover_hash) {
                let _ = bytes;
                paint_border(target, dx, hash);
                covers.paint(target, content, cover, dx, 0, cover_key(content, hash));
            }
        }
    } else if let (Some(bytes), Some(hash)) = (
        app.player.track.cover.as_deref(),
        app.player.track.cover_hash,
    ) {
        let _ = bytes;
        paint_border(target, 0, hash);
        covers.paint(target, content, cover, 0, 0, cover_key(content, hash));
    }
}

pub(super) fn cover_key(area: Rect, hash: u64) -> CoverKey {
    CoverKey {
        hash,
        width: area.width,
        height: area.height,
    }
}
