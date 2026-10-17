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
}
