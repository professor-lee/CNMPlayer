use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

#[derive(Debug, Serialize, Deserialize, Default)]
struct OrderFile {
    order: Vec<String>,

    #[serde(default)]
    last_opened_song: Option<String>,

    #[serde(default)]
    last_album: Option<String>,

    #[serde(default)]
    last_position_song: Option<String>,

    #[serde(default)]
    last_position_sec: Option<u64>,

    // Cached rendered cover ASCII, keyed by "<hash>:<width>x<height>".
    // Stored here to speed up subsequent loads without re-decoding/resizing images.
    #[serde(default)]
    cover: HashMap<String, String>,
}

fn cover_key(hash: u64, width: u16, height: u16) -> String {
    format!("{hash}:{width}x{height}")
}

pub fn read_cover_ascii_cache(folder: &Path, hash: u64, width: u16, height: u16) -> Option<String> {
    let p = folder.join(".order.toml");
    if !p.exists() {
        return None;
    }
    let of = read_order_file(folder)?;
    of.cover.get(&cover_key(hash, width, height)).cloned()
}

pub fn write_cover_ascii_cache(
    folder: &Path,
    hash: u64,
    width: u16,
    height: u16,
    ascii: &str,
) -> Result<bool> {
    // If the file exists but is unreadable/unparseable, avoid clobbering it.
    let p = folder.join(".order.toml");
    let mut of = if p.exists() {
        match read_order_file(folder) {
            Some(v) => v,
            None => return Ok(false),
        }
    } else {
        OrderFile::default()
    };

    let k = cover_key(hash, width, height);
    if let Some(existing) = of.cover.get(&k) {
        if existing == ascii {
            return Ok(false);
        }
    }

    of.cover.insert(k, ascii.to_string());
    write_order_file_struct(folder, &of)?;
    Ok(true)
}

fn read_order_file(folder: &Path) -> Option<OrderFile> {
    let p = folder.join(".order.toml");
    let s = std::fs::read_to_string(p).ok()?;
    toml::from_str(&s).ok()
}

fn write_order_file_struct(folder: &Path, of: &OrderFile) -> Result<()> {
    let content = toml::to_string_pretty(of)?;

    let tmp = folder.join(".order.toml.tmp");
    let dst = folder.join(".order.toml");
    std::fs::write(&tmp, content)?;
    // Best-effort atomic replace.
    let _ = std::fs::remove_file(&dst);
    std::fs::rename(&tmp, &dst)?;
    Ok(())
}
