//! Shared lyric surface rendering.
//!
//! The host overlay, the fullscreen lyric list, the compact visualizer strip
//! and the small-window lyrics all call into this module. Geometry stays owned
//! by each surface; colors, timing, clipping and horizontal scrolling do not.

use crate::lyrics::{LyricLine, snapshot};
use crate::ui::theme::Theme;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct LyricOptions {
    pub(crate) word_lyrics: bool,
    pub(crate) translation: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LyricSurface {
    Overlay,
    SmallWindow,
    Compact,
    Fullscreen,
}

#[derive(Debug, PartialEq, Eq)]
struct RenderKey {
    surface: LyricSurface,
    position_ms: u64,
    seeking: bool,
    width: u16,
    rows: usize,
    options: LyricOptions,
    palette: [(u8, u8, u8); 8],
    capability: crate::ui::theme::ColorCapability,
}

#[derive(Debug)]
struct CachedFrame {
    key: RenderKey,
    text: Text<'static>,
}

/// Per-owner cache: one exact document snapshot and at most two rendered frames.
/// Never key document identity by an address: in-place edits and allocator reuse
/// must invalidate even when every string has the same length. Hits borrow the
/// prepared Text directly, so drawing them does not clone strings or span Vecs.
#[derive(Debug, Default)]
pub(crate) struct LyricRenderCache {
    document: Vec<LyricLine>,
    frames: Vec<CachedFrame>,
}

impl LyricRenderCache {
    // Callers supply their existing frame-grid-quantized position; this cache
    // owns neither clocks nor redraw policy. Seeking is a separate identity bit.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn pair(
        &mut self,
        surface: LyricSurface,
        lines: &[LyricLine],
        position_ms: u64,
        seeking: bool,
        width: u16,
        theme: &Theme,
        options: LyricOptions,
    ) -> &Text<'static> {
        self.render(
            surface,
            lines,
            position_ms,
            seeking,
            2,
            width,
            theme,
            options,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn window(
        &mut self,
        lines: &[LyricLine],
        position_ms: u64,
        seeking: bool,
        rows: usize,
        width: u16,
        theme: &Theme,
        options: LyricOptions,
    ) -> &Text<'static> {
        self.render(
            LyricSurface::Fullscreen,
            lines,
            position_ms,
            seeking,
            rows.max(1),
            width,
            theme,
            options,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn render(
        &mut self,
        surface: LyricSurface,
        lines: &[LyricLine],
        position_ms: u64,
        seeking: bool,
        rows: usize,
        width: u16,
        theme: &Theme,
        options: LyricOptions,
    ) -> &Text<'static> {
        if self.document != lines {
            self.document.clear();
            self.document.extend_from_slice(lines);
            self.frames.clear();
        }
        let p = theme.palette;
        let key = RenderKey {
            surface,
            position_ms,
            seeking,
            width,
            rows,
            options,
            palette: [
                p.text, p.subtext, p.base, p.surface, p.buff, p.accent, p.accent2, p.accent3,
            ],
            capability: theme.capability,
        };
        if let Some(index) = self.frames.iter().position(|frame| frame.key == key) {
            return &self.frames[index].text;
        }
        let rendered = if surface == LyricSurface::Fullscreen {
            render_window(lines, position_ms, rows, width, theme, options)
        } else {
            render_pair(lines, position_ms, width, theme, options)
        };
        let text = Text::from(rendered.into_iter().map(own_line).collect::<Vec<_>>())
            .alignment(ratatui::layout::Alignment::Center);
        if self.frames.len() == 2 {
            self.frames.remove(0);
        }
        self.frames.push(CachedFrame { key, text });
        &self.frames.last().expect("just inserted lyric frame").text
    }
}

fn own_line(line: Line<'_>) -> Line<'static> {
    let mut spans: Vec<Span<'static>> = Vec::with_capacity(line.spans.len());
    for span in line.spans {
        if let Some(previous) = spans.last_mut()
            && previous.style == span.style
        {
            // Words have separate source strings. Join equal styles here too,
            // before allocating another String for a completed/pending word.
            previous.content.to_mut().push_str(&span.content);
        } else {
            spans.push(Span {
                content: span.content.into_owned().into(),
                style: span.style,
            });
        }
    }
    Line {
        spans,
        style: line.style,
        alignment: line.alignment,
    }
}

/// Current + next line (or current + translation) for two-row surfaces.
fn render_pair<'a>(
    lines: &'a [LyricLine],
    position_ms: u64,
    width: u16,
    theme: &Theme,
    options: LyricOptions,
) -> Vec<Line<'a>> {
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

    if let Some(translation) = translation_text(&lines[current], options) {
        rows[1] = if state.current.is_some() {
            render_translation_current(
                &lines[current],
                position_ms,
                state.line_progress,
                width,
                theme,
                options,
            )
        } else {
            render_translation_static(translation, future_color(theme, 0), width)
        };
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
fn render_window<'a>(
    lines: &'a [LyricLine],
    position_ms: u64,
    rows: usize,
    width: u16,
    theme: &Theme,
    options: LyricOptions,
) -> Vec<Line<'a>> {
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
    let line_transition = if state.current.is_some() {
        (position_ms.saturating_sub(lines[current].start_ms) as f32 / 220.0).clamp(0.0, 1.0)
    } else {
        1.0
    };

    // Previous lines are built closest-to-current first, in line/translation
    // pairs. Keep the pairs that fit above the fixed current row.
    let mut previous_pairs: Vec<Vec<Line<'_>>> = Vec::new();
    let mut previous_rows = 0;
    let mut index = current;
    while index > 0 {
        index -= 1;
        let line = &lines[index];
        let distance = current - index;
        let transition = if distance == 1 { line_transition } else { 1.0 };
        let line_color = past_color(theme, distance, transition);
        let mut pair = Vec::with_capacity(2);
        pair.push(render_static(line, line_color, width));
        if let Some(translation) = translation_text(line, options) {
            pair.push(render_translation_static(translation, line_color, width));
        }
        if previous_rows + pair.len() > center {
            break;
        }
        previous_rows += pair.len();
        previous_pairs.push(pair);
    }
    previous_pairs.reverse();

    let mut slot = center - previous_rows;
    for pair in previous_pairs {
        for line in pair {
            output[slot] = line;
            slot += 1;
        }
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

    let mut after = Vec::new();
    if let Some(translation) = translation_text(&lines[current], options) {
        after.push(if state.current.is_some() {
            render_translation_current(
                &lines[current],
                position_ms,
                state.line_progress,
                width,
                theme,
                options,
            )
        } else {
            render_translation_static(translation, future_color(theme, 0), width)
        });
    }

    let mut index = current + 1;
    while index < lines.len() {
        let remaining = rows.saturating_sub(center + 1 + after.len());
        if remaining == 0 {
            break;
        }
        let line = &lines[index];
        let line_color = future_color(theme, index - current);
        let mut pair = Vec::with_capacity(2);
        pair.push(render_static(line, line_color, width));
        if let Some(translation) = translation_text(line, options) {
            pair.push(render_translation_static(translation, line_color, width));
        }
        if pair.len() > remaining {
            break;
        }
        after.extend(pair);
        index += 1;
    }

    for (offset, line) in after.into_iter().enumerate() {
        output[center + 1 + offset] = line;
    }
    output
}

fn translation_text(line: &LyricLine, options: LyricOptions) -> Option<&str> {
    if !options.translation {
        return None;
    }
    line.translation
        .as_deref()
        .filter(|text| !text.trim().is_empty())
}

fn render_current<'a>(
    line: &'a LyricLine,
    position_ms: u64,
    line_progress: f32,
    width: u16,
    theme: &Theme,
    options: LyricOptions,
) -> Line<'a> {
    let mut spans = if line.is_untimed() || !options.word_lyrics {
        // Untimed plain lyrics have no meaningful progress; keep the current
        // line in the solid current color instead of freezing at pending.
        vec![Span::styled(
            line.text.as_str(),
            Style::default()
                .fg(current_line_color(theme, line, position_ms))
                .add_modifier(Modifier::BOLD),
        )]
    } else if line.words.is_empty() {
        progress_spans(&line.text, line_progress, theme, true)
    } else {
        word_spans(theme, line, position_ms)
    };

    let total = spans_width(&spans);
    if total > usize::from(width) && width > 0 {
        let offset = scroll_offset(total, usize::from(width), line.start_ms, position_ms);
        spans = clip_spans(&spans, offset, usize::from(width));
    }
    Line::from(spans)
}

fn progress_spans<'a>(text: &'a str, progress: f32, theme: &Theme, bold: bool) -> Vec<Span<'a>> {
    let count = text.chars().count().max(1) as f32;
    let progress = progress.clamp(0.0, 1.0);
    let mut spans = Vec::new();
    for (index, (byte, ch)) in text.char_indices().enumerate() {
        let char_progress = (progress * count - index as f32).clamp(0.0, 1.0);
        let color = theme.mix(
            theme.palette.subtext,
            theme.palette.accent2,
            0.38 + 0.62 * ease_in_out(char_progress),
        );
        let mut style = Style::default().fg(color);
        if bold {
            style = style.add_modifier(Modifier::BOLD);
        }
        push_slice(&mut spans, text, byte, byte + ch.len_utf8(), style);
    }
    spans
}

/// Adjacent characters with the same style stay one borrowed UTF-8 slice.
fn push_slice<'a>(
    spans: &mut Vec<Span<'a>>,
    text: &'a str,
    start: usize,
    end: usize,
    style: Style,
) {
    if let Some(last) = spans.last_mut()
        && last.style == style
        && let std::borrow::Cow::Borrowed(previous) = last.content
        && previous.len() <= start
        && previous.as_ptr() == text[start - previous.len()..].as_ptr()
    {
        let previous_start = start - previous.len();
        last.content = (&text[previous_start..end]).into();
    } else {
        spans.push(Span::styled(&text[start..end], style));
    }
}

fn current_line_color(theme: &Theme, line: &LyricLine, position_ms: u64) -> ratatui::style::Color {
    let elapsed = position_ms.saturating_sub(line.start_ms) as f32;
    let entry = ease_in_out((elapsed / 220.0).clamp(0.0, 1.0));
    theme.mix(
        theme.palette.subtext,
        theme.palette.accent2,
        0.38 + 0.62 * entry,
    )
}

fn word_spans<'a>(theme: &Theme, line: &'a LyricLine, position_ms: u64) -> Vec<Span<'a>> {
    let mut spans = Vec::new();
    for word in &line.words {
        let count = word.text.chars().count().max(1) as f32;
        for (index, (byte, ch)) in word.text.char_indices().enumerate() {
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
            push_slice(
                &mut spans,
                &word.text,
                byte,
                byte + ch.len_utf8(),
                Style::default().fg(color).add_modifier(Modifier::BOLD),
            );
        }
    }
    spans
}

fn render_static(line: &LyricLine, color: ratatui::style::Color, width: u16) -> Line<'_> {
    if width == 0 {
        return Line::default();
    }
    let text = truncate_columns(&line.text, usize::from(width));
    Line::from(Span::styled(text, Style::default().fg(color)))
}

fn render_translation_static(text: &str, color: ratatui::style::Color, width: u16) -> Line<'_> {
    if width == 0 {
        return Line::default();
    }
    Line::from(Span::styled(
        truncate_columns(text, usize::from(width)),
        Style::default().fg(color),
    ))
}

fn render_translation_current<'a>(
    line: &'a LyricLine,
    position_ms: u64,
    line_progress: f32,
    width: u16,
    theme: &Theme,
    options: LyricOptions,
) -> Line<'a> {
    let Some(text) = translation_text(line, options) else {
        return Line::default();
    };
    if width == 0 {
        return Line::default();
    }

    let mut spans = if line.is_untimed() || !options.word_lyrics {
        // Untimed plain lyrics have no meaningful progress; word mode off also
        // keeps the translation as a single uniform-color span.
        vec![Span::styled(
            text,
            Style::default().fg(current_line_color(theme, line, position_ms)),
        )]
    } else {
        let progress = if line.words.is_empty() {
            line_progress
        } else {
            translation_word_progress(line, position_ms)
        };
        progress_spans(text, progress, theme, false)
    };

    let total = spans_width(&spans);
    if total > usize::from(width) {
        let offset = scroll_offset(total, usize::from(width), line.start_ms, position_ms);
        spans = clip_spans(&spans, offset, usize::from(width));
    }
    Line::from(spans)
}

fn translation_word_progress(line: &LyricLine, position_ms: u64) -> f32 {
    let total = line.words.len() as f32;
    let mut completed = 0.0;
    for word in &line.words {
        if position_ms >= word.end_ms {
            completed += 1.0;
        } else if position_ms > word.start_ms && word.end_ms > word.start_ms {
            completed +=
                (position_ms - word.start_ms) as f32 / (word.end_ms - word.start_ms) as f32;
            break;
        } else {
            break;
        }
    }
    (completed / total).clamp(0.0, 1.0)
}

fn past_color(theme: &Theme, distance: usize, transition: f32) -> ratatui::style::Color {
    let amount =
        (0.45 + 0.10 * distance.saturating_sub(1) as f32).min(0.75) * ease_in_out(transition);
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

fn clip_spans<'a>(spans: &[Span<'a>], offset: usize, width: usize) -> Vec<Span<'a>> {
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
                // A wide glyph is cut by the left edge. Keep its visible
                // cells as blanks so the remaining glyphs stay aligned.
                for _ in start.max(offset)..column.min(end) {
                    text.push(' ');
                }
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

fn truncate_columns(text: &str, width: usize) -> std::borrow::Cow<'_, str> {
    if width == 0 {
        return "".into();
    }
    if text.width() <= width {
        return text.into();
    }
    if width == 1 {
        return "…".into();
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
    output.into()
}

fn spans_width(spans: &[Span<'_>]) -> usize {
    spans.iter().map(|span| span.content.width()).sum()
}

#[cfg(test)]
mod tests;
