use crate::tmplayer::app::state::LyricLine;
use std::collections::hash_map::DefaultHasher;
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::Path;

const MAX_LOCAL_COVER_BYTES: u64 = 8 * 1024 * 1024;

pub fn read_cover_from_folder(dir: &Path) -> Option<(Vec<u8>, u64)> {
    // Common filenames used by many players.
    // Keep this list small and predictable.
    let candidates = [
        "cover", "folder", "front", "album", "artwork", "Cover", "Folder", "Front",
    ];
    let exts = ["jpg", "jpeg", "png"];

    for base in candidates {
        for ext in exts {
            let p = dir.join(format!("{base}.{ext}"));
            if let Some(cover) = read_cover_file(&p) {
                return Some(cover);
            }
        }
    }
    None
}

fn read_cover_file(path: &Path) -> Option<(Vec<u8>, u64)> {
    let metadata = fs::metadata(path).ok()?;
    if !metadata.is_file() {
        return None;
    }

    let len = metadata.len();
    if len == 0 || len > MAX_LOCAL_COVER_BYTES {
        return None;
    }

    let bytes = fs::read(path).ok()?;
    if bytes.is_empty() {
        return None;
    }

    let hash = hash_bytes(&bytes);
    Some((bytes, hash))
}

pub fn parse_lrc(content: &str) -> Option<Vec<LyricLine>> {
    let mut out: Vec<LyricLine> = Vec::new();

    for raw in content.lines() {
        let mut s = raw.trim();
        if s.is_empty() {
            continue;
        }

        // Collect leading [..] tags; keep all time tags, ignore metadata tags like [ti:]
        let mut times: Vec<u64> = Vec::new();
        while let Some(rest) = s.strip_prefix('[') {
            let Some(end) = rest.find(']') else {
                break;
            };
            let tag = &rest[..end];
            if let Some(ms) = parse_lrc_time_tag(tag) {
                times.push(ms);
            }
            s = &rest[end + 1..];
        }

        if times.is_empty() {
            continue;
        }

        let text = s.trim().to_string();
        for t in times {
            out.push(LyricLine {
                start_ms: t,
                text: text.clone(),
            });
        }
    }

    if out.is_empty() {
        return None;
    }
    out.sort_by_key(|l| l.start_ms);
    Some(out)
}

pub fn parse_plain_lyrics(content: &str) -> Option<Vec<LyricLine>> {
    let mut non_empty = content.lines().map(str::trim).filter(|l| !l.is_empty());
    let first = non_empty.next()?.to_string();
    let second = non_empty.next().map(|s| s.to_string());

    let mut out = Vec::new();
    out.push(LyricLine {
        start_ms: 0,
        text: first,
    });
    if let Some(s2) = second {
        out.push(LyricLine {
            start_ms: u64::MAX,
            text: s2,
        });
    }
    Some(out)
}

fn parse_lrc_time_tag(tag: &str) -> Option<u64> {
    // Supports mm:ss, mm:ss.xx, mm:ss.xxx
    // Rejects metadata tags like "ti:xxx" by requiring numeric mm and ss.
    let (mm_s, rest) = tag.split_once(':')?;
    let mm: u64 = mm_s.trim().parse().ok()?;

    let rest = rest.trim();
    let (ss_s, frac_s) = if let Some((a, b)) = rest.split_once('.') {
        (a, Some(b))
    } else {
        (rest, None)
    };
    let ss: u64 = ss_s.trim().parse().ok()?;
    if ss >= 60 {
        // be lenient but avoid obvious non-timestamps
        return None;
    }

    let mut ms: u64 = 0;
    if let Some(frac) = frac_s {
        let frac = frac.trim();
        let digits: String = frac
            .chars()
            .take_while(|c| c.is_ascii_digit())
            .take(3)
            .collect();
        if digits.is_empty() {
            ms = 0;
        } else if digits.len() == 1 {
            ms = digits.parse::<u64>().ok()? * 100;
        } else if digits.len() == 2 {
            ms = digits.parse::<u64>().ok()? * 10;
        } else {
            ms = digits.parse::<u64>().ok()?;
        }
    }

    Some(mm * 60_000 + ss * 1_000 + ms)
}

fn hash_bytes(bytes: &[u8]) -> u64 {
    let mut h = DefaultHasher::new();
    bytes.hash(&mut h);
    h.finish()
}
