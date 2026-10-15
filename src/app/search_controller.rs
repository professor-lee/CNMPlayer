use super::*;

pub(crate) const SEARCH_RESULT_PAGE_SIZE: usize = 100;

/// 无后缀（混合）搜索里作者 / 歌单分区只取最相关的少量条目，不参与分页。
pub(crate) const MIXED_AUX_RESULT_LIMIT: usize = 5;

/// 搜索框滑出动画时长（time-based，与帧率解耦）
pub(crate) const SEARCH_BOX_ANIM_DURATION: Duration = Duration::from_millis(180);

/// 搜索结果条目的种类。带后缀的搜索只产出单一种类；无后缀搜索混合作者 / 歌单 / 单曲。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchItemKind {
    Song,
    Album,
    Artist,
    Playlist,
}

/// 作者卡片的行数：上边框 + 头像两行 + 下边框。
pub const ARTIST_CARD_ROWS: usize = 4;

impl SearchItemKind {
    /// 行右侧的类型标签（与 `SearchScope` 的后缀同源，避免两处字面量）。
    /// 单曲行的右侧位让给时长，故为 None。
    pub fn tag(self) -> Option<&'static str> {
        let scope = self.scope();
        (scope != SearchScope::Single).then(|| scope.suffix())
    }

    /// 条目占用的行数：作者卡片 4 行，其余 1 行。
    pub fn rows(self, card: bool) -> usize {
        if card && self == Self::Artist {
            ARTIST_CARD_ROWS
        } else {
            1
        }
    }

    pub(crate) fn scope(self) -> SearchScope {
        match self {
            Self::Song => SearchScope::Single,
            Self::Album => SearchScope::Album,
            Self::Artist => SearchScope::Author,
            Self::Playlist => SearchScope::Playlist,
        }
    }
}

pub struct SearchItem {
    pub kind: SearchItemKind,
    pub left_label: String,
    pub right_label: String,
    pub song_id: Option<String>,
    pub album_id: Option<String>,
    pub playlist_id: Option<String>,
    pub artist_id: Option<String>,
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub cover_url: Option<String>,
    pub duration_ms: Option<i64>,
    /// 作者条目的头像，复用封面管线。
    pub cover: CoverFetchState,
}

/// 搜索请求的作用域。`Mixed` 对应无后缀搜索（作者 + 歌单 + 单曲并发拉取后合并）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchScope {
    Mixed,
    Single,
    Album,
    Author,
    Playlist,
}

impl SearchScope {
    /// 带后缀的类型；`Mixed` 无后缀，不在此表。
    pub(crate) const SUFFIXED: [SearchScope; 4] = [Self::Single, Self::Album, Self::Author, Self::Playlist];

    pub(crate) fn suffix(self) -> &'static str {
        match self {
            Self::Mixed => "",
            Self::Single => "@single",
            Self::Album => "@album",
            Self::Author => "@author",
            Self::Playlist => "@list",
        }
    }

    /// NCM cloudsearch 的 `type`；`Mixed` 由 `execute_search` 拆成多请求，没有单一 id。
    pub(crate) fn search_type(self) -> Option<i32> {
        match self {
            Self::Mixed => None,
            Self::Single => Some(1),
            Self::Album => Some(10),
            Self::Author => Some(100),
            Self::Playlist => Some(1000),
        }
    }

    pub(crate) fn display_name(self) -> &'static str {
        match self {
            Self::Mixed => "综合",
            Self::Single => "单曲",
            Self::Album => "专辑",
            Self::Author => "作者",
            Self::Playlist => "歌单",
        }
    }
}

pub(crate) fn parse_search_input(raw: &str) -> (String, SearchScope) {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return (String::new(), SearchScope::Mixed);
    }

    let lower = trimmed.to_ascii_lowercase();

    // 后缀表来自 `SearchScope::SUFFIXED`（单一来源）；`@artist` 是同义别名。
    let suffixes = SearchScope::SUFFIXED
        .into_iter()
        .map(|scope| (scope.suffix(), scope))
        .chain([("@artist", SearchScope::Author)]);
    for (suffix, scope) in suffixes {
        if lower.ends_with(suffix) {
            let cut = trimmed.len().saturating_sub(suffix.len());
            let stripped = &trimmed[..cut];
            return (stripped.trim().to_string(), scope);
        }
    }

    (trimmed.to_string(), SearchScope::Mixed)
}

pub(crate) fn is_followed_author_query(keywords: &str, scope: SearchScope) -> bool {
    scope == SearchScope::Author && keywords.trim().is_empty()
}

pub(crate) struct FollowedAuthorPage {
    pub(crate) items: Vec<SearchItem>,
    pub(crate) fetched_count: usize,
    pub(crate) has_more: Option<bool>,
    pub(crate) total_count: Option<usize>,
}

pub(crate) fn parse_followed_author_page(response: &ApiResponse) -> FollowedAuthorPage {
    let items = response
        .body
        .get("data")
        .and_then(|value| value.as_array())
        .or_else(|| {
            response
                .body
                .get("artists")
                .and_then(|value| value.as_array())
        })
        .or_else(|| {
            response
                .body
                .pointer("/result/artists")
                .and_then(|value| value.as_array())
        });

    let fetched_count = items.map(|values| values.len()).unwrap_or_default();
    let parsed_items = items
        .map(|values| parse_author_items(values))
        .unwrap_or_default();

    let has_more = ["/hasMore", "/more", "/data/hasMore", "/result/hasMore"]
        .iter()
        .find_map(|pointer| {
            response
                .body
                .pointer(pointer)
                .and_then(|value| value.as_bool())
        });
    let total_count = ["/count", "/data/count", "/result/count"]
        .iter()
        .find_map(|pointer| parse_usize_value(response.body.pointer(pointer)));

    FollowedAuthorPage {
        items: parsed_items,
        fetched_count,
        has_more,
        total_count,
    }
}

pub(crate) fn followed_author_has_more(page: &FollowedAuthorPage, next_offset: usize) -> bool {
    if page.fetched_count == 0 {
        return false;
    }

    if let Some(has_more) = page.has_more {
        return has_more;
    }

    if let Some(total_count) = page.total_count {
        return next_offset < total_count;
    }

    page.fetched_count >= SEARCH_RESULT_PAGE_SIZE
}

/// 混合搜索的辅助分区：请求失败或非 200 时退化为空分区（该分区不显示），
/// 不影响单曲分区，也不把整次搜索判为失败。
pub(crate) fn parse_optional_search_section(
    response: Result<ApiResponse>,
    scope: SearchScope,
) -> Vec<SearchItem> {
    match response {
        Ok(response) if response_code(&response) == 200 => parse_search_items(&response, scope),
        Ok(response) => {
            log::warn!(
                "{} 分区搜索失败({}): {}",
                scope.display_name(),
                response_code(&response),
                response_message(&response)
            );
            Vec::new()
        }
        Err(err) => {
            log::warn!("{} 分区搜索失败: {}", scope.display_name(), err);
            Vec::new()
        }
    }
}

/// 分区解析器：把 `result.<key>` 的数组转成条目。
pub(crate) type SearchSectionParser = fn(&[Value]) -> Vec<SearchItem>;

/// 按 scope 取对应分区并解析。`Mixed` 没有单一条目种类（由 `execute_mixed_search` 合并三段）。
pub(crate) fn parse_search_items(response: &ApiResponse, scope: SearchScope) -> Vec<SearchItem> {
    let Some(result) = response.body.get("result") else {
        return Vec::new();
    };

    let (key, parse): (&str, SearchSectionParser) = match scope {
        SearchScope::Mixed => return Vec::new(),
        SearchScope::Single => ("songs", parse_song_items),
        SearchScope::Album => ("albums", parse_album_items),
        SearchScope::Author => ("artists", parse_author_items),
        SearchScope::Playlist => ("playlists", parse_playlist_items),
    };

    result
        .get(key)
        .and_then(|value| value.as_array())
        .map(|items| parse(items))
        .unwrap_or_default()
}

pub(crate) fn parse_song_items(items: &[Value]) -> Vec<SearchItem> {
    let mut out = Vec::new();

    for item in items {
        let Some(name) = item.get("name").and_then(|value| value.as_str()) else {
            continue;
        };
        let artist = parse_artists(item).unwrap_or_else(|| "Unknown Artist".to_string());
        let duration = item
            .get("dt")
            .and_then(|value| value.as_i64())
            .or_else(|| item.get("duration").and_then(|value| value.as_i64()))
            .unwrap_or(0);

        out.push(SearchItem {
            kind: SearchItemKind::Song,
            left_label: format!("{} - {}", name, artist),
            right_label: format_duration(duration),
            song_id: parse_value_as_string(item.get("id")),
            album_id: None,
            playlist_id: None,
            artist_id: None,
            title: Some(name.to_string()),
            artist: Some(artist),
            album: item
                .pointer("/al/name")
                .and_then(|value| value.as_str())
                .map(|value| value.to_string()),
            cover_url: first_non_empty(item, &["/al/picUrl", "/album/picUrl"]),
            duration_ms: Some(duration),
            cover: CoverFetchState::default(),
        });
    }

    out
}

pub(crate) fn parse_album_items(items: &[Value]) -> Vec<SearchItem> {
    let mut out = Vec::new();

    for item in items {
        let Some(name) = item.get("name").and_then(|value| value.as_str()) else {
            continue;
        };

        let artist = item
            .pointer("/artist/name")
            .and_then(|value| value.as_str())
            .unwrap_or("Unknown Artist");
        let size = item
            .get("size")
            .and_then(|value| value.as_i64())
            .unwrap_or_default();

        out.push(SearchItem {
            kind: SearchItemKind::Album,
            left_label: format!("{} - {}", name, artist),
            right_label: format!("{} 首", size),
            song_id: None,
            album_id: parse_value_as_string(item.get("id")),
            playlist_id: None,
            artist_id: None,
            title: Some(name.to_string()),
            artist: Some(artist.to_string()),
            album: Some(name.to_string()),
            cover_url: first_non_empty(item, &["/picUrl", "/blurPicUrl"]),
            duration_ms: None,
            cover: CoverFetchState::default(),
        });
    }

    out
}

pub(crate) fn parse_author_items(items: &[Value]) -> Vec<SearchItem> {
    let mut out = Vec::new();

    for item in items {
        let Some(name) = item.get("name").and_then(|value| value.as_str()) else {
            continue;
        };

        let album_size = item
            .get("albumSize")
            .and_then(|value| value.as_i64())
            .unwrap_or_default();

        out.push(SearchItem {
            kind: SearchItemKind::Artist,
            left_label: name.to_string(),
            right_label: format!("{} 张专辑", album_size),
            song_id: None,
            album_id: None,
            playlist_id: None,
            artist_id: parse_value_as_string(item.get("id")),
            title: None,
            artist: Some(name.to_string()),
            album: None,
            cover_url: first_non_empty(item, &["/picUrl", "/img1v1Url", "/avatarUrl"]),
            duration_ms: None,
            cover: CoverFetchState::default(),
        });
    }

    out
}

pub(crate) fn parse_playlist_items(items: &[Value]) -> Vec<SearchItem> {
    let mut out = Vec::new();

    for item in items {
        let Some(name) = item.get("name").and_then(|value| value.as_str()) else {
            continue;
        };

        let creator = item
            .pointer("/creator/nickname")
            .and_then(|value| value.as_str())
            .unwrap_or("Unknown User");
        let count = item
            .get("trackCount")
            .and_then(|value| value.as_i64())
            .unwrap_or_default();

        out.push(SearchItem {
            kind: SearchItemKind::Playlist,
            left_label: format!("{} - {}", name, creator),
            right_label: format!("{} 首", count),
            song_id: None,
            album_id: None,
            playlist_id: parse_value_as_string(item.get("id")),
            artist_id: None,
            title: None,
            artist: None,
            album: None,
            cover_url: first_non_empty(item, &["/coverImgUrl", "/picUrl"]),
            duration_ms: None,
            cover: CoverFetchState::default(),
        });
    }

    out
}

impl App {
    pub(crate) async fn advance_search_focus(&mut self) {
        if self.search.is_empty() {
            return;
        }

        if self.search.focus_next() {
            if self.search.focused_idx + 1 == self.search.results().len() {
                match self.load_more_search_results().await {
                    Ok(_) => {}
                    Err(err) => {
                        self.search.status_line = format!("加载更多失败: {}", err);
                    }
                }
            }
            return;
        }

        let before = self.search.results().len();
        match self.load_more_search_results().await {
            Ok(added) if added > 0 => {
                self.search.set_focus(before);
            }
            Ok(_) => {}
            Err(err) => {
                self.search.status_line = format!("加载更多失败: {}", err);
            }
        }
    }

    /// 搜索页打开作者：立即落占位作者页 + 派发后台拉取（结果由 `tick_author_fetch` 搬进来）。
    pub(crate) fn open_focused_search_author(&mut self) {
        let (artist_id, title, fallback_cover_url) = {
            let Some(item) = self.search.results().get(self.search.focused_idx) else {
                return;
            };

            let Some(artist_id) = item.artist_id.clone() else {
                self.search.status_line = self
                    .lang_text(
                        "当前结果缺少作者 ID，无法打开作者页",
                        "The current result has no author ID",
                    )
                    .to_string();
                return;
            };

            (artist_id, item.left_label.clone(), item.cover_url.clone())
        };

        self.browse.author = AuthorState::placeholder(
            title,
            self.lang_text("正在加载作者…", "Loading artist…")
                .to_string(),
        );
        self.playlist_section_return_snapshot = None;
        self.author_return_page = Page::Search;
        self.page = Page::Author;

        let fut = fetch_author_page_by_id(
            self.api.clone(),
            self.config.language,
            artist_id,
            fallback_cover_url,
        );
        let fut: AuthorFetchTask = Box::pin(async move { Some(fut.await) });
        self.browse.author_fetch = Some(spawn_shared(fut, self.wake.clone()));
    }

    /// 搜索页打开专辑：立即落占位歌单页 + 派发后台拉取（结果由 `tick_playlist_fetch` 搬进来）。
    pub(crate) fn open_focused_search_album(&mut self) {
        let (album_id, title, fallback_cover_url) = {
            let Some(item) = self.search.results().get(self.search.focused_idx) else {
                return;
            };

            let Some(album_id) = item.album_id.clone() else {
                self.search.status_line = self
                    .lang_text(
                        "当前结果缺少专辑 ID，无法打开专辑页",
                        "The current result has no album ID",
                    )
                    .to_string();
                return;
            };

            (
                album_id,
                item.title
                    .clone()
                    .unwrap_or_else(|| item.left_label.clone()),
                item.cover_url.clone(),
            )
        };

        self.browse.playlist.cancel_pagination();
        self.browse.playlist = PlaylistState::placeholder(
            title,
            self.lang_text("正在加载专辑…", "Loading album…")
                .to_string(),
        );
        self.playlist_section_return_snapshot = None;
        self.playlist_return_page = Page::Search;
        self.page = Page::Playlist;

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
    }

    /// 搜索页打开歌单：立即落占位歌单页 + 派发后台拉取（结果由 `tick_playlist_fetch` 搬进来）。
    pub(crate) fn open_focused_search_playlist(&mut self) {
        let (playlist_id, title, fallback_cover_url) = {
            let Some(item) = self.search.results().get(self.search.focused_idx) else {
                return;
            };

            let Some(playlist_id) = item.playlist_id.clone() else {
                self.search.status_line = self
                    .lang_text(
                        "当前结果缺少歌单 ID，无法打开歌单页",
                        "The current result has no playlist ID",
                    )
                    .to_string();
                return;
            };

            (playlist_id, item.left_label.clone(), item.cover_url.clone())
        };

        self.browse.playlist.cancel_pagination();
        self.browse.playlist = PlaylistState::placeholder(
            title,
            self.lang_text("正在加载歌单…", "Loading playlist…")
                .to_string(),
        );
        self.playlist_section_return_snapshot = None;
        self.playlist_return_page = Page::Search;
        self.page = Page::Playlist;

        let fut = fetch_playlist_page(
            self.api.clone(),
            self.config.language,
            playlist_id,
            fallback_cover_url,
            self.browse.home_sidebar.liked_playlist_id.clone(),
            self.browse.home_sidebar.user_id.clone(),
        );
        let fut: PlaylistFetchTask = Box::pin(async move { Some(fut.await) });
        self.downloads.page_kind = PlaylistPageKind::Playlist;
        self.browse.playlist_fetch = Some(PlaylistFetchSlot {
            kind: PlaylistPageKind::Playlist,
            future: spawn_shared(fut, self.wake.clone()),
        });
    }

    pub(crate) async fn activate_focused_search_result(&mut self) {
        let Some(kind) = self
            .search
            .results()
            .get(self.search.focused_idx)
            .map(|item| item.kind)
        else {
            return;
        };

        match kind {
            SearchItemKind::Song => self.play_focused_search_track().await,
            SearchItemKind::Album => self.open_focused_search_album(),
            SearchItemKind::Artist => self.open_focused_search_author(),
            SearchItemKind::Playlist => self.open_focused_search_playlist(),
        }
    }

    pub(crate) async fn handle_search_box_key(&mut self, key: KeyEvent) {
        if key.modifiers.contains(KeyModifiers::CONTROL)
            && matches!(key.code, KeyCode::Char('s') | KeyCode::Char('S'))
        {
            self.close_overlay();
            return;
        }

        match key.code {
            KeyCode::Esc => self.close_overlay(),
            KeyCode::Enter => self.execute_search_from_box().await,
            KeyCode::Backspace => self.input.backspace(),
            KeyCode::Delete => self.input.delete(),
            KeyCode::Left => self.input.move_cursor(-1),
            KeyCode::Right => self.input.move_cursor(1),
            KeyCode::Home => {
                self.input.search_box_cursor = 0;
            }
            KeyCode::End => {
                self.input.search_box_cursor = char_count(&self.input.search_box_input);
            }
            KeyCode::Char(ch)
                if (key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT) =>
            {
                self.input.insert(ch);
            }
            _ => {}
        }

        self.input.search_box_cursor = self
            .input
            .search_box_cursor
            .min(char_count(&self.input.search_box_input));
    }

    pub(crate) fn handle_search_box_click(&mut self, col: u16, row: u16) {
        let Ok((term_w, term_h)) = crossterm::terminal::size() else {
            return;
        };
        if term_w < 20 || term_h < 2 {
            return;
        }

        let visible_h = ((self.input.search_motion.value()
            * f32::from(crate::ui::search_box::TARGET_HEIGHT))
        .round() as u16)
            .min(term_h);
        if visible_h < crate::ui::search_box::TARGET_HEIGHT {
            return;
        }

        let width = (term_w / 2).max(24).min(term_w.saturating_sub(2));
        let area_x = term_w.saturating_sub(width) / 2;
        let area_y = 0_u16;
        let area = HitRect {
            x: area_x,
            y: area_y,
            width,
            height: visible_h,
        };

        if !area.contains(col, row) {
            return;
        }

        let inner_x = area.x.saturating_add(1);
        let inner_y = area.y.saturating_add(1);
        let inner_w = area.width.saturating_sub(2);
        if inner_w == 0 || row != inner_y {
            return;
        }

        if col <= inner_x {
            self.input.search_box_cursor = 0;
            return;
        }

        let max_col = inner_x.saturating_add(inner_w).saturating_sub(1);
        if col >= max_col {
            self.input.search_box_cursor = char_count(&self.input.search_box_input);
            return;
        }

        let rel = col.saturating_sub(inner_x);
        self.input.search_box_cursor =
            char_index_for_display_column(&self.input.search_box_input, rel);
    }

    pub(crate) async fn handle_search_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc | KeyCode::Left => {
                self.page = self.search_return_page;
            }
            KeyCode::Tab | KeyCode::Down => {
                self.advance_search_focus().await;
            }
            KeyCode::BackTab | KeyCode::Up => {
                let _ = self.search.focus_prev();
            }
            KeyCode::Enter => self.activate_focused_search_result().await,
            _ => {}
        }
    }

    pub(crate) fn open_search_box(&mut self) {
        if self.page != Page::Search {
            self.search_return_page = Page::Home;
        }
        self.input.set_text(self.search.query.clone());
        self.input.search_motion.set(
            true,
            Instant::now(),
            SEARCH_BOX_ANIM_DURATION,
            Curve::EaseOut,
        );
        self.overlay = Some(Overlay::SearchBox);
    }

    pub(crate) async fn execute_search_from_box(&mut self) {
        let raw_query = self.input.search_box_input.trim().to_string();
        let (keywords, filter) = parse_search_input(&raw_query);
        if keywords.is_empty() && !is_followed_author_query(&keywords, filter) {
            self.search.status_line = self
                .lang_text("请输入搜索关键词", "Please enter search keywords")
                .to_string();
            return;
        }

        self.search.query = raw_query;
        if let Err(err) = self.execute_search().await {
            self.search.status_line = format!("搜索失败: {}", err);
            self.search.set_results(Vec::new(), 0, false);
        }
        self.playlist_section_return_snapshot = None;
        self.page = Page::Search;
        self.close_overlay();
    }

    pub(crate) async fn execute_search(&mut self) -> Result<()> {
        let (keywords, scope) = parse_search_input(&self.search.query);
        let followed_author_query = is_followed_author_query(&keywords, scope);
        if keywords.is_empty() && !followed_author_query {
            self.search.status_line = "请输入搜索关键词".to_string();
            self.search.set_results(Vec::new(), 0, false);
            return Ok(());
        }

        self.search.scope = scope;
        self.search.next_offset = 0;
        self.search.has_more = true;

        if scope == SearchScope::Mixed {
            return self.execute_mixed_search(&keywords).await;
        }

        let response = if followed_author_query {
            self.api.artist_sublist(SEARCH_RESULT_PAGE_SIZE, 0).await?
        } else {
            let search_type = scope.search_type().unwrap_or(1);
            self.api
                .search(&keywords, search_type, SEARCH_RESULT_PAGE_SIZE, 0)
                .await?
        };
        let code = response_code(&response);
        if code != 200 {
            return Err(anyhow!(
                "请求失败({}): {}",
                code,
                response_message(&response)
            ));
        }

        if followed_author_query {
            let page = parse_followed_author_page(&response);
            let count = page.items.len();
            let next_offset = page.fetched_count;
            let has_more = followed_author_has_more(&page, next_offset);
            let mut items = page.items;
            self.load_search_item_covers(&mut items);
            self.search.set_results(items, next_offset, has_more);
            self.search.status_line = format!("{} 搜索完成，共 {} 条", scope.display_name(), count);
            return Ok(());
        }

        let mut items = parse_search_items(&response, scope);
        let count = items.len();
        self.load_search_item_covers(&mut items);
        self.search
            .set_results(items, count, count >= SEARCH_RESULT_PAGE_SIZE);
        self.search.status_line = format!("{} 搜索完成，共 {} 条", scope.display_name(), count);
        Ok(())
    }

    /// 无后缀搜索：并发拉取作者 / 歌单 / 单曲，按 作者 → 歌单 → 单曲 拼接。
    /// 辅助分区（作者 / 歌单）失败只让该分区为空，不影响单曲结果；只有分页游标跟着单曲走。
    pub(crate) async fn execute_mixed_search(&mut self, keywords: &str) -> Result<()> {
        let mut artists_api = self.api.clone();
        let mut playlists_api = self.api.clone();
        let (songs, artists, playlists) = futures::join!(
            self.api.search(keywords, 1, SEARCH_RESULT_PAGE_SIZE, 0),
            artists_api.search(keywords, 100, MIXED_AUX_RESULT_LIMIT, 0),
            playlists_api.search(keywords, 1000, MIXED_AUX_RESULT_LIMIT, 0),
        );

        let songs = songs?;
        let code = response_code(&songs);
        if code != 200 {
            let message = response_message(&songs);
            return Err(anyhow!("请求失败({}): {}", code, message));
        }

        let mut items = parse_optional_search_section(artists, SearchScope::Author);
        items.extend(parse_optional_search_section(
            playlists,
            SearchScope::Playlist,
        ));
        let song_items = parse_search_items(&songs, SearchScope::Single);
        let song_count = song_items.len();
        items.extend(song_items);

        self.load_search_item_covers(&mut items);
        let total = items.len();
        self.search
            .set_results(items, song_count, song_count >= SEARCH_RESULT_PAGE_SIZE);
        self.search.status_line = format!("搜索完成，共 {} 条", total);
        Ok(())
    }

    /// 作者条目要渲染头像：结果落定时一次性发起封面请求（复用封面管线）。
    pub(crate) fn load_search_item_covers(&self, items: &mut [SearchItem]) {
        for item in items.iter_mut() {
            if item.kind != SearchItemKind::Artist || item.cover.url.is_some() {
                continue;
            }
            if let Some(url) = item.cover_url.clone() {
                item.cover.load(self.api.clone(), url);
            }
        }
    }

    pub(crate) async fn load_more_search_results(&mut self) -> Result<usize> {
        if !self.search.has_more {
            return Ok(0);
        }

        let (keywords, scope) = parse_search_input(&self.search.query);
        let followed_author_query = is_followed_author_query(&keywords, scope);
        if keywords.is_empty() && !followed_author_query {
            return Ok(0);
        }

        // 混合搜索只有末尾的单曲分区可继续分页；辅助分区固定取最相关若干条。
        let search_type = if scope == SearchScope::Mixed {
            1
        } else {
            scope.search_type().unwrap_or(1)
        };
        let response = if followed_author_query {
            self.api
                .artist_sublist(SEARCH_RESULT_PAGE_SIZE, self.search.next_offset)
                .await?
        } else {
            self.api
                .search(
                    &keywords,
                    search_type,
                    SEARCH_RESULT_PAGE_SIZE,
                    self.search.next_offset,
                )
                .await?
        };
        let code = response_code(&response);
        if code != 200 {
            return Err(anyhow!(
                "请求失败({}): {}",
                code,
                response_message(&response)
            ));
        }

        if followed_author_query {
            let mut page = parse_followed_author_page(&response);
            // 追加的条目同样要发起头像请求，否则第二页起的作者卡片没有头像。
            self.load_search_item_covers(&mut page.items);
            let fetched_count = page.fetched_count;
            let added = page.items.len();
            let next_offset = self.search.next_offset.saturating_add(fetched_count);
            let has_more = followed_author_has_more(&page, next_offset);
            self.search.append_results(page.items);
            self.search.next_offset = next_offset;
            self.search.has_more = has_more;

            if added == 0 {
                if self.search.has_more {
                    self.search.status_line =
                        format!("{} 已加载 {} 条", scope.display_name(), self.search.len());
                } else {
                    self.search.status_line = format!(
                        "{} 搜索结果已全部加载，共 {} 条",
                        scope.display_name(),
                        self.search.len()
                    );
                }
                return Ok(0);
            }

            self.search.status_line =
                format!("{} 已加载 {} 条", scope.display_name(), self.search.len());
            return Ok(added);
        }

        // 混合搜索的分页页就是单曲分区。
        let page_scope = match scope {
            SearchScope::Mixed => SearchScope::Single,
            other => other,
        };
        let mut items = parse_search_items(&response, page_scope);
        self.load_search_item_covers(&mut items);
        let added = self.search.append_results(items);
        self.search.next_offset = self.search.next_offset.saturating_add(added);
        self.search.has_more = added >= SEARCH_RESULT_PAGE_SIZE;

        if added == 0 {
            self.search.has_more = false;
            self.search.status_line = format!(
                "{} 搜索结果已全部加载，共 {} 条",
                scope.display_name(),
                self.search.len()
            );
            return Ok(0);
        }

        self.search.status_line =
            format!("{} 已加载 {} 条", scope.display_name(), self.search.len());
        Ok(added)
    }
}



/// 搜索页控制器：持有结果、焦点、分页游标和渲染布局索引。
pub(crate) struct SearchController {
    pub query: String,
    pub focused_idx: usize,
    results: Vec<SearchItem>,
    pub status_line: String,
    pub scope: SearchScope,
    pub next_offset: usize,
    pub has_more: bool,
    pub scroll_rows: usize,
    view_rows: usize,
    card_mode: bool,
    compact_layout_index: Vec<usize>,
    card_layout_index: Vec<usize>,
    compact_total_rows: usize,
    card_total_rows: usize,
    item_ordinals: Vec<usize>,
    generation: u64,
}

impl Default for SearchController {
    fn default() -> Self {
        Self {
            query: String::new(),
            focused_idx: 0,
            results: Vec::new(),
            status_line: "输入关键词后按 Enter 搜索".to_string(),
            scope: SearchScope::Mixed,
            next_offset: 0,
            has_more: false,
            scroll_rows: 0,
            view_rows: 1,
            card_mode: false,
            compact_layout_index: Vec::new(),
            card_layout_index: Vec::new(),
            compact_total_rows: 0,
            card_total_rows: 0,
            item_ordinals: Vec::new(),
            generation: next_list_generation(),
        }
    }
}

impl SearchController {
    fn rebuild_layout_index(&mut self) {
        self.compact_layout_index.clear();
        self.card_layout_index.clear();
        self.item_ordinals.clear();
        self.compact_layout_index.reserve(self.results.len());
        self.card_layout_index.reserve(self.results.len());
        self.item_ordinals.reserve(self.results.len());

        let mut compact_row = 0usize;
        let mut card_row = 0usize;
        let mut counts = [0usize; 4];
        for index in 0..self.results.len() {
            let kind = self.results[index].kind;
            let divider = usize::from(index > 0 && self.results[index - 1].kind != kind);
            self.compact_layout_index.push(compact_row);
            self.card_layout_index.push(card_row);
            let count = match kind {
                SearchItemKind::Song => &mut counts[0],
                SearchItemKind::Album => &mut counts[1],
                SearchItemKind::Artist => &mut counts[2],
                SearchItemKind::Playlist => &mut counts[3],
            };
            *count += 1;
            self.item_ordinals.push(*count);
            compact_row = compact_row.saturating_add(divider + kind.rows(false));
            card_row = card_row.saturating_add(divider + kind.rows(true));
        }
        self.compact_total_rows = compact_row;
        self.card_total_rows = card_row;
    }

    fn layout_index(&self) -> &[usize] {
        if self.card_mode {
            &self.card_layout_index
        } else {
            &self.compact_layout_index
        }
    }

    fn row_span(&self, index: usize) -> usize {
        let kind = self.results[index].kind;
        let divider = usize::from(index > 0 && self.results[index - 1].kind != kind);
        divider + kind.rows(self.card_mode)
    }

    pub fn item_start_row(&self, index: usize) -> usize {
        self.layout_index()
            .get(index)
            .copied()
            .unwrap_or(self.total_rows())
    }

    pub fn item_end_row(&self, index: usize) -> usize {
        self.item_start_row(index) + self.row_span(index)
    }

    pub fn item_ordinal(&self, index: usize) -> usize {
        self.item_ordinals.get(index).copied().unwrap_or(1)
    }

    pub fn divider_rows(&self, index: usize) -> usize {
        usize::from(index > 0 && self.results[index - 1].kind != self.results[index].kind)
    }

    fn total_rows(&self) -> usize {
        if self.card_mode {
            self.card_total_rows
        } else {
            self.compact_total_rows
        }
    }

    fn max_scroll_rows(&self) -> usize {
        self.total_rows().saturating_sub(self.view_rows.max(1))
    }

    fn clamp_scroll(&mut self) {
        self.scroll_rows = self.scroll_rows.min(self.max_scroll_rows());
    }

    fn ensure_focus_visible(&mut self) {
        if self.results.is_empty() {
            self.focused_idx = 0;
            self.scroll_rows = 0;
            return;
        }
        self.focused_idx = self.focused_idx.min(self.results.len() - 1);
        let view = self.view_rows.max(1);
        let start = self.item_start_row(self.focused_idx);
        let end = self.item_end_row(self.focused_idx);
        if end > self.scroll_rows.saturating_add(view) {
            self.scroll_rows = end - view;
        }
        if start < self.scroll_rows {
            self.scroll_rows = start;
        }
        self.clamp_scroll();
    }

    pub fn set_viewport(&mut self, view_rows: usize, card_mode: bool) {
        self.view_rows = view_rows.max(1);
        self.card_mode = card_mode;
        if self.compact_layout_index.len() != self.results.len()
            || self.card_layout_index.len() != self.results.len()
        {
            self.rebuild_layout_index();
        }
        self.ensure_focus_visible();
    }

    pub fn effective_scroll_rows(&self) -> usize {
        self.scroll_rows.min(self.max_scroll_rows())
    }

    /// Returns only entries intersecting the row viewport, using cached starts.
    pub fn visible_range(&self, top: usize, bottom: usize) -> Range<usize> {
        let starts = self.layout_index();
        let mut first = 0usize;
        let mut high = starts.len();
        while first < high {
            let mid = first + (high - first) / 2;
            if self.item_end_row(mid) <= top {
                first = mid + 1;
            } else {
                high = mid;
            }
        }
        let mut last = first;
        high = starts.len();
        while last < high {
            let mid = last + (high - last) / 2;
            if starts[mid] < bottom {
                last = mid + 1;
            } else {
                high = mid;
            }
        }
        first..last
    }

    pub fn page_items(&self) -> usize {
        let top = self.effective_scroll_rows();
        let bottom = top.saturating_add(self.view_rows.max(1));
        self.visible_range(top, bottom).len().max(1)
    }

    pub fn set_focus(&mut self, index: usize) {
        if self.results.is_empty() {
            self.focused_idx = 0;
            self.scroll_rows = 0;
            return;
        }
        self.focused_idx = index.min(self.results.len() - 1);
        self.ensure_focus_visible();
    }

    pub fn focus_next(&mut self) -> bool {
        if self.results.is_empty() || self.focused_idx + 1 >= self.results.len() {
            return false;
        }
        self.focused_idx += 1;
        self.ensure_focus_visible();
        true
    }

    pub fn focus_prev(&mut self) -> bool {
        if self.results.is_empty() || self.focused_idx == 0 {
            return false;
        }
        self.focused_idx -= 1;
        self.ensure_focus_visible();
        true
    }

    pub fn set_results(&mut self, results: Vec<SearchItem>, next_offset: usize, has_more: bool) {
        self.results = results;
        self.focused_idx = 0;
        self.next_offset = next_offset;
        self.has_more = has_more;
        self.scroll_rows = 0;
        self.rebuild_layout_index();
        self.generation = next_list_generation();
        self.ensure_focus_visible();
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn append_results(&mut self, mut results: Vec<SearchItem>) -> usize {
        let added = results.len();
        self.results.append(&mut results);
        self.rebuild_layout_index();
        self.generation = next_list_generation();
        self.clamp_scroll();
        added
    }
    pub fn results(&self) -> &[SearchItem] {
        &self.results
    }

    pub fn cover_mut(&mut self, index: usize) -> &mut super::CoverFetchState {
        &mut self.results[index].cover
    }

    pub fn len(&self) -> usize {
        self.results.len()
    }

    pub fn is_empty(&self) -> bool {
        self.results.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{CoverFetchState, SearchItem};

    fn item(kind: SearchItemKind, label: &str) -> SearchItem {
        SearchItem {
            kind,
            left_label: label.to_string(),
            right_label: String::new(),
            song_id: None,
            album_id: None,
            playlist_id: None,
            artist_id: None,
            title: None,
            artist: None,
            album: None,
            cover_url: None,
            duration_ms: None,
            cover: CoverFetchState::default(),
        }
    }

    #[test]
    fn mixed_sections_cache_rows_and_ordinals() {
        let mut state = SearchController::default();
        state.set_results(
            vec![
                item(SearchItemKind::Artist, "a1"),
                item(SearchItemKind::Artist, "a2"),
                item(SearchItemKind::Song, "s1"),
                item(SearchItemKind::Playlist, "p1"),
            ],
            0,
            false,
        );
        state.set_viewport(20, true);
        assert_eq!(state.item_start_row(0), 0);
        assert_eq!(state.item_start_row(1), 4);
        assert_eq!(state.item_start_row(2), 8);
        assert_eq!(state.item_start_row(3), 10);
        assert_eq!(state.item_ordinal(2), 1);
        assert_eq!(state.item_ordinal(3), 1);
    }

    #[test]
    fn appended_song_section_tracks_card_and_compact_layouts() {
        let mut state = SearchController::default();
        state.set_results(vec![item(SearchItemKind::Artist, "a")], 1, true);
        state.set_viewport(8, true);
        assert_eq!(
            state.append_results(vec![item(SearchItemKind::Song, "s")]),
            1
        );
        assert_eq!(state.item_start_row(1) + state.divider_rows(1), 5);
        state.set_viewport(8, false);
        assert_eq!(state.item_start_row(1) + state.divider_rows(1), 2);
    }

    #[test]
    fn resize_and_focus_keep_visible_range_deterministic() {
        let mut state = SearchController::default();
        state.set_results(
            (0..8)
                .map(|i| item(SearchItemKind::Song, &format!("s{i}")))
                .collect(),
            0,
            false,
        );
        state.set_viewport(3, false);
        state.set_focus(6);
        assert_eq!(state.effective_scroll_rows(), 4);
        assert_eq!(state.visible_range(4, 7), 4..7);
        state.set_viewport(2, false);
        assert_eq!(state.effective_scroll_rows(), 5);
    }
}
