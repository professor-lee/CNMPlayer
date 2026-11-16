//! 听歌上报（`scrobble` 配置项）：一首歌开始出声时开一段收听记录，切走或播放停止时
//! 按实际收听时长（暂停不计）上报网易云 `feedback/weblog`，供“听歌排行”统计。

use super::App;
use super::playback_controller::PlaybackRuntimeState;
use super::playback_controller::PlaybackTrack;
use std::time::Duration;
use std::time::Instant;

/// 少于此时长的收听不上报（与 go-musicfox 一致），避免快速切歌刷出大量记录。
const MIN_REPORTED_LISTEN: Duration = Duration::from_secs(20);
/// 收听时长与歌曲时长相差在此范围内视为播完，否则视为用户切走。
const PLAYEND_TOLERANCE: Duration = Duration::from_secs(10);
/// 退出时等待最后一次上报的上限，避免网络差时拖慢退出。
const EXIT_REPORT_TIMEOUT: Duration = Duration::from_millis(1500);
/// 后台上报的总时限，避免网络挂起时留下长期悬挂的任务。
const REPORT_TIMEOUT: Duration = Duration::from_secs(15);

pub(super) struct ListenSession {
    song_id: String,
    duration: Duration,
    source_id: String,
    listened: Duration,
    playing_since: Option<Instant>,
}

impl ListenSession {
    pub(super) fn new(track: &PlaybackTrack, source_id: Option<&str>) -> Self {
        Self {
            song_id: track.song_id.clone(),
            duration: Duration::from_millis(track.duration_ms.max(0) as u64),
            // 只有数字 id 才是网易云歌单/专辑来源；漫游等本地来源标识不上报。
            source_id: source_id
                .filter(|id| !id.is_empty() && id.bytes().all(|byte| byte.is_ascii_digit()))
                .unwrap_or_default()
                .to_string(),
            listened: Duration::ZERO,
            playing_since: None,
        }
    }

    /// 记录播放状态；只累计处于播放态的时间段。
    pub(super) fn observe(&mut self, playing: bool, now: Instant) {
        match (playing, self.playing_since) {
            (true, None) => self.playing_since = Some(now),
            (false, Some(since)) => {
                self.listened += now.saturating_duration_since(since);
                self.playing_since = None;
            }
            _ => {}
        }
    }

    pub(super) fn listened(&self, now: Instant) -> Duration {
        self.listened
            + self
                .playing_since
                .map_or(Duration::ZERO, |since| now.saturating_duration_since(since))
    }

    /// 达到上报门槛时给出 `(收听秒数, end 类型)`。
    pub(super) fn report(&self, now: Instant) -> Option<(u64, &'static str)> {
        let listened = self.listened(now);
        if listened < MIN_REPORTED_LISTEN {
            return None;
        }
        let end = if self.duration.abs_diff(listened) <= PLAYEND_TOLERANCE {
            "playend"
        } else {
            "ui"
        };
        Some((listened.as_secs(), end))
    }
}

impl App {
    /// 新歌开始出声时调用；上一段记录应已由切歌路径结算。
    pub(super) fn start_listen_session(&mut self, track: &PlaybackTrack) {
        self.finish_listen_session();
        if !self.config.scrobble {
            return;
        }
        let mut session =
            ListenSession::new(track, self.playback.playback_queue_source_id.as_deref());
        session.observe(
            self.playback.playback_state == PlaybackRuntimeState::Playing,
            Instant::now(),
        );
        self.playback.listen = Some(session);
    }

    /// 每个播放 tick 调用：累计收听时长；播放已停止（播完队列、失败）时结算。
    pub(super) fn observe_listen_session(&mut self) {
        let playing = self.playback.playback_state == PlaybackRuntimeState::Playing;
        let stopped = self.playback.playback_state == PlaybackRuntimeState::Stopped
            && !self.playback.has_pending_start();
        let Some(session) = self.playback.listen.as_mut() else {
            return;
        };
        session.observe(playing, Instant::now());
        if stopped {
            self.finish_listen_session();
        }
    }

    /// 结算当前收听记录并在后台上报；关闭上报或未登录时直接丢弃。
    pub(super) fn finish_listen_session(&mut self) {
        if let Some(report) = self.take_listen_report() {
            compio::runtime::spawn(compio::time::timeout(REPORT_TIMEOUT, report)).detach();
        }
    }

    /// 退出前结算并限时等待最后一次上报。
    pub(crate) async fn flush_listen_session_on_exit(&mut self) {
        if let Some(report) = self.take_listen_report() {
            let _ = compio::time::timeout(EXIT_REPORT_TIMEOUT, report).await;
        }
    }

    fn take_listen_report(&mut self) -> Option<impl Future<Output = ()> + 'static> {
        let session = self.playback.listen.take()?;
        if !self.config.scrobble || self.api.session_cookie().is_none() {
            return None;
        }
        let (listened_secs, end) = session.report(Instant::now())?;
        let api = self.api.clone();
        Some(async move {
            match api
                .scrobble(&session.song_id, &session.source_id, listened_secs, end)
                .await
            {
                Ok(response)
                    if response.body.get("code").and_then(|code| code.as_i64()) == Some(200) => {}
                Ok(response) => {
                    log::warn!("scrobble {} rejected: {}", session.song_id, response.body)
                }
                Err(error) => log::warn!("scrobble {} failed: {error:#}", session.song_id),
            }
        })
    }
}

#[cfg(test)]
mod tests;
