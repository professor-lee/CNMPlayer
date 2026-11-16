use super::*;

#[test]
fn playback_memory_preserves_source_cover_and_accepts_older_records() {
    let old = "updated_at = 0\nsource_playlist_id = 'playlist-42'\n";
    let with_cover = format!("{old}source_cover_url = 'https://example.com/playlist.jpg'\n");
    let record: PlaybackSessionRecord = toml::from_str(&with_cover).unwrap();
    let saved: toml::Value = toml::from_str(&toml::to_string(&record).unwrap()).unwrap();
    assert_eq!(
        saved.get("source_cover_url").and_then(toml::Value::as_str),
        Some("https://example.com/playlist.jpg"),
    );
    let old_record: PlaybackSessionRecord = toml::from_str(old).unwrap();
    assert!(old_record.source_cover_url.is_none());
    assert_eq!(
        old_record.source_playlist_id.as_deref(),
        Some("playlist-42")
    );
    assert!(old_record.shuffle.is_none());
}

#[test]
fn shuffle_progress_survives_a_save_round_trip() {
    let shuffle = PlaybackShuffleState {
        history: vec!["1".into(), "2".into()],
        upcoming: vec!["5".into(), "4".into()],
        round: vec!["2".into(), "3".into(), "4".into(), "5".into()],
    };
    let record = PlaybackSessionRecord {
        queue: vec![PlaybackSessionTrack {
            song_id: "3".into(),
            ..PlaybackSessionTrack::default()
        }],
        current_index: Some(0),
        repeat_mode: Some("shuffle".into()),
        shuffle: Some(shuffle.clone()),
        ..PlaybackSessionRecord::default()
    };
    let saved = toml::to_string_pretty(&record).unwrap();
    let restored: PlaybackSessionRecord = toml::from_str(&saved).unwrap();
    assert_eq!(restored.shuffle, Some(shuffle));
}
