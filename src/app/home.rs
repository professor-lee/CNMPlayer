use super::App;
use super::Page;
use super::api::ApiState;
use super::cover_state::CoverFetchState;
use super::response::first_non_empty;
use super::response::parse_value_as_string;
use super::response::response_code;
use anyhow::Result;
use crossterm::event::KeyCode;
use crossterm::event::KeyEvent;
use crossterm::event::KeyModifiers;
use ncm_api::ApiResponse;
use serde_json::Value;

pub(crate) struct HomeTile {
    pub(crate) id: Option<String>,
    pub(crate) title: String,
    pub(crate) subtitle: String,
    pub(crate) cover: CoverFetchState,
}

impl HomeTile {
    fn placeholder_daily() -> Self {
        Self {
            id: Some(HOME_DAILY_RECOMMEND_TILE_ID.to_string()),
            title: "每日推荐".to_string(),
            subtitle: String::new(),
            cover: CoverFetchState::default(),
        }
    }

    fn from_recommendation(
        api: &ApiState,
        id: Option<String>,
        title: String,
        subtitle: String,
        cover_url: Option<String>,
    ) -> Self {
        let mut cover = CoverFetchState::default();
        if let Some(x) = cover_url {
            cover.load(api.clone(), x)
        }
        Self {
            id,
            title,
            subtitle,
            cover,
        }
    }
}

pub(crate) struct HomeState {
    pub(crate) focused_idx: usize,
    pub(super) columns: usize,
    pub(crate) tiles: Vec<HomeTile>,
    pub(crate) status_line: String,
    pub(super) scroll_row_offset: usize,
    pub(super) visible_rows: usize,
}

impl Default for HomeState {
    fn default() -> Self {
        Self {
            focused_idx: 0,
            columns: 1,
            tiles: vec![HomeTile::placeholder_daily()],
            status_line: "方向键/Tab 切换，Enter 进入".to_string(),
            scroll_row_offset: 0,
            visible_rows: 1,
        }
    }
}

impl HomeState {
    fn total_virtual_rows(&self) -> usize {
        if self.tiles.is_empty() {
            return 0;
        }

        let columns = self.columns.max(1);
        let last_virtual = home_tile_real_to_virtual_index(self.tiles.len() - 1, columns);
        last_virtual / columns + 1
    }

    pub(super) fn max_scroll_row_offset(&self) -> usize {
        self.total_virtual_rows()
            .saturating_sub(self.visible_rows.max(1))
    }

    pub(super) fn clamp_scroll_row_offset(&mut self) {
        self.scroll_row_offset = self.scroll_row_offset.min(self.max_scroll_row_offset());
    }

    pub(super) fn ensure_focus_visible(&mut self) {
        if self.tiles.is_empty() {
            self.focused_idx = 0;
            self.scroll_row_offset = 0;
            return;
        }

        self.focused_idx = self.focused_idx.min(self.tiles.len() - 1);
        let columns = self.columns.max(1);
        let focused_row = home_tile_real_to_virtual_index(self.focused_idx, columns) / columns;
        let visible_rows = self.visible_rows.max(1);

        if focused_row < self.scroll_row_offset {
            self.scroll_row_offset = focused_row;
        } else {
            let bottom_row = self
                .scroll_row_offset
                .saturating_add(visible_rows.saturating_sub(1));
            if focused_row > bottom_row {
                self.scroll_row_offset = focused_row.saturating_add(1).saturating_sub(visible_rows);
            }
        }

        self.clamp_scroll_row_offset();
    }

    pub(crate) fn set_columns(&mut self, columns: usize) {
        self.columns = columns.max(1);
        self.ensure_focus_visible();
    }

    pub(crate) fn set_visible_rows(&mut self, visible_rows: usize) {
        self.visible_rows = visible_rows.max(1);
        self.ensure_focus_visible();
    }

    pub(crate) fn effective_scroll_row_offset(&self) -> usize {
        self.scroll_row_offset.min(self.max_scroll_row_offset())
    }

    pub(super) fn set_tiles(&mut self, mut tiles: Vec<HomeTile>) {
        if tiles.is_empty() {
            tiles.push(HomeTile::placeholder_daily());
        }
        self.tiles = tiles;
        self.focused_idx = 0;
        self.scroll_row_offset = 0;
        self.ensure_focus_visible();
    }

    pub(super) fn focus_next(&mut self) {
        if self.tiles.is_empty() {
            return;
        }
        self.focused_idx = (self.focused_idx + 1) % self.tiles.len();
        self.ensure_focus_visible();
    }

    pub(super) fn focus_prev(&mut self) {
        if self.tiles.is_empty() {
            return;
        }
        self.focused_idx = if self.focused_idx == 0 {
            self.tiles.len() - 1
        } else {
            self.focused_idx - 1
        };
        self.ensure_focus_visible();
    }

    pub(super) fn focus_left(&mut self) {
        self.focus_prev();
    }

    pub(super) fn focus_right(&mut self) {
        self.focus_next();
    }

    pub(super) fn focus_up(&mut self) {
        if self.tiles.is_empty() {
            return;
        }

        let step = self.columns.max(1);
        let focused_virtual = home_tile_real_to_virtual_index(self.focused_idx, step);
        if focused_virtual < step {
            return;
        }

        let focused_row = focused_virtual / step;
        let target_virtual = focused_virtual - step;
        if let Some(target) =
            home_tile_virtual_to_real_index(target_virtual, step, self.tiles.len())
        {
            let is_top_edge = focused_row == self.scroll_row_offset;
            self.focused_idx = target;
            if is_top_edge && self.scroll_row_offset > 0 {
                self.scroll_row_offset -= 1;
            }
            self.ensure_focus_visible();
        }
    }

    pub(super) fn focus_down(&mut self) {
        if self.tiles.is_empty() {
            return;
        }

        let step = self.columns.max(1);
        let focused_virtual = home_tile_real_to_virtual_index(self.focused_idx, step);
        let focused_row = focused_virtual / step;
        let target_virtual = focused_virtual.saturating_add(step);

        if let Some(target) =
            home_tile_virtual_to_real_index(target_virtual, step, self.tiles.len())
        {
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
        }
    }
}

pub(super) const HOME_DAILY_RECOMMEND_TILE_ID: &str = "__cnm_daily_recommend_songs__";

pub(super) const HOME_PRIVATE_ROAM_TILE_ID: &str = "__cnm_private_roam__";

const HOME_PINNED_TITLES: [&str; 3] = ["每日推荐", "私人雷达", "私人漫游"];

fn home_tile_real_to_virtual_index(index: usize, columns: usize) -> usize {
    let cols = columns.max(1);
    if cols <= 3 || index < 3 {
        index
    } else {
        index.saturating_add(cols - 3)
    }
}

fn home_tile_virtual_to_real_index(
    virtual_index: usize,
    columns: usize,
    tile_len: usize,
) -> Option<usize> {
    let cols = columns.max(1);

    if cols <= 3 {
        return (virtual_index < tile_len).then_some(virtual_index);
    }

    if virtual_index < 3 {
        return (virtual_index < tile_len).then_some(virtual_index);
    }

    if virtual_index < cols {
        return None;
    }

    let real_index = virtual_index.saturating_sub(cols - 3);
    (real_index < tile_len).then_some(real_index)
}

struct RecommendCard {
    pub(super) id: Option<String>,
    pub(super) title: String,
    pub(super) subtitle: String,
    pub(super) cover_url: Option<String>,
}

pub(super) fn home_daily_song_items(body: &Value) -> Option<&[Value]> {
    body.pointer("/data/dailySongs")
        .and_then(|value| value.as_array().map(Vec::as_slice))
        .or_else(|| {
            body.get("dailySongs")
                .and_then(|value| value.as_array().map(Vec::as_slice))
        })
        .or_else(|| {
            body.pointer("/recommend")
                .and_then(|value| value.as_array().map(Vec::as_slice))
        })
        .or_else(|| {
            body.pointer("/data/recommend")
                .and_then(|value| value.as_array().map(Vec::as_slice))
        })
}

fn normalize_home_pinned_title(title: &str) -> Option<&'static str> {
    let compact: String = title.chars().filter(|ch| !ch.is_whitespace()).collect();

    if compact.contains("私人漫游") {
        return Some("私人漫游");
    }
    if compact.contains("私人雷达") {
        return Some("私人雷达");
    }
    if compact.contains("每日推荐") {
        return Some("每日推荐");
    }

    None
}

/// 歌单封面（私人雷达 tile 用）。
async fn fetch_playlist_cover_url(api: &mut ApiState, playlist_id: &str) -> Option<String> {
    let response = api.playlist_detail(playlist_id).await.ok()?;
    if response_code(&response) != 200 {
        return None;
    }

    if let Some(playlist) = response.body.get("playlist")
        && let Some(cover_url) = first_non_empty(playlist, &["/coverImgUrl", "/picUrl"])
    {
        return Some(cover_url);
    }

    response
        .body
        .pointer("/playlist/tracks")
        .and_then(|value| value.as_array())
        .and_then(|items| items.first())
        .and_then(|track| first_non_empty(track, &["/al/picUrl", "/album/picUrl"]))
        .map(|s| s.to_string())
}

/// 首页推荐 tile：每日推荐 + 推荐歌单卡片，已按固定顺序排好。
pub(super) async fn fetch_home_tiles(api: &mut ApiState, show_more: bool) -> Vec<HomeTile> {
    let mut daily_tile = HomeTile::placeholder_daily();
    if let Ok(response) = api.recommend_songs().await
        && response_code(&response) == 200
        && let Some(songs) = home_daily_song_items(&response.body)
        && let Some(cover_url) = songs
            .iter()
            .find_map(|item| first_non_empty(item, &["/al/picUrl", "/album/picUrl"]))
    {
        daily_tile.cover.load(api.clone(), cover_url);
    }

    let mut cards = Vec::new();

    if let Ok(response) = api.recommend_resource().await {
        cards = parse_recommend_cards(&response, 24);
    }

    if cards.is_empty()
        && let Ok(response) = api.personalized(24).await
    {
        cards = parse_personalized_cards(&response, 24);
    }

    let mut tiles = Vec::with_capacity(cards.len().saturating_add(1));
    tiles.push(daily_tile);

    for card in cards {
        let pinned_title = normalize_home_pinned_title(&card.title);
        if pinned_title == Some("每日推荐") {
            continue;
        }

        let mut tile =
            HomeTile::from_recommendation(api, card.id, card.title, card.subtitle, card.cover_url);

        if pinned_title == Some("私人雷达")
            && let Some(playlist_id) = tile.id.clone()
            && let Some(cover_url) = fetch_playlist_cover_url(api, &playlist_id).await
        {
            tile.cover.load(api.clone(), cover_url);
        }

        if pinned_title == Some("私人漫游") {
            tile.id = Some(HOME_PRIVATE_ROAM_TILE_ID.to_string());
        }

        tiles.push(tile);
    }

    prioritize_home_tiles(api, tiles, show_more)
}

fn prioritize_home_tiles(
    api: &ApiState,
    mut tiles: Vec<HomeTile>,
    show_more: bool,
) -> Vec<HomeTile> {
    let mut pinned = Vec::with_capacity(HOME_PINNED_TITLES.len());

    for target in HOME_PINNED_TITLES {
        if let Some(index) = tiles
            .iter()
            .position(|tile| normalize_home_pinned_title(&tile.title) == Some(target))
        {
            let mut tile = tiles.remove(index);
            tile.title = target.to_string();
            tile.subtitle.clear();
            pinned.push(tile);
            continue;
        }

        if target == "每日推荐" {
            pinned.push(HomeTile::placeholder_daily());
        } else {
            let tile_id = if target == "私人漫游" {
                Some(HOME_PRIVATE_ROAM_TILE_ID.to_string())
            } else {
                None
            };
            pinned.push(HomeTile::from_recommendation(
                api,
                tile_id,
                target.to_string(),
                String::new(),
                None,
            ));
        }
    }

    pinned.extend(tiles);

    if !show_more && pinned.len() > HOME_PINNED_TITLES.len() {
        pinned.truncate(HOME_PINNED_TITLES.len());
    }

    if pinned.is_empty() {
        pinned.push(HomeTile::placeholder_daily());
    }

    pinned
}

fn parse_recommend_cards(response: &ApiResponse, limit: usize) -> Vec<RecommendCard> {
    response
        .body
        .get("recommend")
        .and_then(|value| value.as_array())
        .map(|items| {
            items
                .iter()
                .filter_map(|item| {
                    let title = item
                        .get("name")
                        .and_then(|value| value.as_str())?
                        .to_string();
                    let subtitle = first_non_empty(item, &["/copywriter", "/creator/nickname"])
                        .unwrap_or_else(|| "推荐歌单".to_string());
                    Some(RecommendCard {
                        id: parse_value_as_string(item.get("id")),
                        title,
                        subtitle,
                        cover_url: first_non_empty(item, &["/picUrl", "/coverImgUrl"]),
                    })
                })
                .take(limit.max(1))
                .collect()
        })
        .unwrap_or_default()
}

fn parse_personalized_cards(response: &ApiResponse, limit: usize) -> Vec<RecommendCard> {
    response
        .body
        .get("result")
        .and_then(|value| value.as_array())
        .map(|items| {
            items
                .iter()
                .filter_map(|item| {
                    let title = item
                        .get("name")
                        .and_then(|value| value.as_str())?
                        .to_string();
                    let subtitle = first_non_empty(item, &["/copywriter", "/creator/nickname"])
                        .unwrap_or_else(|| "推荐歌单".to_string());
                    Some(RecommendCard {
                        id: parse_value_as_string(item.get("id")),
                        title,
                        subtitle,
                        cover_url: first_non_empty(item, &["/picUrl", "/coverImgUrl"]),
                    })
                })
                .take(limit.max(1))
                .collect()
        })
        .unwrap_or_default()
}

impl App {
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
                self.browse.home.status_line = format!("已打开 {}", title);
                self.open_browse_page(Page::Playlist);
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
