//! 启动初始化：加载页先出，网络初始化在后台任务里按步骤推进。
//!
//! 终端接管屏幕（`main` 里 `init_terminal`）发生在 `App::new` 之后，所以只有把
//! 登录恢复、推荐加载这些网络步骤搬进后台任务，加载页才可能显示真实进度，
//! 而不是一段屏幕上毫无反馈的等待。
//!
//! 后台任务与主循环之间只传数据（`StartupEvent`），所有状态改写都在主循环
//! 的 `apply_startup_event` 里发生——与封面/歌词 worker 同一套模式。

use std::collections::HashSet;
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::time::{Duration, Instant};

use super::{
    AccountProfile, ApiState, App, HomeTile, Page, PlaylistTrack, QrLoginCode,
    fetch_account_profile, fetch_home_tiles, fetch_liked_song_ids, fetch_private_roam_songs,
    fetch_qr_login_code, fetch_vip_unlocked, session,
};
use crate::data::config::Config;
use crate::launch;

/// 启动初始化的总预算：超出后放弃剩余步骤，用已到手的数据进主界面。
///
/// cyper 客户端没有内置请求超时，只靠网络自身可能长时间挂住；这里给整条
/// 链路一个上限，保证加载页一定会结束。
const STARTUP_INIT_BUDGET: Duration = Duration::from_secs(12);
/// 单步超时上限：一次挂死不许吃光整条预算。
const STARTUP_INIT_STEP_TIMEOUT: Duration = Duration::from_secs(6);

/// 后台初始化任务上报的进度与结果。
enum StartupEvent {
    /// 又有一步完成（累计已完成步数）。
    Step { done: usize },
    /// 存档 cookie 校验通过。
    SessionRestored { cookie: Option<String> },
    /// 存档 cookie 已失效（存档已清除）。
    SessionRejected,
    /// 登录态没校验成（网络等原因），存档保持不动。
    SessionUnavailable,
    /// 会员音质权限。
    Vip { unlocked: bool },
    /// 账号档案。
    Account(AccountProfile),
    ///「我喜欢的音乐」id 集合；失败时为 `None`，沿用旧缓存。
    Liked { ids: Option<HashSet<String>> },
    /// 私人漫游的新歌批次。
    Roam { tracks: Vec<PlaylistTrack> },
    /// 首页推荐 tile。
    Home { tiles: Vec<HomeTile> },
    /// 扫码登录二维码。
    QrCode(QrLoginCode),
    /// 全部结束，后台任务持有的 `ApiState` 交还主循环。
    Done { api: ApiState, deadline: Instant },
}

/// 启动初始化任务与加载页进度。
pub struct StartupInit {
    rx: Option<Receiver<StartupEvent>>,
    step_done: usize,
    step_total: usize,
    step_started_at: Instant,
    finished: bool,
}

impl StartupInit {
    /// 没有后台任务时的占位状态（登录后刷新、进度只跟时间走）。
    pub fn detached() -> Self {
        Self {
            rx: None,
            step_done: 0,
            step_total: 1,
            step_started_at: Instant::now(),
            finished: true,
        }
    }

    /// 启动后台初始化任务；`steps` 是计划步数，决定进度条分母。
    pub fn spawn(
        config: Config,
        api: ApiState,
        cookie: Option<String>,
        skip_roam: bool,
        steps: usize,
    ) -> Self {
        let (tx, rx) = mpsc::channel();
        launch(run_startup_init(api, config, cookie, skip_roam, tx));

        Self {
            rx: Some(rx),
            step_total: steps.max(1),
            step_done: 0,
            step_started_at: Instant::now(),
            finished: false,
        }
    }

    /// 重设计划：脱离真实步数，进度只由时间缓动推进。
    pub fn reset_steps(&mut self, done: usize, total: usize) {
        self.step_done = done;
        self.step_total = total.max(1);
        self.step_started_at = Instant::now();
    }

    /// 计划塌缩到当前进度（登录态不可用时只剩「准备扫码」一步）。
    pub fn collapse_to_done(&mut self) {
        self.reset_steps(self.step_done, self.step_done.max(1));
    }

    /// 推进到 `done` 步（只前进，不回退）。
    pub fn mark_done(&mut self, done: usize) {
        if done <= self.step_done {
            return;
        }
        self.step_done = done.min(self.step_total);
        self.step_started_at = Instant::now();
    }

    /// 主循环自己完成的一步（播放记忆恢复由主循环执行）。
    pub fn complete_step(&mut self) {
        self.mark_done(self.step_done.saturating_add(1));
    }

    pub fn step_done(&self) -> usize {
        self.step_done
    }

    pub fn step_total(&self) -> usize {
        self.step_total
    }

    /// 当前步已进行的时长，用于单步内的进度缓动。
    pub fn step_elapsed(&self) -> f32 {
        self.step_started_at.elapsed().as_secs_f32()
    }

    /// 后台任务是否已经结束（没有任务时视为已结束）。
    pub fn finished(&self) -> bool {
        self.finished
    }

    /// 排空后台事件（非阻塞）。
    fn drain(&mut self) -> Vec<StartupEvent> {
        let Some(rx) = self.rx.as_ref() else {
            return Vec::new();
        };

        let mut events = Vec::new();
        loop {
            match rx.try_recv() {
                Ok(event) => events.push(event),
                Err(TryRecvError::Empty) => break,
                // 任务结束：`Done` 之后残留的事件已全部取出。
                Err(TryRecvError::Disconnected) => {
                    self.finished = true;
                    break;
                }
            }
        }
        events
    }
}

/// 启动计划：待执行步数与加载页结束后的落点。
///
/// 步数对应后台任务真会上报的次数：校验登录态 → 会员权限 → 账号与喜爱列表
/// →（私人漫游）→ 首页推荐，再加上由主循环收尾的播放记忆恢复。
pub(super) fn initial_plan(has_saved_cookie: bool, skip_roam: bool) -> (usize, Page) {
    if !has_saved_cookie {
        // 未登录：只把二维码准备好。
        return (1, Page::Login);
    }

    (5 + usize::from(!skip_roam), Page::Home)
}

/// 在剩余预算内执行一步；超时返回 `None`（该步会被跳过）。
async fn step<F: Future>(deadline: Instant, future: F) -> Option<F::Output> {
    let remaining = deadline
        .saturating_duration_since(Instant::now())
        .min(STARTUP_INIT_STEP_TIMEOUT);
    if remaining.is_zero() {
        return None;
    }

    compio::time::timeout(remaining, future).await.ok()
}

/// 后台初始化：登录恢复 → 会员权限 → 账号与喜爱列表 → 私人漫游 → 首页推荐。
async fn run_startup_init(
    mut api: ApiState,
    config: Config,
    cookie: Option<String>,
    skip_roam: bool,
    tx: Sender<StartupEvent>,
) {
    let lang = config.language;
    let deadline = Instant::now() + STARTUP_INIT_BUDGET;
    let mut done = 0usize;

    let mut restored = false;
    if let Some(cookie) = cookie.as_deref() {
        match step(deadline, api.validate_cookie(cookie)).await {
            Some(Ok(true)) => {
                restored = true;
                let _ = tx.send(StartupEvent::SessionRestored {
                    cookie: api.session_cookie().map(|value| value.to_string()),
                });
            }
            Some(Ok(false)) => {
                let _ = tx.send(StartupEvent::SessionRejected);
            }
            Some(Err(err)) => {
                log::debug!("启动登录态校验失败: {err}");
                let _ = tx.send(StartupEvent::SessionUnavailable);
            }
            None => {
                log::warn!("启动登录态校验超时");
                let _ = tx.send(StartupEvent::SessionUnavailable);
            }
        }
        done += 1;
        let _ = tx.send(StartupEvent::Step { done });
    }

    if !restored {
        // 未登录：只把二维码准备好，加载页随即让位给登录页。
        match step(deadline, fetch_qr_login_code(&mut api, lang)).await {
            Some(Ok(code)) => {
                let _ = tx.send(StartupEvent::QrCode(code));
            }
            Some(Err(err)) => log::debug!("二维码获取失败: {err}"),
            None => log::warn!("二维码获取超时"),
        }
        let _ = tx.send(StartupEvent::Step { done: done.max(1) });
        let _ = tx.send(StartupEvent::Done { api, deadline });
        return;
    }

    match step(deadline, fetch_vip_unlocked(&mut api)).await {
        Some(unlocked) => {
            let _ = tx.send(StartupEvent::Vip { unlocked });
            done += 1;
            let _ = tx.send(StartupEvent::Step { done });
        }
        None => log::warn!("会员音质权限查询超时"),
    }

    match step(deadline, fetch_account_profile(&mut api, lang)).await {
        Some(Ok(profile)) => {
            let uid = profile.uid.clone();
            let _ = tx.send(StartupEvent::Account(profile));
            match step(deadline, fetch_liked_song_ids(&mut api, &uid, lang)).await {
                Some(Ok(ids)) => {
                    let _ = tx.send(StartupEvent::Liked { ids: Some(ids) });
                }
                Some(Err(err)) => log::debug!("喜爱列表刷新失败: {err}"),
                None => log::warn!("喜爱列表刷新超时"),
            }
            done += 1;
            let _ = tx.send(StartupEvent::Step { done });
        }
        Some(Err(err)) => log::debug!("账号信息获取失败: {err}"),
        None => log::warn!("账号信息获取超时"),
    }

    if !skip_roam {
        match step(deadline, fetch_private_roam_songs(&mut api)).await {
            Some(tracks) => {
                if !tracks.is_empty() {
                    let _ = tx.send(StartupEvent::Roam { tracks });
                }
                done += 1;
                let _ = tx.send(StartupEvent::Step { done });
            }
            None => log::warn!("私人漫游刷新超时"),
        }
    }

    match step(
        deadline,
        fetch_home_tiles(&mut api, config.home_more_recommend),
    )
    .await
    {
        Some(tiles) => {
            let _ = tx.send(StartupEvent::Home { tiles });
            done += 1;
            let _ = tx.send(StartupEvent::Step { done });
        }
        None => log::warn!("首页推荐加载超时"),
    }

    let _ = tx.send(StartupEvent::Done { api, deadline });
}

impl App {
    /// 排空启动初始化事件并逐条应用。
    pub(super) async fn tick_startup_init(&mut self) {
        for event in self.startup.init.drain() {
            self.apply_startup_event(event).await;
        }

        // 兜底：任务若没落 `Done` 就结束（事件通道断开），加载页也必须能出去。
        if self.startup.init.finished()
            && self.page == Page::Loading
            && !self.startup.complete_requested
        {
            self.finish_startup_loading();
        }
    }

    async fn apply_startup_event(&mut self, event: StartupEvent) {
        match event {
            StartupEvent::Step { done } => self.startup.init.mark_done(done),
            StartupEvent::SessionRestored { cookie } => {
                self.session_cookie = cookie;
                self.startup.target = Page::Home;
            }
            StartupEvent::SessionRejected => {
                let _ = self.persistence.enqueue_latest(
                    crate::data::persistence::PersistenceKey::Session,
                    session::clear_cookie,
                );
                self.startup.target = Page::Login;
                self.startup.init.collapse_to_done();
            }
            StartupEvent::SessionUnavailable => {
                // 存档保持不动：下次启动还有机会直接恢复登录。
                self.startup.target = Page::Login;
                self.startup.init.collapse_to_done();
            }
            StartupEvent::Vip { unlocked } => self.apply_vip_audio_access(unlocked),
            StartupEvent::Account(profile) => self.apply_account_profile(profile),
            StartupEvent::Liked { ids } => {
                if let Some(ids) = ids {
                    self.apply_liked_song_ids(ids);
                }
            }
            StartupEvent::Roam { tracks } => self.apply_private_roam_refresh(tracks),
            StartupEvent::Home { tiles } => self.apply_home_tiles(tiles),
            StartupEvent::QrCode(code) => self.apply_qr_login_code(code),
            StartupEvent::Done { api, deadline } => {
                self.api = api;
                if self.session_cookie.is_some() {
                    if step(deadline, self.try_restore_playback_memory())
                        .await
                        .is_none()
                    {
                        self.playback.audio_player.stop();
                        self.playback.clear_queue();
                        self.playback.now_playing = None;
                        self.playback.now_playing_liked = false;
                        self.cover_fetch_generation = self.cover_fetch_generation.wrapping_add(1);
                        let _ = self.cover_fetch_tx.send(None);
                        self.cover_fetch_inflight_url = None;
                        self.lyric_fetch_generation = self.lyric_fetch_generation.wrapping_add(1);
                        let _ = self.lyric_fetch_tx.send(None);
                        self.lyric_fetch_inflight_song_id = None;
                        self.set_runtime_status(self.lang_text(
                            "播放记忆恢复超时，跳过本次恢复",
                            "Playback memory restore timed out; skipped this attempt",
                        ));
                    }
                    self.startup.init.complete_step();
                }
                self.finish_startup_loading();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::step;
    use std::cell::Cell;
    use std::rc::Rc;
    use std::time::{Duration, Instant};

    #[compio::test]
    async fn exhausted_startup_budget_does_not_start_restore() {
        let entered = Cell::new(false);
        let result = step(Instant::now(), async {
            entered.set(true);
        })
        .await;
        assert!(result.is_none());
        assert!(!entered.get());
    }

    #[compio::test]
    async fn restore_uses_remaining_budget_and_drops_pending_work() {
        struct Pending(Rc<Cell<bool>>);
        impl Drop for Pending {
            fn drop(&mut self) {
                self.0.set(true);
            }
        }
        let dropped = Rc::new(Cell::new(false));
        let pending = Pending(dropped.clone());
        let result = step(Instant::now() + Duration::from_millis(20), async move {
            let _pending = pending;
            std::future::pending::<()>().await;
        })
        .await;
        assert!(result.is_none());
        assert!(dropped.get());
    }
}
