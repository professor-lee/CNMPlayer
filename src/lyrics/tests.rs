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
