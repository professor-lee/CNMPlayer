use super::ListenSession;
use crate::app::playback_controller::PlaybackTrack;
use std::time::Duration;
use std::time::Instant;

fn track(duration_secs: i64) -> PlaybackTrack {
    PlaybackTrack {
        song_id: "347230".into(),
        title: String::new(),
        artist: String::new(),
        album: String::new(),
        duration_ms: duration_secs * 1000,
        cover_url: None,
        cover: None,
        lyrics: None,
    }
}

#[test]
fn paused_time_is_not_counted_as_listening() {
    let start = Instant::now();
    let mut session = ListenSession::new(&track(240), Some("123"));
    session.observe(true, start);
    session.observe(false, start + Duration::from_secs(30));
    // 暂停 10 分钟后继续播放 15 秒。
    session.observe(true, start + Duration::from_secs(630));
    let now = start + Duration::from_secs(645);
    assert_eq!(session.listened(now), Duration::from_secs(45));
    assert_eq!(session.report(now), Some((45, "ui")));
}

#[test]
fn short_listens_are_not_reported() {
    let start = Instant::now();
    let mut session = ListenSession::new(&track(240), None);
    session.observe(true, start);
    assert_eq!(session.report(start + Duration::from_secs(19)), None);
}

#[test]
fn listening_close_to_full_duration_counts_as_playend() {
    let start = Instant::now();
    let mut session = ListenSession::new(&track(200), None);
    session.observe(true, start);
    assert_eq!(
        session.report(start + Duration::from_secs(193)),
        Some((193, "playend"))
    );
    assert_eq!(
        session.report(start + Duration::from_secs(150)),
        Some((150, "ui"))
    );
}

#[test]
fn only_numeric_netease_sources_are_reported() {
    assert_eq!(
        ListenSession::new(&track(1), Some("987654")).source_id,
        "987654"
    );
    assert_eq!(
        ListenSession::new(&track(1), Some("home-private-roam")).source_id,
        ""
    );
    assert_eq!(ListenSession::new(&track(1), None).source_id, "");
}
