use super::ShuffleOrder;
use crate::app::playback_controller::PlaybackTrack;
use std::collections::HashSet;

fn queue(ids: impl IntoIterator<Item = u32>) -> Vec<PlaybackTrack> {
    ids.into_iter()
        .map(|id| PlaybackTrack {
            song_id: id.to_string(),
            title: String::new(),
            artist: String::new(),
            album: String::new(),
            duration_ms: 0,
            cover_url: None,
            cover: None,
            lyrics: None,
        })
        .collect()
}

fn id(queue: &[PlaybackTrack], index: Option<usize>) -> &str {
    &queue[index.expect("index")].song_id
}

#[test]
fn each_round_plays_every_song_once_before_any_repeat() {
    for seed in 0..32 {
        let queue = queue(0..20);
        let mut order = ShuffleOrder::with_seed(seed);
        let mut current = Some(7);
        // 每一轮从上一轮最后一首之外的 19 首中各播一次。
        for _round in 0..3 {
            let mut seen = HashSet::from([id(&queue, current).to_string()]);
            for _ in 1..queue.len() {
                current = order.next(&queue, current);
                assert!(
                    seen.insert(id(&queue, current).to_string()),
                    "seed {seed}: repeat inside a round"
                );
            }
            assert_eq!(seen.len(), queue.len());
        }
    }
}

#[test]
fn previous_walks_back_real_history_and_next_returns_forward() {
    let queue = queue(0..10);
    let mut order = ShuffleOrder::with_seed(3);
    let mut played = vec![Some(0)];
    let mut current = Some(0);
    for _ in 0..5 {
        current = order.next(&queue, current);
        played.push(current);
    }
    for expected in played.iter().rev().skip(1) {
        current = order.previous(&queue, current);
        assert_eq!(current, *expected);
    }
    assert_eq!(order.previous(&queue, current), None);
    for expected in played.iter().skip(1) {
        current = order.next(&queue, current);
        assert_eq!(current, *expected);
    }
}

#[test]
fn appended_page_joins_the_current_round_without_repeating_played_songs() {
    let mut queue = queue(0..5);
    let mut order = ShuffleOrder::with_seed(11);
    let mut current = Some(2);
    let mut seen = HashSet::from([id(&queue, current).to_string()]);
    for _ in 0..2 {
        current = order.next(&queue, current);
        seen.insert(id(&queue, current).to_string());
    }
    queue.extend(self::queue(5..12));
    for _ in seen.len()..queue.len() {
        current = order.next(&queue, current);
        assert!(seen.insert(id(&queue, current).to_string()));
    }
    assert_eq!(seen.len(), 12);
}

#[test]
fn trimmed_prefix_keeps_history_by_song_not_by_index() {
    let mut queue = queue(0..8);
    let mut order = ShuffleOrder::with_seed(5);
    let mut current = Some(6);
    let mut played = vec![id(&queue, current).to_string()];
    for _ in 0..3 {
        current = order.next(&queue, current);
        played.push(id(&queue, current).to_string());
    }
    // 漫游窗口裁掉前缀：下标整体前移，但歌曲身份不变。
    let current_id = played.last().unwrap().clone();
    queue.retain(|track| track.song_id.parse::<u32>().unwrap() >= 2 || track.song_id == current_id);
    let current = queue.iter().position(|track| track.song_id == current_id);
    let back = order.previous(&queue, current);
    let survivors: Vec<_> = played[..played.len() - 1]
        .iter()
        .rev()
        .filter(|song| queue.iter().any(|track| track.song_id == **song))
        .collect();
    assert_eq!(id(&queue, back), survivors[0].as_str());
}

#[test]
fn directly_picked_song_is_not_scheduled_again_in_the_round() {
    let queue = queue(0..6);
    let mut order = ShuffleOrder::with_seed(9);
    let mut current = order.next(&queue, Some(0));
    // 用户在列表里直接点播另一首。
    let picked = (0..6)
        .find(|&index| Some(index) != current && index != 0)
        .unwrap();
    current = Some(picked);
    let mut seen = HashSet::from(["0".to_string(), id(&queue, current).to_string()]);
    let first = order.previous(&queue, current);
    assert_ne!(first, Some(picked));
    current = order.next(&queue, first);
    assert_eq!(current, Some(picked));
    seen.insert(id(&queue, first).to_string());
    for _ in seen.len()..queue.len() {
        current = order.next(&queue, current);
        assert!(seen.insert(id(&queue, current).to_string()));
    }
}

#[test]
fn single_song_queue_replays_it() {
    let queue = queue([42]);
    let mut order = ShuffleOrder::with_seed(1);
    assert_eq!(order.next(&queue, Some(0)), Some(0));
    assert_eq!(order.next(&queue, Some(0)), Some(0));
}
