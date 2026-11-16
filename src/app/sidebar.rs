use super::App;
use super::Page;
use super::api::ApiState;
use super::auth::extract_current_user_id;
use super::auth::extract_current_user_name;
use super::auth::extract_liked_playlist_id;
use super::input_controller::HitRect;
use super::owned_task::SharedTask;
use super::owned_task::peek_shared_future;
use super::owned_task::spawn_shared;
use super::playlist::PlaylistFetchTask;
use super::playlist::PlaylistPageKind;
use super::playlist::fetch_album_page;
use super::playlist::fetch_playlist_page;
use super::response::parse_value_as_string;
use super::response::response_code;
use super::response::response_message;
use super::response::response_page_has_more;
use crate::data::config::Language;
use crate::render::motion::Curve;
use crate::render::motion::Toggle;
use crate::render::motion::Trail;
use anyhow::Result;
use ncm_api::ApiResponse;
use std::pin::Pin;
use std::time::Duration;
use std::time::Instant;

/// 侧边栏滑出动画时长（time-based，与帧率解耦）。主页与全屏播放页共用。
pub(crate) const SIDEBAR_ANIM_DURATION: Duration = Duration::from_millis(200);

const HOME_SIDEBAR_PLAYLIST_LIMIT: usize = 100;

type HomeSidebarTask = Pin<Box<dyn Future<Output = Option<Result<HomeSidebarFetch, String>>>>>;

/// 侧边栏歌单的一次拉取结果。异步任务不持有 `&mut App`，
/// 取完由 `tick_home_sidebar_fetch` 搬进状态。
#[derive(Clone)]
pub(super) struct HomeSidebarFetch {
    pub(super) user_id: String,
    pub(super) liked_playlist_id: Option<String>,
    pub(super) user_name: String,
    pub(super) created: Vec<HomeSidebarPlaylist>,
    pub(super) collected: Vec<HomeSidebarPlaylist>,
    created_more: bool,
    collected_more: bool,
    /// 收藏专辑（含 EP、Single）：只在收藏歌单取完后才拉，保证「先歌单、后专辑」的纯追加顺序。
    albums_more: bool,
}

#[derive(Clone)]
pub(super) struct HomeSidebarPageFetch {
    pub(super) section: HomeSidebarSection,
    pub(super) kind: HomeSidebarItemKind,
    pub(super) items: Vec<HomeSidebarPlaylist>,
    pub(super) next_offset: usize,
    pub(super) has_more: bool,
}

type HomeSidebarPageTask =
    Pin<Box<dyn Future<Output = Option<Result<HomeSidebarPageFetch, String>>>>>;

pub(super) type HomeSidebarFetchFuture = SharedTask<Result<HomeSidebarFetch, String>>;

pub(super) type HomeSidebarPageFetchFuture = SharedTask<Result<HomeSidebarPageFetch, String>>;

async fn fetch_home_sidebar_playlists(
    mut api: ApiState,
    language: Language,
) -> Result<HomeSidebarFetch, String> {
    let zh = matches!(language, Language::Zh);
    let pick = |z: &'static str, e: &'static str| if zh { z } else { e };

    let account = match api.user_account().await {
        Ok(v) => v,
        Err(_) => api.login_status().await.map_err(|err| {
            format!(
                "{}: {err}",
                pick("账号信息请求失败", "Account request failed")
            )
        })?,
    };
    let account_code = response_code(&account);
    if account_code != 200 {
        return Err(format!(
            "{}({}): {}",
            pick("账号信息请求失败", "Failed to fetch account profile"),
            account_code,
            response_message(&account)
        ));
    }

    let user_id = extract_current_user_id(&account)
        .ok_or_else(|| pick("未找到当前用户 ID", "Current user id not found").to_string())?;
    let user_name = extract_current_user_name(&account)
        .unwrap_or_else(|| pick("当前用户", "Current User").to_string());

    let created_response = api
        .user_playlist_create(&user_id, HOME_SIDEBAR_PLAYLIST_LIMIT, 0)
        .await
        .map_err(|err| {
            format!(
                "{}: {err}",
                pick("创建歌单请求失败", "Created playlists request failed")
            )
        })?;
    let created_code = response_code(&created_response);
    if created_code != 200 {
        return Err(format!(
            "{}({}): {}",
            pick("创建歌单请求失败", "Created playlists request failed"),
            created_code,
            response_message(&created_response)
        ));
    }

    let collected_response = api
        .user_playlist_collect(&user_id, HOME_SIDEBAR_PLAYLIST_LIMIT, 0)
        .await
        .map_err(|err| {
            format!(
                "{}: {err}",
                pick("收藏歌单请求失败", "Collected playlists request failed")
            )
        })?;
    let collected_code = response_code(&collected_response);
    if collected_code != 200 {
        return Err(format!(
            "{}({}): {}",
            pick("收藏歌单请求失败", "Collected playlists request failed"),
            collected_code,
            response_message(&collected_response)
        ));
    }

    let created = parse_home_sidebar_playlists(&created_response);
    let mut collected = parse_home_sidebar_playlists(&collected_response);
    let collected_more = response_page_has_more(
        &collected_response,
        0,
        collected.len(),
        HOME_SIDEBAR_PLAYLIST_LIMIT,
    );
    // 收藏歌单还有下一页时，专辑留给分页在歌单取完后再拉。
    let albums_more = if collected_more {
        true
    } else {
        let albums_response = api
            .album_sublist(HOME_SIDEBAR_PLAYLIST_LIMIT, 0)
            .await
            .map_err(|err| {
                format!(
                    "{}: {err}",
                    pick("收藏专辑请求失败", "Collected albums request failed")
                )
            })?;
        let albums_code = response_code(&albums_response);
        if albums_code != 200 {
            return Err(format!(
                "{}({}): {}",
                pick("收藏专辑请求失败", "Collected albums request failed"),
                albums_code,
                response_message(&albums_response)
            ));
        }
        let albums = parse_home_sidebar_albums(&albums_response);
        let more = response_page_has_more(
            &albums_response,
            0,
            albums.len(),
            HOME_SIDEBAR_PLAYLIST_LIMIT,
        );
        collected.extend(albums);
        more
    };
    Ok(HomeSidebarFetch {
        user_id,
        liked_playlist_id: extract_liked_playlist_id(&account),
        user_name,
        created_more: response_page_has_more(
            &created_response,
            0,
            created.len(),
            HOME_SIDEBAR_PLAYLIST_LIMIT,
        ),
        collected_more,
        albums_more,
        created,
        collected,
    })
}

async fn fetch_home_sidebar_page(
    mut api: ApiState,
    user_id: String,
    section: HomeSidebarSection,
    kind: HomeSidebarItemKind,
    offset: usize,
    language: Language,
) -> Result<HomeSidebarPageFetch, String> {
    let response = match (section, kind) {
        (_, HomeSidebarItemKind::Album) => {
            api.album_sublist(HOME_SIDEBAR_PLAYLIST_LIMIT, offset).await
        }
        (HomeSidebarSection::Created, HomeSidebarItemKind::Playlist) => {
            api.user_playlist_create(&user_id, HOME_SIDEBAR_PLAYLIST_LIMIT, offset)
                .await
        }
        (HomeSidebarSection::Collected, HomeSidebarItemKind::Playlist) => {
            api.user_playlist_collect(&user_id, HOME_SIDEBAR_PLAYLIST_LIMIT, offset)
                .await
        }
    }
    .map_err(|err| err.to_string())?;
    let code = response_code(&response);
    if code != 200 {
        return Err(format!("请求失败({code}): {}", response_message(&response)));
    }

    let items = match kind {
        HomeSidebarItemKind::Playlist => parse_home_sidebar_playlists(&response),
        HomeSidebarItemKind::Album => parse_home_sidebar_albums(&response),
    };
    // 专辑分页从 0 开始时可能本就为空（收藏歌单刚好取完、没有收藏专辑）：不算错误。
    if items.is_empty() && !(kind == HomeSidebarItemKind::Album && offset == 0) {
        return Err(match language {
            Language::Zh => "侧边栏歌单分页为空".to_string(),
            Language::En => "Sidebar playlist page is empty".to_string(),
        });
    }
    let item_count = items.len();
    let next_offset = offset.saturating_add(item_count);
    Ok(HomeSidebarPageFetch {
        section,
        kind,
        items,
        next_offset,
        has_more: response_page_has_more(
            &response,
            offset,
            item_count,
            HOME_SIDEBAR_PLAYLIST_LIMIT,
        ),
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HomeSidebarSection {
    Created,
    Collected,
}

/// 侧边栏条目类型：歌单，或收藏的专辑（含 EP、Single，网易云里都是专辑）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HomeSidebarItemKind {
    Playlist,
    Album,
}

#[derive(Debug, Clone)]
pub(crate) struct HomeSidebarPlaylist {
    pub(crate) kind: HomeSidebarItemKind,
    pub(crate) id: Option<String>,
    pub(crate) title: String,
    pub(crate) creator: String,
    pub(crate) track_count: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct HomeSidebarHit {
    pub(crate) section: HomeSidebarSection,
    pub(crate) index: usize,
}

/// 滚轮落在侧边栏时该滚哪个分区：光标所在分区优先（分区按整块算，列表短时
/// 下方的空白也命中），否则用当前聚焦分区；光标不在侧边栏面板内返回 `None`。
pub(super) fn home_sidebar_wheel_target(
    panel: Option<HitRect>,
    sections: &[(HitRect, HomeSidebarSection)],
    focused: HomeSidebarSection,
    col: u16,
    row: u16,
) -> Option<HomeSidebarSection> {
    let panel = panel?;
    if !panel.contains(col, row) {
        return None;
    }

    Some(
        sections
            .iter()
            .find(|(rect, _)| rect.contains(col, row))
            .map(|(_, section)| *section)
            .unwrap_or(focused),
    )
}

pub(crate) struct HomeSidebarState {
    pub(crate) expanded: bool,
    pub(crate) loading: bool,
    pub(crate) loading_more: bool,
    /// 歌单收藏状态在本程序内被改过：下次展开侧边栏时重新获取（脏标记）。
    pub(super) stale: bool,
    pub(super) user_id: Option<String>,
    pub(super) liked_playlist_id: Option<String>,
    pub(crate) user_name: String,
    pub(crate) created_playlists: Vec<HomeSidebarPlaylist>,
    pub(crate) collected_playlists: Vec<HomeSidebarPlaylist>,
    pub(super) created_has_more: bool,
    pub(super) collected_has_more: bool,
    pub(super) created_next_offset: usize,
    pub(super) collected_next_offset: usize,
    /// 收藏分区里专辑（含 EP、Single）的分页游标：排在收藏歌单之后。
    pub(super) album_has_more: bool,
    pub(super) album_next_offset: usize,
    pub(crate) focused_section: HomeSidebarSection,
    pub(crate) focused_index: usize,
    pub(super) created_focused_index: usize,
    pub(super) collected_focused_index: usize,
    pub(super) created_scroll_offset: usize,
    pub(super) collected_scroll_offset: usize,
    pub(crate) motion: Toggle,
    pub(crate) trail: Trail,
    pub(crate) status_line: String,
    /// Fixed-size rendering scratch, reused while the drawer translates.
    pub(crate) buffer: ratatui::buffer::Buffer,
}

impl Default for HomeSidebarState {
    fn default() -> Self {
        Self {
            expanded: false,
            loading: false,
            loading_more: false,
            stale: false,
            user_id: None,
            liked_playlist_id: None,
            user_name: String::new(),
            created_playlists: Vec::new(),
            collected_playlists: Vec::new(),
            created_has_more: false,
            collected_has_more: false,
            created_next_offset: 0,
            collected_next_offset: 0,
            album_has_more: false,
            album_next_offset: 0,
            focused_section: HomeSidebarSection::Created,
            focused_index: 0,
            created_focused_index: 0,
            collected_focused_index: 0,
            created_scroll_offset: 0,
            collected_scroll_offset: 0,
            motion: Toggle::new(false),
            trail: Trail::new(0.0),
            status_line: String::new(),
            buffer: ratatui::buffer::Buffer::default(),
        }
    }
}

impl HomeSidebarState {
    fn section_memory(&self, section: HomeSidebarSection) -> usize {
        match section {
            HomeSidebarSection::Created => self.created_focused_index,
            HomeSidebarSection::Collected => self.collected_focused_index,
        }
    }

    fn set_section_memory(&mut self, section: HomeSidebarSection, index: usize) {
        match section {
            HomeSidebarSection::Created => {
                self.created_focused_index = index;
            }
            HomeSidebarSection::Collected => {
                self.collected_focused_index = index;
            }
        }
    }

    pub(crate) fn section_scroll_offset(&self, section: HomeSidebarSection) -> usize {
        match section {
            HomeSidebarSection::Created => self.created_scroll_offset,
            HomeSidebarSection::Collected => self.collected_scroll_offset,
        }
    }

    pub(crate) fn set_section_scroll_offset(&mut self, section: HomeSidebarSection, offset: usize) {
        match section {
            HomeSidebarSection::Created => {
                self.created_scroll_offset = offset;
            }
            HomeSidebarSection::Collected => {
                self.collected_scroll_offset = offset;
            }
        }
    }

    fn sync_memory_from_current(&mut self) {
        self.set_section_memory(self.focused_section, self.focused_index);
    }

    pub(super) fn section_len(&self, section: HomeSidebarSection) -> usize {
        match section {
            HomeSidebarSection::Created => self.created_playlists.len(),
            HomeSidebarSection::Collected => self.collected_playlists.len(),
        }
    }
    pub(super) fn section_has_more(&self, section: HomeSidebarSection) -> bool {
        match section {
            HomeSidebarSection::Created => self.created_has_more,
            HomeSidebarSection::Collected => self.collected_has_more || self.album_has_more,
        }
    }

    /// 该分区下一页要拉的 `(条目类型, offset)`：收藏分区先取完歌单再取专辑。
    fn section_next_page(&self, section: HomeSidebarSection) -> (HomeSidebarItemKind, usize) {
        match section {
            HomeSidebarSection::Created => {
                (HomeSidebarItemKind::Playlist, self.created_next_offset)
            }
            HomeSidebarSection::Collected if self.collected_has_more => {
                (HomeSidebarItemKind::Playlist, self.collected_next_offset)
            }
            HomeSidebarSection::Collected => (HomeSidebarItemKind::Album, self.album_next_offset),
        }
    }

    pub(super) fn append_section_page(&mut self, page: HomeSidebarPageFetch) {
        match (page.section, page.kind) {
            (_, HomeSidebarItemKind::Album) => {
                self.collected_playlists.extend(page.items);
                self.album_next_offset = page.next_offset;
                self.album_has_more = page.has_more;
            }
            (HomeSidebarSection::Created, HomeSidebarItemKind::Playlist) => {
                self.created_playlists.extend(page.items);
                self.created_next_offset = page.next_offset;
                self.created_has_more = page.has_more;
            }
            (HomeSidebarSection::Collected, HomeSidebarItemKind::Playlist) => {
                self.collected_playlists.extend(page.items);
                self.collected_next_offset = page.next_offset;
                self.collected_has_more = page.has_more;
            }
        }
        self.clamp_focus();
    }

    pub(super) fn clamp_focus(&mut self) {
        let created_len = self.created_playlists.len();
        let collected_len = self.collected_playlists.len();

        self.created_focused_index = if created_len == 0 {
            0
        } else {
            self.created_focused_index
                .min(created_len.saturating_sub(1))
        };
        self.collected_focused_index = if collected_len == 0 {
            0
        } else {
            self.collected_focused_index
                .min(collected_len.saturating_sub(1))
        };

        if created_len == 0 && collected_len == 0 {
            self.focused_section = HomeSidebarSection::Created;
            self.focused_index = 0;
            return;
        }

        match self.focused_section {
            HomeSidebarSection::Created if created_len == 0 => {
                self.focused_section = HomeSidebarSection::Collected;
            }
            HomeSidebarSection::Collected if collected_len == 0 => {
                self.focused_section = HomeSidebarSection::Created;
            }
            _ => {}
        }

        self.focused_index = self.section_memory(self.focused_section);

        let created_max_start = created_len.saturating_sub(1);
        let collected_max_start = collected_len.saturating_sub(1);
        self.created_scroll_offset = self.created_scroll_offset.min(created_max_start);
        self.collected_scroll_offset = self.collected_scroll_offset.min(collected_max_start);
    }

    fn reset_focus(&mut self) {
        self.created_focused_index = 0;
        self.collected_focused_index = 0;
        self.created_scroll_offset = 0;
        self.collected_scroll_offset = 0;
        self.focused_section = if !self.created_playlists.is_empty() {
            HomeSidebarSection::Created
        } else if !self.collected_playlists.is_empty() {
            HomeSidebarSection::Collected
        } else {
            HomeSidebarSection::Created
        };
        self.focused_index = 0;
        self.clamp_focus();
    }

    pub(super) fn focus_next(&mut self) {
        let len = self.section_len(self.focused_section);
        if len == 0 {
            return;
        }
        self.focused_index = (self.focused_index + 1) % len;
        self.sync_memory_from_current();
    }

    pub(super) fn focus_prev(&mut self) {
        let len = self.section_len(self.focused_section);
        if len == 0 {
            return;
        }
        self.focused_index = if self.focused_index == 0 {
            len - 1
        } else {
            self.focused_index - 1
        };
        self.sync_memory_from_current();
    }

    /// 滚轮滚动一格：在当前分区内移动焦点（到顶/到底即停，不回卷——键盘的
    /// `focus_next/prev` 会绕回，滚轮从列表末尾跳回开头会很突兀）。
    /// 侧边栏的视图跟随焦点，所以这就是它唯一的滚动方式。
    pub(super) fn scroll_by(&mut self, forward: bool) {
        let len = self.section_len(self.focused_section);
        if len == 0 {
            return;
        }

        let next = if forward {
            self.focused_index.saturating_add(1).min(len - 1)
        } else {
            self.focused_index.saturating_sub(1)
        };
        self.focused_index = next;
        self.sync_memory_from_current();
    }

    /// 滚轮落到某个分区：先切到该分区（沿用它的位置记忆），再走一格，
    /// 这样"指着收藏区滚滚轮"不会把创建区的焦点带走。
    pub(super) fn scroll_section_by(&mut self, section: HomeSidebarSection, forward: bool) {
        if section != self.focused_section {
            let index = self.section_memory(section);
            self.set_focus(section, index);
        }
        self.scroll_by(forward);
    }

    pub(super) fn switch_section_prev(&mut self) {
        self.sync_memory_from_current();
        self.focused_section = match self.focused_section {
            HomeSidebarSection::Created => HomeSidebarSection::Collected,
            HomeSidebarSection::Collected => HomeSidebarSection::Created,
        };
        self.clamp_focus();
    }

    pub(super) fn switch_section_next(&mut self) {
        self.sync_memory_from_current();
        self.focused_section = match self.focused_section {
            HomeSidebarSection::Created => HomeSidebarSection::Collected,
            HomeSidebarSection::Collected => HomeSidebarSection::Created,
        };
        self.clamp_focus();
    }

    pub(super) fn set_focus(&mut self, section: HomeSidebarSection, index: usize) {
        self.sync_memory_from_current();
        self.focused_section = section;
        self.focused_index = index;
        self.sync_memory_from_current();
        self.clamp_focus();
    }

    fn focused_playlist(&self) -> Option<&HomeSidebarPlaylist> {
        match self.focused_section {
            HomeSidebarSection::Created => self.created_playlists.get(self.focused_index),
            HomeSidebarSection::Collected => self.collected_playlists.get(self.focused_index),
        }
    }

    pub(crate) fn is_visible(&self) -> bool {
        self.expanded || self.motion.on() || self.trail.value() > 0.0
    }
}

fn parse_home_sidebar_playlists(response: &ApiResponse) -> Vec<HomeSidebarPlaylist> {
    let Some(items) = response
        .body
        .get("playlist")
        .and_then(|value| value.as_array())
        .or_else(|| {
            response
                .body
                .pointer("/data/list")
                .and_then(|value| value.as_array())
        })
        .or_else(|| {
            response
                .body
                .pointer("/data/playlist")
                .and_then(|value| value.as_array())
        })
    else {
        return Vec::new();
    };

    let mut out = Vec::with_capacity(items.len());
    for item in items {
        let Some(title) = item.get("name").and_then(|value| value.as_str()) else {
            continue;
        };

        let track_count = item
            .get("trackCount")
            .and_then(|value| value.as_u64())
            .map(|value| value as usize)
            .or_else(|| {
                item.get("trackCount")
                    .and_then(|value| value.as_i64())
                    .map(|value| value.max(0) as usize)
            })
            .unwrap_or(0);

        out.push(HomeSidebarPlaylist {
            id: parse_value_as_string(item.get("id")),
            title: title.to_string(),
            creator: item
                .pointer("/creator/nickname")
                .and_then(|value| value.as_str())
                .unwrap_or("Unknown User")
                .to_string(),
            track_count,
            kind: HomeSidebarItemKind::Playlist,
        });
    }

    out
}

/// 解析 `album/sublist` 回包（`data[]`）：作者名用 " / " 连接，`size` 为曲目数。
fn parse_home_sidebar_albums(response: &ApiResponse) -> Vec<HomeSidebarPlaylist> {
    let Some(items) = response.body.get("data").and_then(|value| value.as_array()) else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|item| {
            let title = item.get("name").and_then(|value| value.as_str())?;
            let creator = item
                .get("artists")
                .and_then(|value| value.as_array())
                .map(|artists| {
                    artists
                        .iter()
                        .filter_map(|artist| artist.get("name").and_then(|value| value.as_str()))
                        .collect::<Vec<_>>()
                        .join(" / ")
                })
                .unwrap_or_default();
            Some(HomeSidebarPlaylist {
                kind: HomeSidebarItemKind::Album,
                id: parse_value_as_string(item.get("id")),
                title: title.to_string(),
                creator,
                track_count: item
                    .get("size")
                    .and_then(|value| value.as_u64())
                    .unwrap_or(0) as usize,
            })
        })
        .collect()
}

impl App {
    /// 主页侧边栏的滚轮滚动：光标指到哪个分区就滚哪个（没有则滚当前聚焦分区），
    /// 一格一步、到端点即停。侧边栏收起或光标在面板外时不动。
    pub(super) fn scroll_home_sidebar(&mut self, col: u16, row: u16, forward: bool) {
        if !self.browse.home_sidebar.expanded {
            return;
        }

        let Some(section) = home_sidebar_wheel_target(
            self.home_sidebar_panel_hit,
            &self.home_sidebar_section_hits,
            self.browse.home_sidebar.focused_section,
            col,
            row,
        ) else {
            return;
        };

        self.browse.home_sidebar.scroll_section_by(section, forward);
        if forward {
            self.maybe_fetch_home_sidebar_page();
        }
    }

    pub(super) fn tick_home_sidebar_animation(&mut self, now: Instant) {
        self.browse.home_sidebar.motion.tick(now);
        self.browse.home_sidebar.trail.tick(now);
    }

    pub(super) fn animate_home_sidebar(&mut self) {
        let now = Instant::now();
        let target = if self.browse.home_sidebar.expanded {
            1.0
        } else {
            0.0
        };
        self.browse.home_sidebar.motion.set(
            target > 0.5,
            now,
            SIDEBAR_ANIM_DURATION,
            Curve::EaseOut,
        );
        self.browse.home_sidebar.trail.follow(
            target,
            now,
            Duration::ZERO,
            SIDEBAR_ANIM_DURATION,
            Curve::EaseOut,
        );
    }

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
        let has_content = !self.browse.home_sidebar.created_playlists.is_empty()
            || !self.browse.home_sidebar.collected_playlists.is_empty();
        if has_content && !self.browse.home_sidebar.stale {
            self.browse.home_sidebar.reset_focus();
            return;
        }
        // 异步填充：立刻返回，动画照常跑，数据由 tick 搬入。
        // 脏标记重取时旧列表继续显示，结果到达后整体替换。
        if self.browse.home_sidebar_fetch.is_none() {
            self.browse.home_sidebar.stale = false;
            self.browse.home_sidebar.loading = true;
            // 在途的旧分页会追加到新列表上，直接作废。
            self.browse.home_sidebar_page_fetch = None;
            self.browse.home_sidebar.loading_more = false;
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
                // 专辑排在收藏歌单之后：首批拉到的专辑数 = 收藏分区总数 − 收藏歌单数。
                let album_count = self
                    .browse
                    .home_sidebar
                    .collected_playlists
                    .iter()
                    .filter(|item| item.kind == HomeSidebarItemKind::Album)
                    .count();
                self.browse.home_sidebar.collected_next_offset =
                    self.browse.home_sidebar.collected_playlists.len() - album_count;
                self.browse.home_sidebar.album_next_offset = album_count;
                self.browse.home_sidebar.album_has_more = data.albums_more;
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
                // 失败时保留旧列表，下次展开再试。
                self.browse.home_sidebar.stale = true;
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
        let (kind, offset) = self.browse.home_sidebar.section_next_page(section);
        self.browse.home_sidebar.loading_more = true;
        let fut = fetch_home_sidebar_page(
            self.api.clone(),
            user_id,
            section,
            kind,
            offset,
            self.config.language,
        );
        let fut: HomeSidebarPageTask = Box::pin(async move { Some(fut.await) });
        self.browse.home_sidebar_page_fetch = Some(spawn_shared(fut, self.wake.clone()));
    }

    pub(super) fn open_focused_home_sidebar_playlist(&mut self) {
        let (item_id, title, kind) = {
            let Some(item) = self.browse.home_sidebar.focused_playlist() else {
                self.browse.home.status_line = self
                    .lang_text("侧边栏暂无可打开歌单", "No sidebar playlist to open")
                    .to_string();
                return;
            };

            let Some(item_id) = item.id.clone() else {
                self.browse.home.status_line = self
                    .lang_text(
                        "当前歌单缺少 ID，无法打开",
                        "The selected playlist has no ID",
                    )
                    .to_string();
                return;
            };

            (item_id, item.title.clone(), item.kind)
        };

        let api = self.api.clone();
        let language = self.config.language;
        let (task, page_kind): (PlaylistFetchTask, PlaylistPageKind) = match kind {
            HomeSidebarItemKind::Album => {
                let future = fetch_album_page(api, language, item_id, None);
                (
                    Box::pin(async move { Some(future.await) }),
                    PlaylistPageKind::Album,
                )
            }
            HomeSidebarItemKind::Playlist => {
                let future = fetch_playlist_page(
                    api,
                    language,
                    item_id,
                    None,
                    self.browse.home_sidebar.liked_playlist_id.clone(),
                    self.browse.home_sidebar.user_id.clone(),
                );
                (
                    Box::pin(async move { Some(future.await) }),
                    PlaylistPageKind::Playlist,
                )
            }
        };
        // 只在主页绘制侧栏；保留展开与焦点，返回时继续操作原歌单。
        self.dispatch_home_playlist_fetch(title, task, page_kind);
    }
}
