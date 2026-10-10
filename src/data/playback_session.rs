use crate::data::{assets, atomic_file};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PlaybackSessionTrack {
    pub song_id: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub duration_ms: i64,
    pub cover_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlaylistCursor {
    pub source_id: String,
    pub next_offset: usize,
    pub total_tracks: Option<usize>,
    pub has_more: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PlaybackSessionRecord {
    #[serde(default)]
    pub queue: Vec<PlaybackSessionTrack>,
    pub current_index: Option<usize>,
    pub repeat_mode: Option<String>,
    #[serde(default)]
    pub source_playlist_id: Option<String>,
    /// 歌单/专辑的来源封面；旧存档缺少时使用歌曲封面兜底。
    #[serde(default)]
    pub source_cover_url: Option<String>,
    #[serde(default)]
    pub source_cursor: Option<PlaylistCursor>,
    pub updated_at: i64,
}

pub fn load() -> Result<Option<PlaybackSessionRecord>> {
    let path = session_path();
    if !path.is_file() {
        return Ok(None);
    }

    let raw = fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
    let record: PlaybackSessionRecord =
        toml::from_str(&raw).with_context(|| format!("parse {}", path.display()))?;
    if record.queue.is_empty() {
        return Ok(None);
    }

    Ok(Some(record))
}

pub fn save(record: &PlaybackSessionRecord) -> Result<()> {
    let path = session_path();
    let mut payload = record.clone();
    payload.updated_at = now_unix();
    let raw = toml::to_string_pretty(&payload).context("serialize playback session")?;
    atomic_file::write_atomic(&path, raw.as_bytes())
}

pub fn clear() -> Result<()> {
    let path = session_path();
    if path.is_file() {
        fs::remove_file(&path).with_context(|| format!("remove {}", path.display()))?;
    }
    Ok(())
}

fn session_path() -> PathBuf {
    assets::resolve_asset_path(Path::new("playback/session.toml"))
}

fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
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
}
