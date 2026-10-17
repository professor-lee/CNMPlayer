use super::api::ApiState;
use super::response::first_non_empty;
use super::response::parse_value_as_string;
use serde_json::Value;

/// `song/detail` →「点名字进页面」要用的作者 / 专辑 ID。
///
/// 队列与搜索结果只带歌曲 ID 与拼好的显示名（曲目行没有 `ar`/`al` 的 ID），
/// 所以按需解析一次。
pub(super) async fn fetch_song_page_refs(mut api: ApiState, song_id: &str) -> SongPageRefs {
    let Ok(detail) = api.song_detail(song_id).await else {
        return SongPageRefs::default();
    };

    let Some(song) = detail
        .body
        .get("songs")
        .and_then(|value| value.as_array())
        .and_then(|items| items.first())
    else {
        return SongPageRefs::default();
    };

    let artists = song
        .get("ar")
        .and_then(|value| value.as_array())
        .map(|items| {
            items
                .iter()
                .filter_map(|item| {
                    let name = item.get("name").and_then(|value| value.as_str())?;
                    Some((name.to_string(), parse_value_as_string(item.get("id"))))
                })
                .collect()
        })
        .unwrap_or_default();
    let album_id = parse_value_as_string(song.pointer("/al/id"));

    SongPageRefs { artists, album_id }
}

#[derive(Debug, Clone)]
pub(crate) struct PlaylistTrack {
    pub(crate) kind: PlaylistTrackKind,
    pub(crate) id: Option<String>,
    pub(crate) title: String,
    pub(crate) artist: String,
    pub(crate) album: String,
    pub(crate) cover_url: Option<String>,
    pub(crate) duration_ms: i64,
    pub(crate) duration: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PlaylistTrackKind {
    Song,
    Album,
    Ep,
    Single,
}

pub(super) fn parse_tracks(items: &[Value]) -> Vec<PlaylistTrack> {
    let mut tracks = Vec::new();

    for item in items {
        let Some(title) = item.get("name").and_then(|value| value.as_str()) else {
            continue;
        };

        let artist = parse_artists(item)
            .unwrap_or_else(|| "Unknown Artist".to_string())
            .trim()
            .to_string();
        let duration_ms = item
            .get("dt")
            .and_then(|value| value.as_i64())
            .or_else(|| item.get("duration").and_then(|value| value.as_i64()))
            .unwrap_or(0);

        tracks.push(PlaylistTrack {
            kind: PlaylistTrackKind::Song,
            id: parse_value_as_string(item.get("id")),
            title: title.to_string(),
            artist,
            album: item
                .pointer("/al/name")
                .and_then(|value| value.as_str())
                .unwrap_or("Unknown Album")
                .to_string(),
            cover_url: first_non_empty(item, &["/al/picUrl", "/album/picUrl"]),
            duration_ms,
            duration: format_duration(duration_ms),
        });
    }

    tracks
}

/// 多作者显示串的连接符：`parse_artists` 用它把 `ar` 拼成一行，
/// 全屏页信息区再按它切回每段，好让"点谁的名字进谁的页面"。
pub(crate) const ARTIST_SEPARATOR: &str = " / ";

/// 把 `parse_artists` 拼出来的作者行按连接符切回每段（显示顺序 = `ar` 顺序）。
///
/// 空段保留：段序号要和显示位置一一对应，全屏页传回来的序号才对得上。
pub(crate) fn artist_name_segments(line: &str) -> Vec<&str> {
    line.split(ARTIST_SEPARATOR).collect()
}

/// 全屏页点名字进页面时，`song/detail` 里用得上的两样东西。
#[derive(Debug, Default)]
pub(super) struct SongPageRefs {
    /// `ar`：显示顺序的作者（名称 + ID；缺 ID 的条目保留占位，别让后面的下标错位）。
    pub(super) artists: Vec<(String, Option<String>)>,
    pub(super) album_id: Option<String>,
}

/// `song/detail` 里显示串第 `index` 段对应的作者 ID。
///
/// 先按名字匹配（显示串由同一份 `ar` 拼出来，正常都能命中；`ar` 顺序或名字带后缀时也稳），
/// 再按位置兜底（`ar` 顺序 = 显示顺序）；两者都没有就返回 `None`，让调用方只报状态、不换页。
pub(super) fn pick_artist_id(
    artists: &[(String, Option<String>)],
    line: &str,
    index: usize,
) -> Option<String> {
    if let Some(name) = artist_name_segments(line).get(index).copied()
        && let Some((_, id)) = artists
            .iter()
            .find(|(candidate, _)| candidate.as_str() == name)
    {
        return id.clone();
    }

    artists.get(index).and_then(|(_, id)| id.clone())
}

pub(super) fn parse_artists(track: &Value) -> Option<String> {
    let artists = track
        .get("ar")
        .and_then(|value| value.as_array())
        .or_else(|| track.get("artists").and_then(|value| value.as_array()))?;

    let names: Vec<String> = artists
        .iter()
        .filter_map(|item| item.get("name").and_then(|value| value.as_str()))
        .map(|name| name.to_string())
        .collect();

    if names.is_empty() {
        None
    } else {
        Some(names.join(ARTIST_SEPARATOR))
    }
}

pub(super) fn format_duration(duration_ms: i64) -> String {
    let total = (duration_ms.max(0) / 1000) as u64;
    let mm = total / 60;
    let ss = total % 60;
    format!("{:02}:{:02}", mm, ss)
}
