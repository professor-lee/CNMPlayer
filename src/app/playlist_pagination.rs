use super::{
    ApiState, PlaylistPageFetch, PlaylistPageFetchFuture, PlaylistPageTask,
    fetch_playlist_tracks_page, peek_shared, spawn_shared,
};
use crate::data::playback_session::PlaylistCursor;
use std::cell::RefCell;
use std::rc::Rc;

/// Browsing and playback share one source-bound cursor and one cancellable request.
#[derive(Clone)]
pub(super) struct PlaylistPagination(Rc<RefCell<PaginationState>>);

struct PaginationState {
    cursor: PlaylistCursor,
    pending: Option<PlaylistPageFetchFuture>,
    attempted_offset: Option<usize>,
}

impl PlaylistPagination {
    pub fn new(cursor: PlaylistCursor) -> Self {
        Self(Rc::new(RefCell::new(PaginationState {
            cursor,
            pending: None,
            attempted_offset: None,
        })))
    }

    pub fn same_source(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.0, &other.0)
    }

    pub fn cursor(&self) -> PlaylistCursor {
        self.0.borrow().cursor.clone()
    }

    pub fn has_more(&self) -> bool {
        self.0.borrow().cursor.has_more
    }

    pub fn is_pending(&self) -> bool {
        self.0.borrow().pending.is_some()
    }

    pub fn cancel(&self) {
        let mut state = self.0.borrow_mut();
        state.pending = None;
        state.attempted_offset = None;
    }

    pub fn prefetch(&self, api: ApiState) {
        let state = self.0.borrow();
        if state.attempted_offset == Some(state.cursor.next_offset) {
            return;
        }
        drop(state);
        self.request(api);
    }
    pub fn request(&self, api: ApiState) {
        let mut state = self.0.borrow_mut();
        if state.pending.is_some() || !state.cursor.has_more {
            return;
        }
        state.attempted_offset = Some(state.cursor.next_offset);
        let future = fetch_playlist_tracks_page(
            api,
            state.cursor.source_id.clone(),
            state.cursor.next_offset,
            state.cursor.total_tracks,
        );
        let future: PlaylistPageTask = Box::pin(async move { Some(future.await) });
        state.pending = Some(spawn_shared(future));
    }

    /// A shared response is consumed once; only its original source and offset may advance.
    pub fn take_ready(&self) -> Option<Result<PlaylistPageFetch, String>> {
        let mut state = self.0.borrow_mut();
        let result = peek_shared(state.pending.as_ref()?).cloned()?;
        state.pending = None;
        if let Ok(page) = &result {
            if page.source_id != state.cursor.source_id || page.offset != state.cursor.next_offset {
                return None;
            }
            state.cursor.next_offset = page.next_offset;
            state.cursor.has_more = page.has_more;
        }
        Some(result)
    }
}

/// Append the same page to every live consumer of this source, never the current unrelated page.
pub(super) fn apply_page(
    pagination: &PlaylistPagination,
    page: PlaylistPageFetch,
    browse: &mut super::PlaylistState,
    playback_pagination: Option<&PlaylistPagination>,
    queue: &mut Vec<super::PlaybackTrack>,
) -> bool {
    let playing = playback_pagination.is_some_and(|pager| pager.same_source(pagination));
    if playing {
        queue.extend(
            page.tracks
                .iter()
                .filter_map(super::PlaybackTrack::from_playlist_track),
        );
    }
    if browse
        .pagination
        .as_ref()
        .is_some_and(|pager| pager.same_source(pagination))
    {
        browse.append_tracks(page.tracks);
        browse.total_tracks = pagination.cursor().total_tracks;
    }
    playing
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{PlaybackTrack, PlaylistState, PlaylistTrack, PlaylistTrackKind};

    fn track(id: &str) -> PlaylistTrack {
        PlaylistTrack {
            kind: PlaylistTrackKind::Song,
            id: Some(id.to_string()),
            title: id.to_string(),
            artist: "artist".to_string(),
            album: "album".to_string(),
            cover_url: None,
            duration_ms: 1000,
            duration: "00:01".to_string(),
        }
    }

    fn pagination(id: &str) -> PlaylistPagination {
        PlaylistPagination::new(PlaylistCursor {
            source_id: id.to_string(),
            next_offset: 100,
            total_tracks: Some(101),
            has_more: true,
        })
    }

    async fn complete(pagination: &PlaylistPagination, id: &str, offset: usize) {
        let page = PlaylistPageFetch {
            source_id: id.to_string(),
            offset,
            tracks: vec![track("101")],
            next_offset: 101,
            has_more: false,
        };
        let future: PlaylistPageTask = Box::pin(async move { Some(Ok(page)) });
        pagination.0.borrow_mut().pending = Some(spawn_shared(future));
        compio::time::sleep(std::time::Duration::from_millis(1)).await;
    }

    #[compio::test]
    async fn browsing_and_playback_share_page_and_preserve_focus() {
        let pagination = pagination("playlist-a");
        let playback = pagination.clone();
        let mut browse = PlaylistState {
            id: Some("playlist-a".to_string()),
            ..Default::default()
        };
        browse.set_tracks(vec![track("99"), track("100")]);
        browse.set_focus(1);
        browse.pagination = Some(pagination.clone());
        let mut queue: Vec<_> = browse
            .tracks
            .iter()
            .filter_map(PlaybackTrack::from_playlist_track)
            .collect();
        complete(&pagination, "playlist-a", 100).await;
        let page = playback.take_ready().unwrap().unwrap();
        assert!(apply_page(
            &playback,
            page,
            &mut browse,
            Some(&playback),
            &mut queue
        ));
        assert_eq!(
            queue
                .iter()
                .map(|track| track.song_id.as_str())
                .collect::<Vec<_>>(),
            ["99", "100", "101"]
        );
        assert_eq!(
            browse
                .tracks
                .iter()
                .map(|track| track.id.as_deref().unwrap())
                .collect::<Vec<_>>(),
            ["99", "100", "101"]
        );
        assert_eq!(browse.focused_idx, 1);
        assert_eq!(browse.total_tracks, Some(101));
        assert_eq!(pagination.cursor().next_offset, 101);
        assert!(!pagination.has_more());
        assert!(
            pagination.take_ready().is_none(),
            "同一个共享回包不能追加两次"
        );
    }

    #[compio::test]
    async fn switching_to_daily_drops_old_cursor_but_playback_remains_bound() {
        let playing = pagination("playlist-a");
        let mut browse = PlaylistState::default();
        browse.set_tracks(vec![track("100")]);
        browse.pagination = Some(playing.clone());
        browse.total_tracks = Some(101);
        let mut queue: Vec<_> = browse
            .tracks
            .iter()
            .filter_map(PlaybackTrack::from_playlist_track)
            .collect();
        complete(&playing, "playlist-a", 100).await;
        browse.id = Some("daily".to_string());
        browse.set_tracks(vec![track("daily-song")]);
        assert!(browse.pagination.is_none());
        assert_eq!(browse.total_tracks, None);
        assert!(!playing.is_pending());
        assert!(
            playing.take_ready().is_none(),
            "切换来源取消旧页，已就绪结果也不能落地"
        );
        complete(&playing, "playlist-a", 100).await;
        let page = playing.take_ready().unwrap().unwrap();
        apply_page(&playing, page, &mut browse, Some(&playing), &mut queue);
        assert_eq!(browse.tracks[0].id.as_deref(), Some("daily-song"));
        assert_eq!(
            queue
                .iter()
                .map(|track| track.song_id.as_str())
                .collect::<Vec<_>>(),
            ["100", "101"]
        );
    }

    #[compio::test]
    async fn wrong_source_or_offset_cannot_advance_cursor() {
        let pagination = pagination("playlist-a");
        for (source, offset) in [("playlist-b", 100), ("playlist-a", 0)] {
            complete(&pagination, source, offset).await;
            assert!(pagination.take_ready().is_none());
            assert_eq!(pagination.cursor().next_offset, 100);
            assert!(pagination.has_more());
        }
    }
}
