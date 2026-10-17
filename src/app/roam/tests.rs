use super::PrivateRoamState;
use super::bound_private_roam_queue;
use super::merge_private_roam_refresh;
use super::private_roam_window;
use super::sync_private_roam_playlist;
use crate::app::home::HOME_PRIVATE_ROAM_TILE_ID;
use crate::app::playback_controller::PlaybackTrack;
use crate::app::playlist::PlaylistState;
use crate::app::tracks::{PlaylistTrack, PlaylistTrackKind};
use crate::data::private_roam::{PrivateRoamRecord, PrivateRoamTrack};

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

fn numbered_tracks(start: usize, end: usize) -> Vec<PlaylistTrack> {
    (start..end)
        .map(|id| track(&id.to_string(), None))
        .collect()
}

fn playback_tracks(tracks: &[PlaylistTrack]) -> Vec<PlaybackTrack> {
    tracks
        .iter()
        .filter_map(PlaybackTrack::from_playlist_track)
        .collect()
}

#[test]
fn roam_append_evicts_oldest_played_prefix_and_rebases_all_mirrors() {
    let tracks = numbered_tracks(0, 256);
    let mut queue = playback_tracks(&tracks);
    let mut playlist = PlaylistState {
        id: Some(HOME_PRIVATE_ROAM_TILE_ID.to_string()),
        tracks: tracks.clone(),
        focused_idx: 250,
        scroll_offset: 240,
        visible_rows: 20,
        ..PlaylistState::default()
    };
    let mut snapshot = playlist.clone();
    snapshot.focused_idx = 3;
    snapshot.scroll_offset = 0;
    let generation = playlist.generation;
    let mut state = PrivateRoamState {
        tracks,
        last_played_index: Some(255),
        ..PrivateRoamState::default()
    };
    // The selected target, not the previous playback index, anchors eviction.
    let mut current = Some(255);
    let mut page_advance = Some(true);
    queue.extend(state.append_tracks(numbered_tracks(255, 265)));
    bound_private_roam_queue(
        &mut queue,
        &mut current,
        Some(HOME_PRIVATE_ROAM_TILE_ID),
        &mut page_advance,
    );
    sync_private_roam_playlist(&mut playlist, &state.tracks);
    sync_private_roam_playlist(&mut snapshot, &state.tracks);

    assert_eq!(state.tracks.len(), 256);
    assert_eq!(state.tracks[0].id.as_deref(), Some("9"));
    assert_eq!(state.last_played_index, Some(246));
    assert_eq!(current, Some(246));
    let index = current.unwrap();
    assert_eq!(queue[index].song_id, "255");
    assert_eq!(queue[index + 1].song_id, "256");
    assert_eq!(queue.last().unwrap().song_id, "264");
    assert_eq!(queue.len(), 256);
    assert_eq!(page_advance, None);
    assert_eq!(playlist.focused_idx, 241);
    assert_eq!(playlist.scroll_offset, 231);
    assert_eq!(
        playlist.tracks[playlist.focused_idx].id.as_deref(),
        Some("250")
    );
    assert_eq!(playlist.total_tracks, Some(256));
    assert_ne!(playlist.generation, generation);
    assert_eq!(snapshot.focused_idx, 0);
    assert_eq!(snapshot.scroll_offset, 0);
    assert_eq!(snapshot.tracks.len(), 256);
}

#[test]
fn repeated_roam_append_keeps_state_and_queue_bounded_in_upcoming_order() {
    let mut state = PrivateRoamState {
        tracks: numbered_tracks(0, 9),
        ..PrivateRoamState::default()
    };
    let mut queue = playback_tracks(&state.tracks);
    let mut current = None;
    let mut page_advance = None;
    for start in (9..2709).step_by(9) {
        state.last_played_index = Some(state.tracks.len() - 1);
        current = Some(queue.len() - 1);
        queue.extend(state.append_tracks(numbered_tracks(start, start + 9)));
        bound_private_roam_queue(
            &mut queue,
            &mut current,
            Some(HOME_PRIVATE_ROAM_TILE_ID),
            &mut page_advance,
        );
        assert!(state.tracks.len() <= 256);
        assert!(queue.len() <= 256);
        assert_eq!(queue[current.unwrap()].song_id, (start - 1).to_string());
        assert_eq!(queue[current.unwrap() + 1].song_id, start.to_string());
        assert_eq!(state.last_played_index, current);
    }
    assert_eq!(current, Some(246));
    assert_eq!(queue.len(), 256);
    assert_eq!(queue.first().unwrap().song_id, "2453");
    assert_eq!(queue.last().unwrap().song_id, "2708");
}

#[test]
fn legacy_roam_load_prioritizes_current_and_nearest_upcoming_songs() {
    for (current, first, last, rebased) in [
        (Some(300), "300", "555", Some(0)),
        (Some(900), "744", "999", Some(156)),
        (None, "0", "255", None),
    ] {
        let record = PrivateRoamRecord {
            tracks: (0..1000)
                .map(|id| PrivateRoamTrack {
                    song_id: id.to_string(),
                    ..PrivateRoamTrack::default()
                })
                .collect(),
            last_played_index: current,
            last_played_cover_url: Some("remembered-cover".to_string()),
            last_refresh_day: Some(42),
            ..PrivateRoamRecord::default()
        };
        let state = PrivateRoamState::from_record(record);
        assert_eq!(state.tracks.len(), 256);
        assert_eq!(state.tracks.first().unwrap().id.as_deref(), Some(first));
        assert_eq!(state.tracks.last().unwrap().id.as_deref(), Some(last));
        assert_eq!(state.last_played_index, rebased);
        assert_eq!(state.last_refresh_day, Some(42));
        if current.is_some() {
            assert_eq!(state.cover_url.as_deref(), Some("remembered-cover"));
        }
    }
}

#[test]
fn oversized_roam_restore_and_persistence_use_the_same_current_window() {
    let mut queue = playback_tracks(&numbered_tracks(0, 1000));
    let mut current = Some(300);
    let mut page_advance = Some(false);
    // Persistence copies this exact slice and stores its rebased index.
    let (window, persisted_index) = private_roam_window(queue.len(), current);
    assert_eq!(window, 300..556);
    assert_eq!(persisted_index, Some(0));
    bound_private_roam_queue(
        &mut queue,
        &mut current,
        Some(HOME_PRIVATE_ROAM_TILE_ID),
        &mut page_advance,
    );
    assert_eq!(queue.len(), 256);
    assert_eq!(current, persisted_index);
    assert_eq!(queue[0].song_id, "300");
    assert_eq!(queue[1].song_id, "301");
    assert_eq!(queue[255].song_id, "555");
    assert_eq!(page_advance, None);
}

#[test]
fn roam_retention_does_not_change_ordinary_queues_or_browse_pages() {
    for source in [None, Some("ordinary-playlist")] {
        let mut queue = playback_tracks(&numbered_tracks(0, 600));
        let mut current = Some(599);
        let mut page_advance = Some(true);
        bound_private_roam_queue(&mut queue, &mut current, source, &mut page_advance);
        assert_eq!(queue.len(), 600);
        assert_eq!(queue[599].song_id, "599");
        assert_eq!(current, Some(599));
        assert_eq!(page_advance, Some(true));
    }
    let mut playlist = PlaylistState {
        id: Some("ordinary-playlist".to_string()),
        tracks: numbered_tracks(0, 600),
        focused_idx: 599,
        scroll_offset: 580,
        total_tracks: Some(600),
        ..PlaylistState::default()
    };
    let generation = playlist.generation;
    sync_private_roam_playlist(&mut playlist, &numbered_tracks(1000, 1256));
    assert_eq!(playlist.tracks.len(), 600);
    assert_eq!(playlist.focused_idx, 599);
    assert_eq!(playlist.scroll_offset, 580);
    assert_eq!(playlist.total_tracks, Some(600));
    assert_eq!(playlist.generation, generation);
}

#[test]
fn oversized_daily_roam_refresh_retains_current_before_bounded_new_tracks() {
    let (tracks, current) = merge_private_roam_refresh(
        &numbered_tracks(0, 600),
        Some(500),
        numbered_tracks(1000, 1600),
    );
    assert_eq!(tracks.len(), 256);
    assert_eq!(current, Some(0));
    assert_eq!(tracks[0].id.as_deref(), Some("500"));
    assert_eq!(tracks[1].id.as_deref(), Some("1000"));
    assert_eq!(tracks[255].id.as_deref(), Some("1254"));
}

#[test]
fn roam_history_window_clears_invalid_indices_and_handles_empty_history() {
    assert_eq!(private_roam_window(0, Some(0)), (0..0, None));
    assert_eq!(private_roam_window(600, Some(600)), (0..256, None));
    assert_eq!(private_roam_window(3, Some(2)), (0..3, Some(2)));
}

#[test]
fn oversized_initial_roam_fetch_is_bounded_before_reaching_browse() {
    let fetch = super::private_roam_playlist_fetch(
        crate::data::config::Language::En,
        numbered_tracks(0, 1000),
        None,
    );
    assert_eq!(fetch.tracks.len(), 256);
    assert_eq!(fetch.tracks[0].id.as_deref(), Some("0"));
    assert_eq!(fetch.tracks[255].id.as_deref(), Some("255"));
    assert_eq!(fetch.total_tracks, Some(256));
    assert_eq!(fetch.next_offset, 256);
    assert!(!fetch.has_more);
    assert!(!fetch.paginated);
}

#[test]
fn roam_append_preserves_current_with_different_state_and_queue_offsets() {
    for (state_start, queue_start, end, state_index, queue_index) in
        [(100, 200, 356, 246, 155), (100, 0, 256, 155, 246)]
    {
        let mut state = PrivateRoamState {
            tracks: numbered_tracks(state_start, end),
            last_played_index: Some(end - state_start - 1),
            ..PrivateRoamState::default()
        };
        let mut queue = playback_tracks(&numbered_tracks(queue_start, end));
        let mut current = Some(queue.len() - 1);
        let mut page_advance = None;
        queue.extend(state.append_tracks(numbered_tracks(end, end + 9)));
        bound_private_roam_queue(
            &mut queue,
            &mut current,
            Some(HOME_PRIVATE_ROAM_TILE_ID),
            &mut page_advance,
        );
        assert_eq!(state.last_played_index, Some(state_index));
        assert_eq!(current, Some(queue_index));
        assert_eq!(queue[queue_index].song_id, (end - 1).to_string());
        assert_eq!(queue[queue_index + 1].song_id, end.to_string());
        assert_eq!(
            state.tracks[state_index].id.as_deref(),
            Some((end - 1).to_string().as_str())
        );
        assert!(state.tracks.len() <= 256);
        assert!(queue.len() <= 256);
    }
}

#[test]
fn roam_memory_restore_preserves_duplicate_occurrence_after_empty_id_filtering() {
    for (ids, current, expected_index) in [
        (vec!["A", "B", "A", "C"], 2, 2),
        (vec!["A", "", "B", "", "A", "C"], 4, 2),
    ] {
        let state = PrivateRoamState::from_record(PrivateRoamRecord {
            tracks: ids
                .into_iter()
                .enumerate()
                .map(|(index, id)| PrivateRoamTrack {
                    song_id: id.to_string(),
                    title: index.to_string(),
                    ..PrivateRoamTrack::default()
                })
                .collect(),
            last_played_index: Some(current),
            ..PrivateRoamRecord::default()
        });
        assert_eq!(state.last_played_index, Some(expected_index));
        assert_eq!(state.tracks[expected_index].title, current.to_string());
        assert_eq!(state.tracks[expected_index + 1].id.as_deref(), Some("C"));
    }
}

#[test]
fn roam_playback_restore_preserves_duplicate_occurrence_after_empty_id_filtering() {
    use crate::data::playback_session::PlaybackSessionTrack;

    for (ids, current, expected_index) in [
        (vec!["A", "B", "A", "C"], 2, 2),
        (vec!["A", "", "B", "  ", "A", "C"], 4, 2),
    ] {
        let tracks: Vec<_> = ids
            .into_iter()
            .enumerate()
            .map(|(index, id)| PlaybackSessionTrack {
                song_id: id.to_string(),
                title: index.to_string(),
                ..PlaybackSessionTrack::default()
            })
            .collect();
        let (window, _) = private_roam_window(tracks.len(), Some(current));
        let restored_index =
            super::private_roam_restored_index(&tracks, Some(current), &window, |track| {
                !track.song_id.trim().is_empty()
            });
        let retained: Vec<_> = tracks[window]
            .iter()
            .filter(|track| !track.song_id.trim().is_empty())
            .collect();
        assert_eq!(restored_index, Some(expected_index));
        assert_eq!(retained[expected_index].title, current.to_string());
        assert_eq!(retained[expected_index + 1].song_id, "C");
    }
}

#[test]
fn oversized_roam_restore_rebases_occurrence_after_window_and_empty_filtering() {
    let tracks: Vec<_> = (0..600)
        .map(|index| PrivateRoamTrack {
            song_id: match index {
                350 | 400 => String::new(),
                360 | 590 => "A".to_string(),
                591 => "C".to_string(),
                _ => index.to_string(),
            },
            title: index.to_string(),
            ..PrivateRoamTrack::default()
        })
        .collect();
    let (window, _) = private_roam_window(tracks.len(), Some(590));
    assert_eq!(window, 344..600);
    let index = super::private_roam_restored_index(&tracks, Some(590), &window, |track| {
        !track.song_id.is_empty()
    });
    assert_eq!(index, Some(244));
    let state = PrivateRoamState::from_record(PrivateRoamRecord {
        tracks,
        last_played_index: Some(590),
        ..PrivateRoamRecord::default()
    });
    assert_eq!(state.last_played_index, Some(244));
    assert_eq!(state.tracks[244].title, "590");
    assert_eq!(state.tracks[245].id.as_deref(), Some("C"));
}
