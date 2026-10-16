use super::super::owned_task::spawn_shared;
use super::super::playback_controller::PlaybackTrack;
use super::super::playlist::PlaylistPageFetch;
use super::super::playlist::PlaylistPageTask;
use super::super::playlist::PlaylistState;
use super::super::playlist_pagination::PlaylistPagination;
use super::super::playlist_pagination::apply_page;
use super::super::tracks::PlaylistTrack;
use super::super::tracks::PlaylistTrackKind;
use crate::data::playback_session::PlaylistCursor;
use crate::render::wake::WakeSignal;

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
    pagination.0.borrow_mut().pending = Some(spawn_shared(future, WakeSignal::default()));
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
