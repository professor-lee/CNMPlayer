use super::*;
use crate::app::cover_state::CoverFetchState;
use crate::app::search_controller::SearchItem;

fn item(kind: SearchItemKind, label: &str) -> SearchItem {
    SearchItem {
        kind,
        left_label: label.to_string(),
        right_label: String::new(),
        song_id: None,
        album_id: None,
        playlist_id: None,
        artist_id: None,
        title: None,
        artist: None,
        album: None,
        cover_url: None,
        duration_ms: None,
        cover: CoverFetchState::default(),
    }
}

/// 序号按分区重新开始（同 kind 计数），跨窗口滚动时也不受影响。
#[test]
fn ordinal_restarts_per_section() {
    let mut state = SearchController::default();
    state.set_results(
        vec![
            item(SearchItemKind::Artist, "artist-1"),
            item(SearchItemKind::Artist, "artist-2"),
            item(SearchItemKind::Playlist, "playlist-1"),
            item(SearchItemKind::Song, "song-1"),
            item(SearchItemKind::Song, "song-2"),
        ],
        0,
        false,
    );

    assert_eq!(search_item_ordinal(&state, 1), 2);
    assert_eq!(search_item_ordinal(&state, 2), 1);
    assert_eq!(search_item_ordinal(&state, 4), 2);
}
