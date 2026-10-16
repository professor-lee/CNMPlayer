use super::App;
use super::api::ApiState;
use super::home::HOME_PRIVATE_ROAM_TILE_ID;
use super::playback_controller::PlaybackTrack;
use super::response::response_code;
use super::tracks::PlaylistTrack;
use super::tracks::PlaylistTrackKind;
use super::tracks::format_duration;
use super::tracks::parse_tracks;
use crate::data::persistence::PersistenceKey;
use crate::data::private_roam;
use anyhow::Result;
use anyhow::anyhow;
use serde_json::Value;

/// 私人漫游状态：歌曲列表、最后播放位置与展示封面
#[derive(Debug, Clone, Default)]
pub(super) struct PrivateRoamState {
    /// 漫游歌曲列表（权威副本，进入漫游时填充到 playlist）
    pub(super) tracks: Vec<PlaylistTrack>,
    /// 最后播放歌曲在列表中的索引（每日刷新后若保留在首位则为 0）
    pub(super) last_played_index: Option<usize>,
    /// 最后播放的漫游歌曲封面（切到别的列表播放后仍保留）
    pub(super) last_played_cover_url: Option<String>,
    /// 当前应展示的封面：未播放过时为首歌封面，播放后为播放中的歌曲封面
    pub(super) cover_url: Option<String>,
    /// 每日刷新标记：上次刷新的 UTC 天数
    pub(super) last_refresh_day: Option<i64>,
}

/// 拉取一批私人漫游歌曲（接口每次固定返回 3 首，连拉几次去重）
pub(super) async fn fetch_private_roam_songs(api: &mut ApiState) -> Vec<PlaylistTrack> {
    let mut tracks = Vec::new();
    let mut seen = std::collections::HashSet::new();

    for _ in 0..3 {
        let Ok(response) = api.personal_fm_mode("DEFAULT", 20).await else {
            continue;
        };
        if response_code(&response) != 200 {
            continue;
        }
        let Some(songs) = response.body.get("data").and_then(|value| value.as_array()) else {
            continue;
        };
        let normalized = normalize_fm_song_items(songs);
        for track in parse_tracks(&normalized) {
            if let Some(id) = &track.id
                && seen.insert(id.clone())
            {
                tracks.push(track);
            }
        }
    }

    tracks
}

/// 当前 UTC 日期对应的天数（每日刷新标记用）
fn today_day_number() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|value| value.as_secs() as i64 / 86400)
        .unwrap_or_default()
}

/// 每日刷新合并规则：上次最后播放的歌曲保留在首位，其后追加新歌（按 id 去重）
pub(super) fn merge_private_roam_refresh(
    old_tracks: &[PlaylistTrack],
    last_played_index: Option<usize>,
    fetched: Vec<PlaylistTrack>,
) -> (Vec<PlaylistTrack>, Option<usize>) {
    let mut new_tracks = Vec::new();
    let mut kept_last_played = false;

    if let Some(index) = last_played_index
        && let Some(last) = old_tracks.get(index)
    {
        new_tracks.push(last.clone());
        kept_last_played = true;
    }
    for track in fetched {
        if !new_tracks.iter().any(|t| t.id == track.id) {
            new_tracks.push(track);
        }
    }

    let new_index = if kept_last_played { Some(0) } else { None };
    (new_tracks, new_index)
}

/// 私人 FM/漫游接口的歌曲对象用 `album`/`artists` 字段，规整为歌单通用的 `al`/`ar`
pub(super) fn normalize_fm_song_items(items: &[Value]) -> Vec<Value> {
    items
        .iter()
        .map(|item| {
            let mut song = item.clone();
            if song.get("al").is_none()
                && let Some(album) = song.get("album").cloned()
            {
                song["al"] = album;
            }
            if song.get("ar").is_none()
                && let Some(artists) = song.get("artists").cloned()
            {
                song["ar"] = artists;
            }
            song
        })
        .collect()
}

impl App {
    pub(super) async fn load_private_roam_playlist(&mut self) -> Result<()> {
        // 兜底：内存列表为空（首次使用且启动刷新失败过）时现场拉取
        if self.browse.private_roam.tracks.is_empty() {
            let fetched = fetch_private_roam_songs(&mut self.api).await;
            if fetched.is_empty() {
                return Err(anyhow!(
                    self.lang_text("私人漫游为空", "Private roam is empty")
                ));
            }
            self.browse.private_roam.tracks = fetched;
            if self.browse.private_roam.cover_url.is_none() {
                self.browse.private_roam.cover_url = self
                    .browse
                    .private_roam
                    .tracks
                    .first()
                    .and_then(|track| track.cover_url.clone());
            }
            self.persist_private_roam();
        }

        let tracks = self.browse.private_roam.tracks.clone();
        let cover_url = self.browse.private_roam.cover_url.clone();
        let focus_index = self.browse.private_roam.last_played_index;

        // 这一页换成私人漫游：在途的占位拉取作废（同 `apply_playlist_fetch`）。
        self.browse.playlist_fetch = None;
        self.browse.playlist.id = Some(HOME_PRIVATE_ROAM_TILE_ID.to_string());
        self.browse.playlist.title = self.lang_text("私人漫游", "Private Roam").to_string();
        self.browse.playlist.artist = self
            .lang_text("网易云音乐", "Netease Cloud Music")
            .to_string();
        self.browse.playlist.description = self
            .lang_text(
                "来自网易云私人漫游，按 Enter 播放",
                "Private roam songs from Netease. Press Enter to play",
            )
            .to_string();
        self.browse.playlist.set_tracks(tracks);
        // 进入漫游后默认聚焦到最后播放的歌曲
        if let Some(index) = focus_index
            && index < self.browse.playlist.tracks.len()
        {
            self.browse.playlist.focused_idx = index;
            self.browse.playlist.scroll_offset = index;
        }
        if let Some(x) = cover_url {
            self.browse.playlist.cover.load(self.api.clone(), x)
        }
        Ok(())
    }

    /// 今天是否已刷新过私人漫游（刷新判据只有这一处）。
    pub(super) fn private_roam_refreshed_today(&self) -> bool {
        self.browse.private_roam.last_refresh_day == Some(today_day_number())
    }

    /// 应用一批新拉取的漫游歌曲（启动初始化与每日刷新共用）。
    pub(super) fn apply_private_roam_refresh(&mut self, fetched: Vec<PlaylistTrack>) {
        let today = today_day_number();
        if fetched.is_empty() {
            // 拉取失败保留旧列表，下次启动再试
            return;
        }

        let (new_tracks, new_index) = merge_private_roam_refresh(
            &self.browse.private_roam.tracks,
            self.browse.private_roam.last_played_index,
            fetched,
        );

        self.browse.private_roam.tracks = new_tracks;
        self.browse.private_roam.last_played_index = new_index;
        if self.browse.private_roam.cover_url.is_none() {
            self.browse.private_roam.cover_url = self
                .browse
                .private_roam
                .tracks
                .first()
                .and_then(|track| track.cover_url.clone());
        }
        self.browse.private_roam.last_refresh_day = Some(today);
        self.persist_private_roam();
    }

    /// 播放到列表末尾时追加一批新歌
    async fn append_private_roam_songs(&mut self) {
        let fetched = fetch_private_roam_songs(&mut self.api).await;
        if fetched.is_empty() {
            return;
        }

        let mut added = false;
        let mut new_queue_items = Vec::new();
        for track in fetched {
            if !self
                .browse
                .private_roam
                .tracks
                .iter()
                .any(|t| t.id == track.id)
            {
                if let Some(item) = PlaybackTrack::from_playlist_track(&track) {
                    new_queue_items.push(item);
                }
                self.browse.private_roam.tracks.push(track);
                added = true;
            }
        }
        if !added {
            return;
        }

        self.persist_private_roam();

        // 扩展播放队列是播放行为，只看队列来源，不看当前在哪个页面：
        // 否则重启后（页面停在主页）新歌只进列表不进队列，追加等于白做。
        if self.playback_queue_is_roam() {
            self.playback.playback_queue.extend(new_queue_items);
        }

        // 列表页 UI 同步则确实只在该页打开时才需要。
        if self.browse.playlist.id.as_deref() == Some(HOME_PRIVATE_ROAM_TILE_ID) {
            self.browse.playlist.tracks = self.browse.private_roam.tracks.clone();
        }
    }

    /// 播放队列切到某首歌时，记录漫游播放位置与封面；播放到最后一首时追加新歌
    pub(super) async fn track_private_roam_playback(&mut self, track: &PlaybackTrack) {
        let Some(pos) = self
            .browse
            .private_roam
            .tracks
            .iter()
            .position(|t| t.id.as_deref() == Some(track.song_id.as_str()))
        else {
            return;
        };

        let is_last = pos + 1 == self.browse.private_roam.tracks.len();
        self.browse.private_roam.last_played_index = Some(pos);
        if let Some(cover) = self.browse.private_roam.tracks[pos].cover_url.clone() {
            self.browse.private_roam.cover_url = Some(cover.clone());
            self.browse.private_roam.last_played_cover_url = Some(cover.clone());
            // 列表页封面同步为播放到的歌曲封面
            if self.browse.playlist.id.as_deref() == Some(HOME_PRIVATE_ROAM_TILE_ID) {
                self.browse
                    .playlist
                    .cover
                    .load(self.api.clone(), cover.clone());
            }
            // 漫游封面随当前歌曲变化；URL 变化时旧字节必须失效，
            // 否则调度器会因已有缓存而永久跳过新封面下载。
            if self.playback_queue_is_roam()
                && self.playback.playback_queue_cover_url.as_deref() != Some(cover.as_str())
            {
                self.playback.playback_queue_cover = None;
            }
            if self.playback_queue_is_roam() {
                self.playback.playback_queue_cover_url = Some(cover);
            }
        }
        self.sync_home_roam_tile_cover();
        self.persist_private_roam();

        if is_last && self.playback_queue_is_roam() {
            self.append_private_roam_songs().await;
        }
    }

    /// 当前播放队列是否来自私人漫游。
    ///
    /// 判据是随播放记忆持久化的来源 id，而非 `self.playlist.id`——后者是
    /// 当前浏览页面，重启后为 None，会让漫游退化成普通歌单。
    pub(super) fn playback_queue_is_roam(&self) -> bool {
        self.playback.playback_queue_source_id.as_deref() == Some(HOME_PRIVATE_ROAM_TILE_ID)
    }

    fn persist_private_roam(&self) {
        let record = private_roam::PrivateRoamRecord {
            tracks: self
                .browse
                .private_roam
                .tracks
                .iter()
                .map(|track| private_roam::PrivateRoamTrack {
                    song_id: track.id.clone().unwrap_or_default(),
                    title: track.title.clone(),
                    artist: track.artist.clone(),
                    album: track.album.clone(),
                    duration_ms: track.duration_ms,
                    cover_url: track.cover_url.clone(),
                })
                .collect(),
            last_played_index: self.browse.private_roam.last_played_index,
            last_played_cover_url: self.browse.private_roam.last_played_cover_url.clone(),
            last_refresh_day: self.browse.private_roam.last_refresh_day,
            updated_at: 0,
        };
        let _ = self
            .persistence
            .enqueue_latest(PersistenceKey::PrivateRoam, move || {
                private_roam::save(&record)
            });
    }

    pub(super) async fn load_private_roam_memory(&mut self) {
        let result = compio::runtime::spawn_blocking(private_roam::load).await;
        let record = match result {
            Ok(Ok(Some(record))) => record,
            Ok(Ok(None)) => return,
            other => {
                log::error!("private roam memory could not be read: {other:?}");
                return;
            }
        };

        self.browse.private_roam.tracks = record
            .tracks
            .into_iter()
            .filter_map(|track| {
                if track.song_id.is_empty() {
                    return None;
                }
                Some(PlaylistTrack {
                    kind: PlaylistTrackKind::Song,
                    id: Some(track.song_id),
                    title: track.title,
                    artist: track.artist,
                    album: track.album,
                    cover_url: track.cover_url,
                    duration_ms: track.duration_ms,
                    duration: format_duration(track.duration_ms),
                })
            })
            .collect();
        self.browse.private_roam.last_played_index = record.last_played_index;
        self.browse.private_roam.last_played_cover_url = record.last_played_cover_url.clone();
        self.browse.private_roam.last_refresh_day = record.last_refresh_day;
        self.browse.private_roam.cover_url = record.last_played_cover_url.or_else(|| {
            self.browse
                .private_roam
                .tracks
                .first()
                .and_then(|track| track.cover_url.clone())
        });
    }

    pub(super) fn sync_home_roam_tile_cover(&mut self) {
        let Some(url) = self.browse.private_roam.cover_url.clone() else {
            return;
        };
        if let Some(tile) = self.browse.home.tiles.iter_mut().find(|tile| {
            tile.title == "私人漫游" && tile.cover.url.as_deref() != Some(url.as_str())
        }) {
            tile.cover.load(self.api.clone(), url);
        }
    }
}
