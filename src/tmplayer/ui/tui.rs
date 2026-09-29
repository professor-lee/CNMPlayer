use crate::tmplayer::app::state::{AppState, Overlay};
use crate::tmplayer::render::graphics_overlay::GraphicsOverlay;
use crate::tmplayer::ui::components::control_buttons;
use crate::tmplayer::ui::panels::{info_panel, playlist_panel, visual_panel};
use crate::tmplayer::utils::input::Action;
use anyhow::Result;
use crossterm::execute;
use crossterm::terminal::{EnterAlternateScreen, LeaveAlternateScreen};
use crossterm::{event, terminal};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use std::io::{self, Stdout};

#[derive(Debug, Default, Clone, Copy)]
pub struct UiLayout {
    pub full: Rect,
    pub left: Rect,
    pub right: Rect,
    pub left_width: u16,

    pub info_progress: Rect,
    pub info_volume: Rect,
    pub info_controls: Rect,
    /// 标题/爱心行（爱心贴该行右端）。
    pub info_meta: Rect,

    pub info_cover_image: Rect,

    pub playlist_rect: Rect,
    pub playlist_inner: Rect,
    pub playlist_list_inner: Rect,

    pub playlist_cover_image: Rect,

    pub spectrum_rect: Rect,
    /// 当前弹窗（若有）的条目行。
    pub modal_rows: ModalRows,
}

pub struct Tui {
    terminal: Terminal<CrosstermBackend<Stdout>>,
    pub should_quit: bool,
    graphics_overlay: GraphicsOverlay,
}

impl Tui {
    pub fn new(app: &AppState) -> Result<Self> {
        let stdout = io::stdout();
        let backend = CrosstermBackend::new(stdout);
        let terminal = Terminal::new(backend)?;
        Ok(Self {
            terminal,
            should_quit: false,
            graphics_overlay: GraphicsOverlay::new(app.config.graphics_protocol),
        })
    }

    pub fn enter(&mut self) -> Result<()> {
        execute!(
            io::stdout(),
            EnterAlternateScreen,
            event::EnableMouseCapture
        )?;
        terminal::enable_raw_mode()?;
        Ok(())
    }

    pub fn exit(&mut self) -> Result<()> {
        terminal::disable_raw_mode()?;
        execute!(
            io::stdout(),
            event::DisableMouseCapture,
            LeaveAlternateScreen
        )?;
        Ok(())
    }

    /// Terminal resize can clear/lose kitty graphic placements. Mark placements dirty so
    /// the next draw will re-place images.
    pub fn on_resize(&mut self) {}

    pub fn draw(&mut self, app: &mut AppState) -> Result<UiLayout> {
        if app.toast.as_ref().map(|(m, _)| m.as_str()) == Some("Bye") {
            self.should_quit = true;
        }

        // 点击作者名/专辑名这类"退出后交给宿主"的请求：请求一旦写下就退出。
        // 退出判定只在 draw 里做一次，避免每条事件分支各自记一遍。
        if app.exit_request.is_some() {
            self.should_quit = true;
        }

        let mut layout_out = UiLayout::default();

        // 小窗口显示开启时，全屏页过小不再显示提示，而是直接请求退出回主程序。
        // 在 draw 之前检测，避免过小提示闪现一帧。
        if app.config.small_window_display
            && let Ok((width, height)) = terminal::size()
            && (width < 50 || height < 12)
        {
            self.should_quit = true;
            return Ok(layout_out);
        }

        self.terminal.draw(|f| {
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
            let show_right =
                app.config.visualize != crate::tmplayer::data::config::VisualizeMode::Hidden;
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
            layout_out.info_progress = info_l.progress;
            layout_out.info_volume = info_l.volume;
            layout_out.info_controls = info_l.controls;
            layout_out.info_meta = if info_panel::core_rows_visible(&info_l) {
                info_l.meta
            } else {
                Rect::default()
            };

            // For kitty graphics, we draw into the inner area (optional border).
            layout_out.info_cover_image = info_l.cover.inner(ratatui::layout::Margin {
                horizontal: 1,
                vertical: 1,
            });

            // base styling
            f.render_widget(ratatui::widgets::Clear, size);

            let mut base_style = Style::default().fg(app.theme.color_text());
            if !app.config.transparent_background {
                base_style = base_style.bg(app.theme.color_base());
            }
            f.render_widget(ratatui::widgets::Block::default().style(base_style), size);

            info_panel::render(f, left, size.width, app);
            if show_right {
                visual_panel::render(f, lyric_row, spectrum_row, app);
            }

            // playlist overlay slides in/out over left
            if app.overlay == Overlay::Playlist
                || app.playlist_slide_x != app.playlist_slide_target_x
            {
                let collapsing = app.overlay != Overlay::Playlist
                    && app.playlist_slide_x > app.playlist_slide_target_x;

                // 动画推进在 AppState::tick 里完成，渲染只读取当前进度。
                // Slide effect via visible width growth/shrink (x stays at left edge)
                let full_w = left.width as i16;
                let visible_w = (full_w + app.playlist_slide_x).clamp(0, full_w) as u16;
                if visible_w > 0 {
                    let r = Rect {
                        x: left.x,
                        y: left.y,
                        width: visible_w,
                        height: left.height,
                    };
                    layout_out.playlist_rect = r;

                    if collapsing {
                        // Closing animation only needs the panel shell; skip expensive list/cover rendering.
                        f.render_widget(ratatui::widgets::Clear, r);
                        f.render_widget(
                            Block::default()
                                .borders(Borders::ALL)
                                .border_set(crate::tmplayer::ui::borders::SOLID_BORDER)
                                .style(
                                    Style::default()
                                        .fg(app.theme.color_subtext())
                                        .bg(app.theme.color_surface()),
                                ),
                            r,
                        );
                    } else {
                        let pl_layout = playlist_panel::compute_layout(r, app);
                        layout_out.playlist_inner = pl_layout.inner;
                        layout_out.playlist_list_inner = pl_layout.list_inner;
                        layout_out.playlist_cover_image = pl_layout.cover_rect;
                        playlist_panel::render(f, r, app);
                    }
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
                Self::render_hint_in_border(f, app, bottom_row, layout_out.left);
            }

            // Paint kitty images on top of ratatui widgets.
            Self::paint_kitty_images(&mut self.graphics_overlay, f, app, &layout_out);

            // modals (top-most)
            match app.overlay {
                Overlay::SettingsModal => {
                    render_settings_modal(f, size, app, &mut layout_out.modal_rows)
                }
                Overlay::BarSettingsModal => {
                    render_bar_settings_modal(f, size, app, &mut layout_out.modal_rows)
                }
                Overlay::LocalAudioSettingsModal => {
                    render_local_audio_settings_modal(f, size, app, &mut layout_out.modal_rows)
                }
                Overlay::LyricsSettingsModal => {
                    render_lyrics_settings_modal(f, size, app, &mut layout_out.modal_rows)
                }
                Overlay::DownloadSettingsModal | Overlay::DownloadPathEditModal => {
                    render_download_settings_modal(f, size, app, &mut layout_out.modal_rows)
                }
                Overlay::AboutModal => render_about_modal(f, size, app),
                Overlay::AcoustIdModal => render_acoustid_modal(f, size, app),
                Overlay::HelpModal => render_help_modal(f, size, app, &mut layout_out.modal_rows),
                Overlay::EqModal => render_eq_modal(f, size, app),
                _ => {}
            }
        })?;

        Ok(layout_out)
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

    fn paint_kitty_images(
        graphics_overlay: &mut GraphicsOverlay,
        f: &mut ratatui::Frame<'_>,
        app: &mut AppState,
        layout: &UiLayout,
    ) {
        let playlist_overlay_visible = app.overlay == Overlay::Playlist
            || app.playlist_slide_x != app.playlist_slide_target_x
            || layout.playlist_rect.width > 0;

        let info_cover_bytes = if playlist_overlay_visible {
            None
        } else {
            app.player.track.cover.as_deref()
        };

        let playlist_fully_expanded = app.overlay == Overlay::Playlist
            && app.playlist_slide_x == 0
            && app.playlist_slide_target_x == 0;

        let playlist_cover_bytes = if playlist_fully_expanded {
            app.local_view_album_cover.as_deref()
        } else {
            None
        };

        let info_rect = if layout.info_cover_image.width > 0 && layout.info_cover_image.height > 0 {
            Some(layout.info_cover_image)
        } else {
            None
        };

        let playlist_rect =
            if layout.playlist_cover_image.width > 0 && layout.playlist_cover_image.height > 0 {
                Some(layout.playlist_cover_image)
            } else {
                None
            };

        graphics_overlay.paint(
            app,
            f,
            info_cover_bytes,
            playlist_cover_bytes,
            info_rect,
            playlist_rect,
        );
    }
}

fn centered_rect(size: Rect, width: u16, height: u16) -> Rect {
    let w = width.min(size.width.saturating_sub(4)).max(10);
    let h = height.min(size.height.saturating_sub(4)).max(6);
    Rect {
        x: size.x + (size.width.saturating_sub(w)) / 2,
        y: size.y + (size.height.saturating_sub(h)) / 2,
        width: w,
        height: h,
    }
}

/// 当前弹窗里可点击的条目行，按条目序号逐行登记。
///
/// `Copy` 且定长，好跟着 `UiLayout` 一起传出来；渲染时填、`hit_test` 时查，
/// 两边共用同一份矩形，不会各算一遍偏移。
#[derive(Debug, Clone, Copy)]
pub struct ModalRows {
    rows: [(usize, Rect); ModalRows::MAX],
    len: usize,
}

impl ModalRows {
    /// 单个弹窗的行数上限。取最长的一个（按键提示弹窗 17 条）再留些余量；
    /// 超出的行会被丢弃，所以新增更长的弹窗列表时要同步调大。
    pub const MAX: usize = 24;

    /// 登记一行：`index` 是该行代表的**条目序号**，不是登记次序。
    ///
    /// 弹窗列表会被截断（终端太矮）或被滚动（按键提示弹窗），行号与条目号
    /// 并不相等，消费端要的是条目号，所以两者必须分开存。
    /// 超出上限的行被丢弃（渲染本身也会被裁掉）。
    fn push(&mut self, rect: Rect, index: usize) {
        if self.len < Self::MAX && rect.width > 0 && rect.height > 0 {
            self.rows[self.len] = (index, rect);
            self.len += 1;
        }
    }

    /// 某个条目序号画在哪一行（该条目未显示时为 `None`）。
    pub fn get(&self, index: usize) -> Option<Rect> {
        self.rows[..self.len]
            .iter()
            .find(|(row_index, _)| *row_index == index)
            .map(|(_, rect)| *rect)
    }

    /// 已登记的行数（只在测试里用，release 构建不该带上）。
    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.len
    }

    /// 命中的条目序号。
    fn hit(&self, col: u16, row: u16) -> Option<usize> {
        self.rows[..self.len]
            .iter()
            .find(|(_, rect)| contains(*rect, col, row))
            .map(|(index, _)| *index)
    }
}

impl Default for ModalRows {
    fn default() -> Self {
        Self {
            rows: [(0, Rect::default()); Self::MAX],
            len: 0,
        }
    }
}

fn render_settings_modal(
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

    let language_label = match app.language {
        crate::data::config::Language::Zh => "中文",
        crate::data::config::Language::En => "English",
    };

    let items = vec![
        format!("{}: {}", lang_text(app, "主题", "Theme"), app.config.theme),
        format!(
            "{}: {}",
            lang_text(app, "背景透明", "Transparent Background"),
            lang_on_off(app, app.config.transparent_background)
        ),
        format!("{}: {}", lang_text(app, "语言", "Language"), language_label),
        format!(
            "{}: {}",
            lang_text(app, "图形协议", "Graphics"),
            app.config.graphics_protocol.display_name()
        ),
        format!("{}...", lang_text(app, "播放设置", "Playback Settings")),
        format!("{}...", lang_text(app, "按键绑定", "Keybinds")),
        format!("{}...", lang_text(app, "歌词浮窗", "Lyrics Overlay")),
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

fn render_acoustid_modal(f: &mut ratatui::Frame, size: Rect, app: &mut AppState) {
    let area = centered_rect(size, 60, 8);
    f.render_widget(ratatui::widgets::Clear, area);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_set(crate::tmplayer::ui::borders::SOLID_BORDER)
        .title(lang_text(app, "AcoustID API 密钥", "AcoustID API Key"))
        .style(
            Style::default()
                .fg(app.theme.color_subtext())
                .bg(app.theme.color_surface()),
        );
    f.render_widget(block, area);

    let inner = area.inner(ratatui::layout::Margin {
        horizontal: 1,
        vertical: 1,
    });

    let mut lines: Vec<Line> = Vec::new();
    lines.push(Line::styled(
        "",
        Style::default()
            .fg(app.theme.color_subtext())
            .bg(app.theme.color_surface()),
    ));
    lines.push(Line::styled(
        "",
        Style::default().bg(app.theme.color_surface()),
    ));
    lines.push(Line::styled(
        format!(
            "{}: {}",
            lang_text(app, "API 密钥", "API Key"),
            app.acoustid_input
        ),
        Style::default()
            .fg(app.theme.color_text())
            .bg(app.theme.color_surface()),
    ));

    let p = Paragraph::new(lines)
        .style(Style::default().bg(app.theme.color_surface()))
        .wrap(Wrap { trim: true });
    f.render_widget(p, inner);
}

fn render_bar_settings_modal(
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
        .title(lang_text(app, " 播放设置 ", " Playback Settings "))
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

    let bar_number_label = match app.config.bar_number {
        crate::tmplayer::data::config::BarNumber::Auto => lang_text(app, "自动", "Auto"),
        crate::tmplayer::data::config::BarNumber::N16 => "16",
        crate::tmplayer::data::config::BarNumber::N32 => "32",
        crate::tmplayer::data::config::BarNumber::N48 => "48",
        crate::tmplayer::data::config::BarNumber::N64 => "64",
        crate::tmplayer::data::config::BarNumber::N80 => "80",
        crate::tmplayer::data::config::BarNumber::N96 => "96",
    };
    let channels_label = match app.config.bar_channels {
        crate::tmplayer::data::config::BarChannels::Mono => "Mono",
        crate::tmplayer::data::config::BarChannels::Stereo => "Stereo",
    };

    let items = vec![
        format!(
            "{}: {}",
            lang_text(app, "可视化", "Visualization"),
            match app.config.visualize {
                crate::tmplayer::data::config::VisualizeMode::Lyrics =>
                    lang_text(app, "仅歌词", "Lyrics"),
                crate::tmplayer::data::config::VisualizeMode::Hidden =>
                    lang_text(app, "关闭", "Off"),
                crate::tmplayer::data::config::VisualizeMode::Bars =>
                    lang_text(app, "频谱", "Bars"),
                crate::tmplayer::data::config::VisualizeMode::Oscilloscope => {
                    lang_text(app, "示波器", "Oscilloscope")
                }
                crate::tmplayer::data::config::VisualizeMode::Vector => {
                    lang_text(app, "矢量", "Vector")
                }
            }
        ),
        format!(
            "{}: {}",
            lang_text(app, "超级流畅", "Super Smooth"),
            lang_on_off(app, app.config.super_smooth_bar)
        ),
        format!(
            "{}: {}",
            lang_text(app, "频谱间隔", "Bars Gap"),
            lang_on_off(app, app.config.bars_gap)
        ),
        format!(
            "{}: {}",
            lang_text(app, "频谱数", "Bars Count"),
            bar_number_label
        ),
        format!("{}: {}", lang_text(app, "声道", "Channels"), channels_label),
        format!(
            "{}: {}",
            lang_text(app, "封面边框", "Cover Border"),
            lang_on_off(app, app.config.album_border)
        ),
        format!(
            "{}: {}",
            lang_text(app, "音质", "Audio Quality"),
            match app.config.audio_quality {
                crate::tmplayer::data::config::AudioQuality::Standard =>
                    lang_text(app, "标准", "Standard"),
                crate::tmplayer::data::config::AudioQuality::Higher =>
                    lang_text(app, "较高", "Higher"),
                crate::tmplayer::data::config::AudioQuality::Exhigh =>
                    lang_text(app, "极高", "Exhigh"),
                crate::tmplayer::data::config::AudioQuality::Lossless =>
                    lang_text(app, "无损", "Lossless"),
                crate::tmplayer::data::config::AudioQuality::Hires => "Hi-Res",
                crate::tmplayer::data::config::AudioQuality::Jyeffect => {
                    lang_text(app, "高清环绕声", "JYEffect")
                }
                crate::tmplayer::data::config::AudioQuality::Sky => {
                    lang_text(app, "沉浸环绕声", "Sky")
                }
                crate::tmplayer::data::config::AudioQuality::Dolby => {
                    lang_text(app, "杜比全景声", "Dolby")
                }
                crate::tmplayer::data::config::AudioQuality::Jymaster => {
                    lang_text(app, "超清母带", "JYMaster")
                }
            }
        ),
        format!(
            "{}: {}",
            lang_text(app, "播放记忆", "Playback Memory"),
            lang_on_off(app, app.config.playback_memory)
        ),
    ];

    for (idx, text) in items.iter().enumerate() {
        if idx as u16 >= rows[1].height {
            break;
        }
        let style = if idx == app.bar_settings_selected {
            Style::default()
                .fg(app.theme.color_accent2())
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(app.theme.color_text())
        };
        let rect = Rect {
            x: rows[1].x,
            y: rows[1].y + idx as u16,
            width: rows[1].width,
            height: 1,
        };
        f.render_widget(
            Paragraph::new(Line::styled(format!("  {}", text), style)),
            rect,
        );
        modal_rows.push(rect, idx);
    }

    f.render_widget(Paragraph::new(""), rows[2]);
}

fn render_lyrics_settings_modal(
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
        .title(lang_text(app, " 歌词浮窗 ", " Lyrics Overlay "))
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

    // 与主应用设置同构：吸附行只在拖动开启时可改，关闭时灰置。
    let drag_enabled = app.config.page_lyrics_drag;
    let items = [
        format!(
            "{}: {}",
            lang_text(app, "歌词浮窗", "Lyrics Overlay"),
            lang_on_off(app, app.config.page_lyrics)
        ),
        format!(
            "{}: {}",
            lang_text(app, "歌词浮窗拖动", "Lyrics Overlay Drag"),
            lang_on_off(app, drag_enabled)
        ),
        format!(
            "{}: {}",
            lang_text(app, "歌词浮窗边缘吸附", "Lyrics Overlay Edge Snap"),
            lang_on_off(app, app.config.page_lyrics_snap)
        ),
    ];

    for (idx, text) in items.iter().enumerate() {
        if idx as u16 >= rows[1].height {
            break;
        }
        let disabled = idx == 2 && !drag_enabled;
        let style = if idx == app.lyrics_settings_selected {
            if disabled {
                Style::default().fg(app.theme.color_subtext())
            } else {
                Style::default()
                    .fg(app.theme.color_accent2())
                    .add_modifier(Modifier::BOLD)
            }
        } else if disabled {
            Style::default().fg(app.theme.color_subtext())
        } else {
            Style::default().fg(app.theme.color_text())
        };
        let rect = Rect {
            x: rows[1].x,
            y: rows[1].y + idx as u16,
            width: rows[1].width,
            height: 1,
        };
        f.render_widget(
            Paragraph::new(Line::styled(format!("  {}", text), style)),
            rect,
        );
        modal_rows.push(rect, idx);
    }

    f.render_widget(Paragraph::new(""), rows[2]);
}

fn render_download_settings_modal(
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
        crate::tmplayer::data::config::AudioQuality::Standard => lang_text(app, "标准", "Standard"),
        crate::tmplayer::data::config::AudioQuality::Higher => lang_text(app, "较高", "Higher"),
        crate::tmplayer::data::config::AudioQuality::Exhigh => lang_text(app, "极高", "Exhigh"),
        crate::tmplayer::data::config::AudioQuality::Lossless => lang_text(app, "无损", "Lossless"),
        crate::tmplayer::data::config::AudioQuality::Hires => "Hi-Res",
        crate::tmplayer::data::config::AudioQuality::Jyeffect => {
            lang_text(app, "高清环绕声", "JYEffect")
        }
        crate::tmplayer::data::config::AudioQuality::Sky => lang_text(app, "沉浸环绕声", "Sky"),
        crate::tmplayer::data::config::AudioQuality::Dolby => lang_text(app, "杜比全景声", "Dolby"),
        crate::tmplayer::data::config::AudioQuality::Jymaster => {
            lang_text(app, "超清母带", "JYMaster")
        }
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

fn render_local_audio_settings_modal(
    f: &mut ratatui::Frame,
    size: Rect,
    app: &mut AppState,
    modal_rows: &mut ModalRows,
) {
    let area = centered_rect(size, 60, 12);
    f.render_widget(ratatui::widgets::Clear, area);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_set(crate::tmplayer::ui::borders::SOLID_BORDER)
        .title(lang_text(app, "本地音频", "Local Audio"))
        .style(
            Style::default()
                .fg(app.theme.color_subtext())
                .bg(app.theme.color_surface()),
        );
    f.render_widget(block, area);

    let inner = area.inner(ratatui::layout::Margin {
        horizontal: 1,
        vertical: 1,
    });

    let mut lines: Vec<Line> = Vec::new();
    lines.push(Line::styled(
        "",
        Style::default()
            .fg(app.theme.color_subtext())
            .bg(app.theme.color_surface()),
    ));
    lines.push(Line::styled(
        "",
        Style::default().bg(app.theme.color_surface()),
    ));

    let lyrics_fetch_label = format!(
        "{}: {}",
        lang_text(app, "歌词/封面获取", "Lyrics/Cover Fetch"),
        if app.config.lyrics_cover_fetch {
            lang_text(app, "开", "On")
        } else {
            lang_text(app, "关", "Off")
        }
    );
    let lyrics_download_label = format!(
        "{}: {}",
        lang_text(app, "歌词/封面下载", "Lyrics/Cover Download"),
        if app.config.lyrics_cover_download {
            lang_text(app, "开", "On")
        } else {
            lang_text(app, "关", "Off")
        }
    );
    let fingerprint_label = if app.config.acoustid_api_key.trim().is_empty() {
        format!(
            "{}: {} ({})",
            lang_text(app, "音频指纹", "Audio Fingerprint"),
            lang_text(app, "关", "Off"),
            lang_text(app, "需要 API 密钥", "API key required")
        )
    } else {
        format!(
            "{}: {}",
            lang_text(app, "音频指纹", "Audio Fingerprint"),
            if app.config.audio_fingerprint {
                lang_text(app, "开", "On")
            } else {
                lang_text(app, "关", "Off")
            }
        )
    };
    let acoustid_label = format!(
        "{}: {}",
        lang_text(app, "AcoustID API", "AcoustID API"),
        if app.config.acoustid_api_key.trim().is_empty() {
            lang_text(app, "未设置", "Not set")
        } else {
            lang_text(app, "已设置", "Set")
        }
    );
    let resume_label = format!(
        "{}: {}",
        lang_text(app, "记住上次进度", "Resume Last Position"),
        if app.config.resume_last_position {
            lang_text(app, "开", "On")
        } else {
            lang_text(app, "关", "Off")
        }
    );

    let items = [
        lyrics_fetch_label,
        lyrics_download_label,
        fingerprint_label,
        acoustid_label,
        resume_label,
    ];

    for (idx, text) in items.iter().enumerate() {
        let disabled = match idx {
            2 => app.config.acoustid_api_key.trim().is_empty(),
            _ => false,
        };

        let style = if idx == app.local_audio_settings_selected {
            if disabled {
                Style::default()
                    .fg(app.theme.color_subtext())
                    .bg(app.theme.color_surface())
            } else {
                Style::default()
                    .fg(app.theme.color_accent2())
                    .add_modifier(Modifier::BOLD)
            }
        } else if disabled {
            Style::default()
                .fg(app.theme.color_subtext())
                .bg(app.theme.color_surface())
        } else {
            Style::default()
                .fg(app.theme.color_text())
                .bg(app.theme.color_surface())
        };
        lines.push(Line::styled(format!("  {}", text), style));
    }

    let p = Paragraph::new(lines)
        .style(Style::default().bg(app.theme.color_surface()))
        .wrap(Wrap { trim: true });
    f.render_widget(p, inner);

    // 命中区：条目从 inner 的第 3 行起（上面两句是空行）。标签都短于弹窗宽度，
    // 不会被 `Wrap` 折行，故行号与条目号一一对应。
    for idx in 0..items.len() {
        let offset = 2 + idx;
        if offset >= inner.height as usize {
            break;
        }
        modal_rows.push(
            Rect {
                x: inner.x,
                y: inner.y + offset as u16,
                width: inner.width,
                height: 1,
            },
            idx,
        );
    }
}

fn render_about_modal(f: &mut ratatui::Frame, size: Rect, app: &mut AppState) {
    let area = centered_rect(size, 70, 22);
    f.render_widget(ratatui::widgets::Clear, area);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_set(crate::tmplayer::ui::borders::SOLID_BORDER)
        .title(" about ")
        .style(
            Style::default()
                .fg(app.theme.color_subtext())
                .bg(app.theme.color_surface()),
        );
    f.render_widget(block, area);

    let inner = area.inner(ratatui::layout::Margin {
        horizontal: 1,
        vertical: 1,
    });
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(40), Constraint::Percentage(60)])
        .split(inner);

    render_about_braille(f, chunks[0], app);
    render_about_text(f, chunks[1], app);

    let info = crate::tmplayer::data::about::about_info();
    let version = format!("v{}", info.version);
    let y = area.y + area.height.saturating_sub(1);
    let version_area = Rect {
        x: area.x.saturating_add(1),
        y,
        width: area.width.saturating_sub(2),
        height: 1,
    };
    f.render_widget(
        Paragraph::new(version).alignment(Alignment::Center).style(
            Style::default()
                .fg(app.theme.color_subtext())
                .bg(app.theme.color_surface()),
        ),
        version_area,
    );
}

fn render_about_braille(f: &mut ratatui::Frame, area: Rect, app: &mut AppState) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let lines = about_braille_lines(area.width as usize, area.height as usize);
    let p = Paragraph::new(lines).style(
        Style::default()
            .fg(app.theme.color_text())
            .bg(app.theme.color_surface()),
    );
    f.render_widget(p, area);
}

fn render_about_text(f: &mut ratatui::Frame, area: Rect, app: &mut AppState) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let info = crate::tmplayer::data::about::about_info();

    let max_width = area.width as usize;
    if max_width == 0 {
        return;
    }

    let mut rendered: Vec<String> = Vec::new();
    for (k, v) in &info.links {
        let line = if k.eq_ignore_ascii_case("github_url") {
            v.to_string()
        } else {
            format!("{}: {}", k, v)
        };
        rendered.extend(wrap_text(&line, max_width));
    }

    let desc_lines: Vec<String> = wrap_text(&info.description, max_width);
    if !desc_lines.is_empty() {
        rendered.push(String::new());
        rendered.extend(desc_lines);
    }

    let max_line_width = rendered
        .iter()
        .map(|l| unicode_width::UnicodeWidthStr::width(l.as_str()))
        .max()
        .unwrap_or(0)
        .min(max_width);
    let block_h = rendered.len() as u16;
    let block_w = max_line_width.max(1) as u16;
    let offset_x = (area.width.saturating_sub(block_w)) / 2;
    let offset_y = if block_h <= area.height {
        (area.height.saturating_sub(block_h)) / 2
    } else {
        0
    };

    let lines: Vec<Line> = rendered
        .into_iter()
        .map(|l| {
            Line::styled(
                l,
                Style::default()
                    .fg(app.theme.color_text())
                    .bg(app.theme.color_surface()),
            )
        })
        .collect();
    let p = Paragraph::new(lines)
        .style(Style::default().bg(app.theme.color_surface()))
        .wrap(Wrap { trim: false });
    let text_h = area.height.saturating_sub(offset_y).min(block_h.max(1));
    let text_area = Rect {
        x: area.x + offset_x,
        y: area.y + offset_y,
        width: block_w,
        height: text_h,
    };
    f.render_widget(p, text_area);
}

fn wrap_text(s: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return Vec::new();
    }
    if s.is_empty() {
        return vec![String::new()];
    }

    // Wrap on display columns, not char count: CJK text is twice as wide as it
    // is long, and counting chars would clip it.
    let mut out: Vec<String> = Vec::new();
    let mut buf = String::new();
    let mut buf_width = 0usize;
    for ch in s.chars() {
        let ch_width = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
        if buf_width + ch_width > width && !buf.is_empty() {
            out.push(std::mem::take(&mut buf));
            buf_width = 0;
        }
        buf.push(ch);
        buf_width += ch_width;
    }
    if !buf.is_empty() {
        out.push(buf);
    }
    out
}

fn about_braille_lines(width: usize, height: usize) -> Vec<Line<'static>> {
    let blank = " ".repeat(width);
    if width == 0 || height == 0 {
        return Vec::new();
    }

    let info = crate::tmplayer::data::about::about_info();
    let Some(selected) = select_about_braille_art(width, height, &info.braille_images) else {
        return (0..height).map(|_| Line::from(blank.clone())).collect();
    };

    let mut rows: Vec<String> = selected
        .art
        .lines()
        .map(|line| line.trim_end().to_string())
        .collect();

    let mut start = 0usize;
    let mut end = rows.len();
    while start < end && rows[start].trim().is_empty() {
        start += 1;
    }
    while end > start && rows[end - 1].trim().is_empty() {
        end -= 1;
    }
    rows = rows[start..end].to_vec();

    let rows_w = rows
        .iter()
        .map(|line| line.chars().count())
        .max()
        .unwrap_or(0);
    let canvas_w = selected.width.max(rows_w);
    let canvas_h = selected.height.max(rows.len());

    let offset_x = width.saturating_sub(canvas_w) / 2;
    let offset_y = height.saturating_sub(canvas_h) / 2;
    let mut grid: Vec<Vec<char>> = vec![vec![' '; width]; height];

    for (row_idx, row) in rows.iter().enumerate() {
        let gy = offset_y + row_idx;
        if gy >= height {
            break;
        }
        for (col_idx, ch) in row.chars().enumerate() {
            let gx = offset_x + col_idx;
            if gx >= width {
                break;
            }
            grid[gy][gx] = ch;
        }
    }

    grid.into_iter()
        .map(|row| Line::from(row.into_iter().collect::<String>()))
        .collect()
}

fn select_about_braille_art(
    width: usize,
    height: usize,
    arts: &[crate::tmplayer::data::about::BrailleImage],
) -> Option<&crate::tmplayer::data::about::BrailleImage> {
    let mut best_fit: Option<(&crate::tmplayer::data::about::BrailleImage, u128)> = None;
    for art in arts {
        if art.width == 0 || art.height == 0 {
            continue;
        }
        if art.width <= width && art.height <= height {
            let score = (art.width as u128) * (art.height as u128);
            let should_replace = best_fit
                .as_ref()
                .map(|(_, best_score)| score > *best_score)
                .unwrap_or(true);
            if should_replace {
                best_fit = Some((art, score));
            }
        }
    }

    if let Some((art, _)) = best_fit {
        return Some(art);
    }

    arts.iter()
        .filter(|art| art.width > 0 && art.height > 0)
        .min_by_key(|art| {
            let dw = art.width.saturating_sub(width) as u128;
            let dh = art.height.saturating_sub(height) as u128;
            let overflow = dw.saturating_mul(dh).saturating_add(dw).saturating_add(dh);
            let area = (art.width as u128).saturating_mul(art.height as u128);
            (overflow, area)
        })
}

/// 按键提示弹窗的条目 `(说明, 按键)`。
///
/// 渲染与键盘/滚轮翻页共用同一份，避免"条目数写死"与列表实际长度不一致
/// （此前末尾几行因此无法用键盘选中）。
pub fn help_items(app: &AppState) -> Vec<(String, String)> {
    let item = |zh: &'static str, en: &'static str, key: &str| {
        (lang_text(app, zh, en).to_string(), key.to_string())
    };

    vec![
        item("搜索框", "Search Box", &app.config.keybind_search_box),
        item("全屏播放页", "Fullscreen", &app.config.keybind_fullscreen),
        item("设置弹窗", "Settings Modal", &app.config.keybind_settings),
        item("侧边栏", "Sidebar", &app.config.keybind_sidebar),
        item("退出应用", "Quit", &app.config.keybind_quit),
        item(
            "快速上翻页（主程序）",
            "Quick Page Up (Host)",
            &app.config.keybind_page_up,
        ),
        item(
            "快速下翻页（主程序）",
            "Quick Page Down (Host)",
            &app.config.keybind_page_down,
        ),
        item("上一首", "Previous", &app.config.keybind_fullscreen_prev),
        item("下一首", "Next", &app.config.keybind_fullscreen_next),
        item(
            "播放/暂停",
            "Play/Pause",
            &app.config.keybind_fullscreen_toggle_play_pause,
        ),
        item(
            "全屏模式切换",
            "Fullscreen Mode Switch",
            &app.config.keybind_fullscreen_toggle_mode,
        ),
        item(
            "EQ均衡器",
            "EQ Equalizer",
            &app.config.keybind_fullscreen_eq,
        ),
        item(
            "EQ重置",
            "EQ Reset",
            &app.config.keybind_fullscreen_eq_reset,
        ),
        item(
            "收藏/取消收藏",
            "Like/Unlike",
            &app.config.keybind_toggle_like_fullscreen,
        ),
        item(
            "小窗口切换显示",
            "Small Window Switch",
            &app.config.keybind_small_window_toggle,
        ),
        item(
            "下载歌曲（主应用）",
            "Download Song (Host)",
            &app.config.keybind_download,
        ),
        item(
            "下载歌曲（全屏页）",
            "Download Song (Fullscreen)",
            &app.config.keybind_download_fullscreen,
        ),
        item(
            "侧边栏歌单区切换",
            "Sidebar Playlist Section Switch",
            "Ctrl+Up/Down",
        ),
        item("按键绑定", "Keybinds", "Ctrl+K"),
    ]
}

/// 按键提示弹窗的条目数（键盘/滚轮翻页用）。
pub fn help_item_count(app: &AppState) -> usize {
    help_items(app).len()
}

fn render_help_modal(
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
        .title(lang_text(app, " 按键绑定 ", " Keybinds "))
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

    let items = help_items(app);

    let visible_rows = rows[1].height as usize;
    let total_rows = items.len();
    let selected = app.help_keybind_selected.min(total_rows.saturating_sub(1));
    let max_scroll = total_rows.saturating_sub(visible_rows);
    let scroll = if visible_rows == 0 || selected < visible_rows {
        0
    } else {
        (selected + 1 - visible_rows).min(max_scroll)
    };

    for (idx, (label, key)) in items.iter().enumerate().skip(scroll).take(visible_rows) {
        let style = if idx == selected {
            Style::default()
                .fg(app.theme.color_accent2())
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(app.theme.color_text())
        };
        let rect = Rect {
            x: rows[1].x,
            y: rows[1].y + (idx - scroll) as u16,
            width: rows[1].width,
            height: 1,
        };
        f.render_widget(
            Paragraph::new(Line::styled(format!("  {}: {}", label, key), style)),
            rect,
        );
        modal_rows.push(rect, idx);
    }

    f.render_widget(
        Paragraph::new(lang_text(
            app,
            "Up/Down 浏览  Esc 关闭（仅查看）",
            "Up/Down browse  Esc close (view only, no rebinding)",
        ))
        .style(Style::default().fg(app.theme.color_subtext())),
        rows[2],
    );
}

fn render_eq_modal(f: &mut ratatui::Frame, size: Rect, app: &mut AppState) {
    // 需求：柱状条宽 2 格，高度 +12/-12（含 0 行共 25）
    // 额外预留：顶部提示 1 行 + 底部频率/数值 2 行
    let area = centered_rect(size, 44, 31);
    f.render_widget(ratatui::widgets::Clear, area);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_set(crate::tmplayer::ui::borders::SOLID_BORDER)
        .title(lang_text(app, "均衡器", "Equalizer"))
        .style(
            Style::default()
                .fg(app.theme.color_subtext())
                .bg(app.theme.color_surface()),
        );
    f.render_widget(block, area);

    let inner = area.inner(ratatui::layout::Margin {
        horizontal: 1,
        vertical: 1,
    });

    let bg = Style::default().bg(app.theme.color_surface());
    let sub = Style::default()
        .fg(app.theme.color_subtext())
        .bg(app.theme.color_surface());
    let text = Style::default()
        .fg(app.theme.color_text())
        .bg(app.theme.color_surface());
    let selected_bg = Style::default()
        .fg(app.theme.color_base())
        .bg(app.theme.color_accent())
        .add_modifier(Modifier::BOLD);

    // layout inside modal
    if inner.height < 3 {
        return;
    }
    let hint_rect = Rect {
        x: inner.x,
        y: inner.y,
        width: inner.width,
        height: 1,
    };
    let freq_label_rect = Rect {
        x: inner.x,
        y: inner.y + inner.height - 2,
        width: inner.width,
        height: 1,
    };
    let gain_label_rect = Rect {
        x: inner.x,
        y: inner.y + inner.height - 1,
        width: inner.width,
        height: 1,
    };
    let bars_rect = Rect {
        x: inner.x,
        y: inner.y + 1,
        width: inner.width,
        height: inner.height.saturating_sub(3),
    };

    f.render_widget(
        Paragraph::new("").style(sub).wrap(Wrap { trim: true }),
        hint_rect,
    );

    // compute band geometry
    const BANDS: usize = crate::tmplayer::app::state::EQ_BANDS;
    const BAR_W: u16 = 2;
    const GAP: u16 = 1;

    fn fmt_db2(v: f32) -> String {
        let i = v.clamp(-12.0, 12.0).round() as i32;
        format!("{:+03}", i)
    }

    fn fmt_freq(freq_hz: f32) -> String {
        let f = freq_hz.round() as i32;
        if f >= 1000 {
            format!("{}k", f / 1000)
        } else {
            format!("{f}")
        }
    }

    let gains = app.eq.bands_db;
    let freq_labels: Vec<String> = crate::tmplayer::app::state::EQ_FREQS_HZ
        .iter()
        .map(|&f| fmt_freq(f))
        .collect();
    let gain_labels: Vec<String> = gains.iter().map(|&g| fmt_db2(g)).collect();

    // Fit columns to available width (10 bands should still render on typical terminals).
    let gaps_w = GAP.saturating_mul((BANDS as u16).saturating_sub(1));
    let mut cw = if bars_rect.width > gaps_w {
        (bars_rect.width - gaps_w) / (BANDS as u16)
    } else {
        BAR_W
    };
    cw = cw.clamp(BAR_W, 10);
    let total_w: u16 = cw.saturating_mul(BANDS as u16) + gaps_w;
    let x0 = bars_rect.x + (bars_rect.width.saturating_sub(total_w)) / 2;
    let gap = GAP;

    // fixed height: 25 rows => +12..0..-12
    let want_h: u16 = 25;
    let bars_h = if bars_rect.height >= want_h {
        want_h
    } else {
        bars_rect.height.max(3)
    };
    let y0 = bars_rect.y + (bars_rect.height.saturating_sub(bars_h)) / 2;

    // helper: map row index to db
    let row_to_db = |r: i32| -> i32 {
        if bars_h == want_h {
            // r: 0..24 => +12..-12
            12 - r
        } else {
            // fallback scale to +/-12
            let mid = (bars_h as i32) / 2;
            if r == mid {
                0
            } else if r < mid {
                let level = (mid - r) as f32;
                let max = mid.max(1) as f32;
                ((12.0 * (level / max)).round() as i32).clamp(0, 12)
            } else {
                let level = (r - mid) as f32;
                let max = (bars_h as i32 - 1 - mid).max(1) as f32;
                (-(12.0 * (level / max)).round() as i32).clamp(-12, 0)
            }
        }
    };

    let mut lines: Vec<Line> = Vec::with_capacity(bars_h as usize);
    for r in 0..bars_h {
        let rr = r as i32;
        let db_row = row_to_db(rr);

        let mut spans: Vec<ratatui::text::Span> = Vec::new();

        // left padding
        if x0 > bars_rect.x {
            spans.push(ratatui::text::Span::styled(
                " ".repeat((x0 - bars_rect.x) as usize),
                bg,
            ));
        }

        for b in 0..BANDS {
            let gain = gains[b].clamp(-12.0, 12.0).round() as i32;
            let filled = if db_row == 0 {
                false
            } else if db_row > 0 {
                // +1..+12: fill when row <= gain (e.g. gain=3 fills +1..+3)
                gain > 0 && db_row <= gain
            } else {
                // -1..-12: fill when row >= gain (e.g. gain=-5 fills -1..-5)
                gain < 0 && db_row >= gain
            };

            // Each column: center the 2-cell bar within fixed column width.
            let left_pad = cw.saturating_sub(BAR_W) / 2;
            let right_pad = cw.saturating_sub(BAR_W) - left_pad;
            let mut cell = String::new();
            cell.push_str(&" ".repeat(left_pad as usize));
            // 需求：零点(0dB)使用“▓▓”标识。
            if db_row == 0 {
                cell.push_str("▓▓");
            } else {
                cell.push_str(if filled { "██" } else { "░░" });
            }
            cell.push_str(&" ".repeat(right_pad as usize));
            if b + 1 < BANDS {
                cell.push_str(&" ".repeat(gap as usize));
            }

            // 需求：仅去除柱的选中效果（柱体不高亮）
            spans.push(ratatui::text::Span::styled(cell, text));
        }

        // right padding
        let drawn = (cw.saturating_mul(BANDS as u16)
            + gap.saturating_mul((BANDS as u16).saturating_sub(1)))
            + (x0 - bars_rect.x);
        if drawn < bars_rect.width {
            spans.push(ratatui::text::Span::styled(
                " ".repeat((bars_rect.width - drawn) as usize),
                bg,
            ));
        }

        lines.push(Line::from(spans));
    }

    let draw_rect = Rect {
        x: bars_rect.x,
        y: y0,
        width: bars_rect.width,
        height: bars_h,
    };
    f.render_widget(
        Paragraph::new(lines).style(bg).wrap(Wrap { trim: false }),
        draw_rect,
    );

    // bottom labels (two lines): keep frequency + always show numeric gain.
    let mut freq_spans: Vec<ratatui::text::Span> = Vec::new();
    let mut gain_spans: Vec<ratatui::text::Span> = Vec::new();
    if x0 > bars_rect.x {
        let pad = " ".repeat((x0 - bars_rect.x) as usize);
        freq_spans.push(ratatui::text::Span::styled(pad.clone(), bg));
        gain_spans.push(ratatui::text::Span::styled(pad, bg));
    }
    for b in 0..BANDS {
        let style = if b == app.eq_selected {
            selected_bg
        } else {
            sub
        };

        let mut ftxt = freq_labels[b].clone();
        if unicode_width::UnicodeWidthStr::width(ftxt.as_str()) as u16 > cw {
            ftxt = ftxt.chars().take(cw as usize).collect();
        }
        let fpad = cw.saturating_sub(unicode_width::UnicodeWidthStr::width(ftxt.as_str()) as u16);
        let fleft = fpad / 2;
        let fright = fpad - fleft;
        let mut fcell = format!(
            "{}{}{}",
            " ".repeat(fleft as usize),
            ftxt,
            " ".repeat(fright as usize)
        );
        if b + 1 < BANDS {
            fcell.push_str(&" ".repeat(gap as usize));
        }
        freq_spans.push(ratatui::text::Span::styled(fcell, style));

        let mut gtxt = gain_labels[b].clone();
        if unicode_width::UnicodeWidthStr::width(gtxt.as_str()) as u16 > cw {
            gtxt = gtxt.chars().take(cw as usize).collect();
        }
        let gpad = cw.saturating_sub(unicode_width::UnicodeWidthStr::width(gtxt.as_str()) as u16);
        let gleft = gpad / 2;
        let gright = gpad - gleft;
        let mut gcell = format!(
            "{}{}{}",
            " ".repeat(gleft as usize),
            gtxt,
            " ".repeat(gright as usize)
        );
        if b + 1 < BANDS {
            gcell.push_str(&" ".repeat(gap as usize));
        }
        gain_spans.push(ratatui::text::Span::styled(gcell, style));
    }
    f.render_widget(
        Paragraph::new(Line::from(freq_spans)).style(bg),
        freq_label_rect,
    );
    f.render_widget(
        Paragraph::new(Line::from(gain_spans)).style(bg),
        gain_label_rect,
    );
}

/// 下载图标格：爱心左侧隔一格（再左一位，与爱心之间留一个空格）。
/// 只在标题行画得下三格（图标 + 空格 + 爱心）时存在；更窄时整格不画、不可点。
pub(crate) fn download_cell(meta: Rect) -> Option<(u16, u16)> {
    let (heart_x, heart_y) = heart_cell(meta)?;
    if meta.width < 3 {
        return None;
    }
    Some((heart_x.saturating_sub(2), heart_y))
}

/// 标题行爱心所在的单元格（`compose_left_right_line` 把爱心右对齐到该行最后一格）。
///
/// `meta` 为 3 行块，只有首行画标题与爱心；未绘制（尺寸为 0）时返回 `None`。
fn heart_cell(meta: Rect) -> Option<(u16, u16)> {
    if meta.width == 0 || meta.height == 0 {
        return None;
    }
    Some((meta.x + meta.width - 1, meta.y))
}

pub fn hit_test(layout: &UiLayout, app: &AppState, col: u16, row: u16) -> Option<Action> {
    // Eq modal consumes clicks first
    if app.overlay == Overlay::EqModal {
        let area = centered_rect(layout.full, 44, 31);
        let inner = area.inner(ratatui::layout::Margin {
            horizontal: 1,
            vertical: 1,
        });
        if inner.height >= 3 {
            let bars_rect = Rect {
                x: inner.x,
                y: inner.y + 1,
                width: inner.width,
                height: inner.height.saturating_sub(3),
            };

            if contains(bars_rect, col, row) {
                const BANDS: usize = crate::tmplayer::app::state::EQ_BANDS;
                const BAR_W: u16 = 2;
                const GAP: u16 = 1;

                let gaps_w = GAP.saturating_mul((BANDS as u16).saturating_sub(1));
                let mut cw = if bars_rect.width > gaps_w {
                    (bars_rect.width - gaps_w) / (BANDS as u16)
                } else {
                    BAR_W
                };
                cw = cw.clamp(BAR_W, 10);
                let total_w: u16 = cw.saturating_mul(BANDS as u16) + gaps_w;
                let x0 = bars_rect.x + (bars_rect.width.saturating_sub(total_w)) / 2;
                if col < x0 || col >= x0 + total_w {
                    return None;
                }

                // Find band by fixed widths; then require click within the centered BAR_W region.
                let mut band: Option<usize> = None;
                for b in 0..BANDS {
                    let col_start = x0 + (b as u16) * (cw + GAP);
                    let col_end = col_start + cw;
                    if col >= col_start && col < col_end {
                        let left_pad = cw.saturating_sub(BAR_W) / 2;
                        let bar_start = col_start + left_pad;
                        let bar_end = bar_start + BAR_W;
                        if col < bar_start || col >= bar_end {
                            return None;
                        }
                        band = Some(b);
                        break;
                    }
                }

                let Some(band) = band else {
                    return None;
                };

                // fixed height mapping: prefer 25 rows (12..0..-12)
                let want_h: u16 = 25;
                let bars_h = if bars_rect.height >= want_h {
                    want_h
                } else {
                    bars_rect.height.max(3)
                };
                let y0 = bars_rect.y + (bars_rect.height.saturating_sub(bars_h)) / 2;
                if row < y0 || row >= y0 + bars_h {
                    return None;
                }
                let rr = (row - y0) as i32;

                let db_i = if bars_h == want_h {
                    (12 - rr).clamp(-12, 12)
                } else {
                    let mid = (bars_h as i32) / 2;
                    if rr == mid {
                        0
                    } else if rr < mid {
                        let level = (mid - rr) as f32;
                        let max = mid.max(1) as f32;
                        ((12.0 * (level / max)).round() as i32).clamp(0, 12)
                    } else {
                        let level = (rr - mid) as f32;
                        let max = (bars_h as i32 - 1 - mid).max(1) as f32;
                        (-(12.0 * (level / max)).round() as i32).clamp(-12, 0)
                    }
                };

                return Some(Action::EqSetBandDb {
                    band,
                    db: db_i as f32,
                });
            }
        }
    }

    // 其余弹窗盖住整页：未命中弹窗自身的点击一律吞掉，不许穿透到底层的
    // 进度条/音量/控制/播放列表（否则在设置弹窗上点一下就可能误播、
    // 误 seek、误改音量）。播放列表面板只占左栏，不在拦截范围内。
    if app.overlay != Overlay::None && app.overlay != Overlay::Playlist {
        // 设置类弹窗的条目行：单击聚焦（双击在事件循环里判定为 Enter）。
        if let Some(index) = layout.modal_rows.hit(col, row) {
            return Some(Action::ModalSelect(index));
        }
        return None;
    }

    if contains(layout.info_controls, col, row) {
        return control_buttons::hit_test(layout.info_controls, app, col, row);
    }

    // 爱心贴标题行右端，只有那一格可点；meta 下面的艺术字/专辑行没有爱心。
    if let Some((heart_x, heart_y)) = heart_cell(layout.info_meta) {
        if col == heart_x && row == heart_y {
            return Some(Action::ToggleFavorite);
        }
    }

    // 下载图标在爱心左侧一格；下载不可用（Hidden）时整格不画、不可点。
    if app.download_state != crate::tmplayer::DownloadIconState::Hidden
        && let Some((download_x, download_y)) = download_cell(layout.info_meta)
        && col == download_x
        && row == download_y
    {
        return Some(Action::ToggleDownload);
    }

    // 歌单浮层（含滑出动画）画在信息区之上：它盖住的那几行不再是"看得见的名字"，
    // 命中区按空名字处理（空名字本就不返回命中区），以免点在可见的歌单行上被判成
    // "点作者名/专辑名"而退出全屏页。浮层自己的行命中在下面按绘制顺序判定。
    let covered_by_playlist_panel =
        !layout.playlist_rect.is_empty() && contains(layout.playlist_rect, col, row);
    let (artist_hit, album_hit) = if covered_by_playlist_panel {
        ("", "")
    } else {
        (
            app.player.track.artist.as_str(),
            app.player.track.album.as_str(),
        )
    };

    // 作者名贴 meta 块第 2 行左端，多作者按字符位置分段（"A / B" 点谁的名字进谁）：
    // 只有名字画出来的那几格可点，连接符与行尾空白都不算。
    for (index, rect) in info_panel::artist_row_hits(layout.info_meta, artist_hit) {
        if contains(rect, col, row) {
            return Some(Action::OpenAuthorPage(index));
        }
    }

    // 专辑名贴 meta 块第 3 行左端，同样只算画出来的字符：点了打开专辑页。
    if contains(
        info_panel::album_row_rect(layout.info_meta, album_hit),
        col,
        row,
    ) {
        return Some(Action::OpenAlbumPage);
    }

    if let Some(volume) = volume_at(layout, col, row) {
        return Some(Action::SetVolume(volume));
    }

    if contains(layout.info_progress, col, row) {
        return Some(Action::SeekToFraction(ratio_in_track(
            layout.info_progress,
            col,
        )));
    }

    if contains(layout.playlist_list_inner, col, row) {
        // 渲染带虚拟滚动窗口 + 末尾 2 行 footer，命中区必须用同一份映射，
        // 否则列表滚过一屏后点到的不是看到的那首。
        let offset = row.saturating_sub(layout.playlist_list_inner.y) as usize;
        if offset < app.playlist_list_rows {
            return Some(Action::PlaylistSelect(app.playlist_list_scroll + offset));
        }
        return None;
    }

    None
}

/// 滚轮是否落在播放列表面板上（面板打开时才响应滚动聚焦）。
pub fn wheel_over_playlist(layout: &UiLayout, app: &AppState, col: u16, row: u16) -> bool {
    app.overlay == Overlay::Playlist && contains(layout.playlist_rect, col, row)
}

/// 音量条上某列对应的音量（0..=1）；不在条内时返回 `None`（点击用）。
pub fn volume_at(layout: &UiLayout, col: u16, row: u16) -> Option<f32> {
    contains(layout.info_volume, col, row).then(|| ratio_in_bar(layout.info_volume, col))
}

/// 按住拖动时的音量换算：列超出条子按端点钳制、不再要求落在条内，
/// 这样"起点在条内、拖出条外"仍持续生效。
pub fn volume_for_drag(layout: &UiLayout, col: u16) -> Option<f32> {
    (layout.info_volume.width > 2).then(|| ratio_in_bar(layout.info_volume, col))
}

fn contains(r: Rect, col: u16, row: u16) -> bool {
    col >= r.x && col < r.x + r.width && row >= r.y && row < r.y + r.height
}

fn ratio_in_bar(r: Rect, col: u16) -> f32 {
    if r.width <= 2 {
        return 0.0;
    }
    let inner = (r.width - 2) as f32;
    let x = col.saturating_sub(r.x + 1) as f32;
    (x / inner).clamp(0.0, 1.0)
}

fn ratio_in_track(r: Rect, col: u16) -> f32 {
    if r.width <= 1 {
        return 0.0;
    }
    let denom = (r.width - 1) as f32;
    let x = col.saturating_sub(r.x) as f32;
    (x / denom).clamp(0.0, 1.0)
}

pub(crate) fn lang_text<'a>(app: &AppState, zh: &'a str, en: &'a str) -> &'a str {
    match app.language {
        crate::data::config::Language::Zh => zh,
        crate::data::config::Language::En => en,
    }
}

fn lang_on_off(app: &AppState, enabled: bool) -> &'static str {
    match app.language {
        crate::data::config::Language::Zh => {
            if enabled {
                "开"
            } else {
                "关"
            }
        }
        crate::data::config::Language::En => {
            if enabled {
                "On"
            } else {
                "Off"
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tmplayer::ui::theme::{ColorCapability, Theme, ThemeName, ThemePalette};

    fn rect(x: u16, y: u16, width: u16, height: u16) -> Rect {
        Rect {
            x,
            y,
            width,
            height,
        }
    }

    fn state(overlay: Overlay) -> AppState {
        let theme = Theme {
            name: ThemeName::System,
            palette: ThemePalette {
                text: (255, 255, 255),
                subtext: (128, 128, 128),
                base: (0, 0, 0),
                surface: (16, 16, 16),
                buff: (64, 64, 64),
                accent: (255, 0, 0),
                accent2: (0, 255, 0),
                accent3: (0, 0, 255),
            },
            capability: ColorCapability::TrueColor,
        };
        let mut app = AppState::new(
            crate::tmplayer::data::config::Config::default(),
            theme,
            crate::data::config::Language::Zh,
        );
        app.overlay = overlay;
        app
    }

    /// 控件矩形都在左栏同一列上，互不重叠；用于验证"点击到底落在哪"。
    fn page_layout() -> UiLayout {
        UiLayout {
            info_controls: rect(0, 0, 30, 1),
            info_progress: rect(0, 1, 30, 1),
            info_volume: rect(0, 2, 30, 1),
            playlist_list_inner: rect(0, 3, 30, 4),
            ..UiLayout::default()
        }
    }

    /// 弹窗盖住整页：未命中弹窗自身的点击必须被吞掉，不许穿透到底层控件。
    #[test]
    fn modal_overlays_swallow_page_clicks() {
        let layout = page_layout();
        // 播放列表命中要按渲染窗口换算：给一行都放得下的窗口。
        let mut plain = state(Overlay::None);
        plain.playlist_list_scroll = 0;
        plain.playlist_list_rows = layout.playlist_list_inner.height as usize;

        // 先确认坐标确实压在活控件上，否则下面的断言是空的
        assert!(matches!(
            hit_test(&layout, &plain, 0, 1),
            Some(Action::SeekToFraction(_))
        ));
        assert!(matches!(
            hit_test(&layout, &plain, 0, 2),
            Some(Action::SetVolume(_))
        ));
        assert!(matches!(
            hit_test(&layout, &plain, 0, 3),
            Some(Action::PlaylistSelect(_))
        ));

        for overlay in [
            Overlay::SettingsModal,
            Overlay::BarSettingsModal,
            Overlay::LocalAudioSettingsModal,
            Overlay::AboutModal,
            Overlay::AcoustIdModal,
            Overlay::HelpModal,
            Overlay::EqModal,
        ] {
            let app = state(overlay);
            for row in [0u16, 1, 2, 3] {
                assert_eq!(
                    hit_test(&layout, &app, 0, row),
                    None,
                    "{overlay:?} 在 ({0},{row}) 的点击应被吞掉",
                    0
                );
            }
        }
    }

    /// 播放列表面板只占左栏，不拦整页。
    #[test]
    fn playlist_overlay_keeps_the_page_clickable() {
        let layout = page_layout();
        let app = state(Overlay::Playlist);

        assert_eq!(
            hit_test(&layout, &app, 0, 1),
            Some(Action::SeekToFraction(0.0))
        );
    }

    /// 爱心落在标题行（meta 块首行）最后一格，不是块内任意一行。
    #[test]
    fn heart_cell_is_the_last_column_of_the_first_meta_row() {
        let meta = rect(10, 4, 26, 3);

        assert_eq!(heart_cell(meta), Some((10 + 26 - 1, 4)));
        // 同一列的下面两行是艺术字/专辑，不是爱心
        assert_ne!(heart_cell(meta), Some((35, 5)));
        assert_ne!(heart_cell(meta), Some((35, 6)));
    }

    #[test]
    fn heart_cell_is_absent_when_the_row_is_not_drawn() {
        assert_eq!(heart_cell(Rect::default()), None);
        assert_eq!(heart_cell(rect(3, 7, 0, 3)), None);
        assert_eq!(heart_cell(rect(3, 7, 12, 0)), None);
    }

    /// 1 格宽的 meta 行：爱心就在那一格。
    #[test]
    fn heart_cell_handles_a_single_cell_row() {
        assert_eq!(heart_cell(rect(0, 0, 1, 1)), Some((0, 0)));
    }

    /// 标题行的点击落在爱心那一格才切收藏，其余位置不误触。
    #[test]
    fn clicking_the_heart_cell_toggles_favorite() {
        let layout = UiLayout {
            info_meta: rect(2, 5, 20, 3),
            ..UiLayout::default()
        };
        let app = state(Overlay::None);

        assert_eq!(
            hit_test(&layout, &app, 2 + 20 - 1, 5),
            Some(Action::ToggleFavorite)
        );
        assert_eq!(
            hit_test(&layout, &app, 2 + 20 - 2, 5),
            None,
            "标题文字不算爱心"
        );
        assert_eq!(
            hit_test(&layout, &app, 2 + 20 - 1, 6),
            None,
            "专辑行没有爱心"
        );
    }

    /// 编辑态只有路径值段落换底色（标签保持 modal 底色）。
    #[test]
    fn download_path_edit_paints_only_the_value_area() {
        let mut app = state(Overlay::DownloadSettingsModal);
        app.download_root = Some(std::path::PathBuf::from("/tmp/cnmplayer"));
        app.download_path_edit = Some(crate::app::DownloadPathEdit {
            buffer: "/tmp/cnmplayer".to_string(),
            cursor: 5,
            window_col: 0,
        });

        let (rows, buf) = render_to_buffer_sized(80, 24, &mut app, |f, app, rows| {
            render_download_settings_modal(f, f.area(), app, rows)
        });

        let path_row = rows.get(1).expect("路径行");
        let label_x = path_row.x + 2;
        let value_x = label_x + 10; // "下载路径" 4 个 CJK（8 列）+ ": "
        let surface = app.theme.color_surface();
        let buff = app.theme.color_buff();

        let label_cell = &buf[(label_x, path_row.y)];
        assert_eq!(label_cell.symbol(), "下");
        assert_eq!(label_cell.style().bg, Some(surface), "标签不换底色");

        let value_cell = &buf[(value_x, path_row.y)];
        assert_eq!(value_cell.symbol(), "/");
        assert_eq!(value_cell.style().bg, Some(buff), "路径值段落换底色");
        assert_eq!(value_cell.style().fg, Some(app.theme.color_text()));
    }
    /// 标题行右端实际渲染出来的三格：下载图标、空格、爱心（与命中格同源）。
    #[test]
    fn info_title_row_renders_download_then_gap_then_heart() {
        let mut app = state(Overlay::None);
        app.player.track.title = "Title".to_string();
        app.download_state = crate::tmplayer::DownloadIconState::NotDownloaded;

        let (_, buf) = render_to_buffer_sized(120, 40, &mut app, |f, app, _rows| {
            let area = f.area();
            info_panel::render(f, area, area.width, app);
        });

        let left = ratatui::layout::Rect::new(0, 0, 120, 40);
        let layout = info_panel::layout(left, 120);
        let (download_x, y) = download_cell(layout.meta).expect("下载格");
        let (heart_x, _) = heart_cell(layout.meta).expect("爱心格");

        assert_eq!(heart_x, download_x + 2, "下载图标与爱心之间隔一格");
        assert_eq!(
            buf[(download_x, y)].symbol(),
            crate::app::download::ICON_DOWNLOAD.to_string()
        );
        assert_eq!(buf[(download_x + 1, y)].symbol(), " ");
        assert_eq!(buf[(heart_x, y)].symbol(), "\u{f08a}");
    }

    /// 下载图标格在爱心左侧隔一格；画不下三格时留给爱心，不画也不可点。
    #[test]
    fn download_cell_sits_left_of_the_heart() {
        let meta = rect(10, 4, 26, 3);
        assert_eq!(download_cell(meta), Some((10 + 26 - 3, 4)));
        assert_eq!(download_cell(rect(0, 0, 2, 1)), None);
        assert_eq!(download_cell(rect(0, 0, 1, 1)), None);
        assert_eq!(download_cell(Rect::default()), None);
    }

    /// 下载可用时，爱心左边隔一格是下载按钮；下载不可用（Hidden）时整格不命中。
    #[test]
    fn clicking_the_download_cell_toggles_download_only_when_visible() {
        let layout = UiLayout {
            info_meta: rect(2, 5, 20, 3),
            ..UiLayout::default()
        };

        let mut app = state(Overlay::None);
        app.download_state = crate::tmplayer::DownloadIconState::NotDownloaded;
        assert_eq!(
            hit_test(&layout, &app, 2 + 20 - 3, 5),
            Some(Action::ToggleDownload)
        );
        assert_eq!(
            hit_test(&layout, &app, 2 + 20 - 2, 5),
            None,
            "图标与爱心之间是空格"
        );
        assert_eq!(
            hit_test(&layout, &app, 2 + 20 - 1, 5),
            Some(Action::ToggleFavorite),
            "下载图标不影响爱心那一格"
        );

        let hidden = state(Overlay::None);
        assert_eq!(
            hidden.download_state,
            crate::tmplayer::DownloadIconState::Hidden
        );
        assert_eq!(hit_test(&layout, &hidden, 2 + 20 - 3, 5), None);
    }

    /// 作者名贴 meta 块第 2 行左端：只有名字画出来的那几格可点，行尾空白不算。
    #[test]
    fn clicking_the_artist_row_opens_the_author_page() {
        let layout = UiLayout {
            info_meta: rect(2, 5, 20, 3),
            ..UiLayout::default()
        };
        let mut app = state(Overlay::None);
        app.player.track.artist = "Jay".to_string();

        assert_eq!(
            hit_test(&layout, &app, 2, 6),
            Some(Action::OpenAuthorPage(0))
        );
        assert_eq!(
            hit_test(&layout, &app, 4, 6),
            Some(Action::OpenAuthorPage(0))
        );
        assert_eq!(hit_test(&layout, &app, 5, 6), None, "作者行行尾空白不算");
        assert_eq!(hit_test(&layout, &app, 2, 5), None, "标题行不是作者行");
        assert_ne!(
            hit_test(&layout, &app, 2, 7),
            Some(Action::OpenAuthorPage(0)),
            "专辑行不打开作者页"
        );
    }

    /// 多作者（"A / B"）：按字符位置分段，点谁的名字进谁的页面，连接符不算。
    #[test]
    fn clicking_each_artist_segment_opens_that_artist() {
        let layout = UiLayout {
            info_meta: rect(2, 5, 20, 3),
            ..UiLayout::default()
        };
        let mut app = state(Overlay::None);
        app.player.track.artist = "Caffeine / 初音ミク".to_string();

        // 行首在 x=2："Caffeine" 占 2..=9，" / " 占 10..=12，"初音ミク" 占 13..=20。
        assert_eq!(
            hit_test(&layout, &app, 2, 6),
            Some(Action::OpenAuthorPage(0))
        );
        assert_eq!(
            hit_test(&layout, &app, 9, 6),
            Some(Action::OpenAuthorPage(0))
        );
        assert_eq!(hit_test(&layout, &app, 10, 6), None, "连接符不是作者名");
        assert_eq!(hit_test(&layout, &app, 12, 6), None, "连接符不是作者名");
        assert_eq!(
            hit_test(&layout, &app, 13, 6),
            Some(Action::OpenAuthorPage(1))
        );
        assert_eq!(
            hit_test(&layout, &app, 20, 6),
            Some(Action::OpenAuthorPage(1))
        );
        assert_eq!(hit_test(&layout, &app, 21, 6), None, "第二个名字画完了");

        // 行宽不足以放下整段时，只算画出来的那几格（"初" 两格，"音" 放不下）。
        let narrow = UiLayout {
            info_meta: rect(0, 0, 13, 3),
            ..UiLayout::default()
        };
        assert_eq!(
            hit_test(&narrow, &app, 12, 1),
            Some(Action::OpenAuthorPage(1))
        );
        assert_eq!(hit_test(&narrow, &app, 13, 1), None, "meta 只有 13 格");
    }

    /// 专辑名贴 meta 块第 3 行左端：同样只算画出来的字符。
    #[test]
    fn clicking_the_album_row_opens_the_album_page() {
        let layout = UiLayout {
            info_meta: rect(2, 5, 20, 3),
            ..UiLayout::default()
        };
        let mut app = state(Overlay::None);
        app.player.track.artist.clear();
        app.player.track.album = "Album".to_string();

        assert_eq!(hit_test(&layout, &app, 2, 7), Some(Action::OpenAlbumPage));
        assert_eq!(hit_test(&layout, &app, 6, 7), Some(Action::OpenAlbumPage));
        assert_eq!(hit_test(&layout, &app, 7, 7), None, "专辑行行尾空白不算");
        assert_eq!(
            hit_test(&layout, &app, 2, 6),
            None,
            "作者行走作者页（这里作者名为空），不是专辑页"
        );

        let two_rows = UiLayout {
            info_meta: rect(2, 5, 20, 2),
            ..UiLayout::default()
        };
        assert_eq!(
            hit_test(&two_rows, &app, 2, 7),
            None,
            "meta 只有两行时专辑行没画"
        );
    }

    #[test]
    fn playlist_panel_takes_the_clicks_over_the_names_it_covers() {
        // 浮层整块盖住左栏（无封面时列表内区就是整条左栏）：它的行命中排在最后，
        // 但被它盖住的作者名/专辑名必须让位，否则点在可见的歌单行上会退出全屏页。
        let mut app = state(Overlay::Playlist);
        app.player.track.artist = "Jay".to_string();
        app.player.track.album = "Fantasy".to_string();
        app.playlist_list_scroll = 0;
        app.playlist_list_rows = 10;
        let layout = UiLayout {
            info_meta: rect(2, 5, 20, 3),
            playlist_rect: rect(0, 4, 24, 12),
            playlist_list_inner: rect(1, 5, 22, 10),
            ..UiLayout::default()
        };

        assert_eq!(
            hit_test(&layout, &app, 2, 6),
            Some(Action::PlaylistSelect(1))
        );
        assert_eq!(
            hit_test(&layout, &app, 2, 7),
            Some(Action::PlaylistSelect(2))
        );

        // 浮层收起（滑出动画结束）后名字重新可点：命中区只看实际绘制的那块矩形。
        let uncovered = UiLayout {
            info_meta: rect(2, 5, 20, 3),
            ..UiLayout::default()
        };
        assert_eq!(
            hit_test(&uncovered, &app, 2, 6),
            Some(Action::OpenAuthorPage(0))
        );
        assert_eq!(
            hit_test(&uncovered, &app, 2, 7),
            Some(Action::OpenAlbumPage)
        );
    }

    /// 命中宽度按显示宽度算：中日韩名字一个字占两格。
    #[test]
    fn artist_hit_region_uses_display_width() {
        let layout = UiLayout {
            info_meta: rect(0, 0, 10, 3),
            ..UiLayout::default()
        };
        let mut app = state(Overlay::None);
        app.player.track.artist = "周杰伦".to_string();

        assert_eq!(
            hit_test(&layout, &app, 5, 1),
            Some(Action::OpenAuthorPage(0))
        );
        assert_eq!(hit_test(&layout, &app, 6, 1), None, "名字只有 6 格宽");
    }

    /// 作者行没画出来（meta 不够高 / 名字为空）时不登记命中区。
    #[test]
    fn artist_hit_region_is_absent_when_the_row_is_not_drawn() {
        let mut app = state(Overlay::None);
        app.player.track.artist = "Jay".to_string();

        let one_row = UiLayout {
            info_meta: rect(2, 5, 20, 1),
            ..UiLayout::default()
        };
        assert_eq!(hit_test(&one_row, &app, 2, 6), None, "只有标题行");

        let three_rows = UiLayout {
            info_meta: rect(2, 5, 20, 3),
            ..UiLayout::default()
        };
        assert_eq!(
            hit_test(&three_rows, &app, 2, 6),
            Some(Action::OpenAuthorPage(0))
        );

        app.player.track.artist.clear();
        assert_eq!(
            hit_test(&three_rows, &app, 2, 6),
            None,
            "空名字不登记命中区"
        );

        app.player.track.artist = "Jay".to_string();
        assert_eq!(
            hit_test(&UiLayout::default(), &app, 0, 1),
            None,
            "整个信息区没画时零矩形不命中"
        );
    }

    /// 设置弹窗：命中条目行 → ModalSelect(序号)，弹窗内其余位置仍被吞掉。
    #[test]
    fn settings_modal_rows_are_clickable() {
        let mut layout = page_layout();
        layout.modal_rows.push(rect(2, 10, 20, 1), 0);
        layout.modal_rows.push(rect(2, 11, 20, 1), 1);
        layout.modal_rows.push(rect(2, 12, 20, 1), 2);

        let app = state(Overlay::SettingsModal);

        assert_eq!(hit_test(&layout, &app, 5, 10), Some(Action::ModalSelect(0)));
        assert_eq!(hit_test(&layout, &app, 5, 11), Some(Action::ModalSelect(1)));
        assert_eq!(hit_test(&layout, &app, 5, 12), Some(Action::ModalSelect(2)));
        assert_eq!(hit_test(&layout, &app, 5, 13), None, "行外不命中");
        assert_eq!(hit_test(&layout, &app, 0, 1), None, "底层进度条仍被吞掉");
    }

    /// 空矩形与超上限的行不登记（渲染本来就画不出来）。
    #[test]
    fn modal_rows_ignore_degenerate_and_overflowing_rows() {
        let mut rows = ModalRows::default();
        rows.push(Rect::default(), 0);
        assert_eq!(rows.hit(0, 0), None);

        for idx in 0..(ModalRows::MAX + 5) {
            rows.push(rect(0, idx as u16, 10, 1), idx);
        }

        assert_eq!(rows.get(ModalRows::MAX), None, "超上限的行被丢弃");
        assert_eq!(
            rows.get(ModalRows::MAX - 1),
            Some(rect(0, (ModalRows::MAX - 1) as u16, 10, 1))
        );
        assert_eq!(rows.hit(3, 0), Some(0));
    }

    /// 登记的是条目序号而不是登记次序：滚动过的窗口与固定在底部的行
    /// 都要能按条目号取回（否则点击会落到别的条目上）。
    #[test]
    fn modal_rows_key_on_item_index_not_registration_order() {
        let mut rows = ModalRows::default();
        rows.push(rect(0, 5, 10, 1), 7);
        rows.push(rect(0, 6, 10, 1), 8);
        rows.push(rect(0, 9, 10, 1), 11);

        assert_eq!(rows.hit(1, 5), Some(7), "窗口首行回的是条目序号");
        assert_eq!(rows.hit(1, 6), Some(8));
        assert_eq!(rows.hit(1, 9), Some(11));
        assert_eq!(rows.get(7), Some(rect(0, 5, 10, 1)));
        assert_eq!(rows.get(11), Some(rect(0, 9, 10, 1)));
        assert_eq!(rows.get(0), None, "没画出来的条目不登记");
        assert_eq!(rows.get(10), None);
    }

    /// 播放列表命中区必须用渲染的虚拟滚动窗口：滚过一屏后点到的仍是看到的那首，
    /// 末尾两行 footer 不命中。
    #[test]
    fn playlist_click_uses_the_render_window() {
        let layout = UiLayout {
            playlist_list_inner: rect(0, 10, 30, 6),
            ..UiLayout::default()
        };
        let mut app = state(Overlay::Playlist);
        app.playlist_list_scroll = 7;
        app.playlist_list_rows = 4;

        assert_eq!(
            hit_test(&layout, &app, 1, 10),
            Some(Action::PlaylistSelect(7)),
            "首行对应窗口起点"
        );
        assert_eq!(
            hit_test(&layout, &app, 1, 13),
            Some(Action::PlaylistSelect(10))
        );
        assert_eq!(hit_test(&layout, &app, 1, 14), None, "footer 行不命中");
        assert_eq!(hit_test(&layout, &app, 1, 15), None, "footer 行不命中");
    }

    /// 滚轮只在播放列表面板内生效（面板未打开时不响应）。
    #[test]
    fn wheel_scrolls_only_over_the_playlist_panel() {
        let layout = UiLayout {
            playlist_rect: rect(2, 5, 30, 20),
            ..UiLayout::default()
        };

        let open = state(Overlay::Playlist);
        assert!(wheel_over_playlist(&layout, &open, 10, 10));
        assert!(!wheel_over_playlist(&layout, &open, 40, 10), "面板外不响应");
        assert!(
            !wheel_over_playlist(&layout, &state(Overlay::None), 10, 10),
            "面板未打开时不响应"
        );
    }

    /// 音量拖动与点击用同一条换算（否则按住拖会和点一下的落点不一致）。
    #[test]
    fn volume_at_matches_the_volume_bar() {
        let layout = UiLayout {
            info_volume: rect(0, 2, 12, 1),
            ..UiLayout::default()
        };
        assert_eq!(volume_at(&layout, 0, 2), Some(0.0));
        assert_eq!(volume_at(&layout, 11, 2), Some(1.0));
        assert_eq!(volume_at(&layout, 6, 2), Some(0.5));
        assert_eq!(volume_at(&layout, 12, 2), None, "条外不响应");
        assert_eq!(volume_at(&layout, 6, 3), None, "行外不响应");
    }

    /// 拖动一旦从条内开始就不再要求光标留在条上：拖出左右边界按端点钳制，
    /// 上下拖出行也照样生效。
    #[test]
    fn volume_drag_clamps_outside_the_bar() {
        let layout = UiLayout {
            info_volume: rect(0, 2, 12, 1),
            ..UiLayout::default()
        };

        assert_eq!(volume_for_drag(&layout, 0), Some(0.0));
        assert_eq!(volume_for_drag(&layout, 11), Some(1.0));
        assert_eq!(
            volume_for_drag(&layout, 200),
            Some(1.0),
            "拖到右边之外仍生效"
        );
        assert_eq!(volume_for_drag(&layout, 6), Some(0.5));

        // 音量条没有绘制（宽度退化）时不改音量。
        let empty = UiLayout::default();
        assert_eq!(volume_for_drag(&empty, 3), None);
    }

    /// 按键提示弹窗的条目数必须由列表本身决定：写死会让末尾几行选不中
    /// （历史上常量 14 对不上 16 行就是这么来的），小窗口切换也已从设置弹窗搬到这里。
    #[test]
    fn help_items_cover_every_row_and_include_the_moved_hint() {
        let app = state(Overlay::HelpModal);
        let items = help_items(&app);

        assert_eq!(help_item_count(&app), items.len(), "翻页计数与列表同源");
        assert!(items.len() > 14, "条目数应随列表增长，不再写死");
        assert!(
            items.iter().any(|(label, _)| label.contains("小窗口")),
            "“小窗口切换显示”应从设置弹窗移到按键提示里"
        );
    }

    /// 读回一行的实际渲染文本（用 TestBackend 的缓冲，等价于看画面）。
    fn line_text(buf: &ratatui::buffer::Buffer, y: u16) -> String {
        let area = *buf.area();
        (area.x..area.x + area.width)
            .map(|x| buf[(x, y)].symbol())
            .collect()
    }

    /// 去掉空白后再比较：宽字符（CJK）在缓冲里会占一个额外空位。
    fn compact(text: &str) -> String {
        text.chars().filter(|c| !c.is_whitespace()).collect()
    }

    fn render_to_buffer(
        app: &mut AppState,
        draw: impl FnOnce(&mut ratatui::Frame, &mut AppState, &mut ModalRows),
    ) -> (ModalRows, ratatui::buffer::Buffer) {
        render_to_buffer_sized(80, 24, app, draw)
    }

    /// 指定尺寸渲染：矮终端下弹窗条目区会被截断，命中区必须跟着截断。
    fn render_to_buffer_sized(
        width: u16,
        height: u16,
        app: &mut AppState,
        draw: impl FnOnce(&mut ratatui::Frame, &mut AppState, &mut ModalRows),
    ) -> (ModalRows, ratatui::buffer::Buffer) {
        let backend = ratatui::backend::TestBackend::new(width, height);
        let mut terminal = ratatui::Terminal::new(backend).expect("terminal");
        let mut rows = ModalRows::default();
        terminal.draw(|f| draw(f, app, &mut rows)).expect("draw");
        let buf = terminal.backend().buffer().clone();
        (rows, buf)
    }

    /// 歌词浮窗弹窗：命中行必须落在真正画着该条目的那一行上
    /// （否则就是"看得见点不动"或"点到的不是看到的"）。
    #[test]
    fn lyrics_modal_hit_rows_sit_on_the_drawn_rows() {
        let mut app = state(Overlay::LyricsSettingsModal);
        let (rows, buf) = render_to_buffer(&mut app, |f, app, rows| {
            render_lyrics_settings_modal(f, f.area(), app, rows)
        });

        let expected = ["歌词浮窗:", "歌词浮窗拖动:", "歌词浮窗边缘吸附:"];
        assert_eq!(rows.len(), expected.len());

        for (idx, label) in expected.iter().enumerate() {
            let rect = rows.get(idx).expect("登记了命中行");
            let text = line_text(&buf, rect.y);
            assert!(
                compact(&text).contains(&compact(label)),
                "第 {idx} 行画的是 {text:?}"
            );
        }
    }

    /// 按键提示弹窗：命中行落在条目行上，且移到这里的“小窗口切换显示”能显示出来。
    #[test]
    fn help_modal_hit_rows_sit_on_the_drawn_rows() {
        let mut app = state(Overlay::HelpModal);
        let items = help_items(&app);
        let moved = items
            .iter()
            .position(|(label, _)| label.contains("小窗口"))
            .expect("小窗口条目");
        // 选中它，渲染窗口就会滚到它，从而一定在命中区内。
        app.help_keybind_selected = moved;

        let (rows, buf) = render_to_buffer(&mut app, |f, app, rows| {
            render_help_modal(f, f.area(), app, rows)
        });

        assert!(rows.len() > 0, "条目行要登记出来才能点");
        let mut saw_hint = false;
        for idx in 0..rows.len() {
            let rect = rows.get(idx).expect("命中行");
            let text = compact(&line_text(&buf, rect.y));
            assert!(
                items
                    .iter()
                    .any(|(label, key)| text.contains(&compact(label))
                        && text.contains(&compact(key))),
                "命中行落在非条目行上: {text:?}"
            );
            saw_hint |= text.contains(&compact("小窗口"));
        }
        assert!(saw_hint, "“小窗口切换显示”应出现在按键提示弹窗里");
    }

    /// 矮终端下设置弹窗的条目区放不下全部条目，但 about 行仍要按条目序号 12
    /// 登记——否则单击它会选中别的条目、双击会执行别的条目。
    #[test]
    fn settings_modal_about_row_keeps_its_item_index_when_items_are_truncated() {
        let mut app = state(Overlay::SettingsModal);
        let (rows, buf) = render_to_buffer_sized(80, 18, &mut app, |f, app, rows| {
            render_settings_modal(f, f.area(), app, rows)
        });

        let about = rows.get(12).expect("about 行按条目序号 12 登记");
        assert!(
            compact(&line_text(&buf, about.y)).contains("about"),
            "12 号条目行画的是 about"
        );
        assert!(rows.len() < 13, "18 行终端下条目区放不下 12 条");

        let mut layout = page_layout();
        layout.modal_rows = rows;
        assert_eq!(
            hit_test(&layout, &app, about.x, about.y),
            Some(Action::ModalSelect(12)),
            "点 about 行要回条目序号 12"
        );
    }

    /// 按键提示弹窗滚动后，每一行的命中序号必须等于画在该行的条目序号
    /// （否则点“第 2 行显示的那条”会选中上一条）。
    #[test]
    fn help_modal_hit_index_matches_the_row_after_scrolling() {
        let mut app = state(Overlay::HelpModal);
        let items = help_items(&app);
        // 选末条：24 行终端放不下 17 条，渲染窗口必然滚动，滚动量 = 1。
        app.help_keybind_selected = items.len() - 1;

        let (rows, buf) = render_to_buffer(&mut app, |f, app, rows| {
            render_help_modal(f, f.area(), app, rows)
        });

        let mut hits = 0;
        for y in 0..buf.area().height {
            let Some(index) = (0..buf.area().width).find_map(|x| rows.hit(x, y)) else {
                continue;
            };
            let text = compact(&line_text(&buf, y));
            let (label, key) = &items[index];
            assert!(
                text.contains(&compact(label)) && text.contains(&compact(key)),
                "第 {y} 行的命中序号 {index} 与画面 {text:?} 不符"
            );
            hits += 1;
        }
        assert_eq!(hits, rows.len(), "登记的行都该画在屏幕上");
        assert!(hits > 0, "至少要有一行可点");
    }
}
