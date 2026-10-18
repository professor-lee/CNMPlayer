use super::cache_maintenance::{CacheLease, CacheMaintenance, cleanup_orphan_streaming_temps};
use super::streaming::StreamingReaderHandle;
use super::streaming::StreamingStatus;
use crate::STORAGE;
use crate::data::config::Config;
use crate::tmplayer::app::state::EQ_BANDS;
use crate::tmplayer::app::state::EQ_FREQS_HZ;
use crate::tmplayer::app::state::EqSettings;
use crate::tmplayer::audio::lufs_meter::LufsMeter;
use crate::tmplayer::audio::pcm_tap::PcmRing;
use crate::tmplayer::audio::pcm_tap::PcmTap;
use anyhow::Result;
use parking_lot::Condvar;
use parking_lot::Mutex;
use rodio::DeviceSinkBuilder;
use rodio::MixerDeviceSink;
use rodio::Player;
use rodio::Source;
use rodio::cpal::Error;
use rodio::source::SeekError;
use see::sync::Receiver;
use std::fs;
use std::num::NonZero;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicI32;
use std::sync::atomic::Ordering;
use std::thread::JoinHandle;
use std::time::Duration;
pub(super) struct AudioPlayer {
    _device_sink: MixerDeviceSink,
    player: Arc<Player>,
    error: MaybeError,
    cache_dir: PathBuf,
    total_duration: Option<Duration>,
    eq_params: Arc<EqParams>,
    progress_rx: Option<Receiver<(u64, u64)>>,
    seek_worker: SeekWorker,
    stream_handle: Option<StreamingReaderHandle>,
    cache_maintenance: CacheMaintenance,
    cache_lease: Option<CacheLease>,
    /// PCM 时域抽头：`EqSource` 逐样本写入，全屏页示波器读取。
    /// 环的寿命长于单个 `EqSource`，故切歌与跳转时必须显式重置。
    pcm_ring: Arc<PcmRing>,
    /// 400ms Momentary LUFS 计量器；`EqSource` 按批写入，小窗口窄窗读取。
    lufs_meter: Arc<LufsMeter>,
}

pub(super) struct PreparedSource {
    source: Box<dyn Source<Item = f32> + Send + 'static>,
}

pub(super) fn make_prepared_source<S>(
    decoder: S,
    eq_params: Arc<EqParams>,
    pcm_ring: Arc<PcmRing>,
    lufs_meter: Arc<LufsMeter>,
) -> PreparedSource
where
    S: Source<Item = f32> + Send + 'static,
{
    let source = EqSource::new(decoder, eq_params, pcm_ring, lufs_meter);
    PreparedSource {
        source: Box::new(source),
    }
}

#[derive(Default)]
struct SeekState {
    /// 跳转代数：用于丢弃过期的后台 seek 完成通知。
    generation: u64,
    /// 最新的、尚未开始执行的目标；worker 每次只取一个，合并中间请求。
    pending: Option<SeekRequest>,
    /// UI 显示的最新目标，覆盖 worker 当前实际执行的位置。
    pending_target: Option<Duration>,
    /// 当前是否有一个不可抢占的阻塞 seek。
    in_flight: bool,
    shutdown: bool,
}

struct SeekRequest {
    generation: u64,
    target: Duration,
    player: Arc<Player>,
}

struct SeekWorker {
    state: Arc<Mutex<SeekState>>,
    wake: Arc<Condvar>,
    handle: Option<JoinHandle<()>>,
}

impl SeekWorker {
    fn new(state: Arc<Mutex<SeekState>>) -> Self {
        Self::with_executor(state, |player, target| {
            let _ = player.try_seek(target);
        })
    }

    fn with_executor(
        state: Arc<Mutex<SeekState>>,
        execute: impl Fn(Arc<Player>, Duration) + Send + 'static,
    ) -> Self {
        let wake = Arc::new(Condvar::new());
        let worker_state = state.clone();
        let worker_wake = wake.clone();
        let handle = std::thread::spawn(move || {
            loop {
                let request = {
                    let mut state = worker_state.lock();
                    loop {
                        if state.shutdown {
                            return;
                        }
                        let Some(request) = state.pending.take() else {
                            worker_wake.wait(&mut state);
                            continue;
                        };
                        // Admission is checked while holding the state lock. A stop,
                        // track change, or rebuild can therefore invalidate a queued
                        // request before this worker reaches rodio.
                        if request.generation != state.generation {
                            continue;
                        }
                        state.in_flight = true;
                        break request;
                    }
                };

                // This call may block inside rodio/StreamingReader and cannot be
                // preempted. The single worker is the fixed one-wait boundary.
                execute(request.player, request.target);

                let mut state = worker_state.lock();
                state.in_flight = false;
                if state.generation == request.generation && state.pending.is_none() {
                    state.pending_target = None;
                }
                worker_wake.notify_one();
            }
        });

        Self {
            state,
            wake,
            handle: Some(handle),
        }
    }

    fn submit(&self, player: Arc<Player>, target: Duration) {
        let mut state = self.state.lock();
        state.generation = state.generation.wrapping_add(1);
        let generation = state.generation;
        state.pending_target = Some(target);
        state.pending = Some(SeekRequest {
            generation,
            target,
            player,
        });
        self.wake.notify_one();
    }

    fn invalidate(&self) {
        let mut state = self.state.lock();
        state.generation = state.generation.wrapping_add(1);
        state.pending = None;
        state.pending_target = None;
        self.wake.notify_one();
    }

    fn shutdown(&mut self) {
        {
            let mut state = self.state.lock();
            state.shutdown = true;
            state.generation = state.generation.wrapping_add(1);
            state.pending = None;
            state.pending_target = None;
            self.wake.notify_one();
        }
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum AudioPlayerState {
    Playing,
    Paused,
    Stopped,
}

type MaybeError = Arc<Mutex<Option<Error>>>;

fn error_cb(error: MaybeError) -> impl Fn(Error) {
    move |e| {
        let mut error = error.lock();
        *error = Some(e)
    }
}

fn build_player(error: MaybeError) -> Result<(Player, MixerDeviceSink)> {
    let builder = DeviceSinkBuilder::from_default_device()?;
    let sink = builder.with_error_callback(error_cb(error)).open_stream()?;
    let player = Player::connect_new(sink.mixer());
    Ok((player, sink))
}

impl AudioPlayer {
    fn rebuild_on_error(&mut self) -> Result<()> {
        if self.error.lock().is_some() {
            // Never let a queued seek target survive replacing the Player.
            self.seek_worker.invalidate();
            let (player, sink) = build_player(self.error.clone())?;
            let volume = self.player.volume();
            self.player.stop();
            let new_player = Arc::new(player);
            new_player.set_volume(volume);
            self._device_sink = sink;
            self.player = new_player;
            *self.error.lock() = None;
        }
        Ok(())
    }

    fn clear_and_play(
        &mut self,
        src: impl Source + Send + 'static,
        start_paused: bool,
    ) -> Result<()> {
        // Invalidate before stopping/appending: an old worker completion must not
        // consume a new track's intent, and queued work must be admitted again.
        self.seek_worker.invalidate();
        self.rebuild_on_error()?;
        // 切歌前先取消旧流：音频线程可能正阻塞在旧 StreamingReader 的
        // read/seek 等待上，取消后立即释放，append 内部的 sleep_until_end
        // 才不会与下载互相等待形成死锁。
        if let Some(handle) = self.stream_handle.take() {
            handle.cancel();
        }
        if let Some(lease) = self.cache_lease.take() {
            drop(lease);
        }
        let old_player = self.player.clone();
        let volume = old_player.volume();
        old_player.stop();
        // Every track owns a distinct rodio queue. A request already admitted
        // against old_player can finish there, but can never seek the new source.
        let new_player = Arc::new(Player::connect_new(self._device_sink.mixer()));
        new_player.set_volume(volume);
        if start_paused {
            new_player.pause();
        }
        self.player = new_player;
        // 环内还是上一首的样本；不清掉的话示波器会先画一段前一首的波形。
        self.pcm_ring.reset();
        self.player.append(src);
        if !start_paused {
            self.player.play();
        }
        Ok(())
    }

    pub(super) fn new(config: &Config, cache_maintenance: CacheMaintenance) -> Result<Self> {
        let error = Arc::new(Mutex::new(None));
        let (player, sink) = build_player(error.clone())?;
        let cache_root = resolve_cache_root(config);
        let cache_dir = cache_root.join("audio");
        let eq = EqSettings {
            bands_db: config.eq_bands_db,
        };
        let eq_params = Arc::new(EqParams::new());
        eq_params.set_from(eq.clamp());

        let startup_dir = cache_dir.clone();
        let maintenance = cache_maintenance.clone();
        let clean_on_startup = config.cache.clean_on_startup;
        compio::runtime::spawn_blocking(move || {
            let _ = fs::create_dir_all(&startup_dir);
            let _ = cleanup_orphan_streaming_temps(&startup_dir);
            if clean_on_startup {
                maintenance.request(startup_dir);
            }
        })
        .detach();

        let seek_state = Arc::new(Mutex::new(SeekState::default()));
        let seek_worker = SeekWorker::new(seek_state);
        let player = Self {
            _device_sink: sink,
            player: Arc::new(player),
            error,
            cache_dir,
            total_duration: None,
            eq_params,
            progress_rx: None,
            seek_worker,
            stream_handle: None,
            pcm_ring: Arc::new(PcmRing::new()),
            cache_maintenance,
            cache_lease: None,
            lufs_meter: Arc::new(LufsMeter::new()),
        };

        Ok(player)
    }

    pub(super) fn cached_song_path(&self, song_id: &str, quality_level: &str) -> PathBuf {
        let quality = sanitize_cache_key(quality_level);
        let name = format!("{song_id}__{quality}.audio");
        self.cache_dir.join(name)
    }

    /// 共享 PCM 抽头环的句柄，供全屏页示波器读取真实波形。
    pub(super) fn pcm_ring(&self) -> Arc<PcmRing> {
        self.pcm_ring.clone()
    }

    pub(super) fn lufs_meter(&self) -> Arc<LufsMeter> {
        self.lufs_meter.clone()
    }

    pub(super) fn play_prepared(
        &mut self,
        source: PreparedSource,
        stream_handle: Option<StreamingReaderHandle>,
        total_duration: Option<Duration>,
        progress_rx: Option<Receiver<(u64, u64)>>,
        cache_lease: CacheLease,
        start_paused: bool,
    ) -> Result<()> {
        let PreparedSource { source } = source;
        self.clear_and_play(source, start_paused)?;
        self.stream_handle = stream_handle;
        self.total_duration = total_duration;
        self.progress_rx = progress_rx;
        self.cache_lease = Some(cache_lease);
        Ok(())
    }

    pub(super) fn cache_maintenance(&self) -> CacheMaintenance {
        self.cache_maintenance.clone()
    }

    pub(super) fn source_context(&self) -> (Arc<EqParams>, Arc<PcmRing>, Arc<LufsMeter>) {
        (
            self.eq_params.clone(),
            self.pcm_ring.clone(),
            self.lufs_meter.clone(),
        )
    }

    pub(super) fn stream_status(&self) -> StreamingStatus {
        self.stream_handle
            .as_ref()
            .map(StreamingReaderHandle::status)
            .unwrap_or(StreamingStatus::Complete)
    }

    pub(super) fn stream_has_complete_body(&self) -> bool {
        match self.stream_handle.as_ref() {
            Some(handle) => handle.has_complete_body(),
            None => true,
        }
    }

    pub(super) fn playback_failure(&self) -> Option<String> {
        let mut failures = Vec::new();
        if let Some(error) = self.error.lock().as_ref() {
            failures.push(error.to_string());
        }
        if let Some(handle) = self.stream_handle.as_ref()
            && let Some(message) = handle.error_message()
        {
            failures.push(message);
        }
        (!failures.is_empty()).then(|| failures.join("; "))
    }

    pub(super) fn set_eq(&mut self, eq: EqSettings) -> Result<()> {
        self.eq_params.set_from(eq.clamp());
        Ok(())
    }

    pub(super) fn toggle_play_pause(&mut self) {
        if self.player.empty() {
            return;
        }

        if self.player.is_paused() {
            self.player.play();
        } else {
            self.player.pause();
        }
    }

    pub(super) fn state(&self) -> AudioPlayerState {
        if self.player.empty() {
            return AudioPlayerState::Stopped;
        }

        if self.player.is_paused() {
            AudioPlayerState::Paused
        } else {
            AudioPlayerState::Playing
        }
    }

    pub(super) fn stop(&mut self) {
        if let Some(handle) = self.stream_handle.take() {
            handle.cancel();
        }
        // Cancel wakes a reader blocked inside the one in-flight seek; invalidate
        // also drops any latest target waiting behind it.
        self.seek_worker.invalidate();
        if let Some(lease) = self.cache_lease.take() {
            drop(lease);
        }
        self.player.stop();
        // 这里刻意不清 PCM 环：停止后示波器要靠这段残留把波形缓动收回中线。
        // 换歌不会漏看上一首——所有播放入口都经 clear_and_play，那里会清。
        self.progress_rx = None;
        self.total_duration = None;
    }

    pub(super) fn set_volume(&mut self, volume: f32) {
        let volume = volume.clamp(0.0, 1.0);
        self.player.set_volume(volume);
    }

    pub(super) fn volume(&self) -> f32 {
        self.player.volume()
    }

    pub(super) fn duration(&self) -> Option<Duration> {
        self.total_duration
    }

    /// Returns the latest buffered progress (downloaded, total).
    /// Uses watch channel which caches the latest value.
    pub(super) fn recv_progress(&mut self) -> Option<(u64, u64)> {
        self.progress_rx.as_mut().map(|rx| *rx.borrow())
    }

    pub(super) fn seek_to_ratio(
        &mut self,
        ratio: f32,
        fallback_total: Option<Duration>,
    ) -> Result<()> {
        let Some(total) = self.total_duration.or(fallback_total) else {
            return Ok(());
        };

        let target = Duration::from_secs_f32(total.as_secs_f32() * ratio.clamp(0.0, 1.0));
        // Admission and latest-target replacement happen under the worker state
        // lock. The UI returns immediately; no rodio mutex is touched here.
        self.seek_worker.submit(self.player.clone(), target);
        Ok(())
    }

    /// 是否正在后台加载跳转目标（UI 据此显示加载动画）。
    pub(super) fn is_seeking(&self) -> bool {
        // An invalidated old request may remain blocked until stream cancellation
        // wakes it; it must not keep the new track's UI in a loading state.
        self.seek_worker.state.lock().pending_target.is_some()
    }

    /// 用于界面显示的播放位置：后台加载期间直接显示跳转目标。
    pub(super) fn display_position(&self) -> Duration {
        self.seek_worker
            .state
            .lock()
            .pending_target
            .unwrap_or_else(|| self.player.get_pos())
    }

    pub(super) fn position(&self) -> Duration {
        self.player.get_pos()
    }
}

impl Drop for AudioPlayer {
    fn drop(&mut self) {
        // Wake a StreamingReader and stop the current queue before joining the
        // sole worker. An uninterruptible seek remains a fixed one-wait boundary.
        if let Some(handle) = &self.stream_handle {
            handle.cancel();
        }
        self.player.stop();
        self.seek_worker.shutdown();
    }
}

pub(super) struct EqParams {
    bands_db_x10: [AtomicI32; EQ_BANDS],
}

impl EqParams {
    fn new() -> Self {
        Self {
            bands_db_x10: std::array::from_fn(|_| AtomicI32::new(0)),
        }
    }

    fn set_from(&self, eq: EqSettings) {
        let eq = eq.clamp();
        for (idx, value) in eq.bands_db.iter().enumerate() {
            self.bands_db_x10[idx].store((value * 10.0).round() as i32, Ordering::Relaxed);
        }
    }

    fn load_db(&self) -> [f32; EQ_BANDS] {
        std::array::from_fn(|idx| self.bands_db_x10[idx].load(Ordering::Relaxed) as f32 / 10.0)
    }

    fn load_db_x10(&self) -> [i32; EQ_BANDS] {
        std::array::from_fn(|idx| self.bands_db_x10[idx].load(Ordering::Relaxed))
    }
}

struct BiquadCoeffs {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
}

#[derive(Default, Clone, Copy)]
struct BiquadState {
    x1: f32,
    x2: f32,
    y1: f32,
    y2: f32,
}

fn biquad_peaking(fs: f32, f0: f32, q: f32, gain_db: f32) -> BiquadCoeffs {
    let fs = if fs > 0.0 { fs } else { 44100.0 };
    let f0 = f0.clamp(10.0, fs * 0.45);
    let q = q.max(0.001);

    let a = 10.0_f32.powf(gain_db / 40.0);
    let w0 = 2.0 * std::f32::consts::PI * (f0 / fs);
    let cos_w0 = w0.cos();
    let sin_w0 = w0.sin();
    let alpha = sin_w0 / (2.0 * q);

    let b0 = 1.0 + alpha * a;
    let b1 = -2.0 * cos_w0;
    let b2 = 1.0 - alpha * a;
    let a0 = 1.0 + alpha / a;
    let a1 = -2.0 * cos_w0;
    let a2 = 1.0 - alpha / a;

    BiquadCoeffs {
        b0: b0 / a0,
        b1: b1 / a0,
        b2: b2 / a0,
        a1: a1 / a0,
        a2: a2 / a0,
    }
}

fn biquad_process(coeffs: &BiquadCoeffs, state: &mut BiquadState, input: f32) -> f32 {
    let output = coeffs.b0 * input + coeffs.b1 * state.x1 + coeffs.b2 * state.x2
        - coeffs.a1 * state.y1
        - coeffs.a2 * state.y2;
    state.x2 = state.x1;
    state.x1 = input;
    state.y2 = state.y1;
    state.y1 = output;
    output
}

struct EqSource<S>
where
    S: Source<Item = f32>,
{
    inner: S,
    channels: NonZero<u16>,
    idx: usize,
    params: Arc<EqParams>,
    last_db_x10: [i32; EQ_BANDS],
    coeffs: [BiquadCoeffs; EQ_BANDS],
    states: Vec<BiquadState>,
    tap: PcmTap,
}

impl<S> EqSource<S>
where
    S: Source<Item = f32>,
{
    fn new(
        inner: S,
        params: Arc<EqParams>,
        pcm_ring: Arc<PcmRing>,
        lufs_meter: Arc<LufsMeter>,
    ) -> Self {
        let channels = inner.channels();
        let sample_rate = inner.sample_rate().get();
        let fs = sample_rate as f32;
        let eq_db = params.load_db();
        let last_db_x10 = params.load_db_x10();
        let coeffs =
            std::array::from_fn(|idx| biquad_peaking(fs, EQ_FREQS_HZ[idx], 1.0, eq_db[idx]));
        let states = vec![BiquadState::default(); (channels.get() as usize) * EQ_BANDS];

        Self {
            inner,
            channels,
            idx: 0,
            params,
            last_db_x10,
            coeffs,
            states,
            tap: PcmTap::new(
                pcm_ring,
                channels.get() as usize,
                sample_rate,
                Some(lufs_meter),
            ),
        }
    }

    fn state_index(&self, channel: usize, band: usize) -> usize {
        channel * EQ_BANDS + band
    }
}

impl<S> Iterator for EqSource<S>
where
    S: Source<Item = f32>,
{
    type Item = f32;

    fn next(&mut self) -> Option<Self::Item> {
        let current = self.params.load_db_x10();
        if current != self.last_db_x10 {
            let fs = self.inner.sample_rate().get() as f32;
            let eq_db = self.params.load_db();
            self.coeffs =
                std::array::from_fn(|idx| biquad_peaking(fs, EQ_FREQS_HZ[idx], 1.0, eq_db[idx]));
            self.last_db_x10 = current;
        }

        let input = self.inner.next()?;
        let channel =
            (self.idx % (self.channels.get() as usize)).min(self.channels.get() as usize - 1);
        self.idx = self.idx.wrapping_add(1);

        let mut output = input;
        for band in 0..EQ_BANDS {
            let state_idx = self.state_index(channel, band);
            output = biquad_process(&self.coeffs[band], &mut self.states[state_idx], output);
        }
        // PCM 抽头取 EQ 之后、音量之前的样本：波形反映均衡效果，但不随音量缩放。
        // 逐样本只写一次暂存数组，攒够一批才落环（见 pcm_tap 的线程模型说明）。
        self.tap.push_sample(channel, output);
        Some(output)
    }
}

impl<S> Source for EqSource<S>
where
    S: Source<Item = f32>,
{
    fn current_span_len(&self) -> Option<usize> {
        self.inner.current_span_len()
    }

    fn channels(&self) -> NonZero<u16> {
        self.inner.channels()
    }

    fn sample_rate(&self) -> NonZero<u32> {
        self.inner.sample_rate()
    }

    fn total_duration(&self) -> Option<Duration> {
        self.inner.total_duration()
    }

    fn try_seek(&mut self, pos: Duration) -> std::result::Result<(), SeekError> {
        for state in &mut self.states {
            *state = BiquadState::default();
        }
        // 环里是跳转前那一段的样本，留着会让示波器先闪一下旧波形。
        self.tap.reset();
        self.inner.try_seek(pos)
    }
}

pub(super) async fn is_nonempty_file(path: &Path) -> bool {
    compio::fs::metadata(path)
        .await
        .is_ok_and(|metadata| metadata.is_file() && metadata.len() > 0)
}

fn sanitize_cache_key(raw: &str) -> String {
    let mut out = String::new();
    for ch in raw.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
        } else if ch == '-' || ch == '_' {
            out.push(ch);
        }
    }
    if out.is_empty() {
        "exhigh".to_string()
    } else {
        out
    }
}

pub(super) fn resolve_cache_root(config: &Config) -> PathBuf {
    if let Some(custom) = config
        .cache
        .path
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        return PathBuf::from(custom);
    }

    STORAGE.cache.clone()
}

#[cfg(test)]
mod seek_worker_tests;
