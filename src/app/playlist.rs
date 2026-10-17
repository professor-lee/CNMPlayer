use super::App;
use super::Page;
use super::api::ApiState;
use super::cover_state::CoverFetchState;
use super::home::HOME_DAILY_RECOMMEND_TILE_ID;
use super::home::home_daily_song_items;
use super::likes::LikedRefresh;
use super::likes::fetch_liked_refresh;
use super::next_list_generation;
use super::owned_task::SharedTask;
use super::owned_task::peek_shared;
use super::owned_task::spawn_shared;
use super::playback_controller::PlaybackRuntimeState;
use super::playback_controller::PlaybackTrack;
use super::playlist_pagination;
use super::playlist_pagination::PlaylistPagination;
use super::response::first_non_empty;
use super::response::parse_usize_value;
use super::response::response_code;
use super::response::response_message;
use super::text::lang_text;
use super::tracks::PlaylistTrack;
use super::tracks::PlaylistTrackKind;
use super::tracks::fetch_song_page_refs;
use super::tracks::parse_tracks;
use crate::data::config::Language;
use anyhow::Result;
use crossterm::event::KeyCode;
use crossterm::event::KeyEvent;
use serde_json::Value;
use std::pin::Pin;

const PLAYLIST_PAGE_SIZE: usize = 100;

pub(super) type PlaylistFetchFuture = SharedTask<Result<PlaylistFetch, String>>;

pub(super) type PlaylistPageFetchFuture = SharedTask<Result<PlaylistPageFetch, String>>;

/// 装箱后的任务体（`spawn_shared` 的入参类型）。
pub(super) type PlaylistFetchTask =
    Pin<Box<dyn Future<Output = Option<Result<PlaylistFetch, String>>>>>;

pub(super) type PlaylistPageTask =
    Pin<Box<dyn Future<Output = Option<Result<PlaylistPageFetch, String>>>>>;

#[derive(Clone)]
pub(super) struct PlaylistPageFetch {
    pub(super) source_id: String,
    pub(super) offset: usize,
    pub(super) tracks: Vec<PlaylistTrack>,
    pub(super) next_offset: usize,
    pub(super) has_more: bool,
}

/// 歌单页 / 专辑页的在途拉取：句柄旁边记下是哪种页面（成功后文案不同）。
pub(super) struct PlaylistFetchSlot {
    pub(super) kind: PlaylistPageKind,
    pub(super) future: PlaylistFetchFuture,
}

/// 打开的是歌单还是专辑：端点与文案不同，落状态是同一套。
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum PlaylistPageKind {
    Playlist,
    Album,
}

impl PlaylistPageKind {
    fn opened(self, language: Language) -> &'static str {
        match (self, language) {
            (Self::Playlist, Language::Zh) => "已打开歌单",
            (Self::Playlist, Language::En) => "Opened playlist",
            (Self::Album, Language::Zh) => "已打开专辑",
            (Self::Album, Language::En) => "Opened album",
        }
    }

    pub(super) fn failed(self, language: Language) -> &'static str {
        match (self, language) {
            (Self::Playlist, Language::Zh) => "打开歌单失败",
            (Self::Playlist, Language::En) => "Failed to open playlist",
            (Self::Album, Language::Zh) => "打开专辑失败",
            (Self::Album, Language::En) => "Failed to open album",
        }
    }
}

/// 歌单页 / 专辑页的一次拉取结果：`PlaylistState` 里除封面句柄与视口字段外的全部字段。
///
/// 结果要经 `spawn_shared` 搬运，故实现 `Clone`（封面句柄共享输出与取消所有权）。
#[derive(Clone)]
pub(super) struct PlaylistFetch {
    pub(super) id: String,
    pub(super) title: String,
    pub(super) artist: String,
    pub(super) description: String,
    pub(super) cover_url: Option<String>,
    pub(super) tracks: Vec<PlaylistTrack>,
    pub(super) total_tracks: Option<usize>,
    pub(super) next_offset: usize,
    pub(super) has_more: bool,
    pub(super) paginated: bool,
    pub(super) liked: Option<LikedRefresh>,
}

/// 打开的歌单是不是「我喜欢的音乐」：ID 对得上，或标题命中（中英文两种叫法）。
pub(super) fn is_liked_playlist(
    liked_playlist_id: Option<&str>,
    playlist_id: &str,
    title: Option<&str>,
) -> bool {
    if liked_playlist_id == Some(playlist_id) {
        return true;
    }

    let title = title.unwrap_or_default().trim();
    !title.is_empty()
        && (title.contains("我喜欢的音乐") || title.to_ascii_lowercase().contains("liked songs"))
}

/// 后台获取歌单指定分页的歌曲详情，不持有 `&mut App`。
///
/// 使用 `trackIds` 的分页接口，避免 `playlist.detail` 返回的 `tracks` 截断。
pub(super) async fn fetch_playlist_tracks_page(
    mut api: ApiState,
    playlist_id: String,
    offset: usize,
    total_tracks: Option<usize>,
) -> Result<PlaylistPageFetch, String> {
    let response = api
        .playlist_track_all(&playlist_id, PLAYLIST_PAGE_SIZE, offset)
        .await
        .map_err(|err| err.to_string())?;
    let code = response_code(&response);
    if code != 200 {
        return Err(format!("请求失败({code}): {}", response_message(&response)));
    }

    let tracks = response
        .body
        .get("songs")
        .or_else(|| response.body.pointer("/data/songs"))
        .or_else(|| response.body.pointer("/playlist/tracks"))
        .and_then(Value::as_array)
        .map(|items| parse_tracks(items))
        .unwrap_or_default();
    if tracks.is_empty() && total_tracks.is_some_and(|total| offset < total) {
        return Err("歌单分页返回空数据".to_string());
    }

    let track_count = tracks.len();
    let next_offset = total_tracks
        .map(|total| offset.saturating_add(PLAYLIST_PAGE_SIZE).min(total))
        .unwrap_or_else(|| offset.saturating_add(track_count));
    Ok(PlaylistPageFetch {
        source_id: playlist_id,
        offset,
        tracks,
        next_offset,
        has_more: total_tracks
            .map(|total| next_offset < total)
            .unwrap_or(track_count >= PLAYLIST_PAGE_SIZE),
    })
}

/// 拉一次歌单页数据（不借 `&mut App`，可交给 `spawn_shared` 后台跑）。
pub(super) async fn fetch_playlist_page(
    mut api: ApiState,
    language: Language,
    playlist_id: String,
    fallback_cover_url: Option<String>,
    liked_playlist_id: Option<String>,
    uid_hint: Option<String>,
) -> Result<PlaylistFetch, String> {
    let response = api
        .playlist_detail(&playlist_id)
        .await
        .map_err(|err| err.to_string())?;
    let code = response_code(&response);
    if code != 200 {
        return Err(format!(
            "请求失败({}): {}",
            code,
            response_message(&response)
        ));
    }

    let playlist = response
        .body
        .get("playlist")
        .ok_or_else(|| "歌单数据缺失".to_string())?;

    let title = playlist
        .get("name")
        .and_then(|value| value.as_str())
        .unwrap_or("未命名歌单")
        .to_string();

    let liked = if is_liked_playlist(liked_playlist_id.as_deref(), &playlist_id, Some(&title)) {
        fetch_liked_refresh(&mut api, language, uid_hint).await
    } else {
        None
    };

    let artist = playlist
        .pointer("/creator/nickname")
        .and_then(|value| value.as_str())
        .unwrap_or("网易云音乐")
        .to_string();

    let description = first_non_empty(
        playlist,
        &["/description", "/copywriter", "/creator/signature"],
    )
    .unwrap_or_else(|| "暂无简介".to_string());

    let cover_url = first_non_empty(playlist, &["/coverImgUrl", "/picUrl"]).or(fallback_cover_url);

    let total_tracks = parse_usize_value(playlist.get("trackCount"));
    let first_page =
        fetch_playlist_tracks_page(api.clone(), playlist_id.clone(), 0, total_tracks).await?;

    Ok(PlaylistFetch {
        id: playlist_id,
        title,
        artist,
        description,
        cover_url,
        tracks: first_page.tracks,
        total_tracks,
        next_offset: first_page.next_offset,
        has_more: first_page.has_more,
        paginated: true,
        liked,
    })
}

/// 拉取每日推荐歌曲，与普通歌单共用占位页和结果搬运。
pub(super) async fn fetch_daily_recommend_page(
    mut api: ApiState,
    language: Language,
) -> Result<PlaylistFetch, String> {
    let response = api.recommend_songs().await.map_err(|err| err.to_string())?;
    let code = response_code(&response);
    if code != 200 {
        return Err(format!(
            "请求失败({}): {}",
            code,
            response_message(&response)
        ));
    }

    let songs = home_daily_song_items(&response.body).ok_or_else(|| {
        lang_text(
            language,
            "每日推荐数据缺失",
            "Daily recommendations are missing",
        )
        .to_string()
    })?;
    let tracks = parse_tracks(songs);
    if tracks.is_empty() {
        return Err(
            lang_text(language, "每日推荐为空", "Daily recommendations are empty").to_string(),
        );
    }
    let cover_url = tracks.iter().find_map(|track| track.cover_url.clone());

    Ok(PlaylistFetch {
        id: HOME_DAILY_RECOMMEND_TILE_ID.to_string(),
        title: lang_text(language, "每日推荐", "Daily Recommendations").to_string(),
        artist: lang_text(language, "网易云音乐", "Netease Cloud Music").to_string(),
        description: lang_text(
            language,
            "来自网易云每日推荐歌曲，按 Enter 播放",
            "Daily songs from Netease. Press Enter to play",
        )
        .to_string(),
        cover_url,
        tracks,
        total_tracks: None,
        next_offset: 0,
        has_more: false,
        paginated: false,
        liked: None,
    })
}

/// 拉一次专辑页数据（歌单页样式复用同一套落状态）。
///
/// 专辑接口不给每首歌的封面，缺封面的曲目用专辑封面兜底。
pub(super) async fn fetch_album_page(
    mut api: ApiState,
    language: Language,
    album_id: String,
    fallback_cover_url: Option<String>,
) -> Result<PlaylistFetch, String> {
    let response = api.album(&album_id).await.map_err(|err| err.to_string())?;
    let code = response_code(&response);
    if code != 200 {
        return Err(format!(
            "请求失败({}): {}",
            code,
            response_message(&response)
        ));
    }

    let album = response
        .body
        .get("album")
        .or_else(|| response.body.pointer("/data/album"))
        .ok_or_else(|| "专辑数据缺失".to_string())?;

    let title = album
        .get("name")
        .and_then(|value| value.as_str())
        .unwrap_or("未命名专辑")
        .to_string();

    let artist = first_non_empty(album, &["/artist/name", "/artists/0/name"])
        .unwrap_or_else(|| "网易云音乐".to_string());

    let description = first_non_empty(album, &["/description", "/company", "/type", "/subType"])
        .unwrap_or_else(|| lang_text(language, "暂无简介", "No description").to_string());

    let cover_url = first_non_empty(album, &["/picUrl", "/blurPicUrl"]).or(fallback_cover_url);

    let mut tracks = response
        .body
        .get("songs")
        .or_else(|| response.body.pointer("/data/songs"))
        .and_then(|value| value.as_array())
        .map(|items| parse_tracks(items))
        .unwrap_or_default();

    if let Some(album_cover_url) = cover_url.as_ref() {
        for track in &mut tracks {
            let missing_song_cover = track
                .cover_url
                .as_deref()
                .map(|value| value.trim().is_empty())
                .unwrap_or(true);
            if missing_song_cover {
                track.cover_url = Some(album_cover_url.clone());
            }
        }
    }

    Ok(PlaylistFetch {
        id: album_id,
        title,
        artist,
        description,
        cover_url,
        total_tracks: Some(tracks.len()),
        next_offset: tracks.len(),
        has_more: false,
        paginated: false,
        tracks,
        liked: None,
    })
}

/// 全屏页点专辑名要拉的东西：先 `song/detail` 解析出 `al.id`，再拉专辑页数据。
///
/// 与作者页同理：整段不借 `&mut App`，交给 `spawn_shared` 后台跑；
/// 全屏页只有显示名，本机音频 / 无播放时解析不出来，错误写进占位页与状态行。
pub(super) async fn fetch_album_page_from_song(
    api: ApiState,
    language: Language,
    song_id: String,
) -> Result<PlaylistFetch, String> {
    let refs = fetch_song_page_refs(api.clone(), &song_id).await;
    let album_id = refs.album_id.ok_or_else(|| {
        lang_text(
            language,
            "无法解析当前歌曲的专辑",
            "Failed to resolve the album of the current song",
        )
        .to_string()
    })?;

    // 全屏页只有显示名，没有搜索结果那行的封面可以兜底。
    fetch_album_page(api, language, album_id, None).await
}

#[derive(Clone)]
pub(crate) struct PlaylistState {
    pub(crate) id: Option<String>,
    pub(crate) title: String,
    pub(crate) artist: String,
    pub(crate) description: String,
    pub(crate) cover: CoverFetchState,
    pub(crate) focused_idx: usize,
    pub(super) scroll_offset: usize,
    pub(super) visible_rows: usize,
    pub(crate) tracks: Vec<PlaylistTrack>,
    pub(crate) total_tracks: Option<usize>,
    pub(super) pagination: Option<PlaylistPagination>,
    /// 列表代：内容被整体替换时换号（行内图标缓存据此决定是否重建行数据）。
    pub(crate) generation: u64,
}

impl Default for PlaylistState {
    fn default() -> Self {
        Self {
            id: None,
            title: "歌单详情".to_string(),
            artist: "网易云音乐".to_string(),
            description: "从主页进入歌单后加载真实数据。".to_string(),
            cover: CoverFetchState::default(),
            focused_idx: 0,
            scroll_offset: 0,
            visible_rows: 1,
            tracks: Vec::new(),
            total_tracks: None,
            pagination: None,
            generation: next_list_generation(),
        }
    }
}

impl PlaylistState {
    fn max_scroll_offset(&self) -> usize {
        self.tracks.len().saturating_sub(self.visible_rows.max(1))
    }

    fn clamp_scroll_offset(&mut self) {
        self.scroll_offset = self.scroll_offset.min(self.max_scroll_offset());
    }

    pub(super) fn ensure_focus_visible(&mut self) {
        if self.tracks.is_empty() {
            self.focused_idx = 0;
            self.scroll_offset = 0;
            return;
        }

        self.focused_idx = self.focused_idx.min(self.tracks.len() - 1);
        if self.focused_idx < self.scroll_offset {
            self.scroll_offset = self.focused_idx;
        } else {
            let bottom = self
                .scroll_offset
                .saturating_add(self.visible_rows.max(1).saturating_sub(1));
            if self.focused_idx > bottom {
                self.scroll_offset = self
                    .focused_idx
                    .saturating_add(1)
                    .saturating_sub(self.visible_rows.max(1));
            }
        }

        self.clamp_scroll_offset();
    }

    pub(crate) fn set_visible_rows(&mut self, visible_rows: usize) {
        self.visible_rows = visible_rows.max(1);
        self.ensure_focus_visible();
    }

    pub(crate) fn effective_scroll_offset(&self) -> usize {
        self.scroll_offset.min(self.max_scroll_offset())
    }

    pub(super) fn set_focus(&mut self, index: usize) {
        if self.tracks.is_empty() {
            self.focused_idx = 0;
            self.scroll_offset = 0;
            return;
        }

        self.focused_idx = index.min(self.tracks.len() - 1);
        self.ensure_focus_visible();
    }

    pub(super) fn set_tracks(&mut self, tracks: Vec<PlaylistTrack>) {
        self.cancel_pagination();
        self.total_tracks = None;
        self.tracks = tracks;
        self.focused_idx = 0;
        self.scroll_offset = 0;
        self.generation = next_list_generation();
        self.ensure_focus_visible();
    }
    pub(super) fn append_tracks(&mut self, tracks: Vec<PlaylistTrack>) {
        self.tracks.extend(tracks);
        self.generation = next_list_generation();
        self.ensure_focus_visible();
    }

    pub(super) fn cancel_pagination(&mut self) {
        if let Some(pagination) = self.pagination.take() {
            pagination.cancel();
        }
    }

    /// 列表代（行内图标的行数据缓存据此失效）。
    pub(super) fn generation(&self) -> u64 {
        self.generation
    }

    /// 搜索页点歌单/专辑后先落地的占位状态：标题用结果行的名字，数据由
    /// `App::tick_playlist_fetch` 搬进来（数据没到之前 `App::playlist_fetch` 为 `Some`，
    /// 歌单页不响应翻页键）。
    pub(super) fn placeholder(title: String, description: String) -> Self {
        Self {
            title,
            artist: String::new(),
            description,
            tracks: Vec::new(),
            ..Self::default()
        }
    }

    pub(super) fn focus_next(&mut self) -> bool {
        if self.tracks.is_empty() || self.focused_idx + 1 >= self.tracks.len() {
            return false;
        }

        let bottom = self
            .scroll_offset
            .saturating_add(self.visible_rows.max(1).saturating_sub(1));
        let is_bottom_edge = self.focused_idx >= bottom;

        self.focused_idx += 1;
        if is_bottom_edge {
            self.scroll_offset = self
                .scroll_offset
                .saturating_add(1)
                .min(self.max_scroll_offset());
        }
        self.ensure_focus_visible();
        true
    }

    pub(super) fn focus_prev(&mut self) -> bool {
        if self.tracks.is_empty() || self.focused_idx == 0 {
            return false;
        }

        let is_top_edge = self.focused_idx == self.scroll_offset;
        self.focused_idx -= 1;
        if is_top_edge && self.scroll_offset > 0 {
            self.scroll_offset -= 1;
        }
        self.ensure_focus_visible();
        true
    }
}

impl App {
    /// 搬运歌单页 / 专辑页的在途拉取（每帧调用，结果就绪才动状态）。
    pub(super) fn tick_playlist_fetch(&mut self) {
        let Some(slot) = self.browse.playlist_fetch.as_ref() else {
            return;
        };
        let Some(result) = peek_shared(&slot.future).cloned() else {
            return;
        };
        let kind = slot.kind;
        self.browse.playlist_fetch = None;
        self.downloads.page_kind = kind;

        match result {
            Ok(fetch) => {
                let title = fetch.title.clone();
                self.sync_private_roam_fetch(&fetch);
                self.apply_playlist_fetch(fetch);
                self.set_runtime_status(format!("{} {}", kind.opened(self.config.language), title));
            }
            Err(message) => {
                self.browse.playlist.description =
                    format!("{}: {message}", kind.failed(self.config.language));
                self.set_runtime_status(format!(
                    "{}: {message}",
                    kind.failed(self.config.language)
                ));
            }
        }
    }

    pub(super) fn tick_playlist_page_fetch(&mut self) {
        let browse = self.browse.playlist.pagination.clone();
        let playback = self.playback.pagination.clone();
        for pagination in browse.iter().chain(playback.iter().filter(|pager| {
            !browse
                .as_ref()
                .is_some_and(|browse| browse.same_source(pager))
        })) {
            let Some(result) = pagination.take_ready() else {
                continue;
            };
            match result {
                Ok(page) => {
                    if playlist_pagination::apply_page(
                        pagination,
                        page,
                        &mut self.browse.playlist,
                        self.playback.pagination.as_ref(),
                        &mut self.playback.playback_queue,
                    ) {
                        self.persist_playback_memory();
                    }
                }
                Err(error) => {
                    if self
                        .playback
                        .pagination
                        .as_ref()
                        .is_some_and(|pager| pager.same_source(pagination))
                        && self.playback.page_advance.take().is_some()
                    {
                        self.playback.playback_state = PlaybackRuntimeState::Stopped;
                    }
                    self.set_runtime_status(format!(
                        "{}: {error}",
                        self.lang_text("歌单分页加载失败", "Playlist page load failed")
                    ));
                }
            }
        }
    }

    pub(super) fn maybe_fetch_playlist_page(&mut self) {
        if self.page == Page::Playlist
            && self.browse.playlist_fetch.is_none()
            && self.browse.playlist.focused_idx + 1 >= self.browse.playlist.tracks.len()
            && let Some(pagination) = &self.browse.playlist.pagination
        {
            pagination.request(self.api.clone());
        }
    }

    pub(super) fn maybe_prefetch_playback_page(&self) {
        if let Some(index) = self.playback.playback_index
            && index.saturating_add(3) >= self.playback.playback_queue.len()
            && let Some(pagination) = &self.playback.pagination
        {
            pagination.prefetch(self.api.clone());
        }
    }

    /// 歌单页打开专辑（作者专辑区等）：立即落占位专辑页 + 派发后台拉取，
    /// 结果由 `tick_playlist_fetch` 搬进来（宿主循环在这期间照常重绘、照常响应输入）。
    pub(super) fn open_focused_playlist_album(&mut self) {
        let (album_id, title, fallback_cover_url, track_kind) = {
            let Some(track) = self
                .browse
                .playlist
                .tracks
                .get(self.browse.playlist.focused_idx)
            else {
                return;
            };

            let Some(album_id) = track.id.clone() else {
                self.set_runtime_status(self.lang_text(
                    "当前条目缺少专辑 ID，无法打开",
                    "The current item has no album ID",
                ));
                return;
            };

            (
                album_id,
                track.title.clone(),
                track.cover_url.clone(),
                track.kind,
            )
        };

        let is_author_section_album = self.playlist_return_page == Page::Author
            && matches!(
                track_kind,
                PlaylistTrackKind::Album | PlaylistTrackKind::Ep | PlaylistTrackKind::Single
            );
        let section_snapshot = if is_author_section_album {
            Some(self.browse.playlist.clone())
        } else {
            None
        };

        self.browse.playlist.cancel_pagination();
        // 同一页换来源：先保留当前页给切换动画当源快照；Esc 返回用的快照另存一份
        // （有它时按 Esc 回到作者专辑区，而不是回首页）。
        self.preserve_browse_page_for_transition(Page::Playlist);
        self.browse.playlist = PlaylistState::placeholder(
            title,
            self.lang_text("正在加载专辑…", "Loading album…")
                .to_string(),
        );
        self.playlist_section_return_snapshot = section_snapshot;

        let fut = fetch_album_page(
            self.api.clone(),
            self.config.language,
            album_id,
            fallback_cover_url,
        );
        let fut: PlaylistFetchTask = Box::pin(async move { Some(fut.await) });
        self.downloads.page_kind = PlaylistPageKind::Album;
        self.browse.playlist_fetch = Some(PlaylistFetchSlot {
            kind: PlaylistPageKind::Album,
            future: spawn_shared(fut, self.wake.clone()),
        });
        self.open_browse_page(Page::Playlist);
    }

    pub(super) async fn handle_playlist_key(&mut self, key: KeyEvent) {
        // 数据还在路上：占位页上的焦点/条目都没有意义，只留返回键。
        if self.browse.playlist_fetch.is_some() {
            if matches!(key.code, KeyCode::Esc | KeyCode::Left) {
                self.browse.playlist_fetch = None;
                self.close_browse_page(self.playlist_return_page);
            }
            return;
        }

        match key.code {
            KeyCode::Up | KeyCode::BackTab => {
                let _ = self.browse.playlist.focus_prev();
            }
            KeyCode::Down | KeyCode::Tab => {
                let _ = self.browse.playlist.focus_next();
                self.maybe_fetch_playlist_page();
            }
            KeyCode::Enter => self.play_focused_playlist_track().await,
            KeyCode::Esc | KeyCode::Left => {
                if let Some(pagination) = &self.browse.playlist.pagination {
                    pagination.cancel();
                }
                if let Some(snapshot) = self.playlist_section_return_snapshot.take() {
                    self.close_playlist_section(snapshot);
                    return;
                }
                let return_page = match self.playlist_return_page {
                    Page::Author => Page::Author,
                    Page::Search => Page::Search,
                    _ => Page::Home,
                };
                self.close_browse_page(return_page);
            }
            _ => {}
        }
    }

    /// 把拉到的歌单/专辑数据落到状态上（阻塞版与 `tick_playlist_fetch` 共用）。
    ///
    /// 落状态即宣告"这一页换成了新来源"：在途的那次拉取随之作废，否则它迟到时
    /// 会把刚打开的页面覆盖成被放弃的那一份（`tick_playlist_fetch` 只看句柄）。
    pub(super) fn apply_playlist_fetch(&mut self, fetch: PlaylistFetch) {
        let playing_source = self
            .playback
            .pagination
            .as_ref()
            .filter(|pagination| fetch.paginated && pagination.cursor().source_id == fetch.id)
            .cloned();
        if let Some(pagination) = &playing_source
            && self
                .browse
                .playlist
                .pagination
                .as_ref()
                .is_some_and(|current| current.same_source(pagination))
        {
            // Reopening the playback source must not cancel its shared in-flight page.
            self.browse.playlist.pagination = None;
        }
        let liked = self.browse.apply_playlist(fetch, &self.api);
        if let Some(pagination) = playing_source {
            self.browse.playlist.set_tracks(
                self.playback
                    .playback_queue
                    .iter()
                    .map(PlaybackTrack::as_playlist_track)
                    .collect(),
            );
            self.browse.playlist.total_tracks = pagination.cursor().total_tracks;
            self.browse.playlist.pagination = Some(pagination);
        }

        if let Some(liked) = liked {
            if let Some(profile) = liked.profile {
                self.apply_account_profile(profile);
            }
            self.apply_liked_song_ids(liked.ids);
        }
    }
}
