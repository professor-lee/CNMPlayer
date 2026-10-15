use super::*;

pub(crate) const PLAYLIST_PAGE_SIZE: usize = 100;

pub(crate) type PlaylistFetchFuture = SharedTask<Result<PlaylistFetch, String>>;

pub(crate) type PlaylistPageFetchFuture = SharedTask<Result<PlaylistPageFetch, String>>;

/// 装箱后的任务体（`spawn_shared` 的入参类型）。
pub(crate) type PlaylistFetchTask =
    Pin<Box<dyn Future<Output = Option<Result<PlaylistFetch, String>>>>>;

pub(crate) type PlaylistPageTask =
    Pin<Box<dyn Future<Output = Option<Result<PlaylistPageFetch, String>>>>>;

#[derive(Clone)]
pub(crate) struct PlaylistPageFetch {
    pub(crate) source_id: String,
    pub(crate) offset: usize,
    pub(crate) tracks: Vec<PlaylistTrack>,
    pub(crate) next_offset: usize,
    pub(crate) has_more: bool,
}

/// 歌单页 / 专辑页的在途拉取：句柄旁边记下是哪种页面（成功后文案不同）。
pub(crate) struct PlaylistFetchSlot {
    pub(crate) kind: PlaylistPageKind,
    pub(crate) future: PlaylistFetchFuture,
}

/// 打开的是歌单还是专辑：端点与文案不同，落状态是同一套。
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum PlaylistPageKind {
    Playlist,
    Album,
}

impl PlaylistPageKind {
    pub(crate) fn opened(self, language: Language) -> &'static str {
        match (self, language) {
            (Self::Playlist, Language::Zh) => "已打开歌单",
            (Self::Playlist, Language::En) => "Opened playlist",
            (Self::Album, Language::Zh) => "已打开专辑",
            (Self::Album, Language::En) => "Opened album",
        }
    }

    pub(crate) fn failed(self, language: Language) -> &'static str {
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
pub(crate) struct PlaylistFetch {
    pub(crate) id: String,
    pub(crate) title: String,
    pub(crate) artist: String,
    pub(crate) description: String,
    pub(crate) cover_url: Option<String>,
    pub(crate) tracks: Vec<PlaylistTrack>,
    pub(crate) total_tracks: Option<usize>,
    pub(crate) next_offset: usize,
    pub(crate) has_more: bool,
    pub(crate) paginated: bool,
    pub(crate) liked: Option<LikedRefresh>,
}

/// 打开的歌单是不是「我喜欢的音乐」：ID 对得上，或标题命中（中英文两种叫法）。
pub(crate) fn is_liked_playlist(
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
pub(crate) async fn fetch_playlist_tracks_page(
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
pub(crate) async fn fetch_playlist_page(
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

/// 拉一次专辑页数据（歌单页样式复用同一套落状态）。
///
/// 专辑接口不给每首歌的封面，缺封面的曲目用专辑封面兜底。
pub(crate) async fn fetch_album_page(
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
pub(crate) async fn fetch_album_page_from_song(
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
pub struct PlaylistState {
    pub id: Option<String>,
    pub title: String,
    pub artist: String,
    pub description: String,
    pub cover: CoverFetchState,
    pub focused_idx: usize,
    pub scroll_offset: usize,
    pub visible_rows: usize,
    pub tracks: Vec<PlaylistTrack>,
    pub total_tracks: Option<usize>,
    pub(crate) pagination: Option<PlaylistPagination>,
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
    pub(crate) fn max_scroll_offset(&self) -> usize {
        self.tracks.len().saturating_sub(self.visible_rows.max(1))
    }

    pub(crate) fn clamp_scroll_offset(&mut self) {
        self.scroll_offset = self.scroll_offset.min(self.max_scroll_offset());
    }

    pub(crate) fn ensure_focus_visible(&mut self) {
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

    pub fn set_visible_rows(&mut self, visible_rows: usize) {
        self.visible_rows = visible_rows.max(1);
        self.ensure_focus_visible();
    }

    pub fn effective_scroll_offset(&self) -> usize {
        self.scroll_offset.min(self.max_scroll_offset())
    }

    pub fn set_focus(&mut self, index: usize) {
        if self.tracks.is_empty() {
            self.focused_idx = 0;
            self.scroll_offset = 0;
            return;
        }

        self.focused_idx = index.min(self.tracks.len() - 1);
        self.ensure_focus_visible();
    }

    pub fn set_tracks(&mut self, tracks: Vec<PlaylistTrack>) {
        self.cancel_pagination();
        self.total_tracks = None;
        self.tracks = tracks;
        self.focused_idx = 0;
        self.scroll_offset = 0;
        self.generation = next_list_generation();
        self.ensure_focus_visible();
    }
    pub fn append_tracks(&mut self, tracks: Vec<PlaylistTrack>) {
        self.tracks.extend(tracks);
        self.generation = next_list_generation();
        self.ensure_focus_visible();
    }

    pub(crate) fn cancel_pagination(&mut self) {
        if let Some(pagination) = self.pagination.take() {
            pagination.cancel();
        }
    }

    /// 列表代（行内图标的行数据缓存据此失效）。
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// 搜索页点歌单/专辑后先落地的占位状态：标题用结果行的名字，数据由
    /// `App::tick_playlist_fetch` 搬进来（数据没到之前 `App::playlist_fetch` 为 `Some`，
    /// 歌单页不响应翻页键）。
    pub fn placeholder(title: String, description: String) -> Self {
        Self {
            title,
            artist: String::new(),
            description,
            tracks: Vec::new(),
            ..Self::default()
        }
    }

    pub fn focus_next(&mut self) -> bool {
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

    pub fn focus_prev(&mut self) -> bool {
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
    pub(crate) fn tick_playlist_fetch(&mut self) {
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

    pub(crate) fn tick_playlist_page_fetch(&mut self) {
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

    pub(crate) fn maybe_fetch_playlist_page(&mut self) {
        if self.page == Page::Playlist
            && self.browse.playlist_fetch.is_none()
            && self.browse.playlist.focused_idx + 1 >= self.browse.playlist.tracks.len()
            && let Some(pagination) = &self.browse.playlist.pagination
        {
            pagination.request(self.api.clone());
        }
    }

    pub(crate) fn maybe_prefetch_playback_page(&self) {
        if let Some(index) = self.playback.playback_index
            && index.saturating_add(3) >= self.playback.playback_queue.len()
            && let Some(pagination) = &self.playback.pagination
        {
            pagination.prefetch(self.api.clone());
        }
    }

    pub(crate) async fn open_focused_playlist_album(&mut self) {
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

        match self.load_album_detail(&album_id).await {
            Ok(()) => {
                self.playlist_section_return_snapshot = section_snapshot;
                if let (None, Some(url)) = (&self.browse.playlist.cover.image, fallback_cover_url) {
                    self.browse.playlist.cover.load(self.api.clone(), url)
                }
                self.set_runtime_status(format!(
                    "{} {}",
                    self.lang_text("已打开专辑", "Opened album"),
                    title
                ));
            }
            Err(err) => {
                self.set_runtime_status(format!(
                    "{}: {}",
                    self.lang_text("打开专辑失败", "Failed to open album"),
                    err
                ));
            }
        }
    }

    pub(crate) async fn handle_playlist_key(&mut self, key: KeyEvent) {
        // 数据还在路上：占位页上的焦点/条目都没有意义，只留返回键。
        if self.browse.playlist_fetch.is_some() {
            if matches!(key.code, KeyCode::Esc | KeyCode::Left) {
                self.page = self.playlist_return_page;
                self.browse.playlist_fetch = None;
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
                    self.browse.playlist = snapshot;
                    return;
                }
                self.page = match self.playlist_return_page {
                    Page::Author => Page::Author,
                    Page::Search => Page::Search,
                    _ => Page::Home,
                };
            }
            _ => {}
        }
    }

    pub(crate) async fn load_playlist_detail(&mut self, playlist_id: &str) -> Result<()> {
        let fetch = fetch_playlist_page(
            self.api.clone(),
            self.config.language,
            playlist_id.to_string(),
            None,
            self.browse.home_sidebar.liked_playlist_id.clone(),
            self.browse.home_sidebar.user_id.clone(),
        )
        .await
        .map_err(anyhow::Error::msg)?;
        self.apply_playlist_fetch(fetch);
        Ok(())
    }

    /// 把拉到的歌单/专辑数据落到状态上（阻塞版与 `tick_playlist_fetch` 共用）。
    ///
    /// 落状态即宣告"这一页换成了新来源"：在途的那次拉取随之作废，否则它迟到时
    /// 会把刚打开的页面覆盖成被放弃的那一份（`tick_playlist_fetch` 只看句柄）。
    pub(crate) fn apply_playlist_fetch(&mut self, fetch: PlaylistFetch) {
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

    pub(crate) async fn load_daily_recommend_playlist(&mut self) -> Result<()> {
        let response = self.api.recommend_songs().await?;
        let code = response_code(&response);
        if code != 200 {
            return Err(anyhow!(
                "请求失败({}): {}",
                code,
                response_message(&response)
            ));
        }

        let songs = home_daily_song_items(&response.body).ok_or_else(|| {
            anyhow!(self.lang_text("每日推荐数据缺失", "Daily recommendations are missing"))
        })?;

        let tracks = parse_tracks(songs);
        if tracks.is_empty() {
            return Err(anyhow!(
                self.lang_text("每日推荐为空", "Daily recommendations are empty")
            ));
        }

        let cover_url = tracks.iter().find_map(|track| track.cover_url.clone());

        // 这一页换成每日推荐：在途的占位拉取作废（同 `apply_playlist_fetch`）。
        self.browse.playlist_fetch = None;
        self.browse.playlist.id = Some(HOME_DAILY_RECOMMEND_TILE_ID.to_string());
        self.browse.playlist.title = self
            .lang_text("每日推荐", "Daily Recommendations")
            .to_string();
        self.browse.playlist.artist = self
            .lang_text("网易云音乐", "Netease Cloud Music")
            .to_string();
        self.browse.playlist.description = self
            .lang_text(
                "来自网易云每日推荐歌曲，按 Enter 播放",
                "Daily songs from Netease. Press Enter to play",
            )
            .to_string();
        self.browse.playlist.set_tracks(tracks);
        if let Some(x) = cover_url {
            self.browse.playlist.cover.load(self.api.clone(), x)
        }
        Ok(())
    }

    pub(crate) async fn load_album_detail(&mut self, album_id: &str) -> Result<()> {
        let fetch = fetch_album_page(
            self.api.clone(),
            self.config.language,
            album_id.to_string(),
            None,
        )
        .await
        .map_err(anyhow::Error::msg)?;
        self.apply_playlist_fetch(fetch);
        Ok(())
    }
}
