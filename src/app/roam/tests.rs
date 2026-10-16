use super::PrivateRoamState;
use super::merge_private_roam_refresh;
use crate::app::tracks::{PlaylistTrack, PlaylistTrackKind};

fn track(id: &str, cover: Option<&str>) -> PlaylistTrack {
    PlaylistTrack {
        kind: PlaylistTrackKind::Song,
        id: Some(id.to_string()),
        title: id.to_string(),
        artist: String::new(),
        album: String::new(),
        duration: String::new(),
        duration_ms: 0,
        cover_url: cover.map(str::to_string),
    }
}

#[test]
fn unplayed_roam_uses_first_song_cover_on_load_and_restore() {
    for previous_cover in [None, Some("stale-first-cover")] {
        let mut state = PrivateRoamState {
            tracks: vec![track("first", Some("first-cover"))],
            cover_url: previous_cover.map(str::to_string),
            ..PrivateRoamState::default()
        };

        state.refresh_cover();

        assert_eq!(state.cover_url.as_deref(), Some("first-cover"));
    }
}

#[test]
fn unplayed_roam_uses_new_first_song_cover_after_daily_refresh() {
    let mut state = PrivateRoamState {
        tracks: vec![track("old", Some("old-cover"))],
        cover_url: Some("old-cover".to_string()),
        ..PrivateRoamState::default()
    };
    (state.tracks, state.last_played_index) = merge_private_roam_refresh(
        &state.tracks,
        state.last_played_index,
        vec![track("new", Some("new-cover"))],
    );

    state.refresh_cover();

    assert_eq!(state.cover_url.as_deref(), Some("new-cover"));
}

#[test]
fn last_played_roam_cover_wins_on_restore_and_daily_refresh() {
    let mut state = PrivateRoamState {
        tracks: vec![
            track("first", Some("first-cover")),
            track("played", Some("track-cover")),
        ],
        last_played_index: Some(1),
        last_played_cover_url: Some("remembered-cover".to_string()),
        ..PrivateRoamState::default()
    };
    state.refresh_cover();
    assert_eq!(state.cover_url.as_deref(), Some("remembered-cover"));

    (state.tracks, state.last_played_index) = merge_private_roam_refresh(
        &state.tracks,
        state.last_played_index,
        vec![track("new", Some("new-cover"))],
    );
    state.refresh_cover();

    assert_eq!(state.last_played_index, Some(0));
    assert_eq!(state.cover_url.as_deref(), Some("remembered-cover"));
}

#[test]
fn last_played_roam_uses_song_cover_when_cover_memory_is_missing() {
    let mut state = PrivateRoamState {
        tracks: vec![
            track("first", Some("first-cover")),
            track("played", Some("played-cover")),
        ],
        last_played_index: Some(1),
        ..PrivateRoamState::default()
    };

    state.refresh_cover();

    assert_eq!(state.cover_url.as_deref(), Some("played-cover"));
}

#[test]
fn orphaned_last_played_cover_does_not_override_first_song() {
    for index in [None, Some(5)] {
        let mut state = PrivateRoamState {
            tracks: vec![track("first", Some("first-cover"))],
            last_played_index: index,
            last_played_cover_url: Some("orphaned-cover".to_string()),
            ..PrivateRoamState::default()
        };

        state.refresh_cover();

        assert_eq!(state.cover_url.as_deref(), Some("first-cover"));
    }
}

#[test]
fn empty_or_coverless_unplayed_roam_clears_stale_cover() {
    for tracks in [
        Vec::new(),
        vec![track("first", None), track("second", Some("second-cover"))],
    ] {
        let mut state = PrivateRoamState {
            tracks,
            cover_url: Some("stale-cover".to_string()),
            last_played_cover_url: Some("orphaned-cover".to_string()),
            ..PrivateRoamState::default()
        };

        state.refresh_cover();

        assert_eq!(state.cover_url, None);
    }
}
