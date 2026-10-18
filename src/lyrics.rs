//! Shared lyric document: parsing, timing, and the current playback snapshot.
//!
//! Both the host UI and the fullscreen UI consume the same model so that the
//! two surfaces cannot drift apart in line selection or word timing.

use serde::Deserialize;
use serde_json::Value;

/// A word (or short phrase) with its own timing inside a lyric line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LyricWord {
    pub(crate) text: String,
    pub(crate) start_ms: u64,
    pub(crate) end_ms: u64,
}

/// One displayable lyric line.
///
/// `words` is empty for ordinary LRC and plain text; enhanced LRC and YRC can
/// carry word timings. `translation` is optional for every format. `end_ms`
/// is inferred from the next line when the source has no explicit duration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LyricLine {
    pub(crate) start_ms: u64,
    pub(crate) end_ms: Option<u64>,
    pub(crate) text: String,
    pub(crate) words: Vec<LyricWord>,
    pub(crate) translation: Option<String>,
}

/// The playback state needed to render a lyric frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct LyricSnapshot {
    /// `None` before the first line starts.
    pub(crate) current: Option<usize>,
    /// 0..=1 through the current line.
    pub(crate) line_progress: f32,
}

impl LyricLine {
    /// End used for line-progress interpolation when the source omitted one.
    fn effective_end_ms(&self, next_start_ms: Option<u64>) -> u64 {
        self.end_ms
            .filter(|end| *end > self.start_ms)
            .or_else(|| next_start_ms.filter(|end| *end > self.start_ms))
            .unwrap_or_else(|| self.start_ms.saturating_add(5_000))
    }

    /// Plain, unsynchronized lyrics use `u64::MAX` sentinels: the first line
    /// ends at `MAX`, and every subsequent line starts and ends at `MAX`.
    /// They have no playback progress and must render as static text.
    pub(crate) fn is_untimed(&self) -> bool {
        self.start_ms == u64::MAX || self.end_ms == Some(u64::MAX)
    }
}

/// Compute the active line for a playback position.
pub(crate) fn snapshot(lines: &[LyricLine], position_ms: u64) -> LyricSnapshot {
    let mut result = LyricSnapshot {
        current: None,
        line_progress: 0.0,
    };
    if lines.is_empty() {
        return result;
    }

    let next = lines.partition_point(|line| line.start_ms <= position_ms);
    if next == 0 {
        return result;
    }

    let index = next - 1;
    let line = &lines[index];
    let end = line.effective_end_ms(lines.get(index + 1).map(|next| next.start_ms));
    let progress = if end > line.start_ms {
        (position_ms.saturating_sub(line.start_ms) as f32 / (end - line.start_ms) as f32)
            .clamp(0.0, 1.0)
    } else {
        1.0
    };

    result.current = Some(index);
    result.line_progress = progress;
    result
}

/// Quantize a playback position to the global frame grid divided by `divisor`.
///
/// The lyric layer never owns a timer: `ui_fps` is the only frame-rate source.
/// A divisor of 1 follows the global frame clock; 2 halves the lyric update
/// rate (60 -> 30 Hz, 30 -> 15 Hz, 144 -> 72 Hz).
pub(crate) fn quantized_position_ms(position_ms: u64, ui_fps: u32, divisor: u32) -> u64 {
    let fps = u128::from(ui_fps.max(1));
    let divisor = u128::from(divisor.max(1));
    let frame = u128::from(position_ms) * fps / 1000;
    let quantized = (frame / divisor) * divisor;
    u64::try_from(quantized * 1000 / fps).unwrap_or(u64::MAX)
}

/// Parse the body returned by `/lyric/new` or `/lyric`.
///
/// YRC wins when present; otherwise the regular LRC is used. Translation is
/// aligned by timestamp from `ytlrc` (preferred) or `tlyric`. Display settings
/// (`word_lyrics`, `lyric_translation`) are not inputs here: word timings and
/// translations are always retained so toggling them never requires a refetch.
/// Explicit empty LRC timestamps remain on the timeline to clear the current
/// lyric during pauses; a document with no lyric or translation text is absent.
/// JSON records in either source use the same parser and never render as raw
/// JSON. Enhanced LRC consumes only valid `<mm:ss.xx>` tags, retains all lyric
/// text, and bounds its last word by a real trailing tag or following line.
/// Plain fallback retains all non-metadata lines as unsynchronized text.
pub(crate) fn parse_lyric_response(body: &Value) -> Option<Vec<LyricLine>> {
    let yrc = body_text(body, "/yrc/lyric").or_else(|| body_text(body, "/yrc"));
    let lrc = body_text(body, "/lrc/lyric").or_else(|| body_text(body, "/lrc"));

    let yrc_lines = yrc.and_then(parse_yrc);
    let used_yrc = yrc_lines.is_some();
    let mut lines = match yrc_lines {
        Some(lines) => lines,
        None => {
            let source = lrc?;
            parse_lrc(source).or_else(|| parse_plain_lyrics(source))?
        }
    };

    let translation = if used_yrc {
        body_text(body, "/ytlrc/lyric")
            .or_else(|| body_text(body, "/ytlrc"))
            .or_else(|| body_text(body, "/tlyric/lyric"))
            .or_else(|| body_text(body, "/tlyric"))
    } else {
        body_text(body, "/tlyric/lyric").or_else(|| body_text(body, "/tlyric"))
    };
    if let Some(translation) = translation {
        align_translation(&mut lines, translation);
    }

    let has_lyrics = lines.iter().any(|line| {
        !line.text.trim().is_empty()
            || line
                .translation
                .as_deref()
                .is_some_and(|text| !text.trim().is_empty())
    });
    fill_line_end_times(&mut lines);

    has_lyrics.then_some(lines)
}

fn body_text<'a>(body: &'a Value, path: &str) -> Option<&'a str> {
    body.pointer(path)
        .and_then(Value::as_str)
        .filter(|text| !text.trim().is_empty())
}

/// Normalize LRC and per-line JSON through the same document path. Only valid
/// leading square-bracket timestamps are consumed; bracketed lyric text stays.
fn parse_lrc(content: &str) -> Option<Vec<LyricLine>> {
    let mut parsed = Vec::new();
    for raw in content.lines().map(str::trim) {
        if raw.starts_with('{') {
            if let Some(line) = parse_json_yrc_line(raw)
                && !is_metadata_line(&line.text)
            {
                parsed.push((line, false));
            }
            continue;
        }
        let (times, text) = leading_lrc_timestamps(raw);
        if is_metadata_line(text) {
            continue;
        }
        for start_ms in times {
            parsed.push(parse_enhanced_lrc_line(text, start_ms));
        }
    }
    if parsed.is_empty() {
        return None;
    }
    parsed.sort_by_key(|(line, _)| line.start_ms);
    // An open final word ends at the next real line, or is instantaneous when
    // no such bound exists. Never invent a duration for enhanced LRC words.
    for index in 0..parsed.len() {
        let next = parsed.get(index + 1).map(|(line, _)| line.start_ms);
        let (line, open_word) = &mut parsed[index];
        if line.end_ms.is_none() {
            line.end_ms = next.filter(|end| *end > line.start_ms);
        }
        if *open_word
            && let Some(word) = line.words.last_mut()
        {
            word.end_ms = line.end_ms.unwrap_or(word.start_ms).max(word.start_ms);
        }
    }
    Some(parsed.into_iter().map(|(line, _)| line).collect())
}

fn leading_lrc_timestamps(mut text: &str) -> (Vec<u64>, &str) {
    let mut times = Vec::new();
    while let Some(rest) = text.strip_prefix('[') {
        let Some(end) = rest.find(']') else {
            break;
        };
        let Some(ms) = parse_lrc_time_tag(&rest[..end]) else {
            break;
        };
        times.push(ms);
        text = &rest[end + 1..];
    }
    (times, text.trim())
}

/// Return whether the final word still needs the following line's bound.
fn parse_enhanced_lrc_line(content: &str, start_ms: u64) -> (LyricLine, bool) {
    let mut text = String::new();
    let mut words = Vec::new();
    let mut cursor = 0;
    let mut word_start = start_ms;
    let mut last_tag = None;
    for (open, _) in content.match_indices('<') {
        let Some(close) = content[open + 1..].find('>').map(|end| open + 1 + end) else {
            continue;
        };
        let Some(time) = parse_lrc_time_tag(&content[open + 1..close]) else {
            continue;
        };
        let time = time.max(word_start);
        let part = &content[cursor..open];
        text.push_str(part);
        if !part.is_empty() {
            words.push(LyricWord {
                text: part.to_string(),
                start_ms: word_start,
                end_ms: time,
            });
        }
        word_start = time;
        last_tag = Some(time);
        cursor = close + 1;
    }
    let tail = &content[cursor..];
    text.push_str(tail);
    let open_word = last_tag.is_some() && !tail.is_empty();
    if open_word {
        words.push(LyricWord {
            text: tail.to_string(),
            start_ms: word_start,
            end_ms: word_start,
        });
    }
    let end_ms = last_tag.filter(|_| tail.is_empty());
    (
        LyricLine {
            start_ms,
            end_ms,
            text,
            words,
            translation: None,
        },
        open_word,
    )
}

/// Plain fallback retains the complete document without assigning fake times.
fn parse_plain_lyrics(content: &str) -> Option<Vec<LyricLine>> {
    let lines: Vec<_> = content
        .lines()
        .map(str::trim)
        .filter(|line| is_plain_lyric_text(line))
        .enumerate()
        .map(|(index, text)| LyricLine {
            start_ms: if index == 0 { 0 } else { u64::MAX },
            end_ms: Some(u64::MAX),
            text: text.to_string(),
            words: Vec::new(),
            translation: None,
        })
        .collect();
    (!lines.is_empty()).then_some(lines)
}

fn is_plain_lyric_text(line: &str) -> bool {
    if line.is_empty() || line.starts_with('{') || is_metadata_line(line) {
        return false;
    }
    if let Some(rest) = line.strip_prefix('[') {
        // JSON and recognized LRC headers are structural, but arbitrary
        // square/angle brackets (including unclosed ordinary text) are lyric.
        if serde_json::from_str::<Value>(line).is_ok() || rest.starts_with('{') {
            return false;
        }
        if let Some((prefix, _)) = rest.split_once(':') {
            let prefix = prefix.trim();
            if matches!(prefix, "ar" | "ti" | "al" | "by" | "re" | "ve" | "length" | "offset")
                || (!prefix.is_empty() && prefix.bytes().all(|byte| byte.is_ascii_digit()))
            {
                return false;
            }
        }
    }
    true
}

fn parse_timed_lrc(content: &str) -> Vec<(u64, String)> {
    parse_lrc(content)
        .unwrap_or_default()
        .into_iter()
        .map(|line| (line.start_ms, line.text))
        .collect()
}

/// Parse both YRC encodings:
/// - one JSON object per line (`{"t":...,"c":[{"tx":...,"tr":[offset,duration]}]}`)
/// - LRC style (`[start,duration](start,duration,0)word...`)
///
/// Parentheses in lyric text are preserved unless they form a complete word-time tag.
fn parse_yrc(content: &str) -> Option<Vec<LyricLine>> {
    let mut lines = Vec::new();
    for raw in content.lines() {
        let raw = raw.trim();
        if raw.is_empty() {
            continue;
        }
        let parsed = if raw.starts_with('{') {
            parse_json_yrc_line(raw)
        } else if raw.starts_with('[') {
            parse_lrc_style_yrc_line(raw)
        } else {
            None
        };
        if let Some(line) = parsed
            && !line.text.trim().is_empty()
            && !is_metadata_line(&line.text)
        {
            lines.push(line);
        }
    }

    if lines.is_empty() {
        return None;
    }
    sort_and_fill(&mut lines);
    Some(lines)
}

#[derive(Debug, Deserialize)]
struct JsonYrcLine {
    t: u64,
    #[serde(default)]
    c: Vec<JsonYrcWord>,
}

#[derive(Debug, Deserialize)]
struct JsonYrcWord {
    #[serde(default)]
    tx: String,
    #[serde(default)]
    tr: Vec<u64>,
}

fn parse_json_yrc_line(raw: &str) -> Option<LyricLine> {
    let parsed: JsonYrcLine = serde_json::from_str(raw).ok()?;
    let line_start = parsed.t;
    let mut words = Vec::with_capacity(parsed.c.len());
    let mut text = String::new();
    let mut cursor = line_start;
    let mut timed_end = None;

    for item in parsed.c {
        if item.tx.is_empty() {
            continue;
        }
        let (start, end) = if item.tr.len() >= 2 {
            let start = line_start.checked_add(item.tr[0])?;
            let end = start.checked_add(item.tr[1])?;
            timed_end = Some(timed_end.map_or(end, |previous: u64| previous.max(end)));
            (start, end)
        } else {
            // Missing timings are not a license to manufacture 500ms words.
            (cursor, cursor)
        };
        cursor = end;
        text.push_str(&item.tx);
        words.push(LyricWord {
            text: item.tx,
            start_ms: start,
            end_ms: end,
        });
    }

    if text.trim().is_empty() {
        return None;
    }
    if timed_end.is_none() {
        words.clear();
    }
    Some(LyricLine {
        start_ms: line_start,
        end_ms: timed_end,
        text,
        words,
        translation: None,
    })
}

fn parse_lrc_style_yrc_line(raw: &str) -> Option<LyricLine> {
    let close = raw.find(']')?;
    let (line_start, line_duration) = parse_lrc_style_pair(&raw[1..close])?;
    let content = &raw[close + 1..];
    let mut words = Vec::new();
    let mut text = String::new();
    // Only complete word-time tags delimit text; ordinary parentheses belong
    // to the preceding word, including numeric text such as `(2026)`.
    let mut tags = content
        .match_indices('(')
        .filter_map(|(open, _)| {
            let close = open + 1 + content[open + 1..].find(')')?;
            let mut fields = content[open + 1..close].split(',');
            let start = fields.next()?.trim().parse::<u64>().ok()?;
            let duration = fields.next()?.trim().parse::<u64>().ok()?;
            fields.next()?.trim().parse::<u64>().ok()?;
            if fields.next().is_some() {
                return None;
            }
            Some((open, close + 1, start, duration))
        })
        .peekable();

    while let Some((_, text_start, word_start, word_duration)) = tags.next() {
        let text_end = tags.peek().map_or(content.len(), |tag| tag.0);
        let word_text = &content[text_start..text_end];
        if !word_text.is_empty() {
            text.push_str(word_text);
            words.push(LyricWord {
                text: word_text.to_string(),
                start_ms: word_start,
                end_ms: word_start.saturating_add(word_duration),
            });
        }
    }

    if words.is_empty() {
        return None;
    }
    let end_ms = Some(
        words
            .last()
            .map(|word| word.end_ms)
            .unwrap_or(line_start)
            .max(line_start.saturating_add(line_duration)),
    );
    Some(LyricLine {
        start_ms: line_start,
        end_ms,
        text,
        words,
        translation: None,
    })
}

fn parse_lrc_style_pair(text: &str) -> Option<(u64, u64)> {
    let mut parts = text.split(',');
    let start = parts.next()?.trim().parse().ok()?;
    let duration = parts.next()?.trim().parse().ok()?;
    if parts.next().is_some() {
        return None;
    }
    Some((start, duration))
}

fn align_translation(lines: &mut [LyricLine], content: &str) {
    let translations = parse_timed_lrc(content);
    if translations.is_empty() {
        return;
    }

    let mut cursor = 0;
    for line in lines {
        while cursor + 1 < translations.len() && translations[cursor + 1].0 <= line.start_ms {
            cursor += 1;
        }

        let mut best = cursor;
        if cursor + 1 < translations.len()
            && translations[cursor + 1].0.abs_diff(line.start_ms)
                < translations[cursor].0.abs_diff(line.start_ms)
        {
            best = cursor + 1;
        }

        if translations[best].0.abs_diff(line.start_ms) <= 120 {
            let text = translations[best].1.trim();
            if !text.is_empty() {
                line.translation = Some(text.to_string());
            }
        }
    }
}

fn sort_and_fill(lines: &mut [LyricLine]) {
    lines.sort_by_key(|line| line.start_ms);
    fill_line_end_times(lines);
}

fn fill_line_end_times(lines: &mut [LyricLine]) {
    for index in 0..lines.len() {
        if lines[index].end_ms.is_none() {
            let next = lines.get(index + 1).map(|line| line.start_ms);
            lines[index].end_ms = next.filter(|end| *end > lines[index].start_ms);
        }
    }
}

fn is_metadata_line(text: &str) -> bool {
    let text = text.trim();
    let Some(colon) = text.find([':', '：']) else {
        return false;
    };
    let prefix = text[..colon].trim();
    matches!(
        prefix,
        "作词"
            | "作曲"
            | "编曲"
            | "制作"
            | "制作人"
            | "混音"
            | "母带"
            | "录音"
            | "吉他"
            | "贝斯"
            | "和声"
            | "监制"
            | "出品"
            | "词"
            | "曲"
            | "Lyrics"
            | "Composer"
            | "Arranger"
            | "Producer"
    )
}

fn parse_lrc_time_tag(tag: &str) -> Option<u64> {
    let (minutes, rest) = tag.split_once(':')?;
    let minutes = minutes.trim();
    let (seconds, fraction) = match rest.trim().split_once('.') {
        Some((seconds, fraction)) => (seconds.trim(), Some(fraction.trim())),
        None => (rest.trim(), None),
    };
    if minutes.is_empty()
        || seconds.is_empty()
        || !minutes.bytes().all(|byte| byte.is_ascii_digit())
        || !seconds.bytes().all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    let minutes: u64 = minutes.parse().ok()?;
    let seconds: u64 = seconds.parse().ok()?;
    if seconds >= 60 {
        return None;
    }
    let millis = if let Some(fraction) = fraction {
        if fraction.is_empty() || !fraction.bytes().all(|byte| byte.is_ascii_digit()) {
            return None;
        }
        let digits = &fraction[..fraction.len().min(3)];
        digits.parse::<u64>().ok()? * [0, 100, 10, 1][digits.len()]
    } else {
        0
    };
    minutes
        .checked_mul(60_000)?
        .checked_add(seconds * 1_000)?
        .checked_add(millis)
}

#[cfg(test)]
mod tests;
