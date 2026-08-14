use crate::app::{App, HitRect, PlaybackRuntimeState, PlayerBarHitTargets};
use ratatui::Frame;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use std::sync::LazyLock;
use std::time::{Duration, Instant};

/// 脉冲动画的时间基准：进程启动时初始化。
/// 注意不能用 Instant::now().elapsed()——那是“当前时刻到当前时刻”，恒为 0，
/// 会导致波形静止不动。
static ANIM_EPOCH: LazyLock<Instant> = LazyLock::new(Instant::now);
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

pub const PLAYER_BAR_HEIGHT: u16 = 5;

pub fn draw_collapsed_player_bar(frame: &mut Frame, app: &mut App, area: Rect) {
    frame.render_widget(
        Block::default()
            .borders(Borders::TOP)
            .border_style(Style::default().fg(app.theme.color_surface()))
            .style(base_bg_style(app)),
        area,
    );

    let inner = area.inner(ratatui::layout::Margin {
        horizontal: 1,
        vertical: 1,
    });
    if inner.width == 0 || inner.height == 0 {
        return;
    }

    let top = Rect {
        x: inner.x,
        y: inner.y,
        width: inner.width,
        height: 1,
    };
    let bottom = Rect {
        x: inner.x,
        y: inner.y + inner.height.saturating_sub(1),
        width: inner.width,
        height: 1,
    };

    let prev_label = "[]";
    let play_label = if app.playback_state == PlaybackRuntimeState::Playing {
        "[]"
    } else {
        "[]"
    };
    let next_label = "[]";
    let mode_symbol = playback_repeat_symbol(app);
    let controls = format!("{prev_label} {play_label} {next_label} {mode_symbol}");

    let spectrum =
        if app.now_playing.is_some() && app.playback_state != PlaybackRuntimeState::Stopped {
            app.main_spectrum_braille()
        } else {
            " ".repeat(10)
        };

    let controls_w = display_width(&controls) as u16;
    let spectrum_w = display_width(&spectrum).min(10) as u16;

    let controls_col_w = controls_w.saturating_add(2).min(top.width);
    let spectrum_col_w = spectrum_w.min(top.width.saturating_sub(controls_col_w));
    let left_col_w = top
        .width
        .saturating_sub(controls_col_w)
        .saturating_sub(spectrum_col_w);

    let left_rect = Rect {
        x: top.x,
        y: top.y,
        width: left_col_w,
        height: 1,
    };
    let controls_rect = Rect {
        x: left_rect.x + left_rect.width,
        y: top.y,
        width: controls_col_w,
        height: 1,
    };
    let spectrum_rect = Rect {
        x: controls_rect.x + controls_rect.width,
        y: top.y,
        width: spectrum_col_w,
        height: 1,
    };

    let left_text = match app.now_playing.as_ref() {
        Some(track) if !track.title.trim().is_empty() => {
            if app.now_playing_artist_text().trim().is_empty() {
                track.title.clone()
            } else {
                format!("{} - {}", track.title, app.now_playing_artist_text())
            }
        }
        _ => String::new(),
    };

    let left_style = if app.now_playing.is_some() {
        Style::default()
            .fg(app.theme.color_accent3())
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(app.theme.color_subtext())
    };

    let left_render = if app.now_playing.is_some() {
        let heart = if app.now_playing_liked { "" } else { "" };
        compose_left_right_line(&left_text, heart, left_rect.width as usize)
    } else {
        clip_to_display_width(&left_text, left_rect.width as usize)
    };

    frame.render_widget(Paragraph::new(left_render).style(left_style), left_rect);

    frame.render_widget(
        Paragraph::new(controls)
            .style(Style::default().fg(app.theme.color_text()))
            .alignment(Alignment::Center),
        controls_rect,
    );

    frame.render_widget(
        Paragraph::new(spectrum)
            .style(Style::default().fg(app.theme.color_accent2()))
            .alignment(Alignment::Right),
        spectrum_rect,
    );

    let position = app.playback_position();
    let duration = app.playback_duration();
    let time_text = format!("{}/{}", format_mmss(position), format_mmss(duration));
    let time_w = display_width(&time_text) as u16;

    let progress_w = bottom.width.saturating_sub(time_w.saturating_add(1));
    let progress_rect = Rect {
        x: bottom.x,
        y: bottom.y,
        width: progress_w,
        height: 1,
    };
    let time_rect = Rect {
        x: bottom.x + progress_w,
        y: bottom.y,
        width: bottom.width.saturating_sub(progress_w),
        height: 1,
    };

    let mut hits = PlayerBarHitTargets::default();

    let controls_start = controls_rect.x + controls_rect.width.saturating_sub(controls_w) / 2;
    let prev_w = display_width(prev_label) as u16;
    let play_w = display_width(play_label) as u16;
    let next_w = display_width(next_label) as u16;

    let mut x = controls_start;
    hits.prev = Some(HitRect {
        x,
        y: top.y,
        width: prev_w,
        height: 1,
    });
    x = x.saturating_add(prev_w).saturating_add(1);
    hits.play_pause = Some(HitRect {
        x,
        y: top.y,
        width: play_w,
        height: 1,
    });
    x = x.saturating_add(play_w).saturating_add(1);
    hits.next = Some(HitRect {
        x,
        y: top.y,
        width: next_w,
        height: 1,
    });

    if progress_w > 0 {
        let ratio = progress_ratio(position, duration);
        let filled = ((ratio * progress_w as f32).round() as u16).min(progress_w);

        // Get buffer progress if streaming
        let buffer_ratio = app.buffer_progress().and_then(|(downloaded, total)| {
            if total > 0 {
                Some((downloaded as f32 / total as f32).min(1.0))
            } else {
                None
            }
        });
        let buffer_filled = buffer_ratio
            .map(|r| ((r * progress_w as f32).round() as u16).min(progress_w))
            .unwrap_or(0);

        let mut spans = Vec::new();

        if app.is_seeking() {
            // 正在后台加载跳转目标：已播放区域（横线部分）显示从左向右的脉冲波，
            // 颜色 = 当前进度条颜色（主题色 accent3）稍作提亮，随波动态变化，
            // 基础颜色来自主题，不硬编码颜色。
            let phase = ANIM_EPOCH.elapsed().as_secs_f32() * 7.0;
            for x in 0..filled {
                let wave = ((phase - x as f32 * 0.3).sin() * 0.5 + 0.5).clamp(0.0, 1.0);
                let color = app.theme.lighten(app.theme.palette.accent3, 0.25 * wave);
                spans.push(Span::styled(
                    "▁",
                    Style::default().fg(color).add_modifier(Modifier::BOLD),
                ));
            }
            let remaining = progress_w.saturating_sub(filled);
            if remaining > 0 {
                spans.push(Span::styled(
                    "▁".repeat(remaining as usize),
                    Style::default().fg(app.theme.color_surface()),
                ));
            }
        } else if buffer_filled == 0 {
            // No buffer data (cached/unknown progress): accent3 for played, buff for remaining
            if filled > 0 {
                spans.push(Span::styled(
                    "▁".repeat(filled as usize),
                    Style::default()
                        .fg(app.theme.color_accent3())
                        .add_modifier(Modifier::BOLD),
                ));
            }
            let remaining = progress_w.saturating_sub(filled);
            if remaining > 0 {
                spans.push(Span::styled(
                    "▁".repeat(remaining as usize),
                    Style::default().fg(app.theme.color_buff()),
                ));
            }
        } else {
            // accent3 for played, buff for buffered-not-played, surface for not-buffered
            let accent_len = filled.min(buffer_filled);
            if accent_len > 0 {
                spans.push(Span::styled(
                    "▁".repeat(accent_len as usize),
                    Style::default()
                        .fg(app.theme.color_accent3())
                        .add_modifier(Modifier::BOLD),
                ));
            }

            let buffered_not_played = buffer_filled.saturating_sub(filled);
            if buffered_not_played > 0 {
                spans.push(Span::styled(
                    "▁".repeat(buffered_not_played as usize),
                    Style::default().fg(app.theme.color_buff()),
                ));
            }

            let unbuffered = progress_w.saturating_sub(buffer_filled);
            if unbuffered > 0 {
                spans.push(Span::styled(
                    "▁".repeat(unbuffered as usize),
                    Style::default().fg(app.theme.color_surface()),
                ));
            }
        }

        frame.render_widget(
            Paragraph::new(Line::from(spans)).alignment(Alignment::Left),
            progress_rect,
        );

        hits.progress = Some(HitRect {
            x: progress_rect.x,
            y: progress_rect.y,
            width: progress_rect.width,
            height: 1,
        });
    }

    frame.render_widget(
        Paragraph::new(time_text)
            .style(Style::default().fg(app.theme.color_subtext()))
            .alignment(Alignment::Right),
        time_rect,
    );

    app.set_player_bar_hits(hits);
}

fn progress_ratio(position: Duration, duration: Duration) -> f32 {
    if duration.as_millis() == 0 {
        return 0.0;
    }

    (position.as_secs_f32() / duration.as_secs_f32()).clamp(0.0, 1.0)
}

fn format_mmss(value: Duration) -> String {
    let secs = value.as_secs();
    format!("{:02}:{:02}", secs / 60, secs % 60)
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

fn compose_left_right_line(left: &str, right: &str, width: usize) -> String {
    if width == 0 {
        return String::new();
    }

    let right_w = display_width(right).min(width);
    let left_max = width.saturating_sub(right_w + 1);
    let left_text = clip_to_display_width(left, left_max);
    let used = display_width(&left_text) + right_w;
    let pad = width.saturating_sub(used);
    format!("{left_text}{}{right}", " ".repeat(pad))
}

fn playback_repeat_symbol(app: &App) -> &'static str {
    match app.playback_repeat_mode {
        crate::app::PlaybackRepeatMode::Sequence => "",
        crate::app::PlaybackRepeatMode::Shuffle => "",
        crate::app::PlaybackRepeatMode::LoopAll => "",
        crate::app::PlaybackRepeatMode::LoopOne => "",
    }
}
