use super::*;

impl App {
    pub(super) async fn advance_search_focus(&mut self) {
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
    pub(super) fn open_focused_search_author(&mut self) {
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
    pub(super) fn open_focused_search_album(&mut self) {
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
    pub(super) fn open_focused_search_playlist(&mut self) {
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

    pub(super) async fn activate_focused_search_result(&mut self) {
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

    pub(super) async fn handle_search_box_key(&mut self, key: KeyEvent) {
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

    pub(super) fn handle_search_box_click(&mut self, col: u16, row: u16) {
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

    pub(super) async fn handle_search_key(&mut self, key: KeyEvent) {
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

    pub(super) fn open_search_box(&mut self) {
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

    pub(super) async fn execute_search_from_box(&mut self) {
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

    pub(super) async fn execute_search(&mut self) -> Result<()> {
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
    pub(super) async fn execute_mixed_search(&mut self, keywords: &str) -> Result<()> {
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
    pub(super) fn load_search_item_covers(&self, items: &mut [SearchItem]) {
        for item in items.iter_mut() {
            if item.kind != SearchItemKind::Artist || item.cover.url.is_some() {
                continue;
            }
            if let Some(url) = item.cover_url.clone() {
                item.cover.load(self.api.clone(), url);
            }
        }
    }

    pub(super) async fn load_more_search_results(&mut self) -> Result<usize> {
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
