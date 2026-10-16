use super::author::AuthorState;
use super::home::HomeState;
use super::playlist::PlaylistState;
use super::roam::PrivateRoamState;
use super::sidebar::HomeSidebarState;

#[derive(Default)]
pub(crate) struct BrowseController {
    pub(crate) home: HomeState,
    pub(crate) home_sidebar: HomeSidebarState,
    pub(crate) playlist: PlaylistState,
    pub(super) private_roam: PrivateRoamState,
    pub(crate) author: AuthorState,
    pub(super) home_sidebar_fetch: Option<super::sidebar::HomeSidebarFetchFuture>,
    pub(super) home_sidebar_page_fetch: Option<super::sidebar::HomeSidebarPageFetchFuture>,
    pub(super) author_fetch: Option<super::author::AuthorFetchFuture>,
    pub(super) playlist_fetch: Option<super::playlist::PlaylistFetchSlot>,
}

impl BrowseController {
    pub(super) fn reset_pages(&mut self) {
        self.home_sidebar_fetch = None;
        self.home_sidebar_page_fetch = None;
        self.author_fetch = None;
        self.playlist_fetch = None;
        self.author = AuthorState::default();
        self.home = HomeState::default();
        self.home_sidebar = HomeSidebarState::default();
        self.private_roam = PrivateRoamState::default();
        self.playlist = PlaylistState::default();
    }

    pub(super) fn apply_playlist(
        &mut self,
        fetch: super::playlist::PlaylistFetch,
        api: &super::api::ApiState,
    ) -> Option<super::likes::LikedRefresh> {
        self.playlist_fetch = None;
        self.playlist.cancel_pagination();
        self.playlist.id = Some(fetch.id);
        self.playlist.title = fetch.title;
        self.playlist.artist = fetch.artist;
        self.playlist.description = fetch.description;
        self.playlist.set_tracks(fetch.tracks);
        self.playlist.total_tracks = fetch.total_tracks;
        self.playlist.pagination = fetch.paginated.then(|| {
            super::playlist_pagination::PlaylistPagination::new(
                crate::data::playback_session::PlaylistCursor {
                    source_id: self.playlist.id.clone().unwrap_or_default(),
                    next_offset: fetch.next_offset,
                    total_tracks: fetch.total_tracks,
                    has_more: fetch.has_more,
                },
            )
        });
        if let Some(url) = fetch.cover_url {
            self.playlist.cover.load(api.clone(), url);
        }
        fetch.liked
    }

    pub(super) fn apply_author(
        &mut self,
        fetch: super::author::AuthorFetch,
        api: &super::api::ApiState,
    ) {
        self.author_fetch = None;
        self.author.id = Some(fetch.id);
        self.author.title = fetch.title;
        self.author.artist = fetch.artist;
        self.author.description = fetch.description;
        if let Some(url) = fetch.cover_url {
            self.author.cover.load(api.clone(), url);
        }
        self.author.set_tiles(fetch.tiles);
        self.author.hot_songs = fetch.hot_songs;
        self.author.albums = fetch.albums;
        self.author.eps = fetch.eps;
        self.author.singles = fetch.singles;
        self.author.focused_idx = 0;
    }
}
