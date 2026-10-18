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
fn lrc_empty_timestamps_clear_the_current_line() {
    let body = json!({
        "lrc": {"lyric": "[00:00.00]\n[00:01.00]verse\n[00:03.00]\n[00:10.00]next\n[00:12.00]"}
    });
    let lines = parse_lyric_response(&body).unwrap();
    for (position, expected) in [
        (0, ""),
        (1_000, "verse"),
        (2_999, "verse"),
        (3_000, ""),
        (5_000, ""),
        (9_999, ""),
        (10_000, "next"),
        (12_000, ""),
    ] {
        let current = snapshot(&lines, position).current.unwrap();
        assert_eq!(lines[current].text, expected, "position={position}");
    }
}

#[test]
fn timestamp_only_lrc_is_not_a_lyric_document() {
    let body = json!({"lrc": {"lyric": "[00:01.00]\n[00:03.00]   "}});
    assert!(parse_lyric_response(&body).is_none());
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
fn yrc_preserves_parenthesized_word_text() {
    for first_word in [
        "Hello (friend)",
        "Hello ((friend))",
        "Hello (friend",
        "Hello (2026)",
        "Hello (1,2)",
        "Hello (friend) (again)",
    ] {
        let body = json!({
            "yrc": {"lyric": format!("[1000,2000](1000,1000,0){first_word}(2000,1000,0)!")},
            "lrc": {"lyric": format!("[00:01.00]{first_word}!")}
        });
        let lines = parse_lyric_response(&body).unwrap();
        assert_eq!(lines[0].text, format!("{first_word}!"));
        assert_eq!(
            lines[0].words,
            vec![
                LyricWord {
                    text: first_word.into(),
                    start_ms: 1_000,
                    end_ms: 2_000,
                },
                LyricWord {
                    text: "!".into(),
                    start_ms: 2_000,
                    end_ms: 3_000,
                },
            ]
        );
    }
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
fn snapshot_tracks_line_progress() {
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
    assert!((mid.line_progress - 0.35).abs() < 1e-6);
}

#[test]
fn plain_lyrics_use_untimed_sentinels() {
    let multi = parse_plain_lyrics("first\nsecond").unwrap();
    assert!(multi[0].is_untimed());
    assert!(multi[1].is_untimed());
    assert_eq!(multi[0].end_ms, Some(u64::MAX));

    let single = parse_plain_lyrics("only").unwrap();
    assert!(single[0].is_untimed());
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

#[test]
fn response_lrc_preserves_non_timestamp_brackets_and_clear_nodes() {
    let body = json!({"lrc": "[ar:artist]\n[00:01.00][00:03.00][Chorus]Hello\n[00:02.00]\n[00:04.00]作词: credit"});
    let lines = parse_lyric_response(&body).unwrap();
    assert_eq!(lines.len(), 3);
    assert_eq!(lines[0].text, "[Chorus]Hello");
    assert_eq!(lines[1].text, "");
    assert_eq!(lines[2].text, "[Chorus]Hello");
    assert_eq!(lines[0].end_ms, Some(2_000));
}

#[test]
fn response_enhanced_lrc_retains_text_and_uses_real_word_bounds() {
    let body =
        json!({"lrc": "[00:01.00]intro <ordinary><00:02.00>Hello <00:03.00>world\n[00:05.00]next"});
    let lines = parse_lyric_response(&body).unwrap();
    assert_eq!(lines[0].text, "intro <ordinary>Hello world");
    assert_eq!(
        lines[0].words,
        vec![
            LyricWord {
                text: "intro <ordinary>".into(),
                start_ms: 1_000,
                end_ms: 2_000
            },
            LyricWord {
                text: "Hello ".into(),
                start_ms: 2_000,
                end_ms: 3_000
            },
            LyricWord {
                text: "world".into(),
                start_ms: 3_000,
                end_ms: 5_000
            },
        ]
    );
    assert_eq!(lines[0].end_ms, Some(5_000));
    assert!(lines[1].words.is_empty());
}

#[test]
fn response_enhanced_lrc_trailing_zero_and_gap_bounds_are_deterministic() {
    let body = json!({"lrc": "[00:00.00]<00:00.00>A<00:00.00>B<00:02.00>\n[00:05.00]<00:05.00>C<00:06.00>D"});
    let lines = parse_lyric_response(&body).unwrap();
    assert_eq!(lines[0].text, "AB");
    assert_eq!(lines[0].words[0].end_ms, 0);
    assert_eq!(lines[0].words[1].end_ms, 2_000);
    assert_eq!(lines[0].end_ms, Some(2_000));
    assert_eq!(snapshot(&lines, 4_000).current, Some(0));
    assert_eq!(lines[1].words[0].end_ms, 6_000);
    assert_eq!(lines[1].words[1].start_ms, 6_000);
    assert_eq!(lines[1].words[1].end_ms, 6_000);
    assert_eq!(lines[1].end_ms, None);
}

#[test]
fn response_malformed_timestamp_tags_do_not_consume_lyric_text() {
    for suffix in [
        "[00:60.00]text",
        "[00:01.x]text",
        "[00:01.1234oops]text",
        "[00:01.00oops]text",
        "[18446744073709551615:00]text",
    ] {
        let body = json!({"lrc": format!("[00:01.00]{suffix}")});
        assert_eq!(parse_lyric_response(&body).unwrap()[0].text, suffix);
    }
    let body = json!({"lrc": "[00:01.00]<00:99.00>text<00:02.bad>more<unfinished"});
    let lines = parse_lyric_response(&body).unwrap();
    assert_eq!(lines[0].text, "<00:99.00>text<00:02.bad>more<unfinished");
    assert!(lines[0].words.is_empty());
}

#[test]
fn response_normalizes_json_lrc_and_mixed_documents_without_credits() {
    for field in ["lrc", "yrc"] {
        let mut body = json!({});
        body[field] = json!({"lyric": concat!(
            "{\"t\":0,\"c\":[{\"tx\":\"作词: credit\"}]}\n",
            "{\"t\":1000,\"c\":[{\"tx\":\"你\",\"tr\":[0,400]},{\"tx\":\"好\",\"tr\":[400,600]}]}"
        )});
        body["tlyric"] = json!("[00:01.00]hello");
        let lines = parse_lyric_response(&body).unwrap();
        assert_eq!(lines.len(), 1, "field={field}");
        assert_eq!(lines[0].text, "你好");
        assert_eq!(lines[0].words[1].end_ms, 2_000);
        assert_eq!(lines[0].translation.as_deref(), Some("hello"));
    }
    let body = json!({"lrc": concat!(
        "{\"t\":1000,\"c\":[{\"tx\":\"usable untimed words\"}]}\n",
        "{broken JSON\n",
        "[00:03.00]ordinary <angle>\n",
        "{\"t\":4000,\"c\":[{\"tx\":\"end\",\"tr\":[0,0]}]}"
    )});
    let lines = parse_lyric_response(&body).unwrap();
    assert_eq!(
        lines
            .iter()
            .map(|line| line.text.as_str())
            .collect::<Vec<_>>(),
        vec!["usable untimed words", "ordinary <angle>", "end"]
    );
    assert!(lines[0].words.is_empty());
    assert_eq!(lines[0].end_ms, Some(3_000));
    assert_eq!(lines[2].words[0].start_ms, 4_000);
    assert_eq!(lines[2].words[0].end_ms, 4_000);
}

#[test]
fn response_malformed_formats_never_display_raw_json_or_metadata() {
    for source in [
        "{broken JSON",
        "{\"t\":-1,\"c\":[{\"tx\":\"invalid\"}]}",
        "{\"c\":[{\"tx\":\"missing timestamp\"}]}",
        "[ar:artist]\n[ti:title]\n[offset:500]",
        "[18446744073709551615:00]overflow",
        "{\"t\":0,\"c\":[{\"tx\":\"作曲: credit\"}]}",
    ] {
        assert!(
            parse_lyric_response(&json!({"lrc": source})).is_none(),
            "{source}"
        );
    }
    let body = json!({"yrc": "[1000,500](broken)bad", "lrc": "[00:01.00]usable"});
    assert_eq!(parse_lyric_response(&body).unwrap()[0].text, "usable");
}

#[test]
fn response_plain_fallback_retains_the_complete_unsynchronized_document() {
    let lines = parse_lyric_response(&json!({"lrc": "first\nsecond\nthird\nfourth"})).unwrap();
    assert_eq!(
        lines
            .iter()
            .map(|line| line.text.as_str())
            .collect::<Vec<_>>(),
        vec!["first", "second", "third", "fourth"]
    );
    assert!(lines.iter().all(LyricLine::is_untimed));
    assert_eq!(snapshot(&lines, 60_000).current, Some(0));
}

#[test]
fn response_translation_matching_keeps_the_inclusive_120ms_boundary() {
    let body = json!({
        "yrc": "[1000,1000](1000,1000,0)Hello (friend)\n[3000,1000](3000,1000,0)next",
        "ytlrc": "[00:01.12]你好\n[00:03.121]too far",
        "tlyric": "[00:01.00]not preferred"
    });
    let lines = parse_lyric_response(&body).unwrap();
    assert_eq!(lines[0].text, "Hello (friend)");
    assert_eq!(lines[0].translation.as_deref(), Some("你好"));
    assert_eq!(lines[1].translation, None);
}

#[test]
fn response_timestamp_fractions_and_decreasing_word_tags_are_bounded() {
    let lines = parse_lyric_response(&json!({"lrc": concat!(
        "[00:00.1]tenth\n[00:00.12]hundredth\n[00:00.123]millisecond\n",
        "[00:01.00]<00:02.00>A<00:01.00>B<00:03.00>"
    )}))
    .unwrap();
    assert_eq!(lines[0].start_ms, 100);
    assert_eq!(lines[1].start_ms, 120);
    assert_eq!(lines[2].start_ms, 123);
    assert_eq!(lines[3].text, "AB");
    assert_eq!(lines[3].words[0].start_ms, 2_000);
    assert_eq!(lines[3].words[0].end_ms, 2_000);
    assert_eq!(lines[3].words[1].start_ms, 2_000);
    assert_eq!(lines[3].words[1].end_ms, 3_000);
}

#[test]
fn response_json_overflow_and_invalid_yrc_fall_back_without_panicking() {
    for source in [
        "{\"t\":18446744073709551615,\"c\":[{\"tx\":\"overflow\",\"tr\":[1,1]}]}",
        "{\"t\":1,\"c\":[{\"tx\":\"negative\",\"tr\":[0,-1]}]}",
        "[1000,2000,extra](1000,1000,0)bad",
        "[1000,2000](1000,1000)bad",
    ] {
        let body = json!({"yrc": source, "lrc": "[00:01.00]fallback"});
        assert_eq!(
            parse_lyric_response(&body).unwrap()[0].text,
            "fallback",
            "{source}"
        );
    }
    let lines = parse_lyric_response(&json!({"lrc": "[Chorus]one\ntwo\nthree"})).unwrap();
    assert_eq!(lines.len(), 3);
    assert_eq!(lines[0].text, "[Chorus]one");
}

#[test]
fn response_malformed_fields_without_usable_lyrics_remain_absent() {
    for body in [
        json!({"lrc": "{broken JSON"}),
        json!({"yrc": "[broken"}),
        json!({"lrc": "[]"}),
        json!({"lrc": "[\"raw JSON\"]"}),
        json!({"lrc": "[00:01.00]作词: credit"}),
    ] {
        assert!(parse_lyric_response(&body).is_none(), "{body}");
    }
}

#[test]
fn response_plain_brackets_are_not_mistaken_for_structural_tags() {
    let lines =
        parse_lyric_response(&json!({"lrc": "[broken\n[2026]text\n[[Chorus]]\n<ordinary>"}))
            .unwrap();
    assert_eq!(
        lines
            .iter()
            .map(|line| line.text.as_str())
            .collect::<Vec<_>>(),
        vec!["[broken", "[2026]text", "[[Chorus]]", "<ordinary>"]
    );
}

#[test]
fn response_json_metadata_does_not_hide_lrc_or_plain_fallback() {
    let credits = r#"{"t":0,"c":[{"tx":"作词: credit"}]}"#;
    let timed =
        parse_lyric_response(&json!({"lrc": format!("{credits}\n[00:01.00]usable")})).unwrap();
    assert_eq!(timed.len(), 1);
    assert_eq!(timed[0].text, "usable");
    let plain =
        parse_lyric_response(&json!({"lrc": format!("{credits}\none\ntwo\nthree")})).unwrap();
    assert_eq!(
        plain
            .iter()
            .map(|line| line.text.as_str())
            .collect::<Vec<_>>(),
        vec!["one", "two", "three"]
    );
    assert!(plain.iter().all(LyricLine::is_untimed));
}

#[test]
fn response_higher_precision_timestamps_truncate_to_milliseconds() {
    let lines = parse_lyric_response(&json!({"lrc": concat!(
        "[00:01.123456]<00:01.123456>first<00:02.234567>second<00:03.345678>\n",
        "[00:04.456789]next"
    )}))
    .unwrap();
    assert_eq!(lines[0].start_ms, 1_123);
    assert_eq!(lines[0].text, "firstsecond");
    assert_eq!(lines[0].words[0].start_ms, 1_123);
    assert_eq!(lines[0].words[0].end_ms, 2_234);
    assert_eq!(lines[0].words[1].start_ms, 2_234);
    assert_eq!(lines[0].words[1].end_ms, 3_345);
    assert_eq!(lines[0].end_ms, Some(3_345));
    assert_eq!(lines[1].start_ms, 4_456);
}
