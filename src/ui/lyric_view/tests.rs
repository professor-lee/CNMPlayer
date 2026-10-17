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
fn timed_empty_lrc_clears_pair_and_window_current_rows() {
    let body = serde_json::json!({
        "lrc": {"lyric": "[00:01.00]verse\n[00:03.00]\n[00:10.00]next"}
    });
    let lines = crate::lyrics::parse_lyric_response(&body).unwrap();
    for word_lyrics in [false, true] {
        let options = LyricOptions {
            word_lyrics,
            translation: true,
        };
        let pair = render_pair(&lines, 5_000, 30, &theme(), options);
        let window = render_window(&lines, 5_000, 7, 30, &theme(), options);
        for current in [&pair[0], &window[3]] {
            let text: String = current
                .spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect();
            assert_eq!(text, "", "word_lyrics={word_lyrics}");
        }
        let next: String = pair[1]
            .spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect();
        assert_eq!(next, "next");
    }
}

#[test]
fn current_line_and_translation_share_the_pair() {
    let lines = vec![
        line_with_words(),
        LyricLine {
            start_ms: 2_000,
            end_ms: Some(3_000),
            text: "下一句".into(),
            words: Vec::new(),
            translation: None,
        },
    ];
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
    let current: String = rows[0]
        .spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect();
    let second: String = rows[1]
        .spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect();
    assert_eq!(current, "你好");
    assert_eq!(second, "hello");
}

#[test]
fn pair_shows_translation_before_the_line_starts() {
    let lines = vec![
        line_with_words(),
        LyricLine {
            start_ms: 2_000,
            end_ms: Some(3_000),
            text: "下一句".into(),
            words: Vec::new(),
            translation: Some("next".into()),
        },
    ];
    let rows = render_pair(
        &lines,
        0,
        30,
        &theme(),
        LyricOptions {
            word_lyrics: true,
            translation: true,
        },
    );
    let first: String = rows[0]
        .spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect();
    let second: String = rows[1]
        .spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect();
    assert_eq!(first, "你好");
    assert_eq!(second, "hello");
}

#[test]
fn display_toggles_do_not_require_refetching_yrc() {
    let lines = vec![line_with_words()];
    let _ = render_pair(
        &lines,
        1_700,
        30,
        &theme(),
        LyricOptions {
            word_lyrics: false,
            translation: false,
        },
    );
    assert_eq!(lines[0].words.len(), 2, "display off keeps YRC data");
    assert_eq!(
        lines[0].translation.as_deref(),
        Some("hello"),
        "display off keeps translation data"
    );

    let enabled = render_pair(
        &lines,
        1_700,
        30,
        &theme(),
        LyricOptions {
            word_lyrics: true,
            translation: true,
        },
    );
    assert!(enabled[0].spans.len() > 1, "YRC renders after enabling");
}

#[test]
fn translation_is_line_level_when_word_lyrics_is_off() {
    let lines = vec![line_with_words()];
    let rows = render_pair(
        &lines,
        1_700,
        30,
        &theme(),
        LyricOptions {
            word_lyrics: false,
            translation: true,
        },
    );
    assert_eq!(
        rows[1].spans.len(),
        1,
        "translation must not sweep per char"
    );
    assert_eq!(
        rows[0].spans.first().map(|span| span.style.fg),
        rows[1].spans.first().map(|span| span.style.fg),
        "translation color follows the line-level lyric color"
    );
}

#[test]
fn word_mode_without_yrc_sweeps_by_line_progress() {
    let lines = vec![LyricLine {
        start_ms: 1_000,
        end_ms: Some(2_000),
        text: "plain line".into(),
        words: Vec::new(),
        translation: Some("plain translation".into()),
    }];
    let rows = render_pair(
        &lines,
        1_700,
        40,
        &theme(),
        LyricOptions {
            word_lyrics: true,
            translation: true,
        },
    );
    assert!(rows[0].spans.len() > 1, "lyric sweeps per char");
    assert!(rows[1].spans.len() > 1, "translation sweeps per char");
    let active = Some(theme().color_accent2());
    assert_eq!(rows[0].spans.first().and_then(|span| span.style.fg), active);
    assert_eq!(rows[1].spans.first().and_then(|span| span.style.fg), active);
    assert_ne!(
        rows[1].spans.first().map(|span| span.style.fg),
        rows[1].spans.last().map(|span| span.style.fg),
        "later chars are still pending at 70% line progress"
    );
}

#[test]
fn untimed_plain_lyrics_use_uniform_current_color() {
    let lines = vec![
        LyricLine {
            start_ms: 0,
            end_ms: Some(u64::MAX),
            text: "first".into(),
            words: Vec::new(),
            translation: Some("um".into()),
        },
        LyricLine {
            start_ms: u64::MAX,
            end_ms: None,
            text: "second".into(),
            words: Vec::new(),
            translation: None,
        },
    ];
    let rows = render_pair(
        &lines,
        10_000,
        40,
        &theme(),
        LyricOptions {
            word_lyrics: true,
            translation: true,
        },
    );
    assert_eq!(rows[0].spans.len(), 1, "untimed lyric stays one span");
    assert_eq!(rows[0].spans[0].style.fg, Some(theme().color_accent2()));
    assert_eq!(rows[1].spans.len(), 1, "untimed translation stays one span");
    assert_eq!(rows[1].spans[0].style.fg, Some(theme().color_accent2()));
}

#[test]
fn translation_follows_word_progress() {
    let lines = vec![line_with_words()];
    let rows = render_pair(
        &lines,
        1_700,
        30,
        &theme(),
        LyricOptions {
            word_lyrics: true,
            translation: true,
        },
    );
    let translation = &rows[1];
    let first = translation
        .spans
        .first()
        .expect("translation spans")
        .style
        .fg;
    assert!(
        translation.spans.iter().any(|span| span.style.fg != first),
        "translation must reveal progressively"
    );
    assert_eq!(first, Some(theme().color_accent2()));
}

#[test]
fn window_shows_translation_for_every_visible_line() {
    let lines = vec![
        LyricLine {
            start_ms: 0,
            end_ms: Some(1_000),
            text: "zero".into(),
            words: Vec::new(),
            translation: Some("tr zero".into()),
        },
        line_with_words(),
        LyricLine {
            start_ms: 2_000,
            end_ms: Some(3_000),
            text: "two".into(),
            words: Vec::new(),
            translation: Some("tr two".into()),
        },
    ];
    let rows = render_window(
        &lines,
        1_700,
        7,
        30,
        &theme(),
        LyricOptions {
            word_lyrics: true,
            translation: true,
        },
    );
    let texts: Vec<String> = rows
        .iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect()
        })
        .collect();
    assert!(texts.iter().any(|text| text == "tr zero"));
    assert!(texts.iter().any(|text| text == "hello"));
    assert!(texts.iter().any(|text| text == "tr two"));

    let color_of = |needle: &str| {
        rows.iter()
            .find(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect::<String>()
                    == needle
            })
            .and_then(|line| line.spans.first().map(|span| span.style.fg))
            .expect("row color")
    };
    assert_eq!(color_of("tr zero"), color_of("zero"));
    assert_eq!(color_of("tr two"), color_of("two"));
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
    let start_text: String = start[0]
        .spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect();
    let later_text: String = later[0]
        .spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect();
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
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect()
        })
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
    let text: String = rows[0]
        .spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect();
    assert_eq!(text, "你好");
}
