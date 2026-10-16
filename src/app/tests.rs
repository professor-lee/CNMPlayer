
use super::api::ApiState;
use super::author::merge_artist_album_pages;
use super::cover_presentation::CoverPresentation;
use super::cover_state::CoverFetchState;
use super::input_controller::HitRect;
use super::likes::LikeMachine;
use super::likes::LikeToggleFuture;
use super::likes::LikeToggleTask;
use super::likes::LikeVerifyFuture;
use super::likes::LikeVerifyTask;
use super::likes::ToggleOutcome;
use super::metadata::validate_cached_cover;
use super::owned_task::SharedTask;
use super::owned_task::peek_shared_future;
use super::playlist::PlaylistState;
use super::presentation::VU_LUFS_FLOOR;
use super::presentation::lufs_to_bar_level;
use super::presentation::lufs_to_mean_square;
use super::presentation::mean_square_to_lufs;
use super::roam::merge_private_roam_refresh;
use super::roam::normalize_fm_song_items;
use super::search_controller::SearchController;
use super::search_controller::SearchItem;
use super::search_controller::SearchItemKind;
use super::search_controller::SearchScope;
use super::search_controller::parse_search_input;
use super::sidebar::HomeSidebarPageFetch;
use super::sidebar::HomeSidebarPlaylist;
use super::sidebar::HomeSidebarSection;
use super::sidebar::HomeSidebarState;
use super::sidebar::home_sidebar_wheel_target;
use super::startup_controller::startup_loading_progress;
use super::tracks::PlaylistTrack;
use super::tracks::PlaylistTrackKind;
use super::tracks::artist_name_segments;
use super::tracks::pick_artist_id;
use cyper::Client;
use ncm_api::ApiResponse;
use ratatui::style::Style;
use serde_json::Value;
use std::time::Duration;
use std::time::Instant;

#[compio::test]
async fn home_cover_loads_and_paints_when_multiple_tiles_complete_together() {
    use std::io::{Read, Write};
    let image = image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
        24,
        24,
        image::Rgb([220, 60, 30]),
    ));
    let mut encoded = std::io::Cursor::new(Vec::new());
    image
        .write_to(&mut encoded, image::ImageFormat::Png)
        .unwrap();
    let bytes = encoded.into_inner();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        for _ in 0..3 {
            let (mut socket, _) = listener.accept().unwrap();
            let mut request = [0; 2048];
            assert!(socket.read(&mut request).unwrap() > 0);
            write!(
                socket,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                bytes.len()
            )
            .unwrap();
            socket.write_all(&bytes).unwrap();
        }
    });
    let api = ApiState::new(None, Client::builder().no_proxy().build().unwrap()).unwrap();
    let mut tiles = [
        CoverFetchState::default(),
        CoverFetchState::default(),
        CoverFetchState::default(),
    ];
    for (index, cover) in tiles.iter_mut().enumerate() {
        cover.load(api.clone(), format!("http://{address}/{index}"));
    }
    let deadline = Instant::now() + Duration::from_secs(3);
    while tiles
        .iter()
        .any(|cover| peek_shared_future(&cover.image).is_none())
    {
        assert!(Instant::now() < deadline);
        compio::time::sleep(Duration::from_millis(1)).await;
    }
    let mut presentation = CoverPresentation::new(api.wake_signal());
    let mut terminal =
        ratatui::Terminal::new(ratatui::backend::TestBackend::new(8, 4)).unwrap();
    loop {
        presentation.poll();
        terminal
            .draw(|frame| {
                presentation.begin_frame();
                tiles[2].render(
                    frame,
                    &mut presentation,
                    frame.area(),
                    Style::default(),
                    None,
                    false,
                );
            })
            .unwrap();
        presentation.prepare();
        if terminal
            .backend()
            .buffer()
            .content
            .iter()
            .any(|cell| matches!(cell.fg, ratatui::style::Color::Rgb(..)))
        {
            break;
        }
        assert!(Instant::now() < deadline);
        compio::time::sleep(Duration::from_millis(1)).await;
    }
    server.join().unwrap();
}

fn test_artist_album_response(items: &[Value], more: bool) -> ApiResponse {
    ApiResponse {
        status: 200,
        body: serde_json::json!({
            "code": 200,
            "hotAlbums": items,
            "more": more,
        }),
        cookie: Vec::new(),
    }
}

#[test]
fn artist_album_pages_merge_all_release_metadata() {
    let first = vec![serde_json::json!({"id": 1, "name": "first"})];
    let second = vec![
        serde_json::json!({"id": 2, "name": "second"}),
        serde_json::json!({"id": 3, "name": "third"}),
    ];

    let merged = merge_artist_album_pages(vec![
        test_artist_album_response(&first, true),
        test_artist_album_response(&second, false),
    ])
    .unwrap();

    let items = merged
        .body
        .get("hotAlbums")
        .and_then(Value::as_array)
        .unwrap();
    assert_eq!(items.len(), 3);
    assert_eq!(items[0]["name"], "first");
    assert_eq!(items[2]["name"], "third");
}

#[test]
fn artist_album_pages_reject_incomplete_page_instead_of_returning_partial_data() {
    let first = vec![serde_json::json!({"id": 1, "name": "first"})];
    let malformed = ApiResponse {
        status: 200,
        body: serde_json::json!({"code": 200, "more": false}),
        cookie: Vec::new(),
    };

    let result =
        merge_artist_album_pages(vec![test_artist_album_response(&first, true), malformed]);

    assert!(result.is_err());
}

#[test]
fn cached_cover_requires_decodable_image_and_terminal_marker() {
    for format in [image::ImageFormat::Png, image::ImageFormat::Jpeg] {
        let mut encoded = std::io::Cursor::new(Vec::new());
        image::DynamicImage::new_rgb8(2, 2)
            .write_to(&mut encoded, format)
            .unwrap();
        let complete = encoded.into_inner();
        assert_eq!(
            validate_cached_cover(complete.clone()),
            Some(complete.clone())
        );

        let mut incomplete = complete;
        incomplete.pop();
        assert!(validate_cached_cover(incomplete).is_none());
    }

    assert!(validate_cached_cover(Vec::new()).is_none());
    assert!(validate_cached_cover(b"not an image\xff\xd9".to_vec()).is_none());
}

/// 无后缀默认走混合搜索；后缀命中时只搜该类型（`@artist` 与 `@author` 同义）。
#[test]
fn parse_search_input_resolves_scope() {
    assert_eq!(
        parse_search_input("test"),
        ("test".to_string(), SearchScope::Mixed)
    );
    assert_eq!(
        parse_search_input("test @list"),
        ("test".to_string(), SearchScope::Playlist)
    );
    assert_eq!(
        parse_search_input("test @artist"),
        ("test".to_string(), SearchScope::Author)
    );
    assert_eq!(
        parse_search_input("@author"),
        (String::new(), SearchScope::Author)
    );
}

fn search_item(kind: SearchItemKind, label: &str) -> SearchItem {
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

/// 视口按行滚动：底部推进多少行，顶部就退多少行（作者卡片被裁切而不是整块移出）。
#[test]
fn search_scroll_moves_by_rows() {
    let mut state = SearchController::default();
    state.set_results(
        vec![
            search_item(SearchItemKind::Artist, "artist-1"), // 行 0..4（卡片）
            search_item(SearchItemKind::Artist, "artist-2"), // 行 4..8
            search_item(SearchItemKind::Song, "song-1"),     // 行 8..10（分区线 1 + 单曲 1）
            search_item(SearchItemKind::Song, "song-2"),     // 行 10..11
            search_item(SearchItemKind::Song, "song-3"),     // 行 11..12
        ],
        0,
        false,
    );
    state.set_viewport(6, true);

    // 进入第二张卡片：底边对齐 8，顶部退 2 行（底边前进 2 行）。
    state.set_focus(1);
    assert_eq!(state.effective_scroll_rows(), 2);

    // 进入带分区线的单曲：底边 8 → 10，顶部同样只退 2 行。
    assert!(state.focus_next());
    assert_eq!(state.effective_scroll_rows(), 4);

    // 纯单曲：底部只推进 1 行，顶部也只退 1 行（卡片只被裁掉 1 行）。
    assert!(state.focus_next());
    assert_eq!(state.effective_scroll_rows(), 5);

    // 回退：目标条目仍完整可见时不滚动。
    assert!(state.focus_prev());
    assert_eq!(state.effective_scroll_rows(), 5);

    // 继续回退到被裁掉的那张卡片（起点 4 已在视口上方）：顶部只回退 1 行。
    assert!(state.focus_prev());
    assert_eq!(state.effective_scroll_rows(), 4);
}

/// 列表全是卡片时，一次推进就是一个卡片高度，顶部也退一个卡片高度。
#[test]
fn search_scroll_moves_by_card_height_on_author_list() {
    let mut state = SearchController::default();
    state.set_results(
        vec![
            search_item(SearchItemKind::Artist, "artist-1"),
            search_item(SearchItemKind::Artist, "artist-2"),
            search_item(SearchItemKind::Artist, "artist-3"),
        ],
        0,
        false,
    );
    state.set_viewport(6, true);

    state.set_focus(1);
    assert_eq!(state.effective_scroll_rows(), 2);
    assert!(state.focus_next());
    assert_eq!(state.effective_scroll_rows(), 6);
}

/// 视口行数变化后聚焦条目仍然完整可见（窗口缩放 / 小窗模式）。
#[test]
fn search_viewport_resize_keeps_focus_visible() {
    let mut state = SearchController::default();
    state.set_results(
        (0..20)
            .map(|i| search_item(SearchItemKind::Song, &format!("song-{i}")))
            .collect(),
        0,
        false,
    );
    state.set_viewport(5, true);
    state.set_focus(9);
    assert_eq!(state.effective_scroll_rows(), 5);

    state.set_viewport(3, true);
    assert_eq!(state.effective_scroll_rows(), 7);
    assert_eq!(state.page_items(), 3);
}

fn track(id: &str) -> PlaylistTrack {
    PlaylistTrack {
        kind: PlaylistTrackKind::Song,
        id: Some(id.to_string()),
        title: format!("title-{}", id),
        artist: "artist".to_string(),
        album: "album".to_string(),
        cover_url: Some(format!("https://example.com/{}.jpg", id)),
        duration_ms: 1000,
        duration: "00:01".to_string(),
    }
}
#[test]
fn playlist_state_appends_pages_without_resetting_focus() {
    let mut state = PlaylistState::default();
    state.set_tracks(vec![track("a"), track("b")]);
    state.set_focus(1);
    state.total_tracks = Some(4);

    state.append_tracks(vec![track("c"), track("d")]);

    let ids: Vec<Option<String>> = state.tracks.iter().map(|item| item.id.clone()).collect();
    assert_eq!(
        ids,
        vec![
            Some("a".to_string()),
            Some("b".to_string()),
            Some("c".to_string()),
            Some("d".to_string()),
        ]
    );
    assert_eq!(state.focused_idx, 1);
    assert_eq!(state.total_tracks, Some(4));
}

#[test]
fn sidebar_state_appends_requested_section_page() {
    let mut state = HomeSidebarState {
        created_has_more: true,
        created_next_offset: 100,
        created_playlists: vec![HomeSidebarPlaylist {
            id: Some("first".to_string()),
            title: "first".to_string(),
            creator: "creator".to_string(),
            track_count: 1,
        }],
        ..HomeSidebarState::default()
    };
    state.append_section_page(HomeSidebarPageFetch {
        section: HomeSidebarSection::Created,
        items: vec![HomeSidebarPlaylist {
            id: Some("second".to_string()),
            title: "second".to_string(),
            creator: "creator".to_string(),
            track_count: 2,
        }],
        next_offset: 101,
        has_more: false,
    });

    assert_eq!(state.created_playlists.len(), 2);
    assert_eq!(state.created_next_offset, 101);
    assert!(!state.created_has_more);
}

#[test]
fn merge_refresh_keeps_last_played_at_head() {
    let old = vec![track("a"), track("b"), track("c")];
    let fetched = vec![track("x"), track("y"), track("b")];

    let (merged, index) = merge_private_roam_refresh(&old, Some(1), fetched);

    // 最后播放的 b 保留在首位，x/y 追加，重复的 b 跳过
    let ids: Vec<Option<String>> = merged.iter().map(|t| t.id.clone()).collect();
    assert_eq!(
        ids,
        vec![
            Some("b".to_string()),
            Some("x".to_string()),
            Some("y".to_string())
        ]
    );
    assert_eq!(index, Some(0));
}

#[test]
fn merge_refresh_without_playback_replaces_all() {
    let old = vec![track("a"), track("b")];
    let fetched = vec![track("x"), track("y")];

    let (merged, index) = merge_private_roam_refresh(&old, None, fetched);

    let ids: Vec<Option<String>> = merged.iter().map(|t| t.id.clone()).collect();
    assert_eq!(ids, vec![Some("x".to_string()), Some("y".to_string())]);
    assert_eq!(index, None);
}

#[test]
fn startup_progress_never_rewinds_and_stops_before_completion() {
    // 5 步计划：进度只随真实完成的步数与当前步的缓动单调推进，
    // 且在收尾 ramp 之前不越过 96%（留给「即将完成」的视觉余量）。
    let total = 5;
    let mut last = 0.0_f32;
    for done in 0..=total {
        for micros in [0.0_f32, 0.02, 0.31, 0.62, 1.5] {
            let progress = startup_loading_progress(done, total, micros, None, false);
            assert!(
                progress >= last - 1.0e-6,
                "进度回退: done={done} step_elapsed={micros} progress={progress} last={last}"
            );
            assert!(progress <= 0.96 + 1.0e-6, "未完成时越过 96%: {progress}");
            last = progress;
        }
    }
    // 最后一步跑满也只到 96%，不会提前显示 100%。
    assert!(
        startup_loading_progress(total, total, 5.0, None, false) <= 0.96 + 1.0e-6,
        "全部步骤完成后未收尾就已到 100%"
    );
}

#[test]
fn startup_progress_reaches_full_after_completion_ramp() {
    let ramping = startup_loading_progress(5, 5, 0.62, Some(0.0), true);
    let done = startup_loading_progress(5, 5, 0.62, Some(1.0), true);

    assert!(done >= ramping, "收尾 ramp 必须单调向上");
    assert!((done - 1.0).abs() < 1.0e-6, "收尾后应为 1.0，实际 {done}");
}

#[test]
fn merge_refresh_stale_last_played_falls_back_to_fetched() {
    // 最后播放歌曲已不在旧列表（数据异常），退化为全量替换
    let old = vec![track("a")];
    let fetched = vec![track("x")];

    let (merged, index) = merge_private_roam_refresh(&old, Some(5), fetched);

    let ids: Vec<Option<String>> = merged.iter().map(|t| t.id.clone()).collect();
    assert_eq!(ids, vec![Some("x".to_string())]);
    assert_eq!(index, None);
}

#[test]
fn normalize_fm_items_maps_album_and_artists() {
    let items = serde_json::json!([{
        "name": "song",
        "id": 1,
        "album": {"name": "al-name", "picUrl": "http://cover"},
        "artists": [{"name": "artist-a"}],
        "dt": 2000
    }]);

    let normalized = normalize_fm_song_items(items.as_array().unwrap());
    assert_eq!(normalized[0]["al"]["name"], "al-name");
    assert_eq!(normalized[0]["al"]["picUrl"], "http://cover");
    assert_eq!(normalized[0]["ar"][0]["name"], "artist-a");
    // 原有字段不受影响
    assert_eq!(normalized[0]["name"], "song");
}

#[test]
fn lufs_mapping_uses_minus_60_to_zero_range() {
    assert!((mean_square_to_lufs(0.5) - (-3.701)).abs() < 0.01);
    assert!((lufs_to_bar_level(-30.0) - 0.5).abs() < 1.0e-6);
    assert_eq!(lufs_to_bar_level(VU_LUFS_FLOOR), 0.0);
    assert_eq!(lufs_to_bar_level(0.0), 1.0);
    assert_eq!(lufs_to_mean_square(VU_LUFS_FLOOR), 0.0);
}

fn pending_toggle_future() -> LikeToggleFuture {
    let fut: LikeToggleTask = Box::pin(async { None });
    SharedTask::unstarted(fut)
}

fn pending_verify_future() -> LikeVerifyFuture {
    let fut: LikeVerifyTask = Box::pin(async { None });
    SharedTask::unstarted(fut)
}

/// 点击必须立刻改变显示值（乐观），并把请求排进派发队列。
#[test]
fn like_click_is_optimistic_and_queues_one_request() {
    let mut machine = LikeMachine::default();
    machine.set_intent("s1".to_string(), true);

    assert!(machine.displayed("s1"), "点击后立即显示为已收藏");
    assert_eq!(machine.pending_dispatch(), Some(("s1".to_string(), true)));
}

/// 连点两次回到原状态：一个请求都不该发。
#[test]
fn like_double_click_back_to_start_sends_nothing() {
    let mut machine = LikeMachine::default();
    machine.set_intent("s1".to_string(), true);
    machine.set_intent("s1".to_string(), false);

    assert!(!machine.displayed("s1"));
    assert_eq!(machine.pending_dispatch(), None);
    assert_eq!(machine.drop_satisfied_intent(), Some("s1".to_string()));
    assert!(machine.desired.is_none());
}

/// 在途请求未收敛前不再并发派发，但显示跟随最后一次意图。
#[test]
fn like_inflight_toggle_blocks_a_second_request() {
    let mut machine = LikeMachine::default();
    machine.set_intent("s1".to_string(), true);
    machine.begin_toggle("s1".to_string(), true, pending_toggle_future());

    machine.set_intent("s1".to_string(), false);

    assert_eq!(machine.pending_dispatch(), None, "串行化：一次只发一个");
    assert_eq!(
        machine.drop_satisfied_intent(),
        None,
        "在途写入不能满足新意图"
    );
    assert!(!machine.displayed("s1"));
}

/// 旧回包不得覆盖更新的意图：确认值照写，显示与补发按新意图走。
#[test]
fn like_stale_response_keeps_newer_intent() {
    let mut machine = LikeMachine::default();
    machine.set_intent("s1".to_string(), true);
    machine.begin_toggle("s1".to_string(), true, pending_toggle_future());
    machine.set_intent("s1".to_string(), false);
    assert_eq!(machine.drop_satisfied_intent(), None);
    // 真实路径里 pump 先取走回包句柄，再交给状态机收敛
    machine.toggle = None;

    let outcome = machine.on_toggle_result("s1", true, Ok(()));

    assert_eq!(outcome, ToggleOutcome::Superseded);
    assert!(machine.is_confirmed("s1"), "回包写入已确认值");
    assert!(!machine.displayed("s1"), "显示仍按最后一次意图");
    assert_eq!(machine.pending_dispatch(), Some(("s1".to_string(), false)));
}

/// 仍是最新意图的失败要回滚显示并报错。
#[test]
fn like_failure_rolls_back_to_confirmed() {
    let mut machine = LikeMachine::default();
    machine.set_intent("s1".to_string(), true);

    let outcome = machine.on_toggle_result("s1", true, Err("502".to_string()));

    assert_eq!(
        outcome,
        ToggleOutcome::Failed {
            message: "502".to_string()
        }
    );
    assert!(!machine.is_confirmed("s1"));
    assert!(!machine.displayed("s1"), "失败后回滚到已确认值");
    assert_eq!(machine.pending_dispatch(), None, "失败的意图已被放弃");
}

/// 已被取代的旧失败直接忽略，不影响新意图。
#[test]
fn like_stale_failure_is_ignored() {
    let mut machine = LikeMachine::default();
    machine.set_intent("s1".to_string(), true);
    machine.begin_toggle("s1".to_string(), true, pending_toggle_future());
    machine.set_intent("s1".to_string(), false);
    machine.toggle = None;

    let outcome = machine.on_toggle_result("s1", true, Err("timeout".to_string()));

    assert_eq!(outcome, ToggleOutcome::StaleFailure, "旧失败不打断新意图");
    assert!(!machine.displayed("s1"));
    assert!(!machine.is_confirmed("s1"));
    // 新意图（取消收藏）与已确认状态一致，无需再发请求
    assert_eq!(machine.pending_dispatch(), None);
    assert_eq!(machine.drop_satisfied_intent(), Some("s1".to_string()));
}

/// 服务端确认不能覆盖未决意图。
#[test]
fn like_verify_does_not_override_pending_intent() {
    let mut machine = LikeMachine::default();
    machine.begin_verify("s1".to_string(), pending_verify_future());
    machine.set_intent("s1".to_string(), true);

    assert!(
        !machine.on_verify_result("s1", Ok(false)),
        "确认被未决意图挡住"
    );
    assert!(!machine.is_confirmed("s1"));
    assert!(machine.displayed("s1"));
}

/// 无未决意图时确认写入已确认集合；确认失败保持本地缓存。
#[test]
fn like_verify_applies_without_pending_intent() {
    let mut machine = LikeMachine::default();

    assert!(machine.on_verify_result("s1", Ok(true)));
    assert!(machine.is_confirmed("s1"));
    assert!(machine.displayed("s1"));
    assert!(!machine.on_verify_result("s1", Err(())));
    assert!(machine.is_confirmed("s1"), "确认失败不改动本地缓存");
}

/// 未决意图只作用于它自己的曲目。
#[test]
fn like_display_is_per_song() {
    let mut machine = LikeMachine::default();
    machine.set_confirmed("s1", true);
    machine.set_intent("s2".to_string(), true);

    assert!(machine.displayed("s1"), "其他曲目仍按已确认值");
    assert!(machine.displayed("s2"));
    assert!(!machine.is_confirmed("s2"));
}

fn sidebar_item(title: &str) -> HomeSidebarPlaylist {
    HomeSidebarPlaylist {
        id: Some(title.to_string()),
        title: title.to_string(),
        creator: String::new(),
        track_count: 1,
    }
}

fn sidebar_state(created: usize, collected: usize) -> HomeSidebarState {
    let mut state = HomeSidebarState {
        created_playlists: (0..created)
            .map(|i| sidebar_item(&format!("created-{i}")))
            .collect(),
        collected_playlists: (0..collected)
            .map(|i| sidebar_item(&format!("collected-{i}")))
            .collect(),
        ..HomeSidebarState::default()
    };
    state.clamp_focus();
    state
}

/// 滚轮一格一步、到端点即停（不回卷），且焦点记忆跟着走。
#[test]
fn home_sidebar_wheel_steps_and_clamps_at_ends() {
    let mut state = sidebar_state(3, 0);
    state.expanded = true;

    state.scroll_by(true);
    assert_eq!(state.focused_index, 1);
    assert_eq!(state.created_focused_index, 1, "焦点位置写回分区记忆");

    state.scroll_by(true);
    state.scroll_by(true);
    assert_eq!(state.focused_index, 2, "到底即停，不像键盘那样绕回开头");

    state.scroll_by(false);
    assert_eq!(state.focused_index, 1);

    state.focused_index = 0;
    state.scroll_by(false);
    assert_eq!(state.focused_index, 0, "到顶即停");
}

/// 空分区没有可滚的内容，不动焦点。
#[test]
fn home_sidebar_wheel_without_items_does_nothing() {
    let mut state = sidebar_state(0, 0);
    state.expanded = true;

    state.scroll_by(true);

    assert_eq!(state.focused_index, 0);
    assert_eq!(state.focused_section, HomeSidebarSection::Created);
}

/// 指着另一个分区滚：先切过去（沿用该分区的位置记忆）再走一格。
#[test]
fn home_sidebar_wheel_switches_section_then_steps() {
    let mut state = sidebar_state(3, 4);
    state.expanded = true;
    state.collected_focused_index = 2;

    state.scroll_section_by(HomeSidebarSection::Collected, true);

    assert_eq!(state.focused_section, HomeSidebarSection::Collected);
    assert_eq!(state.focused_index, 3, "先回到记忆位置 2，再前进一格");
    assert_eq!(state.created_focused_index, 0, "原分区焦点已存回");
    assert_eq!(state.collected_focused_index, 3);
}

/// 指到空分区时不会把焦点放进空列表：仍留在有内容的分区里走一格。
#[test]
fn home_sidebar_wheel_on_empty_section_keeps_full_one() {
    let mut state = sidebar_state(2, 0);
    state.expanded = true;

    state.scroll_section_by(HomeSidebarSection::Collected, true);

    assert_eq!(state.focused_section, HomeSidebarSection::Created);
    assert_eq!(state.focused_index, 1);
}

/// 分区判定：光标所在分区优先，落在分区之间的空隙时用当前聚焦分区，
/// 面板外或面板未登记则不响应（收起态由调用方先挡掉）。
#[test]
fn home_sidebar_wheel_targets_section_under_cursor() {
    let panel = HitRect {
        x: 0,
        y: 0,
        width: 30,
        height: 20,
    };
    let sections = [
        (
            HitRect {
                x: 0,
                y: 5,
                width: 30,
                height: 6,
            },
            HomeSidebarSection::Created,
        ),
        (
            HitRect {
                x: 0,
                y: 11,
                width: 30,
                height: 6,
            },
            HomeSidebarSection::Collected,
        ),
    ];

    assert_eq!(
        home_sidebar_wheel_target(Some(panel), &sections, HomeSidebarSection::Created, 3, 12),
        Some(HomeSidebarSection::Collected),
        "指到收藏区就滚收藏区"
    );
    assert_eq!(
        home_sidebar_wheel_target(Some(panel), &sections, HomeSidebarSection::Collected, 3, 2),
        Some(HomeSidebarSection::Collected),
        "落在分区外的面板空白处 → 用聚焦分区"
    );
    assert_eq!(
        home_sidebar_wheel_target(Some(panel), &sections, HomeSidebarSection::Created, 40, 12),
        None,
        "面板外不响应"
    );
    assert_eq!(
        home_sidebar_wheel_target(None, &sections, HomeSidebarSection::Created, 3, 12),
        None,
        "面板未登记（侧边栏宽度不足）不响应"
    );
}

/// 作者行按连接符切段：下标与显示位置一一对应（空段保留占位）。
#[test]
fn artist_name_segments_split_the_display_line() {
    assert_eq!(artist_name_segments("Jay"), vec!["Jay"]);
    assert_eq!(
        artist_name_segments("Caffeine / 初音ミク"),
        vec!["Caffeine", "初音ミク"]
    );
    assert_eq!(artist_name_segments("A / B / C"), vec!["A", "B", "C"]);
    assert_eq!(artist_name_segments(""), vec![""]);
}

/// 段序号 → 作者 ID：先认名字（`ar` 顺序不同也对），再按位置兜底，都没有就 None。
#[test]
fn pick_artist_id_matches_the_name_then_the_position() {
    let artists = vec![
        ("初音ミク".to_string(), Some("9001".to_string())),
        ("Caffeine".to_string(), Some("9002".to_string())),
    ];

    assert_eq!(
        pick_artist_id(&artists, "Caffeine / 初音ミク", 0),
        Some("9002".to_string()),
        "点第 1 段：名字匹配到表里的第 2 位"
    );
    assert_eq!(
        pick_artist_id(&artists, "Caffeine / 初音ミク", 1),
        Some("9001".to_string())
    );

    let only_other = vec![("Other".to_string(), Some("1".to_string()))];
    assert_eq!(
        pick_artist_id(&only_other, "A / B", 0),
        Some("1".to_string()),
        "名字对不上时按位置兜底"
    );
    assert_eq!(
        pick_artist_id(&only_other, "A / B", 1),
        None,
        "名字对不上且位置越界"
    );
    assert_eq!(pick_artist_id(&[], "A", 0), None, "没有作者表");
}
