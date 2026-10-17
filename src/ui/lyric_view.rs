//! Shared lyric surface rendering.
//!
//! The host overlay, the fullscreen lyric list, the compact visualizer strip
//! and the small-window lyrics all call into this module. Geometry stays owned
//! by each surface; colors, timing, clipping and horizontal scrolling do not.

use crate::lyrics::{LyricLine, snapshot};
use crate::ui::theme::Theme;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

#[derive(Debug, Clone, Copy)]
pub(crate) struct LyricOptions {
    pub(crate) word_lyrics: bool,
    pub(crate) translation: bool,
}

/// Current + next line (or current + translation) for two-row surfaces.
pub(crate) fn render_pair(
    lines: &[LyricLine],
    position_ms: u64,
    width: u16,
    theme: &Theme,
    options: LyricOptions,
) -> Vec<Line<'static>> {
    let mut rows = vec![Line::default(), Line::default()];
    if lines.is_empty() || width == 0 {
        return rows;
    }

    let state = snapshot(lines, position_ms);
    let current = state.current.unwrap_or(0);
    if current >= lines.len() {
        return rows;
    }

    rows[0] = if state.current.is_some() {
        render_current(
            &lines[current],
            position_ms,
            state.line_progress,
            width,
            theme,
            options,
        )
    } else {
        render_static(&lines[current], future_color(theme, 0), width)
    };

    if options.translation
        && state.current.is_some()
        && let Some(translation) = lines[current]
            .translation
            .as_deref()
            .filter(|text| !text.trim().is_empty())
    {
        rows[1] = render_translation(translation, width, theme);
    } else if let Some(next) = lines.get(current + 1) {
        rows[1] = render_static(next, future_color(theme, 1), width);
    }

    rows
}

/// Centered lyric window for the fullscreen lyric page.
///
/// The current line keeps the row it had before (`rows / 2`); an optional
/// translation consumes the row directly below it. The total number of rows is
/// unchanged, so the surrounding layout does not move.
pub(crate) fn render_window(
    lines: &[LyricLine],
    position_ms: u64,
    rows: usize,
    width: u16,
    theme: &Theme,
    options: LyricOptions,
) -> Vec<Line<'static>> {
    let rows = rows.max(1);
    let mut output = vec![Line::default(); rows];
    if lines.is_empty() || width == 0 {
        return output;
    }

    let state = snapshot(lines, position_ms);
    let current = state.current.unwrap_or(0);
    if current >= lines.len() {
        return output;
    }

    let center = rows / 2;
    let previous_start = current.saturating_sub(center);
    let previous_count = current - previous_start;
    let previous_slot = center - previous_count;

    for (offset, index) in (previous_start..current).enumerate() {
        if previous_slot + offset >= center {
            break;
        }
        let distance = current - index;
        output[previous_slot + offset] =
            render_static(&lines[index], past_color(theme, distance), width);
    }

    if state.current.is_some() {
        output[center] = render_current(
            &lines[current],
            position_ms,
            state.line_progress,
            width,
            theme,
            options,
        );
    } else {
        output[center] = render_static(&lines[current], future_color(theme, 0), width);
    }

    let mut slot = center + 1;
    if state.current.is_some()
        && options.translation
        && let Some(translation) = lines[current]
            .translation
            .as_deref()
            .filter(|text| !text.trim().is_empty())
        && slot < rows
    {
        output[slot] = render_translation(translation, width, theme);
        slot += 1;
    }

    let mut index = current + 1;
    while slot < rows && index < lines.len() {
        output[slot] = render_static(
            &lines[index],
            future_color(theme, index - current),
            width,
        );
        slot += 1;
        index += 1;
    }

    output
}

fn render_current(
    line: &LyricLine,
    position_ms: u64,
    line_progress: f32,
    width: u16,
    theme: &Theme,
    options: LyricOptions,
) -> Line<'static> {
    let pending = pending_color(theme);

    let mut spans = if options.word_lyrics && !line.words.is_empty() {
        word_spans(theme, line, position_ms)
    } else {
        let progress = ease_in_out(line_progress);
        let color = if position_ms <= line.start_ms {
            pending
        } else {
            theme.mix(
                blend_tuple(theme.palette.subtext, theme.palette.accent2, 0.38),
                theme.palette.accent2,
                progress,
            )
        };
        vec![Span::styled(
            line.text.clone(),
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        )]
    };

    let total = spans_width(&spans);
    if total > usize::from(width) && width > 0 {
        let offset = scroll_offset(total, usize::from(width), line.start_ms, position_ms);
        spans = clip_spans(&spans, offset, usize::from(width));
    }
    Line::from(spans)
}

fn word_spans(theme: &Theme, line: &LyricLine, position_ms: u64) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    for word in &line.words {
        let chars: Vec<char> = word.text.chars().collect();
        let count = chars.len().max(1) as f32;
        for (index, ch) in chars.iter().enumerate() {
            let progress = if position_ms >= word.end_ms {
                1.0
            } else if position_ms <= word.start_ms {
                0.0
            } else {
                let word_progress = (position_ms.saturating_sub(word.start_ms) as f32
                    / (word.end_ms - word.start_ms) as f32)
                    .clamp(0.0, 1.0);
                (word_progress * count - index as f32).clamp(0.0, 1.0)
            };
            // Pending color is the same palette-derived mix used by the
            // line-level path: 38% from subtext toward accent2.
            let color = theme.mix(
                theme.palette.subtext,
                theme.palette.accent2,
                0.38 + 0.62 * ease_in_out(progress),
            );
            spans.push(Span::styled(
                ch.to_string(),
                Style::default()
                    .fg(color)
                    .add_modifier(Modifier::BOLD),
            ));
        }
    }
    spans
}

fn render_static(line: &LyricLine, color: ratatui::style::Color, width: u16) -> Line<'static> {
    if width == 0 {
        return Line::default();
    }
    let text = truncate_columns(&line.text, usize::from(width));
    Line::from(Span::styled(text, Style::default().fg(color)))
}

fn render_translation(text: &str, width: u16, theme: &Theme) -> Line<'static> {
    if width == 0 {
        return Line::default();
    }
    Line::from(Span::styled(
        truncate_columns(text, usize::from(width)),
        Style::default().fg(translation_color(theme)),
    ))
}

fn pending_color(theme: &Theme) -> ratatui::style::Color {
    theme.mix(theme.palette.subtext, theme.palette.accent2, 0.38)
}

fn past_color(theme: &Theme, distance: usize) -> ratatui::style::Color {
    let amount = (0.45 + 0.10 * distance.saturating_sub(1) as f32).min(0.75);
    theme.mix(theme.palette.accent2, theme.palette.subtext, amount)
}

fn future_color(theme: &Theme, distance: usize) -> ratatui::style::Color {
    if distance <= 1 {
        theme.color_subtext()
    } else {
        let amount = (0.18 * (distance - 1) as f32).min(0.55);
        theme.mix(theme.palette.subtext, theme.palette.base, amount)
    }
}

fn translation_color(theme: &Theme) -> ratatui::style::Color {
    theme.mix(theme.palette.subtext, theme.palette.base, 0.20)
}

fn blend_tuple(
    from: (u8, u8, u8),
    to: (u8, u8, u8),
    amount: f32,
) -> (u8, u8, u8) {
    let amount = amount.clamp(0.0, 1.0);
    let mix = |a: u8, b: u8| {
        (a as f32 + (b as f32 - a as f32) * amount).round() as u8
    };
    (
        mix(from.0, to.0),
        mix(from.1, to.1),
        mix(from.2, to.2),
    )
}

fn ease_in_out(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn scroll_offset(
    total_width: usize,
    viewport: usize,
    line_start_ms: u64,
    position_ms: u64,
) -> usize {
    let max_offset = total_width.saturating_sub(viewport);
    if max_offset == 0 || position_ms <= line_start_ms {
        return 0;
    }

    // Five cells per second, with a short dwell at both ends. The cycle is
    // driven by playback time, so pausing freezes it and seeking re-bases it.
    const CELL_MS: u64 = 200;
    const START_DWELL_MS: u64 = 600;
    const END_DWELL_MS: u64 = 600;
    let travel_ms = max_offset as u64 * CELL_MS;
    let cycle = START_DWELL_MS + travel_ms + END_DWELL_MS;
    let elapsed = (position_ms - line_start_ms) % cycle;
    if elapsed < START_DWELL_MS {
        0
    } else if elapsed < START_DWELL_MS + travel_ms {
        (((elapsed - START_DWELL_MS) / CELL_MS) as usize).min(max_offset)
    } else {
        max_offset
    }
}

fn clip_spans(spans: &[Span<'static>], offset: usize, width: usize) -> Vec<Span<'static>> {
    if width == 0 {
        return Vec::new();
    }
    let end = offset.saturating_add(width);
    let mut output = Vec::new();
    let mut column = 0usize;

    for span in spans {
        let mut text = String::new();
        for ch in span.content.chars() {
            let char_width = ch.width().unwrap_or(0);
            let start = column;
            column = column.saturating_add(char_width);
            if column <= offset {
                continue;
            }
            if start < offset {
                continue;
            }
            if column > end {
                break;
            }
            text.push(ch);
        }
        if !text.is_empty() {
            output.push(Span::styled(text, span.style));
        }
        if column >= end {
            break;
        }
    }
    output
}

fn truncate_columns(text: &str, width: usize) -> String {
    if width == 0 {
        return String::new();
    }
    if text.width() <= width {
        return text.to_string();
    }
    if width == 1 {
        return "…".to_string();
    }

    let mut output = String::new();
    let mut used = 0usize;
    for ch in text.chars() {
        let char_width = ch.width().unwrap_or(0);
        if used + char_width > width - 1 {
            break;
        }
        output.push(ch);
        used += char_width;
    }
    output.push('…');
    output
}

fn spans_width(spans: &[Span<'static>]) -> usize {
    spans
        .iter()
        .map(|span| span.content.width())
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lyrics::LyricWord;
    use crate::ui::theme::{ColorCapability, Theme, ThemePalette};

    fn theme() -> Theme {
        Theme {
            name: "test".into(),
            capability: ColorCapability::TrueColor,
            palette: ThemePalette {
                text: (230, 230, 230),
                subtext: (160, 160, 170),
                base: (30, 30, 40),
                surface: (40, 40, 50),
                buff: (50, 50, 60),
                accent: (120, 150, 220),
                accent2: (160, 210, 140),
                accent3: (230, 120, 120),
            },
        }
    }

    fn line_with_words() -> LyricLine {
        LyricLine {
            start_ms: 1_000,
            end_ms: Some(2_000),
            text: "你好".into(),
            words: vec![
                LyricWord {
                    text: "你".into(),
                    start_ms: 1_000,
                    end_ms: 1_400,
                },
                LyricWord {
                    text: "好".into(),
                    start_ms: 1_400,
                    end_ms: 2_000,
                },
            ],
            translation: Some("hello".into()),
        }
    }

    #[test]
    fn current_line_and_translation_share_the_pair() {
        let lines = vec![line_with_words(), LyricLine {
            start_ms: 2_000,
            end_ms: Some(3_000),
            text: "下一句".into(),
            words: Vec::new(),
            translation: None,
        }];
        let rows = render_pair(
            &lines,
            1_700,
            20,
            &theme(),
            LyricOptions {
                word_lyrics: true,
                translation: true,
            },
        );
        let current: String = rows[0].spans.iter().map(|span| span.content.as_ref()).collect();
        let second: String = rows[1].spans.iter().map(|span| span.content.as_ref()).collect();
        assert_eq!(current, "你好");
        assert_eq!(second, "hello");
    }

    #[test]
    fn horizontal_scroll_moves_with_playback() {
        let line = LyricLine {
            start_ms: 1_000,
            end_ms: Some(20_000),
            text: "abcdefghijklmnopqrstuvwxyz".into(),
            words: Vec::new(),
            translation: None,
        };
        let lines = vec![line];
        let options = LyricOptions {
            word_lyrics: false,
            translation: false,
        };
        let start = render_pair(&lines, 1_000, 10, &theme(), options);
        let later = render_pair(&lines, 4_000, 10, &theme(), options);
        let start_text: String = start[0].spans.iter().map(|span| span.content.as_ref()).collect();
        let later_text: String = later[0].spans.iter().map(|span| span.content.as_ref()).collect();
        assert_ne!(start_text, later_text);
    }

    #[test]
    fn window_keeps_the_current_row_even_with_translation() {
        let lines = vec![
            LyricLine {
                start_ms: 0,
                end_ms: Some(1_000),
                text: "previous".into(),
                words: Vec::new(),
                translation: None,
            },
            line_with_words(),
        ];
        let rows = render_window(
            &lines,
            1_700,
            5,
            20,
            &theme(),
            LyricOptions {
                word_lyrics: true,
                translation: true,
            },
        );
        let texts: Vec<String> = rows
            .iter()
            .map(|line| line.spans.iter().map(|span| span.content.as_ref()).collect())
            .collect();
        assert_eq!(texts[2], "你好");
        assert_eq!(texts[3], "hello");
    }

    #[test]
    fn no_color_theme_still_returns_visible_text() {
        let mut theme = theme();
        theme.capability = ColorCapability::NoColor;
        let rows = render_pair(
            &[line_with_words()],
            1_700,
            20,
            &theme,
            LyricOptions {
                word_lyrics: true,
                translation: true,
            },
        );
        let text: String = rows[0].spans.iter().map(|span| span.content.as_ref()).collect();
        assert_eq!(text, "你好");
    }
}
