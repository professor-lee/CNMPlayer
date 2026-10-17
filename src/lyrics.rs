//! Shared lyric document: parsing, timing, and the current playback snapshot.
//!
//! Both the host UI and the fullscreen UI consume the same model so that the
//! two surfaces cannot drift apart in line selection or word timing.

use serde::Deserialize;
use serde_json::Value;

/// A word (or short phrase) with its own timing inside a lyric line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LyricWord {
    pub text: String,
    pub start_ms: u64,
    pub end_ms: u64,
}

/// One displayable lyric line.
///
/// `words` is empty for ordinary LRC lines; `translation` is optional for both
/// LRC and YRC. `end_ms` is inferred from the next line when the source has no
/// explicit duration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LyricLine {
    pub start_ms: u64,
    pub end_ms: Option<u64>,
    pub text: String,
    pub words: Vec<LyricWord>,
    pub translation: Option<String>,
}

/// The playback state needed to render a lyric frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LyricSnapshot {
    /// `None` before the first line starts.
    pub current: Option<usize>,
    /// 0..=1 through the current line.
    pub line_progress: f32,
    /// Active word index, if the playhead is inside a word.
    pub word: Option<usize>,
    /// 0..=1 through the active word.
    pub word_progress: f32,
}

impl LyricLine {
    /// End used for line-progress interpolation when the source omitted one.
    pub fn effective_end_ms(&self, next_start_ms: Option<u64>) -> u64 {
        self.end_ms
            .filter(|end| *end > self.start_ms)
            .or_else(|| next_start_ms.filter(|end| *end > self.start_ms))
            .unwrap_or_else(|| self.start_ms.saturating_add(5_000))
    }
}

/// Compute the active line/word for a playback position.
pub fn snapshot(lines: &[LyricLine], position_ms: u64) -> LyricSnapshot {
    let mut result = LyricSnapshot {
        current: None,
        line_progress: 0.0,
        word: None,
        word_progress: 0.0,
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

    if let Some((word_index, word)) = line.words.iter().enumerate().find(|(_, word)| {
        position_ms >= word.start_ms && (word.end_ms <= word.start_ms || position_ms < word.end_ms)
    }) {
        result.word = Some(word_index);
        result.word_progress = if word.end_ms > word.start_ms {
            (position_ms.saturating_sub(word.start_ms) as f32
                / (word.end_ms - word.start_ms) as f32)
                .clamp(0.0, 1.0)
        } else {
            1.0
        };
    }

    result
}

/// Quantize a playback position to the global frame grid divided by `divisor`.
///
/// The lyric layer never owns a timer: `ui_fps` is the only frame-rate source.
/// A divisor of 1 follows the global frame clock; 2 halves the lyric update
/// rate (60 -> 30 Hz, 30 -> 15 Hz, 144 -> 72 Hz).
pub fn quantized_position_ms(position_ms: u64, ui_fps: u32, divisor: u32) -> u64 {
    let fps = u64::from(ui_fps.max(1));
    let divisor = u64::from(divisor.max(1));
    let frame = position_ms.saturating_mul(fps) / 1000;
    let quantized = (frame / divisor) * divisor;
    quantized.saturating_mul(1000) / fps
}

/// Parse the body returned by `/lyric/new` or `/lyric`.
///
/// YRC wins when present; otherwise the regular LRC is used. Translation is
/// aligned by timestamp from `ytlrc` (preferred) or `tlyric`.
pub fn parse_lyric_response(body: &Value) -> Option<Vec<LyricLine>> {
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

    lines.retain(|line| {
        !line.text.trim().is_empty()
            || line
                .translation
                .as_deref()
                .is_some_and(|text| !text.trim().is_empty())
    });
    fill_line_end_times(&mut lines);

    (!lines.is_empty()).then_some(lines)
}

fn body_text<'a>(body: &'a Value, path: &str) -> Option<&'a str> {
    body.pointer(path)
        .and_then(Value::as_str)
        .filter(|text| !text.trim().is_empty())
}

/// Parse ordinary LRC text (`[mm:ss.xx]line`).
pub fn parse_lrc(content: &str) -> Option<Vec<LyricLine>> {
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
pub fn parse_plain_lyrics(content: &str) -> Option<Vec<LyricLine>> {
    let mut non_empty = content
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty());
    let first = non_empty.next()?.to_string();
    let second = non_empty.next().map(str::to_string);

    let mut lines = vec![LyricLine {
        start_ms: 0,
        end_ms: second.as_ref().map(|_| u64::MAX),
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
pub fn parse_yrc(content: &str) -> Option<Vec<LyricLine>> {
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
    let mut rest = content;

    while let Some(open) = rest.find('(') {
        rest = &rest[open + 1..];
        let Some(close) = rest.find(')') else {
            break;
        };
        let timing = &rest[..close];
        rest = &rest[close + 1..];

        let Some((word_start, word_duration)) = parse_lrc_style_pair(timing) else {
            continue;
        };
        let word_text = rest.split('(').next().unwrap_or("").to_string();
        if !word_text.is_empty() {
            text.push_str(&word_text);
            words.push(LyricWord {
                text: word_text,
                start_ms: word_start,
                end_ms: word_start.saturating_add(word_duration),
            });
        }
        if let Some(next) = rest.find('(') {
            rest = &rest[next..];
        } else {
            break;
        }
    }

    if words.is_empty() {
        return None;
    }
    let end_ms = words
        .last()
        .map(|word| word.end_ms)
        .or_else(|| line_duration.checked_add(line_start));
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
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_lrc_with_multiple_timestamps() {
        let lines = parse_lrc("[00:01.00][00:03.50]hello\n[00:02.00]world").unwrap();
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0].start_ms, 1_000);
        assert_eq!(lines[1].start_ms, 2_000);
        assert_eq!(lines[2].start_ms, 3_500);
        assert_eq!(lines[0].text, "hello");
        assert_eq!(lines[0].end_ms, Some(2_000));
    }

    #[test]
    fn parses_json_yrc_words() {
        let raw = r#"{"t":1000,"c":[{"tx":"你","tr":[0,400]},{"tx":"好","tr":[400,600]}]}"#;
        let lines = parse_yrc(raw).unwrap();
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].start_ms, 1_000);
        assert_eq!(lines[0].text, "你好");
        assert_eq!(lines[0].words[1].start_ms, 1_400);
        assert_eq!(lines[0].words[1].end_ms, 2_000);
        assert_eq!(lines[0].end_ms, Some(2_000));
    }

    #[test]
    fn parses_lrc_style_yrc_words() {
        let raw = "[1000,1200](1000,400,0)你(1400,600,0)好";
        let lines = parse_yrc(raw).unwrap();
        assert_eq!(lines[0].words.len(), 2);
        assert_eq!(lines[0].words[0].start_ms, 1_000);
        assert_eq!(lines[0].words[1].end_ms, 2_000);
    }

    #[test]
    fn merges_translation_by_nearest_timestamp() {
        let body = json!({
            "lrc": {"lyric": "[00:01.00]hello\n[00:03.00]world"},
            "tlyric": {"lyric": "[00:01.05]你好\n[00:03.10]世界"}
        });
        let lines = parse_lyric_response(&body).unwrap();
        assert_eq!(lines[0].translation.as_deref(), Some("你好"));
        assert_eq!(lines[1].translation.as_deref(), Some("世界"));
    }

    #[test]
    fn yrc_wins_over_lrc_and_keeps_translation() {
        let body = json!({
            "yrc": {"lyric": "{\"t\":1000,\"c\":[{\"tx\":\"你\",\"tr\":[0,400]},{\"tx\":\"好\",\"tr\":[400,600]}]}"},
            "lrc": {"lyric": "[00:01.00]你好"},
            "ytlrc": {"lyric": "[00:01.00]hello"}
        });
        let lines = parse_lyric_response(&body).unwrap();
        assert_eq!(lines[0].words.len(), 2);
        assert_eq!(lines[0].translation.as_deref(), Some("hello"));
    }

    #[test]
    fn snapshot_tracks_line_and_word_progress() {
        let lines = vec![LyricLine {
            start_ms: 1_000,
            end_ms: Some(3_000),
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
            translation: None,
        }];

        let before = snapshot(&lines, 900);
        assert_eq!(before.current, None);

        let mid = snapshot(&lines, 1_700);
        assert_eq!(mid.current, Some(0));
        assert_eq!(mid.word, Some(1));
        assert!((mid.word_progress - 0.5).abs() < 1e-6);
    }

    #[test]
    fn snapshot_stays_on_completed_line_after_its_end() {
        let lines = vec![
            LyricLine {
                start_ms: 1_000,
                end_ms: Some(2_000),
                text: "a".into(),
                words: Vec::new(),
                translation: None,
            },
            LyricLine {
                start_ms: 3_000,
                end_ms: Some(4_000),
                text: "b".into(),
                words: Vec::new(),
                translation: None,
            },
        ];
        let state = snapshot(&lines, 2_500);
        assert_eq!(state.current, Some(0));
        assert_eq!(state.line_progress, 1.0);
    }

    #[test]
    fn quantized_position_follows_global_fps() {
        assert_eq!(quantized_position_ms(1_010, 60, 2), 1_000);
        assert_eq!(quantized_position_ms(1_000, 60, 1), 1_000);
        assert_eq!(quantized_position_ms(1_000, 30, 2), 1_000);
        assert!(quantized_position_ms(1_050, 30, 2) <= 1_050);
    }
}
