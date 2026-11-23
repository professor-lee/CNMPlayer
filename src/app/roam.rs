use super::App;
use super::Page;
use super::api::ApiState;
use super::home::HOME_PRIVATE_ROAM_TILE_ID;
use super::owned_task::SharedTask;
use super::owned_task::peek_shared;
use super::owned_task::spawn_shared;
use super::playback_controller::PlaybackTrack;
use super::playlist::PlaylistFetch;
use super::playlist::PlaylistState;
use super::response::response_code;
use super::text::lang_text;
use super::tracks::PlaylistTrack;
use super::tracks::PlaylistTrackKind;
use super::tracks::format_duration;
use super::tracks::parse_tracks;
use crate::data::config::Language;
use crate::data::persistence::PersistenceKey;
use crate::data::playback_session;
use crate::data::private_roam;
use crate::data::private_roam::PrivateRoamMode;
use crate::status;
use serde_json::Value;

const PRIVATE_ROAM_TRACK_LIMIT: usize = 256;

/// Keep the current song and nearest upcoming songs before older history.
pub(super) fn private_roam_window(
    len: usize,
    current: Option<usize>,
) -> (std::ops::Range<usize>, Option<usize>) {
    let current = current.filter(|&index| index < len);
    let start = current
        .unwrap_or(0)
        .min(len.saturating_sub(PRIVATE_ROAM_TRACK_LIMIT));
    let end = start.saturating_add(PRIVATE_ROAM_TRACK_LIMIT).min(len);
    (start..end, current.map(|index| index - start))
}

/// Preserve the selected occurrence while invalid entries in its window are removed.
pub(super) fn private_roam_restored_index<T>(
    tracks: &[T],
    current: Option<usize>,
    window: &std::ops::Range<usize>,
    is_valid: impl Fn(&T) -> bool,
) -> Option<usize> {
    let current = current?;
    if !window.contains(&current) || !is_valid(tracks.get(current)?) {
        return None;
    }
    Some(
        tracks[window.start..current]
            .iter()
            .filter(|track| is_valid(*track))
            .count(),
    )
}

/// Index of the first song after the current queue track in authoritative history.
///
/// Queue and history windows can have the same length but different offsets, so a
/// length comparison cannot prove that history has a continuation. Prefer the
/// recorded occurrence when it still identifies the current song; otherwise fall
/// back to the first matching song id.
pub(super) fn private_roam_continuation_start(
    tracks: &[PlaylistTrack],
    current: &PlaybackTrack,
    last_played_index: Option<usize>,
) -> Option<usize> {
    let song_id = current.song_id.as_str();
    let current_index = last_played_index
        .filter(|&index| {
            tracks
                .get(index)
                .is_some_and(|track| track.id.as_deref() == Some(song_id))
        })
        .or_else(|| {
            tracks
                .iter()
                .position(|track| track.id.as_deref() == Some(song_id))
        })?;
    (current_index + 1 < tracks.len()).then_some(current_index + 1)
}

pub(super) fn bound_private_roam_queue(
    queue: &mut Vec<PlaybackTrack>,
    current: &mut Option<usize>,
    source_id: Option<&str>,
    page_advance: &mut bool,
) {
    if source_id != Some(HOME_PRIVATE_ROAM_TILE_ID) {
        return;
    }
    let (window, index) = private_roam_window(queue.len(), *current);
    queue.truncate(window.end);
    queue.drain(..window.start);
    *current = index;
    *page_advance = false;
}

fn sync_private_roam_playlist(playlist: &mut PlaylistState, tracks: &[PlaylistTrack]) {
    if playlist.id.as_deref() != Some(HOME_PRIVATE_ROAM_TILE_ID) {
        return;
    }
    let removed_prefix = tracks
        .first()
        .and_then(|first| {
            playlist
                .tracks
                .iter()
                .position(|track| track.id == first.id)
        })
        .unwrap_or(0);
    let remap = |index: usize| {
        playlist
            .tracks
            .get(index)
            .and_then(|old| tracks.iter().position(|track| track.id == old.id))
            .unwrap_or_else(|| {
                index
                    .saturating_sub(removed_prefix)
                    .min(tracks.len().saturating_sub(1))
            })
    };
    let focus = remap(playlist.focused_idx);
    let scroll = remap(playlist.scroll_offset);
    playlist.tracks = tracks.to_vec();
    playlist.focused_idx = focus;
    playlist.scroll_offset = scroll;
    playlist.ensure_focus_visible();
    playlist.total_tracks = Some(tracks.len());
    playlist.pagination = None;
    playlist.generation = super::next_list_generation();
}

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
    /// 推荐模式：每日刷新、追加与列表兜底拉取都按它请求
    pub(super) mode: PrivateRoamMode,
}

impl PrivateRoamState {
    fn from_record(record: private_roam::PrivateRoamRecord) -> Self {
        let (window, _) = private_roam_window(record.tracks.len(), record.last_played_index);
        let last_played_index = private_roam_restored_index(
            &record.tracks,
            record.last_played_index,
            &window,
            |track| !track.song_id.is_empty(),
        );
        let tracks = record
            .tracks
            .into_iter()
            .skip(window.start)
            .take(window.len())
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
            .collect::<Vec<_>>();
        let mut state = Self {
            tracks,
            last_played_index,
            last_played_cover_url: record.last_played_cover_url,
            last_refresh_day: record.last_refresh_day,
            mode: record.mode,
            cover_url: None,
        };
        state.refresh_cover();
        state
    }

    fn bound_history(&mut self) {
        let (window, index) = private_roam_window(self.tracks.len(), self.last_played_index);
        self.tracks.truncate(window.end);
        self.tracks.drain(..window.start);
        self.last_played_index = index;
    }

    fn append_tracks(&mut self, fetched: Vec<PlaylistTrack>) -> Vec<PlaybackTrack> {
        let mut queue_items = Vec::new();
        for track in fetched {
            if !self.tracks.iter().any(|old| old.id == track.id) {
                if let Some(item) = PlaybackTrack::from_playlist_track(&track) {
                    queue_items.push(item);
                }
                self.tracks.push(track);
            }
        }
        self.bound_history();
        queue_items
    }

    fn refresh_cover(&mut self) {
        let last_played = self
            .last_played_index
            .and_then(|index| self.tracks.get(index));
        self.cover_url = match last_played {
            Some(track) => self
                .last_played_cover_url
                .as_ref()
                .or(track.cover_url.as_ref())
                .cloned(),
            None => self
                .tracks
                .first()
                .and_then(|track| track.cover_url.clone()),
        };
    }
}

/// 在途的模式切换：目标模式与按它重新拉取的歌曲。
pub(super) struct RoamModeSwitch {
    mode: PrivateRoamMode,
    task: SharedTask<Vec<PlaylistTrack>>,
}

fn private_roam_mode_label(language: Language, mode: PrivateRoamMode) -> &'static str {
    match mode {
        PrivateRoamMode::Default => lang_text(language, "默认", "Default"),
        PrivateRoamMode::Familiar => lang_text(language, "熟悉", "Familiar"),
        PrivateRoamMode::Explore => lang_text(language, "探索", "Explore"),
    }
}

/// 拉取一批私人漫游歌曲（接口每次固定返回 3 首，连拉几次去重）
pub(super) async fn fetch_private_roam_songs(
    api: &mut ApiState,
    mode: PrivateRoamMode,
) -> Vec<PlaylistTrack> {
    let mut tracks = Vec::new();
    let mut seen = std::collections::HashSet::new();

    for _ in 0..3 {
        let Ok(response) = api.personal_fm_mode(mode.api_value(), 20).await else {
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
                if tracks.len() == PRIVATE_ROAM_TRACK_LIMIT {
                    return tracks;
                }
            }
        }
    }

    tracks
}

fn private_roam_playlist_fetch(
    language: Language,
    mut tracks: Vec<PlaylistTrack>,
    cover_url: Option<String>,
) -> PlaylistFetch {
    tracks.truncate(PRIVATE_ROAM_TRACK_LIMIT);
    let total = tracks.len();
    PlaylistFetch {
        id: HOME_PRIVATE_ROAM_TILE_ID.to_string(),
        title: lang_text(language, "私人漫游", "Private Roam").to_string(),
        artist: lang_text(language, "网易云音乐", "Netease Cloud Music").to_string(),
        description: lang_text(
            language,
            "来自网易云私人漫游，按 Enter 播放",
            "Private roam songs from Netease. Press Enter to play",
        )
        .to_string(),
        cover_url,
        tracks,
        total_tracks: Some(total),
        next_offset: total,
        has_more: false,
        paginated: false,
        liked: None,
        subscribed: None,
    }
}

pub(super) async fn fetch_private_roam_page(
    mut api: ApiState,
    language: Language,
    mode: PrivateRoamMode,
) -> Result<PlaylistFetch, String> {
    let tracks = fetch_private_roam_songs(&mut api, mode).await;
    if tracks.is_empty() {
        return Err(lang_text(language, "私人漫游为空", "Private roam is empty").to_string());
    }
    let cover_url = tracks.first().and_then(|track| track.cover_url.clone());
    Ok(private_roam_playlist_fetch(language, tracks, cover_url))
}

/// 当前 UTC 日期对应的天数（每日刷新标记用）
fn today_day_number() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|value| value.as_secs() as i64 / 86400)
        .unwrap_or_default()
}

/// 启动时是否刷新私人漫游：今天没刷过，且漫游列表为空或恢复的队列不来自漫游。
///
/// 队列来自漫游时刷新只保留最后播放的那首，队列里后续的旧歌在列表里找不到，
/// 播放跟踪、追加与续播随之失效；列表为空时跳过则漫游队列播完就停，必须拉取。
fn private_roam_refresh_due(refreshed_today: bool, roam_empty: bool, queue_is_roam: bool) -> bool {
    !refreshed_today && (roam_empty || !queue_is_roam)
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
        if new_tracks.len() == PRIVATE_ROAM_TRACK_LIMIT {
            break;
        }
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
    pub(super) fn load_cached_private_roam_playlist(&mut self) -> bool {
        self.browse.private_roam.bound_history();
        if self.browse.private_roam.tracks.is_empty() {
            return false;
        }

        let fetch = private_roam_playlist_fetch(
            self.config.language,
            self.browse.private_roam.tracks.clone(),
            self.browse.private_roam.cover_url.clone(),
        );
        let focus_index = self.browse.private_roam.last_played_index;
        if fetch.cover_url.is_none() {
            self.browse.playlist.cover = Default::default();
        }
        // 统一替换列表代、分页与在途 slot，迟到的旧结果不得覆盖缓存页。
        self.apply_playlist_fetch(fetch);
        // 进入漫游后默认聚焦到最后播放的歌曲。
        if let Some(index) = focus_index
            && index < self.browse.playlist.tracks.len()
        {
            self.browse.playlist.set_focus(index);
        }
        true
    }

    /// 仅由当前占位页的成功回包更新权威副本；取消的任务不会进入此处。
    pub(super) fn sync_private_roam_fetch(&mut self, fetch: &PlaylistFetch) {
        if fetch.id != HOME_PRIVATE_ROAM_TILE_ID || fetch.tracks.is_empty() {
            return;
        }
        self.playback.roam_append = None;

        self.browse.private_roam.tracks = fetch.tracks.clone();
        self.browse.private_roam.bound_history();
        self.browse.private_roam.refresh_cover();
        self.sync_home_roam_tile_cover();
        self.persist_private_roam();
    }

    /// 今天是否已刷新过私人漫游（按天判断只有这一处）。
    pub(super) fn private_roam_refreshed_today(&self) -> bool {
        self.browse.private_roam.last_refresh_day == Some(today_day_number())
    }

    /// 启动时是否刷新私人漫游，须在恢复播放记忆之前调用：此时内存队列还是空的，
    /// 队列来源只能读播放记忆存档。跳过时不写 `last_refresh_day`，
    /// 下次队列不来自漫游的启动补刷。
    pub(super) async fn private_roam_startup_refresh_due(&self) -> bool {
        let refreshed_today = self.private_roam_refreshed_today();
        let roam_empty = self.browse.private_roam.tracks.is_empty();
        // 关闭播放记忆时启动不恢复队列；存档读不出时恢复同样失败，都按非漫游处理。
        let queue_is_roam = !refreshed_today
            && !roam_empty
            && self.config.playback_memory
            && matches!(
                compio::runtime::spawn_blocking(playback_session::load).await,
                Ok(Ok(Some(record)))
                    if record.source_playlist_id.as_deref() == Some(HOME_PRIVATE_ROAM_TILE_ID)
            );
        private_roam_refresh_due(refreshed_today, roam_empty, queue_is_roam)
    }

    /// 应用一批新拉取的漫游歌曲（启动初始化与每日刷新共用）。
    pub(super) fn apply_private_roam_refresh(&mut self, fetched: Vec<PlaylistTrack>) {
        let today = today_day_number();
        if fetched.is_empty() {
            // 拉取失败保留旧列表，下次启动再试
            return;
        }
        self.playback.roam_append = None;

        let (new_tracks, new_index) = merge_private_roam_refresh(
            &self.browse.private_roam.tracks,
            self.browse.private_roam.last_played_index,
            fetched,
        );

        self.browse.private_roam.tracks = new_tracks;
        self.browse.private_roam.last_played_index = new_index;
        self.browse.private_roam.refresh_cover();
        self.sync_home_roam_tile_cover();
        self.browse.private_roam.last_refresh_day = Some(today);
        self.sync_private_roam_browse_mirrors();
        self.persist_private_roam();
    }

    /// 播放到列表末尾时追加一批新歌
    fn append_private_roam_songs(&mut self) {
        if self.playback.roam_append.is_some() {
            return;
        }
        let mut api = self.api.clone();
        let mode = self.browse.private_roam.mode;
        self.playback.roam_append = Some(super::owned_task::spawn_shared(
            Box::pin(async move { Some(fetch_private_roam_songs(&mut api, mode).await) }),
            self.api.wake_signal(),
        ));
    }

    pub(super) fn tick_playback_roam_append(&mut self) {
        let Some(fetched) =
            super::owned_task::peek_shared_future(&self.playback.roam_append).cloned()
        else {
            return;
        };
        self.playback.roam_append = None;
        if fetched.is_empty() {
            return;
        }

        let new_queue_items = self.browse.private_roam.append_tracks(fetched);

        // Queue ownership, not the visible page, decides whether playback extends.
        if self.playback_queue_is_roam() {
            self.playback.playback_queue.extend(new_queue_items);
            bound_private_roam_queue(
                &mut self.playback.playback_queue,
                &mut self.playback.playback_index,
                self.playback.playback_queue_source_id.as_deref(),
                &mut self.playback.page_advance,
            );
        }
        self.sync_private_roam_browse_mirrors();
        self.persist_private_roam();
    }

    fn sync_private_roam_browse_mirrors(&mut self) {
        let tracks = &self.browse.private_roam.tracks;
        sync_private_roam_playlist(&mut self.browse.playlist, tracks);
        if let Some(snapshot) = self.playlist_section_return_snapshot.as_mut() {
            sync_private_roam_playlist(snapshot, tracks);
        }
    }

    /// 播放队列切到某首歌时，记录漫游播放位置与封面；播放到最后一首时追加新歌
    pub(super) fn track_private_roam_playback(&mut self, track: &PlaybackTrack) {
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
            self.append_private_roam_songs();
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
        let (window, last_played_index) = private_roam_window(
            self.browse.private_roam.tracks.len(),
            self.browse.private_roam.last_played_index,
        );
        let record = private_roam::PrivateRoamRecord {
            tracks: self.browse.private_roam.tracks[window]
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
            last_played_index,
            last_played_cover_url: self.browse.private_roam.last_played_cover_url.clone(),
            last_refresh_day: self.browse.private_roam.last_refresh_day,
            mode: self.browse.private_roam.mode,
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

        self.browse.private_roam = PrivateRoamState::from_record(record);
        self.persist_private_roam();
    }

    pub(super) fn sync_home_roam_tile_cover(&mut self) {
        let url = self.browse.private_roam.cover_url.as_deref();
        if let Some(tile) = self.browse.home.tiles.iter_mut().find(|tile| {
            tile.id.as_deref() == Some(HOME_PRIVATE_ROAM_TILE_ID)
                && tile.cover.url.as_deref() != url
        }) {
            match url {
                Some(url) => tile.cover.load(self.api.clone(), url.to_string()),
                None => tile.cover = Default::default(),
            }
        }
    }

    /// 当前页是否为已载入的私人漫游页（占位页没有 id，不算）。
    pub(super) fn private_roam_page_active(&self) -> bool {
        self.page == Page::Playlist
            && self.browse.playlist.id.as_deref() == Some(HOME_PRIVATE_ROAM_TILE_ID)
    }

    /// 界面显示的模式：切换在途时显示目标模式。
    fn private_roam_display_mode(&self) -> PrivateRoamMode {
        self.browse
            .private_roam_mode_switch
            .as_ref()
            .map_or(self.browse.private_roam.mode, |switch| switch.mode)
    }

    /// 歌单页标题后缀：浏览私人漫游时为当前模式名，其余歌单为 `None`。
    pub(crate) fn private_roam_title_mode(&self) -> Option<&'static str> {
        (self.browse.playlist.id.as_deref() == Some(HOME_PRIVATE_ROAM_TILE_ID)).then(|| {
            private_roam_mode_label(self.config.language, self.private_roam_display_mode())
        })
    }

    /// 私人漫游页按键：轮换到下一模式并按新模式重新拉取列表。
    /// 连按时以最后一次为准，覆盖在途拉取；输入路径不 `await` 网络。
    pub(super) fn cycle_private_roam_mode_hotkey(&mut self) {
        if !self.private_roam_page_active() {
            return;
        }
        let mode = self.private_roam_display_mode().next();
        let mut api = self.api.clone();
        let task = spawn_shared(
            Box::pin(async move { Some(fetch_private_roam_songs(&mut api, mode).await) }),
            self.api.wake_signal(),
        );
        self.browse.private_roam_mode_switch = Some(RoamModeSwitch { mode, task });
        status::info(format!(
            "{}{}",
            self.lang_text("正在切换私人漫游模式：", "Switching private roam mode: "),
            private_roam_mode_label(self.config.language, mode)
        ));
    }

    pub(super) fn tick_private_roam_mode_switch(&mut self) {
        let Some(switch) = self.browse.private_roam_mode_switch.as_ref() else {
            return;
        };
        let Some(fetched) = peek_shared(&switch.task).cloned() else {
            return;
        };
        let mode = switch.mode;
        self.browse.private_roam_mode_switch = None;
        if fetched.is_empty() {
            // 拉取失败保留旧模式与旧列表，标题随之回到旧模式。
            status::error(format!(
                "{}{}",
                self.lang_text(
                    "私人漫游模式切换失败，仍为：",
                    "Private roam mode switch failed; still: "
                ),
                private_roam_mode_label(self.config.language, self.browse.private_roam.mode)
            ));
            return;
        }
        self.apply_private_roam_mode(mode, fetched);
    }

    /// 切换模式后整表换成新模式的歌曲。在播漫游时当前歌曲留在首位、队列其后换成新歌：
    /// 播放跟踪、追加与续播都按歌曲 id 在列表里定位当前歌曲，旧歌留在队列里会失联。
    fn apply_private_roam_mode(&mut self, mode: PrivateRoamMode, fetched: Vec<PlaylistTrack>) {
        // 旧模式的追加结果不得混进新列表。
        self.playback.roam_append = None;
        let queue_is_roam = self.playback_queue_is_roam();
        let playing = queue_is_roam
            .then(|| self.current_song_id())
            .flatten()
            .and_then(|id| {
                self.browse
                    .private_roam
                    .tracks
                    .iter()
                    .position(|track| track.id.as_deref() == Some(id.as_str()))
            });
        let (tracks, last_played_index) =
            merge_private_roam_refresh(&self.browse.private_roam.tracks, playing, fetched);
        let roam = &mut self.browse.private_roam;
        roam.mode = mode;
        roam.tracks = tracks;
        roam.last_played_index = last_played_index;
        roam.refresh_cover();

        if queue_is_roam
            && let Some(current) = self
                .playback
                .playback_index
                .filter(|&index| index < self.playback.playback_queue.len())
        {
            let skip = usize::from(last_played_index.is_some());
            let upcoming = self.browse.private_roam.tracks[skip..]
                .iter()
                .filter_map(PlaybackTrack::from_playlist_track);
            self.playback.playback_queue.truncate(current + 1);
            self.playback.playback_queue.extend(upcoming);
            bound_private_roam_queue(
                &mut self.playback.playback_queue,
                &mut self.playback.playback_index,
                self.playback.playback_queue_source_id.as_deref(),
                &mut self.playback.page_advance,
            );
            self.persist_playback_memory();
        }

        self.sync_home_roam_tile_cover();
        self.sync_private_roam_browse_mirrors();
        if self.browse.playlist.id.as_deref() == Some(HOME_PRIVATE_ROAM_TILE_ID) {
            self.browse
                .playlist
                .set_focus(last_played_index.unwrap_or(0));
            let cover = self.browse.private_roam.cover_url.clone();
            if self.browse.playlist.cover.url != cover {
                match cover {
                    Some(url) => self.browse.playlist.cover.load(self.api.clone(), url),
                    None => self.browse.playlist.cover = Default::default(),
                }
            }
        }
        self.persist_private_roam();
        status::info(format!(
            "{}{}",
            self.lang_text("私人漫游：", "Private roam: "),
            private_roam_mode_label(self.config.language, mode)
        ));
    }
}

#[cfg(test)]
mod tests;
