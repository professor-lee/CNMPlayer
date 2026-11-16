//! 随机播放序列：一轮内每首歌只出现一次，“上一首”沿真实播放历史回退。
//!
//! 序列按歌曲 id 记账而不是按队列下标，所以分页追加、漫游裁剪前缀等改动
//! 队列的路径不需要逐个通知；每次取下一首/上一首前与当前队列对账即可：
//! 移出队列的歌从历史和待播中删去，新加入的歌单独洗牌后排在本轮剩余曲目之后。

use super::playback_controller::PlaybackTrack;
use crate::data::playback_session::PlaybackShuffleState;
use std::collections::HashSet;
use std::hash::BuildHasher;
use std::hash::RandomState;

/// 历史只用于“上一首”回退，长度设上限避免长时间随机播放无限增长。
const HISTORY_LIMIT: usize = 1024;

pub(super) struct ShuffleOrder {
    /// 序列认定的当前歌曲；与队列当前项不同说明用户直接点播了别的歌。
    current: Option<String>,
    /// 已播放的歌，末尾为最近一首（不含 `current`）。
    history: Vec<String>,
    /// 本轮待播的歌，末尾为下一首。
    upcoming: Vec<String>,
    /// 本轮已经排过的歌（已播 + 待播 + 当前）；不在其中的队列歌曲视为新加入。
    round: HashSet<String>,
    rng: SplitMix64,
}

impl ShuffleOrder {
    pub(super) fn new() -> Self {
        Self::with_seed(RandomState::new().hash_one(0u64))
    }

    pub(super) fn with_seed(seed: u64) -> Self {
        Self {
            current: None,
            history: Vec::new(),
            upcoming: Vec::new(),
            round: HashSet::new(),
            rng: SplitMix64(seed),
        }
    }

    /// 换队列、清空队列或切换播放模式时丢弃旧序列与历史。
    pub(super) fn reset(&mut self) {
        self.current = None;
        self.history.clear();
        self.upcoming.clear();
        self.round.clear();
    }

    /// 播放记忆用的洗牌进度；`current` 由记忆里的当前曲恢复。
    pub(super) fn snapshot(&self) -> PlaybackShuffleState {
        let mut round: Vec<String> = self.round.iter().cloned().collect();
        round.sort_unstable();
        PlaybackShuffleState {
            history: self.history.clone(),
            upcoming: self.upcoming.clone(),
            round,
        }
    }

    /// 恢复播放记忆里的洗牌进度；下次对账时以队列当前曲为准。
    pub(super) fn restore(&mut self, state: PlaybackShuffleState) {
        self.reset();
        let skip = state.history.len().saturating_sub(HISTORY_LIMIT);
        self.history = state.history.into_iter().skip(skip).collect();
        // 待播必须算在本轮内，否则对账会把它们当作新歌再排一次。
        self.round = state.round.into_iter().collect();
        self.round.extend(state.upcoming.iter().cloned());
        self.upcoming = state.upcoming;
    }

    /// 本轮下一首；本轮播完后重新洗一轮。队列只有一首时重播当前曲。
    pub(super) fn next(
        &mut self,
        queue: &[PlaybackTrack],
        current: Option<usize>,
    ) -> Option<usize> {
        self.sync(queue, current);
        if queue.is_empty() {
            return None;
        }
        if self.upcoming.is_empty() {
            self.start_round(queue);
        }
        let Some(id) = self.upcoming.pop() else {
            return current.filter(|&index| index < queue.len());
        };
        if let Some(previous) = self.current.replace(id) {
            self.push_history(previous);
        }
        self.current.as_deref().and_then(|id| position(queue, id))
    }

    /// 回到上一首实际播放过的歌；没有历史时返回 `None`（与顺序播放在首曲时一致）。
    /// 被回退的当前曲放回待播末尾，再按下一首会回到它。
    pub(super) fn previous(
        &mut self,
        queue: &[PlaybackTrack],
        current: Option<usize>,
    ) -> Option<usize> {
        self.sync(queue, current);
        while let Some(id) = self.history.pop() {
            if let Some(index) = position(queue, &id) {
                if let Some(now) = self.current.replace(id) {
                    self.upcoming.push(now);
                }
                return Some(index);
            }
        }
        None
    }

    /// 与队列对账：去掉已移出队列的歌、记下点播、把新加入的歌排进待播。
    pub(super) fn sync(&mut self, queue: &[PlaybackTrack], current: Option<usize>) {
        let ids: HashSet<&str> = queue.iter().map(|track| track.song_id.as_str()).collect();
        self.history.retain(|id| ids.contains(id.as_str()));
        self.upcoming.retain(|id| ids.contains(id.as_str()));
        self.round.retain(|id| ids.contains(id.as_str()));

        let current_id = current
            .and_then(|index| queue.get(index))
            .map(|track| track.song_id.as_str());
        if self.current.as_deref() != current_id {
            if let Some(previous) = self.current.take()
                && ids.contains(previous.as_str())
            {
                self.push_history(previous);
            }
            if let Some(id) = current_id {
                // 直接点播的歌算作本轮已播，不再从待播里排到它。
                self.upcoming.retain(|upcoming| upcoming != id);
                self.round.insert(id.to_string());
            }
            self.current = current_id.map(str::to_string);
        }

        let mut seen = HashSet::new();
        let mut added: Vec<String> = queue
            .iter()
            .map(|track| track.song_id.as_str())
            .filter(|id| !self.round.contains(*id) && seen.insert(*id))
            .map(str::to_string)
            .collect();
        self.shuffle(&mut added);
        self.round.extend(added.iter().cloned());
        // 待播从末尾取，插到开头即排在本轮已洗好的曲目之后。
        self.upcoming.splice(0..0, added);
    }

    /// 本轮还没播的歌数（截至上次对账）。
    pub(super) fn remaining(&self) -> usize {
        self.upcoming.len()
    }

    /// 对账后本轮是否已经播完。
    pub(super) fn round_exhausted(
        &mut self,
        queue: &[PlaybackTrack],
        current: Option<usize>,
    ) -> bool {
        self.sync(queue, current);
        self.upcoming.is_empty()
    }

    fn shuffle(&mut self, ids: &mut [String]) {
        for i in (1..ids.len()).rev() {
            let j = self.rng.below(i + 1);
            ids.swap(i, j);
        }
    }

    fn start_round(&mut self, queue: &[PlaybackTrack]) {
        self.round.clear();
        for track in queue {
            if self.round.insert(track.song_id.clone())
                && self.current.as_deref() != Some(track.song_id.as_str())
            {
                self.upcoming.push(track.song_id.clone());
            }
        }
        let mut upcoming = std::mem::take(&mut self.upcoming);
        self.shuffle(&mut upcoming);
        self.upcoming = upcoming;
    }

    fn push_history(&mut self, id: String) {
        if self.history.len() == HISTORY_LIMIT {
            self.history.remove(0);
        }
        self.history.push(id);
    }
}

fn position(queue: &[PlaybackTrack], id: &str) -> Option<usize> {
    queue.iter().position(|track| track.song_id == id)
}

struct SplitMix64(u64);

impl SplitMix64 {
    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// `0..n` 内的均匀整数（`n` 远小于 2^64，取模偏差可忽略）。
    fn below(&mut self, n: usize) -> usize {
        (self.next_u64() % n as u64) as usize
    }
}

#[cfg(test)]
mod tests;
