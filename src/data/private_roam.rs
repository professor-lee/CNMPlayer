use crate::data::assets;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PrivateRoamTrack {
    pub song_id: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub duration_ms: i64,
    pub cover_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PrivateRoamRecord {
    #[serde(default)]
    pub tracks: Vec<PrivateRoamTrack>,
    /// 上次播放歌曲在列表中的索引
    pub last_played_index: Option<usize>,
    /// 最后播放的漫游歌曲封面（切到别的列表播放后仍保留）
    pub last_played_cover_url: Option<String>,
    /// 每日刷新标记：上次刷新的日期（UTC 天数）
    pub last_refresh_day: Option<i64>,
    pub updated_at: i64,
}

pub fn load() -> Result<Option<PrivateRoamRecord>> {
    let path = session_path();
    if !path.is_file() {
        return Ok(None);
    }

    let raw = fs::read_to_string(&path)?;
    let record: PrivateRoamRecord = toml::from_str(&raw).unwrap_or_default();
    if record.tracks.is_empty() && record.last_played_cover_url.is_none() {
        return Ok(None);
    }

    Ok(Some(record))
}

pub fn save(record: &PrivateRoamRecord) -> Result<()> {
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
    assets::resolve_asset_path(Path::new("private_roam/session.toml"))
}

fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_secs() as i64)
        .unwrap_or_default()
}
