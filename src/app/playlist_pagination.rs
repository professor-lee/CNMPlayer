use super::api::ApiState;
use super::owned_task::peek_shared;
use super::owned_task::spawn_shared;
use super::playlist::PlaylistPageFetch;
use super::playlist::PlaylistPageFetchFuture;
use super::playlist::PlaylistPageTask;
use super::playlist::fetch_playlist_tracks_page;
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
    pub(super) fn new(cursor: PlaylistCursor) -> Self {
        Self(Rc::new(RefCell::new(PaginationState {
            cursor,
            pending: None,
            attempted_offset: None,
        })))
    }

    pub(super) fn same_source(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.0, &other.0)
    }

    pub(super) fn cursor(&self) -> PlaylistCursor {
        self.0.borrow().cursor.clone()
    }

    pub(super) fn has_more(&self) -> bool {
        self.0.borrow().cursor.has_more
    }

    pub(super) fn is_pending(&self) -> bool {
        self.0.borrow().pending.is_some()
    }

    pub(super) fn cancel(&self) {
        let mut state = self.0.borrow_mut();
        state.pending = None;
        state.attempted_offset = None;
    }

    pub(super) fn prefetch(&self, api: ApiState) {
        let state = self.0.borrow();
        if state.attempted_offset == Some(state.cursor.next_offset) {
            return;
        }
        drop(state);
        self.request(api);
    }
    pub(super) fn request(&self, api: ApiState) {
        let mut state = self.0.borrow_mut();
        if state.pending.is_some() || !state.cursor.has_more {
            return;
        }
        state.attempted_offset = Some(state.cursor.next_offset);
        let wake = api.wake_signal();
        let future = fetch_playlist_tracks_page(
            api,
            state.cursor.source_id.clone(),
            state.cursor.next_offset,
            state.cursor.total_tracks,
        );
        let future: PlaylistPageTask = Box::pin(async move { Some(future.await) });
        state.pending = Some(spawn_shared(future, wake));
    }

    /// A shared response is consumed once; only its original source and offset may advance.
    pub(super) fn take_ready(&self) -> Option<Result<PlaylistPageFetch, String>> {
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
    browse: &mut super::playlist::PlaylistState,
    playback_pagination: Option<&PlaylistPagination>,
    queue: &mut Vec<super::playback_controller::PlaybackTrack>,
) -> bool {
    let playing = playback_pagination.is_some_and(|pager| pager.same_source(pagination));
    if playing {
        queue.extend(
            page.tracks
                .iter()
                .filter_map(super::playback_controller::PlaybackTrack::from_playlist_track),
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
mod tests;
