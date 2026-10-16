use super::super::cover_state::CoverFetchState;
use super::super::search_controller::SearchController;
use super::super::search_controller::SearchItem;
use super::super::search_controller::SearchItemKind;

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

#[test]
fn mixed_sections_cache_rows_and_ordinals() {
    let mut state = SearchController::default();
    state.set_results(
        vec![
            item(SearchItemKind::Artist, "a1"),
            item(SearchItemKind::Artist, "a2"),
            item(SearchItemKind::Song, "s1"),
            item(SearchItemKind::Playlist, "p1"),
        ],
        0,
        false,
    );
    state.set_viewport(20, true);
    assert_eq!(state.item_start_row(0), 0);
    assert_eq!(state.item_start_row(1), 4);
    assert_eq!(state.item_start_row(2), 8);
    assert_eq!(state.item_start_row(3), 10);
    assert_eq!(state.item_ordinal(2), 1);
    assert_eq!(state.item_ordinal(3), 1);
}

#[test]
fn appended_song_section_tracks_card_and_compact_layouts() {
    let mut state = SearchController::default();
    state.set_results(vec![item(SearchItemKind::Artist, "a")], 1, true);
    state.set_viewport(8, true);
    assert_eq!(
        state.append_results(vec![item(SearchItemKind::Song, "s")]),
        1
    );
    assert_eq!(state.item_start_row(1) + state.divider_rows(1), 5);
    state.set_viewport(8, false);
    assert_eq!(state.item_start_row(1) + state.divider_rows(1), 2);
}

#[test]
fn resize_and_focus_keep_visible_range_deterministic() {
    let mut state = SearchController::default();
    state.set_results(
        (0..8)
            .map(|i| item(SearchItemKind::Song, &format!("s{i}")))
            .collect(),
        0,
        false,
    );
    state.set_viewport(3, false);
    state.set_focus(6);
    assert_eq!(state.effective_scroll_rows(), 4);
    assert_eq!(state.visible_range(4, 7), 4..7);
    state.set_viewport(2, false);
    assert_eq!(state.effective_scroll_rows(), 5);
}

[Showing lines 1220-1295 of 1296. Use :1296 to continue]