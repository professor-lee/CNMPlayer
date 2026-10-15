use super::*;

pub(crate) const ARTIST_ALBUM_PAGE_SIZE: usize = 60;

pub(crate) type AuthorFetchFuture = SharedTask<Result<AuthorFetch, String>>;

/// 装箱后的任务体（`spawn_shared` 的入参类型）。
pub(crate) type AuthorFetchTask = Pin<Box<dyn Future<Output = Option<Result<AuthorFetch, String>>>>>;

/// 作者页四个接口的原始回包（`None` = 该请求失败）；发行物请求会先完成全部分页。
///
/// 解析见 `App::build_author_page`。
pub(crate) struct AuthorResponses {
    pub(crate) detail: Option<ApiResponse>,
    pub(crate) desc: Option<ApiResponse>,
    pub(crate) top_song: Option<ApiResponse>,
    pub(crate) album: Option<ApiResponse>,
}

/// 作者页的一次拉取结果：`AuthorState` 里除封面句柄与视口字段外的全部字段。
///
/// 结果要经 `spawn_shared` 搬运，故实现 `Clone`（封面句柄共享输出与取消所有权）。
#[derive(Clone)]
pub(crate) struct AuthorFetch {
    pub(crate) id: String,
    pub(crate) title: String,
    pub(crate) artist: String,
    pub(crate) description: String,
    pub(crate) cover_url: Option<String>,
    pub(crate) tiles: Vec<AuthorTile>,
    pub(crate) hot_songs: Vec<PlaylistTrack>,
    pub(crate) albums: Vec<PlaylistTrack>,
    pub(crate) eps: Vec<PlaylistTrack>,
    pub(crate) singles: Vec<PlaylistTrack>,
}

/// 四个 `artist/*` 接口一次并发拉取；发行物接口在自己的后台 future 内顺序遍历分页。
///
/// 它们彼此独立，按仓库既有做法用 `futures::join!`：`cyper::Client` 是 `!Send`，
/// 只能同一个 runtime 里并发，不能各自 spawn。
pub(crate) async fn fetch_artist_responses(
    api: &ApiState,
    artist_id: &str,
) -> Result<AuthorResponses, String> {
    let mut detail_api = api.clone();
    let mut desc_api = api.clone();
    let mut top_song_api = api.clone();
    let album_api = api.clone();
    let album_fetch = fetch_all_artist_albums(album_api, artist_id);
    let (detail, desc, top_song, album) = futures::join!(
        detail_api.artist_detail(artist_id),
        desc_api.artist_desc(artist_id),
        top_song_api.artist_top_song(artist_id),
        album_fetch,
    );
    let album = album.map_err(|error| format!("artist album pagination failed: {error}"))?;

    Ok(AuthorResponses {
        detail: detail.ok(),
        desc: desc.ok(),
        top_song: top_song.ok(),
        album: Some(album),
    })
}

/// 后台顺序遍历歌手发行物的所有分页。
///
/// 分页中途失败直接返回错误，不把已取得的首批数据冒充成完整列表。
pub(crate) async fn fetch_all_artist_albums(
    mut api: ApiState,
    artist_id: &str,
) -> Result<ApiResponse, String> {
    let mut pages = Vec::new();
    let mut offset = 0usize;

    loop {
        let page = api
            .artist_album(artist_id, ARTIST_ALBUM_PAGE_SIZE, offset)
            .await
            .map_err(|error| format!("offset {offset}: {error:#}"))?;
        let item_count = artist_album_items(&page.body)
            .map(|items| items.len())
            .ok_or_else(|| format!("offset {offset}: response has no album list"))?;
        let more = page
            .body
            .get("more")
            .and_then(Value::as_bool)
            .unwrap_or(false);

        pages.push(page);
        if !more {
            break;
        }
        if item_count == 0 {
            return Err(format!("offset {offset}: empty page marked as more"));
        }
        offset = offset
            .checked_add(item_count)
            .ok_or_else(|| "artist album pagination offset overflowed".to_string())?;
    }

    merge_artist_album_pages(pages)
}

pub(crate) fn merge_artist_album_pages(pages: Vec<ApiResponse>) -> Result<ApiResponse, String> {
    let mut pages = pages.into_iter();
    let Some(mut merged) = pages.next() else {
        return Err("artist album pagination returned no pages".to_string());
    };

    for mut page in pages {
        let items = take_artist_album_items(&mut page.body)
            .ok_or_else(|| "artist album page has no album list".to_string())?;
        let target = artist_album_items_mut(&mut merged.body)
            .ok_or_else(|| "merged artist album response has no album list".to_string())?;
        target.extend(items);
    }

    Ok(merged)
}

pub(crate) fn artist_album_items(body: &Value) -> Option<&[Value]> {
    if let Some(items) = body.get("hotAlbums").and_then(Value::as_array) {
        return Some(items);
    }
    body.pointer("/artist/albums")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
}

pub(crate) fn artist_album_items_mut(body: &mut Value) -> Option<&mut Vec<Value>> {
    if body.get("hotAlbums").and_then(Value::as_array).is_some() {
        return body.get_mut("hotAlbums").and_then(Value::as_array_mut);
    }
    body.pointer_mut("/artist/albums")
        .and_then(Value::as_array_mut)
}

pub(crate) fn take_artist_album_items(body: &mut Value) -> Option<Vec<Value>> {
    if body.get("hotAlbums").and_then(Value::as_array).is_some() {
        return body
            .get_mut("hotAlbums")
            .and_then(Value::as_array_mut)
            .map(std::mem::take);
    }
    body.pointer_mut("/artist/albums")
        .and_then(Value::as_array_mut)
        .map(std::mem::take)
}

/// 全屏页点作者名要拉的东西：先 `song/detail` 解析出段对应的作者 ID，再拉作者页数据。
///
/// 整段不借 `&mut App`，交给 `spawn_shared` 后台跑，宿主循环照常重绘。
pub(crate) async fn fetch_author_page(
    api: ApiState,
    language: Language,
    song_id: String,
    index: usize,
    artist_line: String,
) -> Result<AuthorFetch, String> {
    let refs = fetch_song_page_refs(api.clone(), &song_id).await;
    let artist_id = pick_artist_id(&refs.artists, &artist_line, index).ok_or_else(|| {
        lang_text(
            language,
            "无法解析当前歌曲的作者",
            "Failed to resolve the artist of the current song",
        )
        .to_string()
    })?;

    // 全屏页只有显示名，没有搜索结果那行的封面可以兜底。
    fetch_author_page_by_id(api, language, artist_id, None).await
}

/// 拉一次作者页数据（作者 ID 已确定）。
///
/// `fallback_cover_url` 是搜索结果里那行的封面：接口没给头像时用它兜底。
pub(crate) async fn fetch_author_page_by_id(
    api: ApiState,
    language: Language,
    artist_id: String,
    fallback_cover_url: Option<String>,
) -> Result<AuthorFetch, String> {
    let responses = fetch_artist_responses(&api, &artist_id).await?;
    let mut fetch = App::build_author_page(&api, language, &artist_id, responses)?;
    if fetch.cover_url.is_none() {
        fetch.cover_url = fallback_cover_url;
    }
    Ok(fetch)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthorTileKind {
    HotSong,
    Album,
    Ep,
    Single,
}

#[derive(Clone)]
pub struct AuthorTile {
    pub kind: AuthorTileKind,
    pub title: String,
    pub subtitle: String,
    pub cover: CoverFetchState,
}

impl AuthorTile {
    pub(crate) fn placeholder() -> Self {
        Self {
            kind: AuthorTileKind::Album,
            title: "暂无内容".to_string(),
            subtitle: "No content".to_string(),
            cover: CoverFetchState::default(),
        }
    }

    pub(crate) fn from_album(
        api: &ApiState,
        title: String,
        subtitle: String,
        cover_url: Option<String>,
        kind: AuthorTileKind,
    ) -> Self {
        let mut cover = CoverFetchState::default();
        if let Some(x) = cover_url {
            cover.load(api.clone(), x)
        }
        Self {
            kind,
            title,
            subtitle,
            cover,
        }
    }
}

pub struct AuthorState {
    pub id: Option<String>,
    pub title: String,
    pub artist: String,
    pub description: String,
    pub cover: CoverFetchState,
    pub focused_idx: usize,
    pub columns: usize,
    pub scroll_row_offset: usize,
    pub visible_rows: usize,
    pub tiles: Vec<AuthorTile>,
    pub hot_songs: Vec<PlaylistTrack>,
    pub albums: Vec<PlaylistTrack>,
    pub eps: Vec<PlaylistTrack>,
    pub singles: Vec<PlaylistTrack>,
}

impl Default for AuthorState {
    fn default() -> Self {
        Self {
            id: None,
            title: "作者页".to_string(),
            artist: "网易云音乐".to_string(),
            description: "从搜索结果进入作者页后加载真实数据。".to_string(),
            cover: CoverFetchState::default(),
            focused_idx: 0,
            columns: 1,
            scroll_row_offset: 0,
            visible_rows: 1,
            tiles: vec![AuthorTile::placeholder()],
            hot_songs: Vec::new(),
            albums: Vec::new(),
            eps: Vec::new(),
            singles: Vec::new(),
        }
    }
}

impl AuthorState {
    /// 点作者名后先落地的占位状态：标题就是点中的那段名字，数据由 `App::tick_author_fetch`
    /// 搬进来（数据没到之前 `App::author_fetch` 为 `Some`，作者页不响应翻页键）。
    pub fn placeholder(title: String, description: String) -> Self {
        Self {
            title,
            artist: String::new(),
            description,
            tiles: Vec::new(),
            ..Self::default()
        }
    }

    pub(crate) fn total_rows(&self) -> usize {
        if self.tiles.is_empty() {
            0
        } else {
            (self.tiles.len() - 1) / self.columns.max(1) + 1
        }
    }

    pub(crate) fn max_scroll_row_offset(&self) -> usize {
        self.total_rows().saturating_sub(self.visible_rows.max(1))
    }

    pub(crate) fn clamp_scroll_row_offset(&mut self) {
        self.scroll_row_offset = self.scroll_row_offset.min(self.max_scroll_row_offset());
    }

    pub(crate) fn ensure_focus_visible(&mut self) {
        if self.tiles.is_empty() {
            self.focused_idx = 0;
            self.scroll_row_offset = 0;
            return;
        }

        self.focused_idx = self.focused_idx.min(self.tiles.len() - 1);
        let focused_row = self.focused_idx / self.columns.max(1);
        if focused_row < self.scroll_row_offset {
            self.scroll_row_offset = focused_row;
        } else {
            let bottom_row = self
                .scroll_row_offset
                .saturating_add(self.visible_rows.max(1).saturating_sub(1));
            if focused_row > bottom_row {
                self.scroll_row_offset = focused_row
                    .saturating_add(1)
                    .saturating_sub(self.visible_rows.max(1));
            }
        }

        self.clamp_scroll_row_offset();
    }

    pub fn set_tiles(&mut self, mut tiles: Vec<AuthorTile>) {
        if tiles.is_empty() {
            tiles.push(AuthorTile::placeholder());
        }
        self.tiles = tiles;
        self.focused_idx = 0;
        self.scroll_row_offset = 0;
        self.ensure_focus_visible();
    }

    pub fn set_columns(&mut self, columns: usize) {
        self.columns = columns.max(1);
        self.ensure_focus_visible();
    }

    pub fn set_visible_rows(&mut self, visible_rows: usize) {
        self.visible_rows = visible_rows.max(1);
        self.ensure_focus_visible();
    }

    pub fn effective_scroll_row_offset(&self) -> usize {
        self.scroll_row_offset.min(self.max_scroll_row_offset())
    }

    pub fn set_focus(&mut self, index: usize) {
        if self.tiles.is_empty() {
            self.focused_idx = 0;
            self.scroll_row_offset = 0;
            return;
        }

        self.focused_idx = index.min(self.tiles.len() - 1);
        self.ensure_focus_visible();
    }

    pub fn focus_next(&mut self) -> bool {
        if self.tiles.is_empty() {
            return false;
        }

        self.focused_idx = if self.focused_idx + 1 < self.tiles.len() {
            self.focused_idx + 1
        } else {
            0
        };
        self.ensure_focus_visible();
        true
    }

    pub fn focus_prev(&mut self) -> bool {
        if self.tiles.is_empty() {
            return false;
        }

        self.focused_idx = if self.focused_idx == 0 {
            self.tiles.len() - 1
        } else {
            self.focused_idx - 1
        };
        self.ensure_focus_visible();
        true
    }

    pub fn focus_left(&mut self) {
        self.focus_prev();
    }

    pub fn focus_right(&mut self) {
        self.focus_next();
    }

    pub fn focus_up(&mut self) -> bool {
        if self.tiles.is_empty() {
            return false;
        }

        let step = self.columns.max(1);
        if self.focused_idx < step {
            return false;
        }

        let focused_row = self.focused_idx / step;
        let is_top_edge = focused_row == self.scroll_row_offset;
        self.focused_idx -= step;
        if is_top_edge && self.scroll_row_offset > 0 {
            self.scroll_row_offset -= 1;
        }
        self.ensure_focus_visible();
        true
    }

    pub fn focus_down(&mut self) -> bool {
        if self.tiles.is_empty() {
            return false;
        }

        let step = self.columns.max(1);
        let target = self.focused_idx + step;
        if target >= self.tiles.len() {
            return false;
        }

        let focused_row = self.focused_idx / step;
        let bottom_edge_row = self
            .scroll_row_offset
            .saturating_add(self.visible_rows.max(1).saturating_sub(1));
        let is_bottom_edge = focused_row >= bottom_edge_row;
        self.focused_idx = target;
        if is_bottom_edge {
            self.scroll_row_offset = self
                .scroll_row_offset
                .saturating_add(1)
                .min(self.max_scroll_row_offset());
        }
        self.ensure_focus_visible();
        true
    }
}

pub(crate) fn first_non_empty_intro_text(value: &Value) -> Option<String> {
    value
        .get("introduction")
        .and_then(|item| item.as_array())
        .and_then(|items| {
            items
                .iter()
                .find_map(|intro| first_non_empty(intro, &["/txt", "/ti"]))
        })
}

pub(crate) fn artist_album_kind(item: &Value) -> AuthorTileKind {
    let type_text = item
        .get("type")
        .and_then(|value| value.as_str())
        .unwrap_or_default();
    let sub_type_text = item
        .get("subType")
        .and_then(|value| value.as_str())
        .unwrap_or_default();

    let type_lower = type_text.to_ascii_lowercase();
    let sub_type_lower = sub_type_text.to_ascii_lowercase();

    let is_single = type_lower.contains("single")
        || sub_type_lower.contains("single")
        || type_text.contains("单曲")
        || sub_type_text.contains("单曲");
    if is_single {
        return AuthorTileKind::Single;
    }

    let is_ep =
        type_lower.contains("ep") || sub_type_lower.contains("ep") || type_text.contains("EP");
    if is_ep {
        return AuthorTileKind::Ep;
    }

    AuthorTileKind::Album
}

impl App {
    /// 搬运作者页的在途拉取（每帧调用，结果就绪才动状态）。
    pub(crate) fn tick_author_fetch(&mut self) {
        let Some(result) = peek_shared_future(&self.browse.author_fetch).cloned() else {
            return;
        };
        self.browse.author_fetch = None;

        match result {
            Ok(fetch) => {
                self.playlist_section_return_snapshot = None;
                let title = fetch.title.clone();
                self.apply_author_fetch(fetch);
                self.set_runtime_status(format!(
                    "{} {}",
                    self.lang_text("已打开作者", "Opened artist"),
                    title
                ));
            }
            Err(message) => {
                self.browse.author.description = format!(
                    "{}: {message}",
                    self.lang_text("作者页加载失败", "Failed to load the artist page")
                );
                self.set_runtime_status(format!(
                    "{}: {message}",
                    self.lang_text("打开作者页失败", "Failed to open the artist page"),
                ));
            }
        }
    }

    pub(crate) async fn play_focused_author_tile(&mut self) {
        let Some(item) = self.browse.author.tiles.get(self.browse.author.focused_idx) else {
            return;
        };

        let (section_title, tracks, section_cover) = match item.kind {
            AuthorTileKind::HotSong => (
                self.lang_text("热门歌曲", "Hot Songs").to_string(),
                self.browse.author.hot_songs.clone(),
                self.browse
                    .author
                    .hot_songs
                    .first()
                    .and_then(|track| track.cover_url.clone())
                    .or_else(|| self.browse.author.cover.url.clone()),
            ),
            AuthorTileKind::Album => (
                self.lang_text("专辑", "Albums").to_string(),
                self.browse.author.albums.clone(),
                self.browse
                    .author
                    .albums
                    .first()
                    .and_then(|track| track.cover_url.clone())
                    .or_else(|| self.browse.author.cover.url.clone()),
            ),
            AuthorTileKind::Ep => (
                "EP".to_string(),
                self.browse.author.eps.clone(),
                self.browse
                    .author
                    .eps
                    .first()
                    .and_then(|track| track.cover_url.clone())
                    .or_else(|| self.browse.author.cover.url.clone()),
            ),
            AuthorTileKind::Single => (
                "Single".to_string(),
                self.browse.author.singles.clone(),
                self.browse
                    .author
                    .singles
                    .first()
                    .and_then(|track| track.cover_url.clone())
                    .or_else(|| self.browse.author.cover.url.clone()),
            ),
        };

        if tracks.is_empty() {
            self.set_runtime_status(self.lang_text(
                "当前分类暂无可用内容",
                "This section has no available items",
            ));
            return;
        }

        self.playlist_return_page = Page::Author;
        self.playlist_section_return_snapshot = None;
        // 这一页换成作者页分区：在途的占位拉取作废（同 `apply_playlist_fetch`）。
        self.browse.playlist_fetch = None;
        self.browse.playlist.id = self
            .browse
            .author
            .id
            .as_ref()
            .map(|id| format!("artist:{}:{}", id, section_title));
        self.browse.playlist.title = format!("{} · {}", self.browse.author.title, section_title);
        self.browse.playlist.artist = self.browse.author.title.clone();
        self.browse.playlist.description = self
            .lang_text(
                "按 Enter 进入专辑或播放歌曲，Esc 返回作者页",
                "Press Enter to open album or play song, Esc to return",
            )
            .to_string();
        self.browse.playlist.set_tracks(tracks);
        if let Some(x) = section_cover {
            self.browse.playlist.cover.load(self.api.clone(), x)
        }
        self.page = Page::Playlist;
    }

    pub(crate) async fn handle_author_key(&mut self, key: KeyEvent) {
        // 数据还在路上：占位页上的焦点/条目都没有意义，只留返回键。
        if self.browse.author_fetch.is_some() {
            if key.code == KeyCode::Esc {
                self.page = self.author_return_page;
                self.browse.author_fetch = None;
            }
            return;
        }

        match key.code {
            KeyCode::Tab => {
                let _ = self.browse.author.focus_next();
            }
            KeyCode::BackTab => {
                let _ = self.browse.author.focus_prev();
            }
            KeyCode::Left => self.browse.author.focus_left(),
            KeyCode::Right => self.browse.author.focus_right(),
            KeyCode::Up => {
                let _ = self.browse.author.focus_up();
            }
            KeyCode::Down => {
                let _ = self.browse.author.focus_down();
            }
            KeyCode::Enter => self.play_focused_author_tile().await,
            KeyCode::Esc => {
                self.page = self.author_return_page;
            }
            _ => {}
        }
    }

    /// 把拉到的作者页数据落到状态上（`tick_author_fetch` 搬运结果时用）。
    ///
    /// 同 `apply_playlist_fetch`：新数据落地即在途拉取作废，免得迟到的旧结果覆盖它。
    pub(crate) fn apply_author_fetch(&mut self, fetch: AuthorFetch) {
        self.browse.apply_author(fetch, &self.api);
    }

    /// 解析 `artist/*` 的回包（网络部分见 `fetch_artist_responses`）。
    ///
    /// 不借 `&mut self`：全屏页那条把整段解析连同请求一起丢给 `shot_and_share` 后台跑。
    pub(crate) fn build_author_page(
        api: &ApiState,
        language: Language,
        artist_id: &str,
        responses: AuthorResponses,
    ) -> Result<AuthorFetch, String> {
        let AuthorResponses {
            detail,
            desc,
            top_song,
            album,
        } = responses;

        if detail.is_none() && desc.is_none() && top_song.is_none() && album.is_none() {
            return Err(
                lang_text(language, "作者数据获取失败", "Failed to fetch artist data").to_string(),
            );
        }

        let mut title = String::new();
        let mut description = String::new();
        let mut cover_url = None;

        if let Some(response) = detail.as_ref()
            && response_code(response) == 200
        {
            title = first_non_empty(
                &response.body,
                &["/data/artist/name", "/artist/name", "/data/name"],
            )
            .unwrap_or_default();
            cover_url = first_non_empty(
                &response.body,
                &[
                    "/data/artist/avatarUrl",
                    "/data/artist/cover",
                    "/artist/picUrl",
                    "/artist/img1v1Url",
                    "/artist/avatarUrl",
                ],
            );
            description = first_non_empty(
                &response.body,
                &["/data/artist/briefDesc", "/artist/briefDesc"],
            )
            .unwrap_or_default();
        }

        if title.is_empty()
            && let Some(response) = top_song.as_ref()
            && let Some(first_song) = response
                .body
                .get("songs")
                .and_then(|value| value.as_array())
                .and_then(|songs| songs.first())
        {
            title = parse_artists(first_song).unwrap_or_default();
        }

        if let Some(response) = desc.as_ref()
            && response_code(response) == 200
        {
            if let Some(text) = first_non_empty(&response.body, &["/briefDesc", "/data/briefDesc"])
                && !text.trim().is_empty()
            {
                description = text;
            }

            if description.trim().is_empty()
                && let Some(text) = first_non_empty_intro_text(&response.body)
            {
                description = text;
            }
        }

        if title.trim().is_empty() {
            title = lang_text(language, "未知作者", "Unknown Author").to_string();
        }

        if description.trim().is_empty() {
            description =
                lang_text(language, "暂无作者简介", "No author description yet").to_string();
        }

        let mut hot_songs = Vec::new();
        let mut albums = Vec::new();
        let mut eps = Vec::new();
        let mut singles = Vec::new();

        if let Some(response) = top_song.as_ref()
            && response_code(response) == 200
            && let Some(items) = response
                .body
                .get("songs")
                .and_then(|value| value.as_array())
        {
            hot_songs = parse_tracks(items);
        }

        if let Some(response) = album.as_ref()
            && response_code(response) == 200
        {
            let album_items = response
                .body
                .get("hotAlbums")
                .and_then(|value| value.as_array())
                .or_else(|| {
                    response
                        .body
                        .pointer("/artist/albums")
                        .and_then(|value| value.as_array())
                });

            if let Some(items) = album_items {
                for item in items {
                    let Some(name) = item.get("name").and_then(|value| value.as_str()) else {
                        continue;
                    };

                    let size = item
                        .get("size")
                        .and_then(|value| value.as_i64())
                        .unwrap_or_default();

                    let kind = artist_album_kind(item);
                    let cover_url = first_non_empty(item, &["/picUrl", "/blurPicUrl"]);
                    let track = PlaylistTrack {
                        kind: match kind {
                            AuthorTileKind::HotSong => PlaylistTrackKind::Song,
                            AuthorTileKind::Album => PlaylistTrackKind::Album,
                            AuthorTileKind::Ep => PlaylistTrackKind::Ep,
                            AuthorTileKind::Single => PlaylistTrackKind::Single,
                        },
                        id: parse_value_as_string(item.get("id")),
                        title: name.to_string(),
                        artist: first_non_empty(item, &["/artist/name", "/artists/0/name"])
                            .unwrap_or_else(|| title.clone()),
                        album: name.to_string(),
                        cover_url,
                        duration_ms: 0,
                        duration: format!("{} {}", size, lang_text(language, "首", "tracks")),
                    };

                    match kind {
                        AuthorTileKind::HotSong | AuthorTileKind::Album => albums.push(track),
                        AuthorTileKind::Ep => eps.push(track),
                        AuthorTileKind::Single => singles.push(track),
                    }
                }
            }
        }

        let hot_count = hot_songs.len();
        let album_count = albums.len();
        let ep_count = eps.len();
        let single_count = singles.len();

        let tiles = vec![
            AuthorTile::from_album(
                api,
                lang_text(language, "热门歌曲", "Hot Songs").to_string(),
                format!("{} {}", hot_count, lang_text(language, "首", "tracks")),
                hot_songs
                    .first()
                    .and_then(|track| track.cover_url.clone())
                    .or_else(|| cover_url.clone()),
                AuthorTileKind::HotSong,
            ),
            AuthorTile::from_album(
                api,
                lang_text(language, "专辑", "Albums").to_string(),
                format!("{} {}", album_count, lang_text(language, "张", "items")),
                albums
                    .first()
                    .and_then(|track| track.cover_url.clone())
                    .or_else(|| cover_url.clone()),
                AuthorTileKind::Album,
            ),
            AuthorTile::from_album(
                api,
                "EP".to_string(),
                format!("{} {}", ep_count, lang_text(language, "张", "items")),
                eps.first()
                    .and_then(|track| track.cover_url.clone())
                    .or_else(|| cover_url.clone()),
                AuthorTileKind::Ep,
            ),
            AuthorTile::from_album(
                api,
                "Single".to_string(),
                format!("{} {}", single_count, lang_text(language, "张", "items")),
                singles
                    .first()
                    .and_then(|track| track.cover_url.clone())
                    .or_else(|| cover_url.clone()),
                AuthorTileKind::Single,
            ),
        ];

        let artist = match language {
            Language::Zh => format!(
                "热门 {} · 专辑 {} · EP {} · Single {}",
                hot_count, album_count, ep_count, single_count
            ),
            Language::En => format!(
                "Hot {} · Albums {} · EP {} · Singles {}",
                hot_count, album_count, ep_count, single_count
            ),
        };

        Ok(AuthorFetch {
            id: artist_id.to_string(),
            title,
            artist,
            description,
            cover_url,
            tiles,
            hot_songs,
            albums,
            eps,
            singles,
        })
    }
}
