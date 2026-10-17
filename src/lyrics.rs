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
/// `words` is empty for ordinary LRC lines; `translation` is optional for both
/// LRC and YRC. `end_ms` is inferred from the next line when the source has no
/// explicit duration.
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
    /// ends at `MAX`, the optional second line starts at `MAX`. They have no
    /// meaningful playback progress and must render as static text.
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

/// Parse ordinary LRC text (`[mm:ss.xx]line`).
fn parse_lrc(content: &str) -> Option<Vec<LyricLine>> {
    let timed = parse_timed_lrc(content);
    if timed.is_empty() {
        return None;
    }

    let mut lines: Vec<LyricLine> = timed
        .into_iter()
        .map(|(start_ms, text)| LyricLine {
            start_ms,
            end_ms: None,
            text,
            words: Vec::new(),
            translation: None,
        })
        .collect();
    sort_and_fill(&mut lines);
    Some(lines)
}

/// Fallback for plain, unsynchronized lyrics.
fn parse_plain_lyrics(content: &str) -> Option<Vec<LyricLine>> {
    let mut non_empty = content
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty());
    let first = non_empty.next()?.to_string();
    let second = non_empty.next().map(str::to_string);

    let mut lines = vec![LyricLine {
        start_ms: 0,
        end_ms: Some(u64::MAX),
        text: first,
        words: Vec::new(),
        translation: None,
    }];
    if let Some(second) = second {
        lines.push(LyricLine {
            start_ms: u64::MAX,
            end_ms: None,
            text: second,
            words: Vec::new(),
            translation: None,
        });
    }
    Some(lines)
}

fn parse_timed_lrc(content: &str) -> Vec<(u64, String)> {
    let mut out = Vec::new();
    for raw in content.lines() {
        let mut text = raw.trim();
        if text.is_empty() {
            continue;
        }

        let mut times = Vec::new();
        while let Some(rest) = text.strip_prefix('[') {
            let Some(end) = rest.find(']') else {
                break;
            };
            if let Some(ms) = parse_lrc_time_tag(&rest[..end]) {
                times.push(ms);
            }
            text = &rest[end + 1..];
        }
        if times.is_empty() {
            continue;
        }

        let line = text.trim().to_string();
        for start in times {
            out.push((start, line.clone()));
        }
    }
    out.sort_by_key(|(start, _)| *start);
    out
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
    #[serde(default)]
    t: i64,
    #[serde(default)]
    c: Vec<JsonYrcWord>,
}

#[derive(Debug, Deserialize)]
struct JsonYrcWord {
    #[serde(default)]
    tx: String,
    #[serde(default)]
    tr: Vec<i64>,
}

fn parse_json_yrc_line(raw: &str) -> Option<LyricLine> {
    let parsed: JsonYrcLine = serde_json::from_str(raw).ok()?;
    let line_start = u64::try_from(parsed.t).ok()?;
    let mut words = Vec::with_capacity(parsed.c.len());
    let mut text = String::new();
    let mut cursor = line_start;

    for item in parsed.c {
        if item.tx.is_empty() {
            continue;
        }
        let (start, end) = if item.tr.len() >= 2 {
            let offset = item.tr[0].max(0) as u64;
            let duration = item.tr[1].max(0) as u64;
            let start = line_start.saturating_add(offset);
            (start, start.saturating_add(duration))
        } else {
            (cursor, cursor.saturating_add(500))
        };
        cursor = end;
        text.push_str(&item.tx);
        words.push(LyricWord {
            text: item.tx,
            start_ms: start,
            end_ms: end,
        });
    }

    if words.is_empty() {
        return None;
    }
    let end_ms = words.last().map(|word| word.end_ms);
    Some(LyricLine {
        start_ms: line_start,
        end_ms,
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
    let duration = parts.next().unwrap_or("0").trim().parse().ok()?;
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
    let minutes: u64 = minutes.trim().parse().ok()?;
    let rest = rest.trim();
    let (seconds, fraction) = match rest.split_once('.') {
        Some((seconds, fraction)) => (seconds, Some(fraction)),
        None => (rest, None),
    };
    let seconds: u64 = seconds.trim().parse().ok()?;
    if seconds >= 60 {
        return None;
    }

    let mut millis = 0;
    if let Some(fraction) = fraction {
        let digits: String = fraction
            .trim()
            .chars()
            .take_while(|c| c.is_ascii_digit())
            .take(3)
            .collect();
        if !digits.is_empty() {
            millis = match digits.len() {
                1 => digits.parse::<u64>().ok()? * 100,
                2 => digits.parse::<u64>().ok()? * 10,
                _ => digits.parse::<u64>().ok()?,
            };
        }
    }

    Some(minutes * 60_000 + seconds * 1_000 + millis)
}

#[cfg(test)]
mod tests;
