use super::{AuthorState, HomeSidebarState, HomeState, PlaylistState, PrivateRoamState};

#[derive(Default)]
pub(crate) struct BrowseController {
    pub home: HomeState,
    pub home_sidebar: HomeSidebarState,
    pub playlist: PlaylistState,
    pub private_roam: PrivateRoamState,
    pub author: AuthorState,
    pub(super) home_sidebar_fetch: Option<super::HomeSidebarFetchFuture>,
    pub(super) home_sidebar_page_fetch: Option<super::HomeSidebarPageFetchFuture>,
    pub(super) author_fetch: Option<super::AuthorFetchFuture>,
    pub(super) playlist_fetch: Option<super::PlaylistFetchSlot>,
    pub(super) playlist_page_fetch: Option<super::PlaylistPageFetchFuture>,
}

impl BrowseController {
    pub fn reset_pages(&mut self) {
        self.home_sidebar_fetch = None;
        self.home_sidebar_page_fetch = None;
        self.author_fetch = None;
        self.playlist_fetch = None;
        self.playlist_page_fetch = None;
        self.author = AuthorState::default();
        self.home = HomeState::default();
        self.home_sidebar = HomeSidebarState::default();
        self.private_roam = PrivateRoamState::default();
    }

    pub(super) fn apply_playlist(
        &mut self,
        fetch: super::PlaylistFetch,
        api: &super::ApiState,
    ) -> Option<super::LikedRefresh> {
        self.playlist_fetch = None;
        self.playlist_page_fetch = None;
        self.playlist.id = Some(fetch.id);
        self.playlist.title = fetch.title;
        self.playlist.artist = fetch.artist;
        self.playlist.description = fetch.description;
        self.playlist.total_tracks = fetch.total_tracks;
        self.playlist.next_offset = fetch.next_offset;
        self.playlist.has_more = fetch.has_more;
        self.playlist.loading_more = false;
        self.playlist.set_tracks(fetch.tracks);
        if let Some(url) = fetch.cover_url {
            self.playlist.cover.load(api.clone(), url);
        }
        fetch.liked
    }

    pub(super) fn apply_author(&mut self, fetch: super::AuthorFetch, api: &super::ApiState) {
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
