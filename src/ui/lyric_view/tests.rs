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
            translation: false,
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

fn bilingual_gap_lyrics() -> Vec<LyricLine> {
    crate::lyrics::parse_lyric_response(&serde_json::json!({
        "lrc": {"lyric": "[00:00.00]\n[00:01.00]verse\n[00:03.00]\n[00:05.00]\n[00:10.00]next\n[00:12.00]"},
        "tlyric": {"lyric": "[00:01.00]verse translation\n[00:10.00]next translation"}
    }))
    .unwrap()
}

#[test]
fn translated_pair_previews_original_and_translation_together_during_gaps() {
    let lines = bilingual_gap_lyrics();
    let theme = theme();
    for word_lyrics in [false, true] {
        let mut cache = LyricRenderCache::default();
        let options = LyricOptions {
            word_lyrics,
            translation: true,
        };
        for (position, expected) in [
            (0, ["verse", "verse translation"]),
            (1_500, ["verse", "verse translation"]),
            (3_000, ["next", "next translation"]),
            (9_999, ["next", "next translation"]),
            (10_000, ["next", "next translation"]),
            (12_000, ["", ""]),
            (5_000, ["next", "next translation"]),
        ] {
            let text = cache.pair(
                LyricSurface::Compact,
                &lines,
                position,
                false,
                40,
                &theme,
                options,
            );
            let actual: Vec<String> = text
                .lines
                .iter()
                .map(|row| row.spans.iter().map(|span| span.content.as_ref()).collect())
                .collect();
            assert_eq!(
                actual, expected,
                "position={position}, word_lyrics={word_lyrics}"
            );
            if matches!(position, 0 | 3_000 | 9_999 | 5_000) {
                for row in &text.lines {
                    assert_eq!(row.spans[0].style.fg, Some(theme.color_subtext()));
                }
            }
        }
    }
}

#[test]
fn translation_toggle_restores_original_current_and_next_line_layout() {
    let lines = bilingual_gap_lyrics();
    let theme = theme();
    for word_lyrics in [false, true] {
        let mut cache = LyricRenderCache::default();
        for (position, translation, expected) in [
            (0, false, ["", "verse"]),
            (1_500, false, ["verse", ""]),
            (3_000, false, ["", ""]),
            (5_000, false, ["", "next"]),
            (5_000, true, ["next", "next translation"]),
            (5_000, false, ["", "next"]),
            (10_000, false, ["next", ""]),
            (12_000, false, ["", ""]),
        ] {
            let text = cache.pair(
                LyricSurface::Compact,
                &lines,
                position,
                false,
                40,
                &theme,
                LyricOptions {
                    word_lyrics,
                    translation,
                },
            );
            let actual: Vec<String> = text
                .lines
                .iter()
                .map(|row| row.spans.iter().map(|span| span.content.as_ref()).collect())
                .collect();
            assert_eq!(
                actual, expected,
                "position={position}, translation={translation}"
            );
        }
    }
}

#[test]
fn translated_pair_does_not_mix_an_untranslated_line_with_the_next_original() {
    let mut lines = bilingual_gap_lyrics();
    lines.retain(|line| !line.text.is_empty());
    lines[0].translation = None;
    let rows = render_pair(
        &lines,
        1_500,
        40,
        &theme(),
        LyricOptions {
            word_lyrics: true,
            translation: true,
        },
    );
    let actual: Vec<String> = rows
        .iter()
        .map(|row| row.spans.iter().map(|span| span.content.as_ref()).collect())
        .collect();
    assert_eq!(actual, ["verse", ""]);
}

#[test]
fn translated_window_keeps_pairs_intact_at_viewport_edges() {
    let lines = bilingual_gap_lyrics();
    let theme = theme();
    let mut cache = LyricRenderCache::default();
    let options = LyricOptions {
        word_lyrics: true,
        translation: true,
    };
    for rows in 1..=9 {
        for position in [0, 1_500, 5_000, 10_000, 12_000] {
            let text = cache.window(&lines, position, false, rows, 40, &theme, options);
            let actual: Vec<String> = text
                .lines
                .iter()
                .map(|row| row.spans.iter().map(|span| span.content.as_ref()).collect())
                .collect();
            if position == 1_500 {
                if rows == 1 {
                    assert_eq!(actual, [""]);
                } else {
                    let current = (rows / 2).min(rows - 2);
                    assert_eq!(actual[current], "verse");
                    assert_eq!(actual[current + 1], "verse translation");
                }
            }
            for (index, row) in actual.iter().enumerate() {
                match row.as_str() {
                    "verse" => assert_eq!(
                        actual.get(index + 1).map(String::as_str),
                        Some("verse translation")
                    ),
                    "next" => assert_eq!(
                        actual.get(index + 1).map(String::as_str),
                        Some("next translation")
                    ),
                    "verse translation" => assert_eq!(
                        index.checked_sub(1).map(|i| actual[i].as_str()),
                        Some("verse")
                    ),
                    "next translation" => assert_eq!(
                        index.checked_sub(1).map(|i| actual[i].as_str()),
                        Some("next")
                    ),
                    "" => {}
                    other => panic!("unexpected lyric row: {other}"),
                }
            }
        }
    }
}

#[test]
fn untranslated_window_preserves_current_row_and_independent_next_lines() {
    let lines = bilingual_gap_lyrics();
    let theme = theme();
    let mut cache = LyricRenderCache::default();
    for (position, expected) in [
        (1_500, ["", "", "", "verse", "", "", "next"]),
        (5_000, ["", "verse", "", "", "next", "", ""]),
    ] {
        let text = cache.window(
            &lines,
            position,
            false,
            7,
            40,
            &theme,
            LyricOptions {
                word_lyrics: true,
                translation: false,
            },
        );
        let actual: Vec<String> = text
            .lines
            .iter()
            .map(|row| row.spans.iter().map(|span| span.content.as_ref()).collect())
            .collect();
        assert_eq!(actual, expected, "position={position}");
    }
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
fn long_lrc_lyrics_hold_each_segment_until_its_playback_phase() {
    let lines = vec![LyricLine {
        start_ms: 1_000,
        end_ms: Some(5_000),
        text: "lyric sample".into(),
        words: Vec::new(),
        translation: Some("LYRIC SAMPLE".into()),
    }];
    let theme = theme();
    for word_lyrics in [false, true] {
        let mut cache = LyricRenderCache::default();
        for (position, expected) in [
            (1_000, ["lyric samp", "LYRIC SAMP"]),
            (2_000, ["lyric samp", "LYRIC SAMP"]),
            (2_999, ["lyric samp", "LYRIC SAMP"]),
            (3_000, ["ric sample", "RIC SAMPLE"]),
            (4_999, ["ric sample", "RIC SAMPLE"]),
            (25_000, ["ric sample", "RIC SAMPLE"]),
            (1_500, ["lyric samp", "LYRIC SAMP"]),
            (1_500, ["lyric samp", "LYRIC SAMP"]),
        ] {
            let text = cache.pair(
                LyricSurface::Compact,
                &lines,
                position,
                false,
                10,
                &theme,
                LyricOptions {
                    word_lyrics,
                    translation: true,
                },
            );
            let actual: Vec<String> = text
                .lines
                .iter()
                .map(|row| row.spans.iter().map(|span| span.content.as_ref()).collect())
                .collect();
            assert_eq!(
                actual, expected,
                "position={position}, word_lyrics={word_lyrics}"
            );
        }
    }
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
    let lines = [line_with_words()];
    let rows = render_pair(
        &lines,
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

#[test]
fn cache_invalidates_equal_length_replacement_and_reuses_hits() {
    let mut cache = LyricRenderCache::default();
    let options = LyricOptions {
        word_lyrics: false,
        translation: false,
    };
    let first = vec![LyricLine {
        start_ms: 0,
        end_ms: Some(10_000),
        text: "first".into(),
        words: Vec::new(),
        translation: None,
    }];
    let second = vec![LyricLine {
        start_ms: 0,
        end_ms: Some(10_000),
        text: "other".into(),
        words: Vec::new(),
        translation: None,
    }];
    let one = cache.pair(
        LyricSurface::Overlay,
        &first,
        1_000,
        false,
        20,
        &theme(),
        options,
    );
    let one_ptr = one as *const _;
    assert_eq!(one.lines[0].spans[0].content.as_ref(), "first");
    let hit = cache.pair(
        LyricSurface::Overlay,
        &first,
        1_000,
        false,
        20,
        &theme(),
        options,
    );
    assert_eq!(hit as *const _, one_ptr);
    let replaced = cache.pair(
        LyricSurface::Overlay,
        &second,
        1_000,
        false,
        20,
        &theme(),
        options,
    );
    assert_eq!(replaced.lines[0].spans[0].content.as_ref(), "other");
}

#[test]
fn cache_key_includes_palette_capability_and_seek_state() {
    let lines = vec![line_with_words()];
    let options = LyricOptions {
        word_lyrics: true,
        translation: true,
    };
    let mut cache = LyricRenderCache::default();
    let mut changed = theme();
    let a = cache.pair(
        LyricSurface::Overlay,
        &lines,
        1_700,
        false,
        20,
        &changed,
        options,
    );
    let color_a = a.lines[0].spans[0].style.fg;
    changed.palette.accent2 = (1, 2, 3);
    let b = cache.pair(
        LyricSurface::Overlay,
        &lines,
        1_700,
        false,
        20,
        &changed,
        options,
    );
    assert_ne!(b.lines[0].spans[0].style.fg, color_a);
    changed.capability = ColorCapability::NoColor;
    let c = cache.pair(
        LyricSurface::Overlay,
        &lines,
        1_700,
        true,
        20,
        &changed,
        options,
    );
    assert_eq!(
        c.lines[0].spans[0].style.fg,
        Some(ratatui::style::Color::Reset)
    );
}

#[test]
fn cache_preserves_cjk_and_pre_first_output() {
    let lines = vec![line_with_words()];
    let mut cache = LyricRenderCache::default();
    let options = LyricOptions {
        word_lyrics: true,
        translation: true,
    };
    let pre = cache.pair(
        LyricSurface::SmallWindow,
        &lines,
        0,
        false,
        20,
        &theme(),
        options,
    );
    assert_eq!(
        pre.lines[0]
            .spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect::<String>(),
        "你好"
    );
    let window = cache.window(&lines, 1_700, false, 5, 20, &theme(), options);
    assert_eq!(
        window.lines[2]
            .spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect::<String>(),
        "你好"
    );
}

#[test]
fn cache_invalidates_in_place_text_translation_and_word_timing() {
    let mut cache = LyricRenderCache::default();
    let mut lines = vec![line_with_words()];
    let options = LyricOptions {
        word_lyrics: true,
        translation: true,
    };
    let initial = cache
        .pair(
            LyricSurface::Overlay,
            &lines,
            1_700,
            false,
            20,
            &theme(),
            options,
        )
        .clone();
    // Mutate the existing allocation, so pointer+length identity would be stale.
    lines[0].words[0].text.replace_range(.., "您");
    lines[0].text.replace_range(.., "您好");
    lines[0]
        .translation
        .as_mut()
        .unwrap()
        .replace_range(.., "world");
    let edited = cache
        .pair(
            LyricSurface::Overlay,
            &lines,
            1_700,
            false,
            20,
            &theme(),
            options,
        )
        .clone();
    assert_eq!(
        edited.lines[0]
            .spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect::<String>(),
        "您好"
    );
    assert_eq!(
        edited.lines[1]
            .spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect::<String>(),
        "world"
    );
    assert_ne!(initial, edited);
    lines[0].words[0].end_ms = 3_000;
    let retimed = cache.pair(
        LyricSurface::Overlay,
        &lines,
        1_700,
        false,
        20,
        &theme(),
        options,
    );
    assert_ne!(retimed, &edited);
}

#[test]
fn yrc_scroll_uses_sung_characters_and_holds_during_word_gaps() {
    let lines = vec![LyricLine {
        start_ms: 1_000,
        end_ms: Some(31_000),
        text: "lyric sample".into(),
        words: vec![
            LyricWord {
                text: "ly".into(),
                start_ms: 1_000,
                end_ms: 3_000,
            },
            LyricWord {
                text: "ric sample".into(),
                start_ms: 9_000,
                end_ms: 19_000,
            },
        ],
        translation: Some("LYRIC SAMPLE".into()),
    }];
    for word_lyrics in [false, true] {
        for (position, expected) in [
            (3_000, "lyric samp"),
            (8_999, "lyric samp"),
            (12_999, "lyric samp"),
            (13_000, "ric sample"),
            (30_000, "ric sample"),
            (1_500, "lyric samp"),
        ] {
            let rows = render_pair(
                &lines,
                position,
                10,
                &theme(),
                LyricOptions {
                    word_lyrics,
                    translation: true,
                },
            );
            let text: String = rows[0].spans.iter().map(|s| s.content.as_ref()).collect();
            let translation: String = rows[1].spans.iter().map(|s| s.content.as_ref()).collect();
            assert_eq!(text, expected, "position={position}");
            assert_eq!(translation, expected.to_uppercase());
        }
    }
}

#[test]
fn segmented_scroll_clips_wide_characters_without_drifting_or_looping() {
    let lines = vec![LyricLine {
        start_ms: 1_000,
        end_ms: Some(5_000),
        text: "你好世界AB".into(),
        words: Vec::new(),
        translation: None,
    }];
    for (position, expected) in [
        (1_000, "你好"),
        (2_999, "你好"),
        (3_000, " 界AB"),
        (20_000, " 界AB"),
        (1_500, "你好"),
    ] {
        let rows = render_pair(
            &lines,
            position,
            5,
            &theme(),
            LyricOptions {
                word_lyrics: true,
                translation: false,
            },
        );
        let text: String = rows[0].spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(text, expected);
    }
}

#[test]
fn cache_preserves_untimed_document_and_bounds_surface_entries() {
    let lines: Vec<_> = ["first", "second", "third", "fourth"]
        .into_iter()
        .enumerate()
        .map(|(index, text)| LyricLine {
            start_ms: if index == 0 { 0 } else { u64::MAX },
            end_ms: Some(u64::MAX),
            text: text.into(),
            words: Vec::new(),
            translation: None,
        })
        .collect();
    let mut cache = LyricRenderCache::default();
    let options = LyricOptions {
        word_lyrics: true,
        translation: false,
    };
    let mut changed_theme = theme();
    for (position, width, rows) in [(2_000, 20, 7), (9_000, 20, 9), (9_000, 3, 5)] {
        let expected = render_window(&lines, position, rows, width, &changed_theme, options);
        let actual = cache.window(
            &lines,
            position,
            false,
            rows,
            width,
            &changed_theme,
            options,
        );
        assert_eq!(actual.lines, expected);
    }
    let narrow = cache.pair(
        LyricSurface::Overlay,
        &lines,
        9_000,
        false,
        3,
        &changed_theme,
        options,
    );
    assert_eq!(narrow.lines[1].spans[0].content.as_ref(), "se…");
    changed_theme.palette.surface = (1, 2, 3);
    cache.pair(
        LyricSurface::Compact,
        &lines,
        9_000,
        false,
        20,
        &changed_theme,
        options,
    );
    assert_eq!(cache.frames.last().unwrap().key.palette[3], (1, 2, 3));
    assert_eq!(cache.frames.len(), 2);
}

#[test]
fn styled_glyph_runs_are_borrowed_and_coalesced_before_caching() {
    let text = "你好世界AB";
    let spans = progress_spans(text, 0.0, &theme(), true);
    assert_eq!(
        spans.len(),
        1,
        "pending glyphs share one style and allocation"
    );
    assert!(matches!(spans[0].content, std::borrow::Cow::Borrowed(_)));
    assert_eq!(spans[0].content.as_ref(), text);
}

#[test]
fn cache_borrowed_widget_matches_paragraph_output_and_toggle_invalidation() {
    use ratatui::buffer::Buffer;
    use ratatui::layout::{Alignment, Rect};
    use ratatui::widgets::{Paragraph, Widget};
    let lines = vec![line_with_words()];
    let mut cache = LyricRenderCache::default();
    let area = Rect::new(0, 0, 9, 2);
    for options in [
        LyricOptions {
            word_lyrics: true,
            translation: true,
        },
        LyricOptions {
            word_lyrics: false,
            translation: true,
        },
        LyricOptions {
            word_lyrics: false,
            translation: false,
        },
    ] {
        let expected = render_pair(&lines, 1_700, area.width, &theme(), options);
        let mut old = Buffer::empty(area);
        Paragraph::new(expected)
            .alignment(Alignment::Center)
            .render(area, &mut old);
        let mut new = Buffer::empty(area);
        cache
            .pair(
                LyricSurface::Overlay,
                &lines,
                1_700,
                false,
                area.width,
                &theme(),
                options,
            )
            .render(area, &mut new);
        assert_eq!(new, old);
        if !options.word_lyrics {
            assert_eq!(cache.frames.last().unwrap().text.lines[0].spans.len(), 1);
        }
        if !options.translation {
            assert!(cache.frames.last().unwrap().text.lines[1].spans.is_empty());
        }
    }
}

#[test]
fn cached_completed_words_share_one_owned_style_run() {
    let lines = vec![line_with_words()];
    let options = LyricOptions {
        word_lyrics: true,
        translation: true,
    };
    let mut cache = LyricRenderCache::default();
    let rows = cache.pair(
        LyricSurface::Compact,
        &lines,
        2_000,
        false,
        20,
        &theme(),
        options,
    );
    assert_eq!(rows.lines[0].spans.len(), 1);
    assert_eq!(rows.lines[0].spans[0].content.as_ref(), "你好");
    assert_eq!(
        rows.lines[0].spans[0].style.fg,
        Some(theme().color_accent2())
    );
}
