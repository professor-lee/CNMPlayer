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

/// 单个浅色脉冲带从左端扫到右端的周期（秒）
const PULSE_PERIOD_SECS: f32 = 1.4;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

pub const PLAYER_BAR_HEIGHT: u16 = 5;

/// 收藏爱心（Nerd Font PUA）：实心 = 已收藏，空心 = 未收藏。
/// 用码位转义书写，免得复制粘贴时被编辑器换成别的字形。
const HEART_LIKED: &str = "\u{f004}";
const HEART_UNLIKED: &str = "\u{f08a}";

/// 控制行 `{prev} {play} {next} {mode}` 的命中区。
///
/// 串以空格分隔、在 `controls_rect` 内居中（`Alignment::Center`），
/// 因此各 token 的起点等于「串首 + 前缀显示宽度」——与 render 同源。
/// 串被窗口裁掉时模式符号不登记，免得留下点不动的隐形按钮。
fn control_hit_rects(controls_rect: Rect, labels: [&str; 4]) -> PlayerBarHitTargets {
    let y = controls_rect.y;
    let widths: [u16; 4] = labels.map(|label| display_width(label) as u16);
    let total_w: u16 = widths.iter().sum::<u16>() + 3;

    let start = controls_rect.x + controls_rect.width.saturating_sub(total_w) / 2;
    let second = start.saturating_add(widths[0]).saturating_add(1);
    let third = second.saturating_add(widths[1]).saturating_add(1);
    let fourth = third.saturating_add(widths[2]).saturating_add(1);

    let rect_at = |x: u16, width: u16| HitRect {
        x,
        y,
        width,
        height: 1,
    };

    let right = controls_rect.x.saturating_add(controls_rect.width);
    let place = |x: u16, width: u16| -> Option<HitRect> {
        (width > 0 && x.saturating_add(width) <= right).then(|| rect_at(x, width))
    };

    PlayerBarHitTargets {
        prev: place(start, widths[0]),
        play_pause: place(second, widths[1]),
        next: place(third, widths[2]),
        progress: None,
        like: None,
        download: None,
        mode: place(fourth, widths[3]),
    }
}

/// 爱心命中区：爱心贴左列右端（与 `compose_left_right_line` 的右对齐同源），
/// 宽度不足时不登记，避免留下点不动的隐形按钮。
///
/// 渲染路径现在直接调 `right_suffix_hits`（要顺带登记下载按钮），
/// 这个包装只留给单测。
#[cfg(test)]
fn heart_hit_rect(left_rect: Rect, heart: &str) -> Option<HitRect> {
    right_suffix_hits(left_rect, heart, None).1
}

/// 左列右端的「下载图标 + 空格 + 爱心」命中区（与渲染同源）。
///
/// 爱心贴最右端；下载图标在它左侧隔一格。任一段放不下就不登记（也不画），
/// 避免留下点不动的隐形按钮。返回 `(下载, 爱心)`。
fn right_suffix_hits(
    left_rect: Rect,
    heart: &str,
    download: Option<char>,
) -> (Option<HitRect>, Option<HitRect>) {
    let heart_w = display_width(heart) as u16;
    if heart_w == 0 || left_rect.width < heart_w {
        return (None, None);
    }

    let heart_x = left_rect.x + left_rect.width - heart_w;
    let heart_hit = HitRect {
        x: heart_x,
        y: left_rect.y,
        width: heart_w,
        height: 1,
    };

    let Some(glyph) = download else {
        return (None, Some(heart_hit));
    };

    let glyph = glyph.to_string();
    let glyph_w = display_width(&glyph) as u16;
    let needed = glyph_w + 1;
    if glyph_w == 0 || heart_x < left_rect.x.saturating_add(needed) {
        return (None, Some(heart_hit));
    }

    let download_hit = HitRect {
        x: heart_x - needed,
        y: left_rect.y,
        width: glyph_w,
        height: 1,
    };
    (Some(download_hit), Some(heart_hit))
}

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
    let mode_symbol = app.playback_repeat_mode.symbol();
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

    // 爱心由 compose_left_right_line 贴左列右端，命中区用同一算法倒推，
    // 免得两处各写一份宽度计算；下载图标（可用时）在它左侧隔一格。
    let download_state = app.current_download_state();
    let download_glyph = download_state
        .map(|state| crate::app::download::state_glyph(state, app.download_spinner_phase()));
    let download_style = match download_state {
        Some(crate::app::download::DownloadState::Downloading) => Style::default()
            .fg(app.theme.color_accent2())
            .add_modifier(Modifier::BOLD),
        Some(crate::app::download::DownloadState::Done) => {
            Style::default().fg(app.theme.color_accent3())
        }
        _ => Style::default().fg(app.theme.color_subtext()),
    };

    let mut like_hit = None;
    let mut download_hit = None;
    let left_render = if app.now_playing.is_some() {
        let heart = if app.now_playing_liked {
            HEART_LIKED
        } else {
            HEART_UNLIKED
        };
        let suffix = match download_glyph {
            Some(glyph) => format!("{glyph} {heart}"),
            None => heart.to_string(),
        };
        (download_hit, like_hit) = right_suffix_hits(left_rect, heart, download_glyph);
        compose_left_right_line(&left_text, &suffix, left_rect.width as usize)
    } else {
        clip_to_display_width(&left_text, left_rect.width as usize)
    };

    // 右端图标段单独上色：从右往左剥出「爱心 → 空格 → 下载图标」。
    let mut spans: Vec<Span> = Vec::new();
    let mut tail: Vec<(String, Style)> = Vec::new();
    let mut head = left_render.as_str();
    if app.now_playing.is_some() {
        let heart = if app.now_playing_liked {
            HEART_LIKED
        } else {
            HEART_UNLIKED
        };
        if let Some(stripped) = head.strip_suffix(heart) {
            head = stripped;
            if let Some(glyph) = download_glyph
                && let Some(stripped) = head.strip_suffix(glyph)
                && let Some(stripped) = stripped.strip_suffix(' ')
            {
                head = stripped;
                tail.push((glyph.to_string(), download_style));
                tail.push((" ".to_string(), left_style));
            }
            tail.push((heart.to_string(), left_style));
        }
    }
    spans.push(Span::styled(head.to_string(), left_style));
    spans.extend(
        tail.into_iter()
            .map(|(text, style)| Span::styled(text, style)),
    );

    frame.render_widget(Paragraph::new(Line::from(spans)), left_rect);

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

    let mut hits = control_hit_rects(
        controls_rect,
        [prev_label, play_label, next_label, mode_symbol],
    );
    hits.like = like_hit;
    hits.download = download_hit;

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
            // 正在后台加载跳转目标：单个浅色脉冲带从已播放区域最左侧向右移动，
            // 先慢后快（二次缓动）；颜色 = 当前进度条颜色（主题色 accent3）
            // 稍作提亮，基础颜色来自主题，不硬编码颜色。
            let cycle = (ANIM_EPOCH.elapsed().as_secs_f32() / PULSE_PERIOD_SECS).fract();
            let eased = cycle * cycle; // 先慢后快
            let width = filled.max(1) as f32;
            let center = eased * (width - 1.0);
            let band_half = (width * 0.08).max(1.0);
            for x in 0..filled {
                let d = (x as f32 - center).abs();
                let amount = if d <= band_half {
                    0.25 * (d / band_half * std::f32::consts::FRAC_PI_2).cos()
                } else {
                    0.0
                };
                let color = app.theme.lighten(app.theme.palette.accent3, amount);
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

#[cfg(test)]
mod tests {
    use super::*;

    fn row(x: u16, width: u16) -> Rect {
        Rect {
            x,
            y: 3,
            width,
            height: 1,
        }
    }

    /// 爱心命中区必须正好落在 `compose_left_right_line` 画出的那一格上。
    #[test]
    fn heart_hit_rect_matches_the_right_aligned_glyph() {
        for width in 1..14u16 {
            let area = row(7, width);
            for heart in [HEART_LIKED, HEART_UNLIKED] {
                let line = compose_left_right_line("a fairly long title", heart, width as usize);
                let hit = heart_hit_rect(area, heart).expect("宽度够时应登记命中区");

                assert!(line.ends_with(heart), "爱心贴右端，实际是 {line:?}");
                assert_eq!(display_width(&line), width as usize, "整行宽度不变");
                assert_eq!(hit.x, area.x + width - 1, "命中区落在串尾那一格");
                assert_eq!(hit.width, 1);
            }
        }
    }

    /// 左列宽度为 0（极窄窗口）时不登记，避免出现点不动的隐形按钮。
    #[test]
    fn heart_hit_rect_is_absent_when_the_column_is_blank() {
        assert!(heart_hit_rect(row(0, 0), HEART_LIKED).is_none());
    }

    /// 下载按钮在爱心左侧隔一格；同一行里两者互不重叠。
    #[test]
    fn download_hit_rect_sits_one_gap_left_of_the_heart() {
        let area = row(5, 20);
        let (download, heart) = right_suffix_hits(area, HEART_UNLIKED, Some('\u{ec74}'));
        let download = download.expect("宽度够时应登记下载按钮");
        let heart = heart.expect("爱心格");

        assert_eq!(heart.x, area.x + area.width - 1);
        assert_eq!(
            download.x + download.width + 1,
            heart.x,
            "下载图标与爱心之间正好隔一格"
        );
        assert_eq!(download.width, 1);
        assert_eq!(download.y, area.y);
    }

    /// 左列放不下「图标 + 空格 + 爱心」时，只登记爱心（也不该画下载图标）。
    #[test]
    fn download_hit_rect_is_absent_when_the_gap_does_not_fit() {
        // 宽度 1：只够爱心；宽度 2：够爱心 + 一格空格，但放不下图标。
        for width in [1u16, 2] {
            let (download, heart) = right_suffix_hits(row(0, width), HEART_LIKED, Some('⠋'));
            assert!(download.is_none(), "宽度 {width} 不该登记下载按钮");
            assert!(heart.is_some(), "宽度 {width} 仍应有爱心");
        }
    }

    /// 下载图标本身必须是 1 格宽（否则左列排版会漂）。
    #[test]
    fn download_glyphs_are_single_cell() {
        use crate::app::download::{DownloadState, state_glyph};
        let phase = Duration::from_millis(0);
        for state in [
            DownloadState::NotDownloaded,
            DownloadState::Downloading,
            DownloadState::Done,
        ] {
            let glyph = state_glyph(state, phase);
            assert_eq!(
                display_width(&glyph.to_string()),
                1,
                "{glyph:?} 应为 1 格宽"
            );
        }
    }

    /// 符号被吞成空串时播放栏会错位并留下点不到的按钮——这条直接兜住。
    #[test]
    fn player_bar_glyphs_are_single_cell() {
        for heart in [HEART_LIKED, HEART_UNLIKED] {
            assert_eq!(display_width(heart), 1, "爱心应为 1 格宽：{heart:?}");
        }
        for mode in [
            crate::app::PlaybackRepeatMode::Sequence,
            crate::app::PlaybackRepeatMode::Shuffle,
            crate::app::PlaybackRepeatMode::LoopAll,
            crate::app::PlaybackRepeatMode::LoopOne,
        ] {
            let symbol = mode.symbol();
            assert_eq!(display_width(symbol), 1, "模式符号应为 1 格宽：{symbol:?}");
        }
    }

    /// 控制行命中区与 render 的居中串同源：各段起点 = 串首 + 前缀显示宽度，
    /// 末段右端 = 整串右端。
    #[test]
    fn control_hit_rects_track_the_centered_label_row() {
        let labels = ["[<]", "[>]", "[>]", "M"];
        let area = row(4, 30);
        let hits = control_hit_rects(area, labels);

        let joined = labels.join(" ");
        let row_w = display_width(&joined) as u16;
        let start = area.x + (area.width - row_w) / 2;

        let prev = hits.prev.expect("prev 命中区");
        let play = hits.play_pause.expect("play 命中区");
        let next = hits.next.expect("next 命中区");
        let mode = hits.mode.expect("mode 命中区");

        assert_eq!(prev.x, start);
        assert_eq!(play.x, start + display_width("[<] ") as u16);
        assert_eq!(next.x, start + display_width("[<] [>] ") as u16);
        assert_eq!(mode.x, start + display_width("[<] [>] [>] ") as u16);
        assert_eq!(
            mode.x + mode.width,
            start + row_w,
            "命中区不多不少覆盖控制串"
        );
        for hit in [prev, play, next, mode] {
            assert_eq!(hit.y, area.y);
            assert_eq!(hit.height, 1);
            assert!(hit.width > 0);
        }
    }

    /// 窗口比控制串窄时整段越界的 token 不登记（否则会在频谱列上留下幽灵按钮）。
    #[test]
    fn control_hit_rects_skip_tokens_clipped_by_a_narrow_row() {
        let labels = ["[<]", "[>]", "[>]", "M"];
        let row_w = display_width(&labels.join(" ")) as u16;

        let fits = control_hit_rects(row(0, row_w), labels);
        assert!(fits.mode.is_some(), "刚好放得下时应登记模式命中区");

        let clipped = control_hit_rects(row(0, row_w - 1), labels);
        assert!(clipped.mode.is_none(), "末段越界时不登记");
        assert!(clipped.prev.is_some(), "首段仍在区域内");
    }
}
