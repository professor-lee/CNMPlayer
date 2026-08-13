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
use ratatui::text::Line;
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

    pub info_cover_image: Rect,

    pub playlist_rect: Rect,
    pub playlist_inner: Rect,
    pub playlist_list_inner: Rect,

    pub playlist_cover_image: Rect,

    pub spectrum_rect: Rect,
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

        let mut layout_out = UiLayout::default();

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

            let cols = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Percentage(33), Constraint::Percentage(67)])
                .split(size);
            layout_out.left = cols[0];
            layout_out.right = cols[1];
            layout_out.left_width = cols[0].width;

            // right: lyrics (10%) + spectrum (rest)
            let lyric_h = ((cols[1].height as f32) * 0.10).round() as u16;
            let lyric_h = lyric_h.clamp(3, cols[1].height.saturating_sub(6));
            let rows = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Length(lyric_h), Constraint::Min(1)])
                .split(cols[1]);

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

            let info_l = info_panel::layout(cols[0]);
            layout_out.info_progress = info_l.progress;
            layout_out.info_volume = info_l.volume;
            layout_out.info_controls = info_l.controls;

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

            info_panel::render(f, cols[0], app);
            visual_panel::render(f, rows[0], rows[1], app);

            // playlist overlay slides in/out over left
            if app.overlay == Overlay::Playlist
                || app.playlist_slide_x != app.playlist_slide_target_x
            {
                let collapsing = app.overlay != Overlay::Playlist
                    && app.playlist_slide_x > app.playlist_slide_target_x;

                // advance animation
                let step: i16 = 4;
                if app.playlist_slide_x < app.playlist_slide_target_x {
                    app.playlist_slide_x =
                        (app.playlist_slide_x + step).min(app.playlist_slide_target_x);
                } else if app.playlist_slide_x > app.playlist_slide_target_x {
                    app.playlist_slide_x =
                        (app.playlist_slide_x - step).max(app.playlist_slide_target_x);
                }

                // Slide effect via visible width growth/shrink (x stays at left edge)
                let full_w = cols[0].width as i16;
                let visible_w = (full_w + app.playlist_slide_x).clamp(0, full_w) as u16;
                if visible_w > 0 {
                    let r = Rect {
                        x: cols[0].x,
                        y: cols[0].y,
                        width: visible_w,
                        height: cols[0].height,
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
                let hint_text = lang_text(app, "Ctrl+K 打开按键绑定", "Ctrl+K open keybinds");
                let hint_area = Rect {
                    x: size.x,
                    y: size.y + size.height.saturating_sub(1),
                    width: size.width,
                    height: 1,
                };
                let mut hint_style = Style::default().fg(app.theme.color_subtext());
                if !app.config.transparent_background {
                    hint_style = hint_style.bg(app.theme.color_base());
                }
                f.render_widget(
                    Paragraph::new(format!(" {}", hint_text)).style(hint_style),
                    hint_area,
                );
            }

            // Paint kitty images on top of ratatui widgets.
            Self::paint_kitty_images(&mut self.graphics_overlay, f, app, &layout_out);

            // modals (top-most)
            match app.overlay {
                Overlay::SettingsModal => render_settings_modal(f, size, app),
                Overlay::BarSettingsModal => render_bar_settings_modal(f, size, app),
                Overlay::LocalAudioSettingsModal => render_local_audio_settings_modal(f, size, app),
                Overlay::AboutModal => render_about_modal(f, size, app),
                Overlay::AcoustIdModal => render_acoustid_modal(f, size, app),
                Overlay::HelpModal => render_help_modal(f, size, app),
                Overlay::EqModal => render_eq_modal(f, size, app),
                _ => {}
            }
        })?;

        Ok(layout_out)
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

fn render_settings_modal(f: &mut ratatui::Frame, size: Rect, app: &mut AppState) {
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
        format!(
            "{}: {}",
            lang_text(app, "显示提示", "Show Hints"),
            lang_on_off(app, app.config.show_hints)
        ),
        format!(
            "{}: {}",
            lang_text(app, "主页更多推荐", "More Home Recommendations"),
            lang_on_off(app, app.config.home_more_recommend)
        ),
        lang_text(app, "退出登录", "Logout").to_string(),
        "about".to_string(),
    ];

    for (idx, text) in items.iter().enumerate() {
        let style = if idx == app.settings_selected {
            Style::default()
                .fg(app.theme.color_accent2())
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(app.theme.color_text())
        };
        f.render_widget(
            Paragraph::new(Line::styled(format!("  {}", text), style)),
            Rect {
                x: rows[1].x,
                y: rows[1].y + idx as u16,
                width: rows[1].width,
                height: 1,
            },
        );
    }

    f.render_widget(Paragraph::new(""), rows[2]);
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

fn render_bar_settings_modal(f: &mut ratatui::Frame, size: Rect, app: &mut AppState) {
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
                crate::tmplayer::data::config::VisualizeMode::Off => lang_text(app, "关闭", "Off"),
                crate::tmplayer::data::config::VisualizeMode::Bars =>
                    lang_text(app, "频谱", "Bars"),
                crate::tmplayer::data::config::VisualizeMode::Oscilloscope => {
                    lang_text(app, "示波器", "Oscilloscope")
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
            lang_text(app, "页面歌词", "Page Lyrics"),
            lang_on_off(app, app.config.page_lyrics)
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
        let style = if idx == 0 && !crate::tmplayer::audio::cava::is_available() {
            Style::default().fg(app.theme.color_subtext())
        } else if idx == app.bar_settings_selected {
            Style::default()
                .fg(app.theme.color_accent2())
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(app.theme.color_text())
        };
        f.render_widget(
            Paragraph::new(Line::styled(format!("  {}", text), style)),
            Rect {
                x: rows[1].x,
                y: rows[1].y + idx as u16,
                width: rows[1].width,
                height: 1,
            },
        );
    }

    f.render_widget(Paragraph::new(""), rows[2]);
}

fn render_local_audio_settings_modal(f: &mut ratatui::Frame, size: Rect, app: &mut AppState) {
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

    let max_line_len = rendered
        .iter()
        .map(|l| l.chars().count())
        .max()
        .unwrap_or(0)
        .min(max_width);
    let block_h = rendered.len() as u16;
    let block_w = max_line_len.max(1) as u16;
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

    let mut out: Vec<String> = Vec::new();
    let mut buf = String::new();
    for ch in s.chars() {
        if buf.chars().count() >= width {
            out.push(buf);
            buf = String::new();
        }
        buf.push(ch);
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

fn render_help_modal(f: &mut ratatui::Frame, size: Rect, app: &mut AppState) {
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

    let items = [
        (
            lang_text(app, "搜索框", "Search Box"),
            app.config.keybind_search_box.as_str(),
        ),
        (
            lang_text(app, "全屏播放页", "Fullscreen"),
            app.config.keybind_fullscreen.as_str(),
        ),
        (
            lang_text(app, "设置弹窗", "Settings Modal"),
            app.config.keybind_settings.as_str(),
        ),
        (
            lang_text(app, "侧边栏", "Sidebar"),
            app.config.keybind_sidebar.as_str(),
        ),
        (
            lang_text(app, "退出应用", "Quit"),
            app.config.keybind_quit.as_str(),
        ),
        (
            lang_text(app, "快速上翻页（主程序）", "Quick Page Up (Host)"),
            app.config.keybind_page_up.as_str(),
        ),
        (
            lang_text(app, "快速下翻页（主程序）", "Quick Page Down (Host)"),
            app.config.keybind_page_down.as_str(),
        ),
        (
            lang_text(app, "上一首", "Previous"),
            app.config.keybind_fullscreen_prev.as_str(),
        ),
        (
            lang_text(app, "下一首", "Next"),
            app.config.keybind_fullscreen_next.as_str(),
        ),
        (
            lang_text(app, "播放/暂停", "Play/Pause"),
            app.config.keybind_fullscreen_toggle_play_pause.as_str(),
        ),
        (
            lang_text(app, "全屏模式切换", "Fullscreen Mode Switch"),
            app.config.keybind_fullscreen_toggle_mode.as_str(),
        ),
        (
            lang_text(app, "EQ均衡器", "EQ Equalizer"),
            app.config.keybind_fullscreen_eq.as_str(),
        ),
        (
            lang_text(app, "EQ重置", "EQ Reset"),
            app.config.keybind_fullscreen_eq_reset.as_str(),
        ),
        (
            lang_text(app, "收藏/取消收藏", "Like/Unlike"),
            app.config.keybind_toggle_like_fullscreen.as_str(),
        ),
        (
            lang_text(app, "侧边栏歌单区切换", "Sidebar Playlist Section Switch"),
            "Ctrl+Up/Down",
        ),
        (lang_text(app, "按键绑定", "Keybinds"), "Ctrl+K"),
    ];

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
        f.render_widget(
            Paragraph::new(Line::styled(format!("  {}: {}", label, key), style)),
            Rect {
                x: rows[1].x,
                y: rows[1].y + (idx - scroll) as u16,
                width: rows[1].width,
                height: 1,
            },
        );
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

    if contains(layout.info_controls, col, row) {
        return control_buttons::hit_test(layout.info_controls, app, col, row);
    }

    if contains(layout.info_volume, col, row) {
        return Some(Action::SetVolume(ratio_in_bar(layout.info_volume, col)));
    }

    if contains(layout.info_progress, col, row) {
        return Some(Action::SeekToFraction(ratio_in_track(
            layout.info_progress,
            col,
        )));
    }

    if contains(layout.playlist_list_inner, col, row) {
        let idx = row.saturating_sub(layout.playlist_list_inner.y) as usize;
        return Some(Action::PlaylistSelect(idx));
    }

    None
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

fn lang_text<'a>(app: &AppState, zh: &'a str, en: &'a str) -> &'a str {
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
