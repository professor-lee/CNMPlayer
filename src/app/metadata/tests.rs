use super::*;
use ncm_api::ApiResponse;
use serde_json::json;
use std::collections::VecDeque;

fn response(body: serde_json::Value) -> anyhow::Result<ApiResponse> {
    Ok(ApiResponse {
        status: 200,
        body,
        cookie: Vec::new(),
    })
}

async fn fetch_responses(
    responses: impl IntoIterator<Item = anyhow::Result<ApiResponse>>,
) -> (Option<Vec<LyricLine>>, usize) {
    let mut responses: VecDeque<_> = responses.into_iter().collect();
    let mut calls = 0;
    let lyrics = fetch_lyrics_from(async |_new_endpoint| {
        calls += 1;
        responses.pop_front().expect("unexpected lyric retry")
    })
    .await;
    (lyrics, calls)
}

#[compio::test]
async fn successful_empty_and_nolyric_responses_are_known_absent() {
    for body in [
        json!({"code": 200, "lrc": {"lyric": "  "}, "yrc": null}),
        json!({"code": 200, "nolyric": true}),
        json!({"code": "200", "uncollected": true}),
        json!({"code": 200}),
    ] {
        let (lyrics, calls) = fetch_responses([response(body.clone()), response(body)]).await;
        assert!(lyrics.is_some_and(|lines| lines.is_empty()));
        assert_eq!(calls, 2, "absence must still consult the fallback endpoint");
    }
}

#[compio::test]
async fn positive_fallback_wins_over_absence_and_failures() {
    for first in [
        response(json!({"code": 200, "nolyric": true})),
        response(json!({"code": 200, "lrc": {"lyric": ""}})),
        response(json!({"code": 502})),
        response(json!({"code": 200, "yrc": {"lyric": "[broken"}})),
        Err(anyhow::anyhow!("network unavailable")),
    ] {
        let (lyrics, calls) = fetch_responses([
            first,
            response(json!({"code": 200, "lrc": {"lyric": "[00:01.00]fallback verse"}})),
        ])
        .await;
        assert_eq!(lyrics.unwrap()[0].text, "fallback verse");
        assert_eq!(calls, 2);
    }
}

#[compio::test]
async fn positive_new_lyrics_do_not_request_legacy_fallback() {
    let (lyrics, calls) = fetch_responses([response(json!({
        "code": 200,
        "yrc": {"lyric": "[1000,1000](1000,1000,0)new verse"}
    }))])
    .await;
    assert_eq!(lyrics.unwrap()[0].text, "new verse");
    assert_eq!(calls, 1);
}

#[compio::test]
async fn api_network_and_malformed_lyrics_remain_retryable() {
    let failures = || {
        vec![
            response(json!({"code": 502})),
            response(json!({"code": 502, "lrc": {"lyric": "must not accept error text"}})),
            response(json!({"lrc": {"lyric": "missing success code"}})),
            Ok(ApiResponse {
                status: 500,
                body: json!({"code": 200}),
                cookie: Vec::new(),
            }),
            response(json!({"code": 200, "yrc": {"lyric": "[broken"}})),
            response(json!({"code": 200, "lrc": {"lyric": 42}})),
            response(json!({"code": 200, "lrc": ["invalid shape"]})),
            response(json!({"code": 200, "lrc": {"unexpected": "nonempty payload"}})),
            response(json!({"code": 200, "nolyric": true, "yrc": {"lyric": "[broken"}})),
            response(json!({"code": 200, "lrc": {"lyric": "{broken JSON"}})),
            Err(anyhow::anyhow!("network unavailable")),
            Err(anyhow::anyhow!("response JSON decoding failed")),
        ]
    };
    for failed in failures() {
        let (lyrics, calls) =
            fetch_responses([failed, response(json!({"code": 200, "nolyric": true}))]).await;
        assert!(
            lyrics.is_none(),
            "failure must not become a negative cache entry"
        );
        assert_eq!(calls, 2);
    }
    for failed in failures() {
        let (lyrics, _) =
            fetch_responses([response(json!({"code": 200, "nolyric": true})), failed]).await;
        assert!(lyrics.is_none());
    }
}

fn track(song_id: &str) -> PlaybackTrack {
    PlaybackTrack {
        song_id: song_id.into(),
        title: String::new(),
        artist: String::new(),
        album: String::new(),
        duration_ms: 0,
        cover_url: None,
        cover: None,
        lyrics: None,
    }
}

#[test]
fn known_absence_suppresses_current_and_queue_retries() {
    let mut current = Some(track("song"));
    let mut queue = vec![track("song")];
    let mut inflight = Some("song".into());
    apply_lyric_result(
        LyricFetchResult {
            song_id: "song".into(),
            generation: 4,
            lyrics: Some(Vec::new()),
        },
        4,
        &mut inflight,
        &mut current,
        &mut queue,
        Some(0),
    );
    assert!(inflight.is_none());
    assert!(
        current
            .as_ref()
            .unwrap()
            .lyrics
            .as_ref()
            .unwrap()
            .is_empty()
    );
    assert!(queue[0].lyrics.as_ref().unwrap().is_empty());
    let now = Instant::now();
    assert!(!lyric_fetch_due(current.as_ref(), None, None, now));
    assert!(!lyric_fetch_due(Some(&queue[0]), None, None, now));
}

#[test]
fn failed_lyrics_remain_eligible_after_existing_retry_delay() {
    let mut current = Some(track("song"));
    let mut queue = vec![track("song")];
    let mut inflight = Some("song".into());
    apply_lyric_result(
        LyricFetchResult {
            song_id: "song".into(),
            generation: 4,
            lyrics: None,
        },
        4,
        &mut inflight,
        &mut current,
        &mut queue,
        Some(0),
    );
    assert!(current.as_ref().unwrap().lyrics.is_none());
    assert!(queue[0].lyrics.is_none());
    assert!(inflight.is_none());
    let attempted = Instant::now();
    assert!(!lyric_fetch_due(
        current.as_ref(),
        None,
        Some(attempted),
        attempted
    ));
    let ready = attempted + Duration::from_millis(LYRICS_FETCH_RETRY_MS);
    assert!(lyric_fetch_due(
        current.as_ref(),
        None,
        Some(attempted),
        ready
    ));
    assert!(lyric_fetch_due(Some(&queue[0]), None, None, ready));
    assert!(!lyric_fetch_due(
        current.as_ref(),
        Some("song"),
        None,
        ready
    ));
}

#[test]
fn stale_lyrics_cannot_poison_current_or_queue_absence_cache() {
    for (song_id, generation) in [("song", 3), ("old-song", 4)] {
        let mut current = Some(track("song"));
        let mut queue = vec![track("song")];
        let mut inflight = Some("song".into());
        apply_lyric_result(
            LyricFetchResult {
                song_id: song_id.into(),
                generation,
                lyrics: Some(Vec::new()),
            },
            4,
            &mut inflight,
            &mut current,
            &mut queue,
            Some(0),
        );
        assert!(current.unwrap().lyrics.is_none());
        assert!(queue[0].lyrics.is_none());
        assert_eq!(inflight.as_deref(), Some("song"));
    }
}

#[test]
fn lyric_result_does_not_overwrite_existing_or_unrelated_queue_lyrics() {
    let mut current = Some(track("song"));
    let mut queue = vec![track("another-song")];
    let mut inflight = Some("song".into());
    apply_lyric_result(
        LyricFetchResult {
            song_id: "song".into(),
            generation: 4,
            lyrics: Some(Vec::new()),
        },
        4,
        &mut inflight,
        &mut current,
        &mut queue,
        Some(0),
    );
    assert!(current.as_ref().unwrap().lyrics.is_some());
    assert!(queue[0].lyrics.is_none());
    let positive = parse_lyric_response(&json!({"lrc": {"lyric": "[00:01.00]existing"}})).unwrap();
    current.as_mut().unwrap().lyrics = Some(positive);
    apply_lyric_result(
        LyricFetchResult {
            song_id: "song".into(),
            generation: 4,
            lyrics: Some(Vec::new()),
        },
        4,
        &mut inflight,
        &mut current,
        &mut queue,
        Some(0),
    );
    assert_eq!(current.unwrap().lyrics.unwrap()[0].text, "existing");
}
