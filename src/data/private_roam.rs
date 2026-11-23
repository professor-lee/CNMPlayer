use crate::data::{assets, atomic_file};
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

/// 私人漫游（私人 FM）推荐模式；序列化值即接口的 `mode` 参数。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum PrivateRoamMode {
    #[default]
    Default,
    Familiar,
    Explore,
}

impl PrivateRoamMode {
    /// 接口 `mode` 参数值。
    pub fn api_value(self) -> &'static str {
        match self {
            Self::Default => "DEFAULT",
            Self::Familiar => "FAMILIAR",
            Self::Explore => "EXPLORE",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PrivateRoamRecord {
    #[serde(default)]
    pub tracks: Vec<PrivateRoamTrack>,
    pub last_played_index: Option<usize>,
    pub last_played_cover_url: Option<String>,
    pub last_refresh_day: Option<i64>,
    #[serde(default)]
    pub mode: PrivateRoamMode,
    pub updated_at: i64,
}

pub fn load() -> Result<Option<PrivateRoamRecord>> {
    let path = session_path();
    if !path.is_file() {
        return Ok(None);
    }

    let raw = fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
    let record: PrivateRoamRecord =
        toml::from_str(&raw).with_context(|| format!("parse {}", path.display()))?;
    if record.tracks.is_empty() && record.last_played_cover_url.is_none() {
        return Ok(None);
    }

    Ok(Some(record))
}

pub fn save(record: &PrivateRoamRecord) -> Result<()> {
    let path = session_path();
    let mut payload = record.clone();
    payload.updated_at = now_unix();
    let raw = toml::to_string_pretty(&payload).context("serialize private roam session")?;
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
    assets::resolve_asset_path(Path::new("private_roam/session.toml"))
}

fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_secs() as i64)
        .unwrap_or(0)
}
