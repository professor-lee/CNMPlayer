use crate::data::assets;
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

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PlaybackSessionRecord {
    #[serde(default)]
    pub queue: Vec<PlaybackSessionTrack>,
    pub current_index: Option<usize>,
    pub repeat_mode: Option<String>,
    /// 队列来源列表的 id（如私人漫游的 tile id）。
    ///
    /// 私人漫游有「播完自动在尾部追加新歌」「封面跟随当前播放歌曲」等
    /// 依赖来源的行为，重启后必须能还原出队列来自哪个列表。
    /// `serde(default)` 保证旧存档（无此字段）仍可读入。
    #[serde(default)]
    pub source_playlist_id: Option<String>,
    pub updated_at: i64,
}

pub fn load() -> Result<Option<PlaybackSessionRecord>> {
    let path = session_path();
    if !path.is_file() {
        return Ok(None);
    }

    let raw = fs::read_to_string(&path)?;
    let record: PlaybackSessionRecord = toml::from_str(&raw).unwrap_or_default();
    if record.queue.is_empty() {
        return Ok(None);
    }

    Ok(Some(record))
}

pub fn save(record: &PlaybackSessionRecord) -> Result<()> {
    let path = session_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("mkdir {}", parent.display()))?;
    }

    let mut payload = record.clone();
    payload.updated_at = now_unix();
    let raw = toml::to_string_pretty(&payload).unwrap_or_default();
    fs::write(&path, raw).with_context(|| format!("write {}", path.display()))?;
    Ok(())
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
        .unwrap_or_default()
}
