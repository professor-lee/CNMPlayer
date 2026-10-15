use super::*;

impl App {
    pub(super) async fn toggle_home_sidebar(&mut self) {
        if self.page != Page::Home || self.overlay.is_some() {
            return;
        }
        if self.browse.home_sidebar.is_visible() {
            self.browse.home_sidebar.expanded = false;
            self.animate_home_sidebar();
            return;
        }
        self.browse.home_sidebar.expanded = true;
        self.animate_home_sidebar();
        if !self.browse.home_sidebar.created_playlists.is_empty()
            || !self.browse.home_sidebar.collected_playlists.is_empty()
        {
            self.browse.home_sidebar.reset_focus();
            return;
        }
        // 异步填充：立刻返回，动画照常跑，数据由 tick 搬入。
        if self.browse.home_sidebar_fetch.is_none() {
            self.browse.home_sidebar.loading = true;
            let fut = fetch_home_sidebar_playlists(self.api.clone(), self.config.language);
            let fut: HomeSidebarTask = Box::pin(async move { Some(fut.await) });
            self.browse.home_sidebar_fetch = Some(spawn_shared(fut, self.wake.clone()));
        }
    }

    /// 搬运侧边栏歌单的异步结果（每帧调用，结果就绪才动状态）。
    pub(super) fn tick_home_sidebar_fetch(&mut self) {
        let Some(result) = peek_shared_future(&self.browse.home_sidebar_fetch).cloned() else {
            return;
        };
        self.browse.home_sidebar_fetch = None;
        self.browse.home_sidebar.loading = false;

        match result {
            Ok(data) => {
                self.browse.home_sidebar.user_id = Some(data.user_id);
                self.browse.home_sidebar.liked_playlist_id = data.liked_playlist_id;
                self.browse.home_sidebar.user_name = data.user_name;
                self.browse.home_sidebar.created_playlists = data.created;
                self.browse.home_sidebar.collected_playlists = data.collected;
                self.browse.home_sidebar.created_has_more = data.created_more;
                self.browse.home_sidebar.collected_has_more = data.collected_more;
                self.browse.home_sidebar.created_next_offset =
                    self.browse.home_sidebar.created_playlists.len();
                self.browse.home_sidebar.collected_next_offset =
                    self.browse.home_sidebar.collected_playlists.len();
                self.browse.home_sidebar.clamp_focus();
                self.browse.home_sidebar.status_line = match self.config.language {
                    Language::Zh => format!(
                        "创建 {} 个，收藏 {} 个",
                        self.browse.home_sidebar.created_playlists.len(),
                        self.browse.home_sidebar.collected_playlists.len()
                    ),
                    Language::En => format!(
                        "{} created, {} collected",
                        self.browse.home_sidebar.created_playlists.len(),
                        self.browse.home_sidebar.collected_playlists.len()
                    ),
                };
                self.browse.home.status_line = self.browse.home_sidebar.status_line.clone();
                self.browse.home_sidebar.reset_focus();
            }
            Err(err) => {
                let text = format!(
                    "{}: {}",
                    self.lang_text("主页歌单加载失败", "Failed to load home playlists"),
                    err
                );
                self.browse.home_sidebar.status_line = text.clone();
                self.browse.home.status_line = text;
            }
        }
    }

    pub(super) fn tick_home_sidebar_page_fetch(&mut self) {
        let Some(result) = peek_shared_future(&self.browse.home_sidebar_page_fetch).cloned() else {
            return;
        };
        self.browse.home_sidebar_page_fetch = None;
        self.browse.home_sidebar.loading_more = false;

        match result {
            Ok(page) => {
                self.browse.home_sidebar.append_section_page(page);
                self.browse.home_sidebar.status_line = match self.config.language {
                    Language::Zh => format!(
                        "创建 {} 个，收藏 {} 个",
                        self.browse.home_sidebar.created_playlists.len(),
                        self.browse.home_sidebar.collected_playlists.len()
                    ),
                    Language::En => format!(
                        "{} created, {} collected",
                        self.browse.home_sidebar.created_playlists.len(),
                        self.browse.home_sidebar.collected_playlists.len()
                    ),
                };
            }
            Err(error) => {
                self.browse.home_sidebar.status_line = format!(
                    "{}: {error}",
                    self.lang_text("歌单分页加载失败", "Playlist page load failed")
                );
            }
        }
    }

    pub(super) fn maybe_fetch_home_sidebar_page(&mut self) {
        if !self.browse.home_sidebar.expanded
            || self.browse.home_sidebar.loading
            || self.browse.home_sidebar.loading_more
            || self.browse.home_sidebar_page_fetch.is_some()
        {
            return;
        }

        let section = self.browse.home_sidebar.focused_section;
        let len = self.browse.home_sidebar.section_len(section);
        if len == 0
            || self.browse.home_sidebar.focused_index + 1 < len
            || !self.browse.home_sidebar.section_has_more(section)
        {
            return;
        }
        let Some(user_id) = self.browse.home_sidebar.user_id.clone() else {
            return;
        };
        let offset = self.browse.home_sidebar.section_next_offset(section);
        self.browse.home_sidebar.loading_more = true;
        let fut = fetch_home_sidebar_page(
            self.api.clone(),
            user_id,
            section,
            offset,
            self.config.language,
        );
        let fut: HomeSidebarPageTask = Box::pin(async move { Some(fut.await) });
        self.browse.home_sidebar_page_fetch = Some(spawn_shared(fut, self.wake.clone()));
    }

    pub(super) async fn open_focused_home_sidebar_playlist(&mut self) {
        let (playlist_id, title) = {
            let Some(item) = self.browse.home_sidebar.focused_playlist() else {
                self.browse.home.status_line = self
                    .lang_text("侧边栏暂无可打开歌单", "No sidebar playlist to open")
                    .to_string();
                return;
            };

            let Some(playlist_id) = item.id.clone() else {
                self.browse.home.status_line = self
                    .lang_text(
                        "当前歌单缺少 ID，无法打开",
                        "The selected playlist has no ID",
                    )
                    .to_string();
                return;
            };

            (playlist_id, item.title.clone())
        };

        if self.is_liked_playlist(&playlist_id, Some(&title)) {
            let _ = self.refresh_liked_song_cache().await;
            self.refresh_now_playing_like_state();
        }

        self.browse.home.status_line =
            format!("{} {}", self.lang_text("正在加载", "Loading"), title);

        match self.load_playlist_detail(&playlist_id).await {
            Ok(()) => {
                self.playlist_return_page = Page::Home;
                self.playlist_section_return_snapshot = None;
                self.browse.home_sidebar.expanded = false;
                self.browse.home_sidebar.motion.set(
                    false,
                    Instant::now(),
                    SIDEBAR_ANIM_DURATION,
                    Curve::EaseOut,
                );
                self.page = Page::Playlist;
                self.browse.home.status_line =
                    format!("{} {}", self.lang_text("已打开", "Opened"), title);
            }
            Err(err) => {
                self.browse.home.status_line = format!(
                    "{}: {}",
                    self.lang_text("打开歌单失败", "Failed to open playlist"),
                    err
                );
            }
        }
    }

    pub(super) async fn handle_home_key(&mut self, key: KeyEvent) {
        if self.browse.home_sidebar.is_visible() {
            if key.modifiers.contains(KeyModifiers::CONTROL) {
                match key.code {
                    KeyCode::Up => {
                        self.browse.home_sidebar.switch_section_prev();
                        return;
                    }
                    KeyCode::Down => {
                        self.browse.home_sidebar.switch_section_next();
                        return;
                    }
                    _ => {}
                }
            }
            match key.code {
                KeyCode::Esc => {
                    self.browse.home_sidebar.expanded = false;
                    self.animate_home_sidebar();
                }
                KeyCode::Up | KeyCode::BackTab => self.browse.home_sidebar.focus_prev(),
                KeyCode::Down | KeyCode::Tab => {
                    let section = self.browse.home_sidebar.focused_section;
                    let len = self.browse.home_sidebar.section_len(section);
                    let at_end = len > 0 && self.browse.home_sidebar.focused_index + 1 == len;
                    if at_end && self.browse.home_sidebar.section_has_more(section) {
                        self.maybe_fetch_home_sidebar_page();
                    } else {
                        self.browse.home_sidebar.focus_next();
                        self.maybe_fetch_home_sidebar_page();
                    }
                }
                KeyCode::Enter => self.open_focused_home_sidebar_playlist().await,
                _ => {}
            }
            return;
        }

        match key.code {
            KeyCode::Tab => self.browse.home.focus_next(),
            KeyCode::BackTab => self.browse.home.focus_prev(),
            KeyCode::Left => self.browse.home.focus_left(),
            KeyCode::Right => self.browse.home.focus_right(),
            KeyCode::Up => self.browse.home.focus_up(),
            KeyCode::Down => self.browse.home.focus_down(),
            KeyCode::Enter => self.enter_home_tile().await,
            _ => {}
        }
    }

    pub(super) async fn enter_home_tile(&mut self) {
        if self.browse.home.tiles.is_empty() {
            return;
        }

        let focused = self
            .browse
            .home
            .focused_idx
            .min(self.browse.home.tiles.len() - 1);
        let title = self.browse.home.tiles[focused].title.clone();
        let Some(playlist_id) = self.browse.home.tiles[focused].id.clone() else {
            self.browse.home.status_line = "当前块暂无可用歌单".to_string();
            return;
        };

        self.browse.home.status_line = format!("正在加载 {}", title);
        let result = if playlist_id == HOME_DAILY_RECOMMEND_TILE_ID {
            self.load_daily_recommend_playlist().await
        } else if playlist_id == HOME_PRIVATE_ROAM_TILE_ID {
            self.load_private_roam_playlist().await
        } else {
            self.load_playlist_detail(&playlist_id).await
        };

        match result {
            Ok(()) => {
                self.playlist_return_page = Page::Home;
                self.playlist_section_return_snapshot = None;
                self.page = Page::Playlist;
                self.browse.home.status_line = format!("已打开 {}", title);
            }
            Err(err) => {
                self.browse.home.status_line = format!("打开歌单失败: {}", err);
            }
        }
    }

    pub(super) async fn load_home_recommendations(&mut self) -> Result<()> {
        let tiles = fetch_home_tiles(&mut self.api, self.config.home_more_recommend).await;
        self.apply_home_tiles(tiles);
        Ok(())
    }

    /// 应用首页推荐 tile（首页刷新与启动初始化共用）。
    pub(super) fn apply_home_tiles(&mut self, tiles: Vec<HomeTile>) {
        self.browse.home.set_tiles(tiles);
        // 私人漫游 tile 封面：未播放过时为首歌封面，播放后为最后播放歌曲的封面
        self.sync_home_roam_tile_cover();
        self.browse.home.status_line = self
            .lang_text(
                "方向键/Tab 切换，Enter 打开歌单",
                "Use arrows/Tab to focus, Enter to open playlist",
            )
            .to_string();
    }
}
