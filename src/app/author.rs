use super::*;

impl App {
    /// 搬运作者页的在途拉取（每帧调用，结果就绪才动状态）。
    pub(super) fn tick_author_fetch(&mut self) {
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

    pub(super) async fn play_focused_author_tile(&mut self) {
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

    pub(super) async fn handle_author_key(&mut self, key: KeyEvent) {
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
    pub(super) fn apply_author_fetch(&mut self, fetch: AuthorFetch) {
        self.browse.apply_author(fetch, &self.api);
    }

    /// 解析 `artist/*` 的回包（网络部分见 `fetch_artist_responses`）。
    ///
    /// 不借 `&mut self`：全屏页那条把整段解析连同请求一起丢给 `shot_and_share` 后台跑。
    pub(super) fn build_author_page(
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
