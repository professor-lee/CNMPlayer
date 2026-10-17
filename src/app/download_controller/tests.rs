use super::super::download::DownloadRequest;
use super::super::download::DownloadState;
use super::super::download::DownloadTarget;
use super::super::download_controller::PendingDownloads;
use crate::data::config::AudioQuality;
use std::path::PathBuf;

fn request(song_id: &str) -> DownloadRequest {
    DownloadRequest {
        song_id: song_id.to_string(),
        level: AudioQuality::Exhigh,
        title: song_id.to_string(),
        artist: "artist".to_string(),
        album: "album".to_string(),
        target: DownloadTarget {
            dir: PathBuf::from("downloads"),
            base: song_id.to_string(),
        },
    }
}

#[test]
fn second_toggle_cancels_pending_without_admission() {
    let mut pending = PendingDownloads::default();
    pending.defer(pending.origin(), request("first"));
    assert!(pending.take_ready(|_| None).is_none());
    assert!(pending.cancel("first"));
    assert!(
        pending
            .take_ready(|_| panic!("cancelled intent must not look up a file"))
            .is_none()
    );
    assert!(!pending.cancel("first"));
}

#[test]
fn single_click_is_admitted_once_when_absence_becomes_known() {
    let mut pending = PendingDownloads::default();
    pending.defer(pending.origin(), request("first"));
    assert!(pending.take_ready(|_| None).is_none());
    assert!(pending.take_ready(|_| None).is_none());
    let admitted = pending
        .take_ready(|_| Some(DownloadState::NotDownloaded))
        .unwrap();
    assert_eq!(admitted.song_id, "first");
    assert_eq!(admitted.target.dir, PathBuf::from("downloads"));
    assert!(
        pending
            .take_ready(|_| panic!("intent must be consumed once"))
            .is_none()
    );
}

#[test]
fn different_songs_keep_click_order_and_can_be_cancelled_independently() {
    let mut pending = PendingDownloads::default();
    let origin = pending.origin();
    pending.defer(origin, request("first"));
    pending.defer(origin, request("cancelled"));
    pending.defer(origin, request("third"));
    assert!(pending.cancel("cancelled"));
    assert!(
        pending
            .take_ready(|request| {
                assert_eq!(request.song_id, "first");
                None
            })
            .is_none()
    );
    assert_eq!(
        pending
            .take_ready(|_| Some(DownloadState::NotDownloaded))
            .unwrap()
            .song_id,
        "first"
    );
    assert_eq!(
        pending
            .take_ready(|_| Some(DownloadState::NotDownloaded))
            .unwrap()
            .song_id,
        "third"
    );
    assert!(pending.is_empty());
}

#[test]
fn existing_or_busy_songs_do_not_produce_replacement_requests() {
    let mut pending = PendingDownloads::default();
    let origin = pending.origin();
    pending.defer(origin, request("existing"));
    pending.defer(origin, request("busy"));
    pending.defer(origin, request("absent"));
    let admitted = pending
        .take_ready(|request| match request.song_id.as_str() {
            "existing" => Some(DownloadState::Done),
            "busy" => Some(DownloadState::Downloading),
            "absent" => Some(DownloadState::NotDownloaded),
            _ => panic!("unexpected request"),
        })
        .unwrap();
    assert_eq!(admitted.song_id, "absent");
    assert!(pending.is_empty());
}

#[test]
fn root_change_clears_waiters_and_rejects_old_root_origin() {
    let mut pending = PendingDownloads::default();
    let old_origin = pending.origin();
    pending.defer(old_origin, request("waiting"));
    pending.invalidate_root();
    pending.defer(old_origin, request("late_old_root"));
    assert!(
        pending
            .take_ready(|_| panic!("stale roots must not query or admit"))
            .is_none()
    );
    pending.defer(pending.origin(), request("new_root"));
    assert_eq!(
        pending
            .take_ready(|_| Some(DownloadState::NotDownloaded))
            .unwrap()
            .song_id,
        "new_root"
    );
}

#[test]
fn logout_clears_waiters_and_rejects_prelogout_origin() {
    let mut pending = PendingDownloads::default();
    let old_origin = pending.origin();
    pending.defer(old_origin, request("waiting"));
    pending.invalidate_session();
    pending.defer(old_origin, request("late_old_session"));
    assert!(
        pending
            .take_ready(|_| panic!("old sessions must not query or admit"))
            .is_none()
    );
    pending.defer(pending.origin(), request("new_session"));
    assert_eq!(
        pending
            .take_ready(|_| Some(DownloadState::NotDownloaded))
            .unwrap()
            .song_id,
        "new_session"
    );
}
