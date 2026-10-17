use super::App;
use super::Page;
use super::download::DownloadEvent;
use super::download::DownloadManager;
use super::download::DownloadRequest;
use super::download::DownloadRow;
use super::download::DownloadRowCache;
use super::download::DownloadState;
use super::download::DownloadTarget;
use super::playlist::PlaylistPageKind;
use super::search_controller::SearchItem;
use super::search_controller::SearchItemKind;
use super::tracks::PlaylistTrack;
use super::tracks::PlaylistTrackKind;
use std::collections::VecDeque;
use std::path::Path;
use std::path::PathBuf;
use std::time::Duration;
use std::time::Instant;

/// 一次性生成歌单页 / 专辑页的整页行数据：文件名与磁盘缓存 key 都在这里做完，
/// 之后每帧只查状态表（列表代或下载根目录变化时才重建）。所有下载都直接落根目录。
fn playlist_download_rows(
    tracks: &[PlaylistTrack],
    root: Option<&Path>,
) -> Vec<Option<DownloadRow>> {
    let Some(root) = root else {
        return vec![None; tracks.len()];
    };

    tracks
        .iter()
        .map(|track| {
            if track.kind != PlaylistTrackKind::Song {
                return None;
            }
            let song_id = track.id.clone()?;
            let target = DownloadTarget {
                dir: root.to_path_buf(),
                base: crate::app::download::download_file_stem(
                    &track.title,
                    &track.artist,
                    &track.album,
                ),
            };
            Some(DownloadRow::new(song_id, target))
        })
        .collect()
}

/// 搜索页同理：只有单曲行有图标，且不落专辑子文件夹。
fn search_download_rows(results: &[SearchItem], root: Option<&Path>) -> Vec<Option<DownloadRow>> {
    let Some(root) = root else {
        return vec![None; results.len()];
    };
    let dir = root.to_path_buf();

    results
        .iter()
        .map(|item| {
            if item.kind != SearchItemKind::Song {
                return None;
            }
            let song_id = item.song_id.clone()?;
            let target = DownloadTarget {
                dir: dir.clone(),
                base: crate::app::download::download_file_stem(
                    item.title.as_deref().unwrap_or(&item.left_label),
                    item.artist.as_deref().unwrap_or_default(),
                    item.album.as_deref().unwrap_or_default(),
                ),
            };
            Some(DownloadRow::new(song_id, target))
        })
        .collect()
}

/// 一次下载的候选歌曲：UI 侧决定落点所需的全部信息。
pub(super) struct DownloadCandidate {
    pub(super) song_id: String,
    pub(super) title: String,
    pub(super) artist: String,
    pub(super) album: String,
}

impl App {
    /// 当前播放歌曲的下载图标状态（每帧缓存一次，全屏页只读不再查磁盘）。
    pub(crate) fn current_download_state(&self) -> Option<DownloadState> {
        if self.downloads.root.is_none() || self.playback.now_playing.is_none() {
            return None;
        }
        Some(self.downloads.now_playing_state)
    }

    /// 供全屏页每帧刷新的缓存：任务表与磁盘状态都封在这一处。
    fn refresh_current_download_state(&mut self) {
        let state = match self.playback.now_playing.clone() {
            Some(track) => self
                .download_state_for_candidate(&DownloadCandidate {
                    song_id: track.song_id,
                    title: track.title,
                    artist: track.artist,
                    album: track.album,
                })
                .unwrap_or(DownloadState::NotDownloaded),
            None => DownloadState::NotDownloaded,
        };
        self.downloads.now_playing_state = state;
    }

    /// 重新解析下载根目录（设置里改了路径、或全屏页同步回来时调用）。
    pub(crate) fn refresh_download_root(&mut self) {
        let next =
            crate::app::download::resolve_download_root(self.config.download_path.as_deref());
        if next != self.downloads.root {
            self.downloads.pending_intents.invalidate_root();
            self.downloads.root = next;
            self.downloads.manager.clear_disk_cache();
            // 行数据的目录部分变了：整页行数据重建。
            self.downloads.rows_epoch = self.downloads.rows_epoch.wrapping_add(1);
        }
    }

    /// 当前聚焦的单曲（歌单页 / 搜索页）与它所在页面的下载上下文。
    fn focused_download_song(&self) -> Option<DownloadCandidate> {
        match self.page {
            Page::Playlist => self.playlist_download_candidate(self.browse.playlist.focused_idx),
            Page::Search => self.search_download_candidate(self.search.focused_idx),
            _ => None,
        }
    }

    /// 歌单页 / 专辑页某一行对应的下载候选（非单曲行返回 `None`）。
    pub(super) fn playlist_download_candidate(&self, index: usize) -> Option<DownloadCandidate> {
        let track = self.browse.playlist.tracks.get(index)?;
        if track.kind != PlaylistTrackKind::Song {
            return None;
        }
        Some(DownloadCandidate {
            song_id: track.id.clone()?,
            title: track.title.clone(),
            artist: track.artist.clone(),
            album: track.album.clone(),
        })
    }

    /// 搜索页某一行对应的下载候选（仅单曲行）。
    pub(super) fn search_download_candidate(&self, index: usize) -> Option<DownloadCandidate> {
        let item = self.search.results().get(index)?;
        if item.kind != SearchItemKind::Song {
            return None;
        }
        Some(DownloadCandidate {
            song_id: item.song_id.clone()?,
            title: item
                .title
                .clone()
                .unwrap_or_else(|| item.left_label.clone()),
            artist: item.artist.clone().unwrap_or_default(),
            album: item.album.clone().unwrap_or_default(),
        })
    }

    /// 下载图标的动画相位（time-based：空闲节流下也按真实时间推进）。
    pub(crate) fn download_spinner_phase(&self) -> Duration {
        self.downloads.spinner_start.elapsed()
    }

    /// 歌单页 / 专辑页的行内图标：每帧调一次；列表代与任务版本都不变时几乎零成本。
    pub(crate) fn refresh_playlist_downloads(&mut self) {
        let epoch = (self.browse.playlist.generation(), self.downloads.rows_epoch);
        let root = self.downloads.root.clone();
        self.downloads
            .refresh_playlist(epoch, &self.browse.playlist.tracks, root.as_deref());
    }

    /// 歌单页 / 专辑页某行的图标状态（先调 `refresh_playlist_downloads`）。
    pub(crate) fn playlist_download_state_at(&self, index: usize) -> Option<DownloadState> {
        self.downloads.playlist_cache.state_at(index)
    }

    /// 搜索页的行内图标：每帧调一次。
    pub(crate) fn refresh_search_downloads(&mut self) {
        let epoch = (self.search.generation(), self.downloads.rows_epoch);
        let root = self.downloads.root.clone();
        self.downloads
            .refresh_search(epoch, self.search.results(), root.as_deref());
    }

    /// 搜索页某行的图标状态（先调 `refresh_search_downloads`）。
    pub(crate) fn search_download_state_at(&self, index: usize) -> Option<DownloadState> {
        self.downloads.search_cache.state_at(index)
    }

    fn download_state_for_candidate(
        &mut self,
        candidate: &DownloadCandidate,
    ) -> Option<DownloadState> {
        let root = self.downloads.root.clone()?;
        let target = DownloadTarget {
            dir: root,
            base: crate::app::download::download_file_stem(
                &candidate.title,
                &candidate.artist,
                &candidate.album,
            ),
        };
        Some(self.downloads.manager.state_of(&candidate.song_id, &target))
    }

    /// 主应用 Ctrl+Alt+D：下载聚焦的单曲（在途则取消）。
    pub(super) async fn download_focused_song(&mut self) {
        let Some(candidate) = self.focused_download_song() else {
            self.set_runtime_status(self.lang_text(
                "当前列表里没有可下载的单曲",
                "No downloadable song in this list",
            ));
            return;
        };
        self.toggle_download(candidate);
    }

    /// 全屏页 Ctrl+D / 点下载图标：下载当前播放的单曲（在途则取消）。
    pub(crate) fn download_current_song(&mut self) {
        let Some(track) = self.playback.now_playing.clone() else {
            self.set_runtime_status(
                self.lang_text("当前没有正在播放的歌曲", "Nothing is playing right now"),
            );
            return;
        };
        self.toggle_download(DownloadCandidate {
            song_id: track.song_id,
            title: track.title,
            artist: track.artist,
            album: track.album,
        });
    }

    /// 发起 / 取消下载（图标点击、两处快捷键共用这一条路径）。
    pub(super) fn toggle_download(&mut self, candidate: DownloadCandidate) {
        if self.downloads.pending_intents.cancel(&candidate.song_id)
            || self.downloads.manager.is_downloading(&candidate.song_id)
        {
            self.downloads.manager.cancel(&candidate.song_id);
            self.set_runtime_status(format!(
                "{}: {}",
                self.lang_text("已取消下载", "Download cancelled"),
                candidate.title
            ));
            return;
        }

        if self.downloads.manager.is_busy(&candidate.song_id) {
            self.set_runtime_status(self.lang_text(
                "正在取消上一任务，请稍候",
                "Cancelling the previous download, please wait",
            ));
            return;
        }

        let Some(root) = self.downloads.root.clone() else {
            self.set_runtime_status(self.lang_text(
                "下载不可用：没有可用的下载目录",
                "Download unavailable: no usable download directory",
            ));
            return;
        };

        let target = DownloadTarget {
            dir: root,
            base: crate::app::download::download_file_stem(
                &candidate.title,
                &candidate.artist,
                &candidate.album,
            ),
        };
        let level = self
            .config
            .download_audio_quality
            .clamp_for_vip(self.vip_audio_unlocked);
        let request = DownloadRequest {
            song_id: candidate.song_id.clone(),
            level,
            title: candidate.title.clone(),
            artist: candidate.artist,
            album: candidate.album,
            target,
        };
        let state = self
            .downloads
            .manager
            .known_state_of(&request.song_id, &request.target);
        if !self.downloads.pending_intents.is_empty() || state.is_none() {
            let origin = self.downloads.pending_intents.origin();
            self.downloads.pending_intents.defer(origin, request);
            self.set_runtime_status(self.lang_text(
                "正在检查已有下载，请稍候",
                "Checking existing downloads, please wait",
            ));
            return;
        }
        match state {
            Some(DownloadState::Done) => {
                self.set_runtime_status(self.lang_text("歌曲已下载", "Song already downloaded"));
                return;
            }
            Some(DownloadState::Downloading) => return,
            Some(DownloadState::NotDownloaded) => {}
            None => unreachable!("unknown disk status was deferred"),
        }

        let queued = self.downloads.manager.is_active();
        match self.downloads.manager.enqueue(&self.api, request) {
            Ok(()) => {
                let status = if queued {
                    format!(
                        "{}: {} ({})",
                        self.lang_text("已加入下载队列", "Queued for download"),
                        candidate.title,
                        level.as_api_level()
                    )
                } else {
                    format!(
                        "{}: {} ({})",
                        self.lang_text("开始下载", "Downloading"),
                        candidate.title,
                        level.as_api_level()
                    )
                };
                self.set_runtime_status(status);
            }
            Err(message) => self.set_runtime_status(message),
        }
    }

    /// 每帧搬运下载结果：完成 / 失败 / 取消都写状态行。
    pub(super) fn tick_download(&mut self) {
        self.refresh_current_download_state();
        for event in self.downloads.poll() {
            match event {
                DownloadEvent::Started { title, level } => {
                    self.set_runtime_status(format!(
                        "{}: {title} ({})",
                        self.lang_text("开始下载", "Downloading"),
                        level.as_api_level()
                    ));
                }
                DownloadEvent::Finished {
                    title,
                    path,
                    level,
                    file_type,
                    tag_error,
                } => {
                    let mut text = format!(
                        "{}: {} [{}.{}] -> {}",
                        self.lang_text("已下载", "Downloaded"),
                        title,
                        level,
                        file_type,
                        path.display()
                    );
                    if let Some(error) = tag_error {
                        text.push_str(&format!(
                            " ({}{error})",
                            self.lang_text("标签写入失败: ", "tag write failed: ")
                        ));
                    }
                    self.set_runtime_status(text);
                }
                DownloadEvent::Failed { title, error } => {
                    self.set_runtime_status(format!(
                        "{}: {title}: {error}",
                        self.lang_text("下载失败", "Download failed")
                    ));
                }
                DownloadEvent::Cancelled { title } => {
                    self.set_runtime_status(format!(
                        "{}: {title}",
                        self.lang_text("已取消下载", "Download cancelled")
                    ));
                }
            }
        }
        loop {
            let request = {
                let downloads = &mut self.downloads;
                let manager = &mut downloads.manager;
                downloads.pending_intents.take_ready(|request| {
                    if manager.is_busy(&request.song_id) {
                        Some(DownloadState::Downloading)
                    } else {
                        manager.known_state_of(&request.song_id, &request.target)
                    }
                })
            };
            let Some(request) = request else { break };
            let status = format!(
                "{}: {} ({})",
                if self.downloads.manager.is_active() {
                    self.lang_text("已加入下载队列", "Queued for download")
                } else {
                    self.lang_text("开始下载", "Downloading")
                },
                request.title,
                request.level.as_api_level()
            );
            match self.downloads.manager.enqueue(&self.api, request) {
                Ok(()) => self.set_runtime_status(status),
                Err(message) => self.set_runtime_status(message),
            }
        }
    }
}

/// Download UI/cache state. Worker ownership remains in DownloadManager.
pub(super) struct DownloadController {
    pub(super) manager: DownloadManager,
    pub(super) playlist_cache: DownloadRowCache,
    pub(super) search_cache: DownloadRowCache,
    pub(super) rows_epoch: u64,
    pub(super) spinner_start: Instant,
    pub(super) now_playing_state: DownloadState,
    pub(super) root: Option<PathBuf>,
    pub(super) page_kind: PlaylistPageKind,
    pub(super) pending_intents: PendingDownloads,
}
impl DownloadController {
    pub(super) fn poll(&mut self) -> Vec<DownloadEvent> {
        self.manager.poll()
    }

    fn refresh_playlist(
        &mut self,
        epoch: (u64, u64),
        tracks: &[super::tracks::PlaylistTrack],
        root: Option<&std::path::Path>,
    ) {
        self.playlist_cache.refresh(epoch, &mut self.manager, || {
            playlist_download_rows(tracks, root)
        });
    }

    fn refresh_search(
        &mut self,
        epoch: (u64, u64),
        results: &[super::search_controller::SearchItem],
        root: Option<&std::path::Path>,
    ) {
        self.search_cache.refresh(epoch, &mut self.manager, || {
            search_download_rows(results, root)
        });
    }
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
struct DownloadOrigin {
    root: u64,
    session: u64,
}

struct PendingDownload {
    origin: DownloadOrigin,
    request: DownloadRequest,
}

/// User clicks waiting for disk status retain their order and original target.
/// Only explicit toggles cancel them; polling never replays a toggle.
#[derive(Default)]
pub(super) struct PendingDownloads {
    origin: DownloadOrigin,
    queue: VecDeque<PendingDownload>,
}

impl PendingDownloads {
    fn origin(&self) -> DownloadOrigin {
        self.origin
    }

    pub(super) fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }

    pub(super) fn cancel(&mut self, song_id: &str) -> bool {
        let Some(index) = self
            .queue
            .iter()
            .position(|pending| pending.request.song_id == song_id)
        else {
            return false;
        };
        self.queue.remove(index);
        true
    }

    fn defer(&mut self, origin: DownloadOrigin, request: DownloadRequest) {
        if origin == self.origin {
            self.queue.push_back(PendingDownload { origin, request });
        }
    }

    fn invalidate_root(&mut self) {
        self.origin.root = self.origin.root.wrapping_add(1);
        self.queue.clear();
    }

    pub(super) fn invalidate_session(&mut self) {
        self.origin.session = self.origin.session.wrapping_add(1);
        self.queue.clear();
    }

    /// Unknown files block later clicks; existing/busy songs are discarded, never replaced.
    pub(super) fn take_ready(
        &mut self,
        mut state: impl FnMut(&DownloadRequest) -> Option<DownloadState>,
    ) -> Option<DownloadRequest> {
        loop {
            let pending = self.queue.front()?;
            if pending.origin != self.origin {
                self.queue.pop_front();
                continue;
            }
            match state(&pending.request) {
                None => return None,
                Some(DownloadState::NotDownloaded) => {
                    return self.queue.pop_front().map(|pending| pending.request);
                }
                Some(DownloadState::Done | DownloadState::Downloading) => {
                    self.queue.pop_front();
                }
            }
        }
    }
}

#[cfg(test)]
mod tests;
