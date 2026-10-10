use crate::STORAGE;
use crate::app::streaming::{StreamingReader, StreamingReaderHandle};
use crate::data::config::{CacheCleanStrategy, Config};
use crate::tmplayer::app::state::{EQ_BANDS, EQ_FREQS_HZ, EqSettings};
use crate::tmplayer::audio::lufs_meter::LufsMeter;
use crate::tmplayer::audio::pcm_tap::{PcmRing, PcmTap};
use anyhow::{Context, Result};
use parking_lot::{Condvar, Mutex};
use rodio::cpal::Error;
use rodio::decoder::DecoderBuilder;
use rodio::source::SeekError;
use rodio::{DeviceSinkBuilder, MixerDeviceSink, Player, Source};
use see::sync::Receiver;
use std::fs;
use std::fs::File;
use std::io::BufReader;
use std::num::NonZero;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicI32, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, SystemTime};
pub struct AudioPlayer {
    _device_sink: MixerDeviceSink,
    player: Arc<Player>,
    error: MaybeError,
    cache_dir: PathBuf,
    total_duration: Option<Duration>,
    eq_params: Arc<EqParams>,
    progress_rx: Option<Receiver<(u64, u64)>>,
    seek_worker: SeekWorker,
    stream_handle: Option<StreamingReaderHandle>,
    /// PCM 时域抽头：`EqSource` 逐样本写入，全屏页示波器读取。
    /// 环的寿命长于单个 `EqSource`，故切歌与跳转时必须显式重置。
    pcm_ring: Arc<PcmRing>,
    /// 400ms Momentary LUFS 计量器；`EqSource` 按批写入，小窗口窄窗读取。
    lufs_meter: Arc<LufsMeter>,
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
pub enum AudioPlayerState {
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

    fn clear_and_play(&mut self, src: impl Source + Send + 'static) -> Result<()> {
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
        let old_player = self.player.clone();
        let volume = old_player.volume();
        old_player.stop();
        // Every track owns a distinct rodio queue. A request already admitted
        // against old_player can finish there, but can never seek the new source.
        let new_player = Arc::new(Player::connect_new(self._device_sink.mixer()));
        new_player.set_volume(volume);
        self.player = new_player;
        // 环内还是上一首的样本；不清掉的话示波器会先画一段前一首的波形。
        self.pcm_ring.reset();
        self.player.append(src);
        self.player.play();
        Ok(())
    }

    pub fn new(config: &Config) -> Result<Self> {
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
        let cache_policy = config.cache.clone();
        compio::runtime::spawn_blocking(move || {
            let _ = fs::create_dir_all(&startup_dir);
            if cache_policy.clean_on_startup {
                let _ = cleanup_cache_dir(&startup_dir, &cache_policy);
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
            lufs_meter: Arc::new(LufsMeter::new()),
        };

        Ok(player)
    }

    pub fn cached_song_path(&self, song_id: &str, quality_level: &str) -> PathBuf {
        let quality = sanitize_cache_key(quality_level);
        let name = format!("{song_id}__{quality}.audio");
        self.cache_dir.join(name)
    }

    /// 共享 PCM 抽头环的句柄，供全屏页示波器读取真实波形。
    pub fn pcm_ring(&self) -> Arc<PcmRing> {
        self.pcm_ring.clone()
    }

    pub fn lufs_meter(&self) -> Arc<LufsMeter> {
        self.lufs_meter.clone()
    }

    pub async fn play_from_file(&mut self, file_path: &Path) -> Result<()> {
        let file_path = file_path.to_path_buf();
        let decoder = compio::runtime::spawn_blocking(move || -> Result<_> {
            let file = File::open(file_path)?;
            let builder = DecoderBuilder::new().with_byte_len(file.metadata()?.len());
            Ok(builder.with_data(BufReader::new(file)).build()?)
        })
        .await
        .map_err(|_| anyhow::anyhow!("audio decoder task panicked"))??;
        let total_duration = decoder.total_duration();
        let source = EqSource::new(
            decoder,
            self.eq_params.clone(),
            self.pcm_ring.clone(),
            self.lufs_meter.clone(),
        );
        self.clear_and_play(source)?;
        self.stream_handle = None;
        self.total_duration = total_duration;
        self.progress_rx = None;
        Ok(())
    }

    pub async fn play_streaming(
        &mut self,
        reader: StreamingReader,
        progress_rx: Receiver<(u64, u64)>,
    ) -> Result<()> {
        struct PendingDecode(Option<StreamingReaderHandle>);
        impl Drop for PendingDecode {
            fn drop(&mut self) {
                if let Some(handle) = self.0.take() {
                    handle.cancel();
                }
            }
        }
        let mut pending = PendingDecode(Some(StreamingReaderHandle::from(&reader)));
        let builder = DecoderBuilder::new().with_byte_len(reader.total());
        let f = move || builder.with_data(BufReader::new(reader)).build();
        let decoder = compio::runtime::spawn_blocking(f)
            .await
            .map_err(|_| anyhow::anyhow!("streaming decoder task panicked"))??;
        let stream_handle = pending.0.take();
        let total_duration = decoder.total_duration();
        let source = EqSource::new(
            decoder,
            self.eq_params.clone(),
            self.pcm_ring.clone(),
            self.lufs_meter.clone(),
        );
        self.clear_and_play(source)?;
        self.stream_handle = stream_handle;
        self.total_duration = total_duration;
        self.progress_rx = Some(progress_rx);
        Ok(())
    }

    pub fn set_eq(&mut self, eq: EqSettings) -> Result<()> {
        self.eq_params.set_from(eq.clamp());
        Ok(())
    }

    pub fn toggle_play_pause(&mut self) {
        if self.player.empty() {
            return;
        }

        if self.player.is_paused() {
            self.player.play();
        } else {
            self.player.pause();
        }
    }

    pub fn state(&self) -> AudioPlayerState {
        if self.player.empty() {
            return AudioPlayerState::Stopped;
        }

        if self.player.is_paused() {
            AudioPlayerState::Paused
        } else {
            AudioPlayerState::Playing
        }
    }

    pub fn stop(&mut self) {
        if let Some(handle) = self.stream_handle.take() {
            handle.cancel();
        }
        // Cancel wakes a reader blocked inside the one in-flight seek; invalidate
        // also drops any latest target waiting behind it.
        self.seek_worker.invalidate();
        self.player.stop();
        // 这里刻意不清 PCM 环：停止后示波器要靠这段残留把波形缓动收回中线。
        // 换歌不会漏看上一首——所有播放入口都经 clear_and_play，那里会清。
        self.progress_rx = None;
        self.total_duration = None;
    }

    pub fn set_volume(&mut self, volume: f32) {
        let volume = volume.clamp(0.0, 1.0);
        self.player.set_volume(volume);
    }

    pub fn volume(&self) -> f32 {
        self.player.volume()
    }

    pub fn duration(&self) -> Option<Duration> {
        self.total_duration
    }

    /// Returns the latest buffered progress (downloaded, total).
    /// Uses watch channel which caches the latest value.
    pub fn recv_progress(&mut self) -> Option<(u64, u64)> {
        self.progress_rx.as_mut().map(|rx| *rx.borrow())
    }

    pub fn seek_to_ratio(&mut self, ratio: f32, fallback_total: Option<Duration>) -> Result<()> {
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
    pub fn is_seeking(&self) -> bool {
        // An invalidated old request may remain blocked until stream cancellation
        // wakes it; it must not keep the new track's UI in a loading state.
        self.seek_worker.state.lock().pending_target.is_some()
    }

    /// 用于界面显示的播放位置：后台加载期间直接显示跳转目标。
    pub fn display_position(&self) -> Duration {
        self.seek_worker
            .state
            .lock()
            .pending_target
            .unwrap_or_else(|| self.player.get_pos())
    }

    pub fn position(&self) -> Duration {
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

struct EqParams {
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

pub async fn is_nonempty_file(path: &Path) -> bool {
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

pub(crate) fn resolve_cache_root(config: &Config) -> PathBuf {
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

#[derive(Debug, Clone)]
struct CacheEntry {
    path: PathBuf,
    size: u64,
    modified: SystemTime,
}

pub(crate) fn cleanup_cache_dir(
    cache_dir: &Path,
    policy: &crate::data::config::CacheConfig,
) -> Result<()> {
    let mut entries = list_cache_entries(cache_dir)?;

    if matches!(
        policy.clean_strategy,
        CacheCleanStrategy::Age | CacheCleanStrategy::Both
    ) && policy.max_age_days > 0
    {
        let now = SystemTime::now();
        let ttl = Duration::from_secs(policy.max_age_days.saturating_mul(24 * 60 * 60));
        entries.retain(|entry| {
            let expired = now
                .duration_since(entry.modified)
                .map(|elapsed| elapsed > ttl)
                .unwrap_or(false);
            if expired {
                let _ = fs::remove_file(&entry.path);
                return false;
            }
            true
        });
    }

    if matches!(
        policy.clean_strategy,
        CacheCleanStrategy::Size | CacheCleanStrategy::Both
    ) && policy.max_size_mb > 0
    {
        let limit_bytes = policy.max_size_mb.saturating_mul(1024 * 1024);
        let mut total_bytes = entries.iter().map(|entry| entry.size).sum::<u64>();

        if total_bytes > limit_bytes {
            entries.sort_by_key(|entry| entry.modified);
            for entry in entries {
                if total_bytes <= limit_bytes {
                    break;
                }
                if fs::remove_file(&entry.path).is_ok() {
                    total_bytes = total_bytes.saturating_sub(entry.size);
                }
            }
        }
    }

    Ok(())
}

fn list_cache_entries(cache_dir: &Path) -> Result<Vec<CacheEntry>> {
    let mut out = Vec::new();

    if !cache_dir.is_dir() {
        return Ok(out);
    }

    for entry in fs::read_dir(cache_dir)
        .with_context(|| format!("read cache dir failed: {}", cache_dir.display()))?
    {
        let entry = entry?;
        let path = entry.path();
        // Active writers own temporary files; maintenance must never delete them.
        if path.extension().is_some_and(|ext| ext == "part") {
            continue;
        }

        let metadata = entry
            .metadata()
            .with_context(|| format!("read cache metadata failed: {}", path.display()))?;
        if !metadata.is_file() {
            continue;
        }
        let modified = metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH);

        out.push(CacheEntry {
            path,
            size: metadata.len(),
            modified,
        });
    }

    Ok(out)
}

#[cfg(test)]
mod seek_worker_tests {
    use super::*;
    use std::sync::atomic::AtomicBool;
    use std::sync::mpsc::{self, Receiver as TestReceiver, Sender};

    #[derive(Default)]
    struct Gate {
        open: Mutex<bool>,
        wake: Condvar,
    }

    impl Gate {
        fn wait(&self) {
            let mut open = self.open.lock();
            while !*open {
                self.wake.wait(&mut open);
            }
        }

        fn release(&self) {
            *self.open.lock() = true;
            self.wake.notify_all();
        }
    }

    struct ProbeSource {
        id: u8,
        seeks: Sender<(u8, Duration)>,
        gate: Arc<Gate>,
    }

    impl Iterator for ProbeSource {
        type Item = f32;

        fn next(&mut self) -> Option<f32> {
            Some(0.0)
        }
    }

    impl Source for ProbeSource {
        fn current_span_len(&self) -> Option<usize> {
            None
        }

        fn channels(&self) -> rodio::ChannelCount {
            NonZero::new(1).unwrap()
        }

        fn sample_rate(&self) -> rodio::SampleRate {
            NonZero::new(1000).unwrap()
        }

        fn total_duration(&self) -> Option<Duration> {
            Some(Duration::from_secs(10))
        }

        fn try_seek(&mut self, target: Duration) -> std::result::Result<(), SeekError> {
            self.seeks.send((self.id, target)).unwrap();
            self.gate.wait();
            Ok(())
        }
    }

    struct Probe {
        worker: SeekWorker,
        mixer: rodio::mixer::Mixer,
        events: TestReceiver<(u8, Duration)>,
        sender: Sender<(u8, Duration)>,
        gate: Arc<Gate>,
        render_stop: Arc<AtomicBool>,
        render: Option<JoinHandle<()>>,
    }

    impl Probe {
        fn new(execute: impl Fn(Arc<Player>, Duration) + Send + 'static) -> Self {
            let (mixer, mut output) =
                rodio::mixer::mixer(NonZero::new(1).unwrap(), NonZero::new(1000).unwrap());
            let render_stop = Arc::new(AtomicBool::new(false));
            let stop = render_stop.clone();
            let render = std::thread::spawn(move || {
                while !stop.load(Ordering::SeqCst) {
                    // A clocked software audio callback, not an executor poll.
                    for _ in 0..5 {
                        let _ = output.next();
                    }
                    std::thread::sleep(Duration::from_millis(5));
                }
            });
            let (sender, events) = mpsc::channel();
            Self {
                worker: SeekWorker::with_executor(
                    Arc::new(Mutex::new(SeekState::default())),
                    execute,
                ),
                mixer,
                events,
                sender,
                gate: Arc::new(Gate::default()),
                render_stop,
                render: Some(render),
            }
        }

        fn player(&self, id: u8) -> Arc<Player> {
            let player = Arc::new(Player::connect_new(&self.mixer));
            player.append(ProbeSource {
                id,
                seeks: self.sender.clone(),
                gate: self.gate.clone(),
            });
            player
        }

        fn event(&self) -> (u8, Duration) {
            self.events.recv_timeout(Duration::from_secs(3)).unwrap()
        }

        fn idle(&self) {
            let mut state = self.worker.state.lock();
            while state.in_flight || state.pending.is_some() {
                let wait = self
                    .worker
                    .wake
                    .wait_for(&mut state, Duration::from_secs(3));
                assert!(!wait.timed_out(), "seek worker did not become idle");
            }
        }
    }

    impl Drop for Probe {
        fn drop(&mut self) {
            {
                let mut state = self.worker.state.lock();
                state.shutdown = true;
                state.pending = None;
            }
            self.gate.release();
            self.worker.shutdown();
            self.render_stop.store(true, Ordering::SeqCst);
            if let Some(render) = self.render.take() {
                let _ = render.join();
            }
        }
    }

    #[test]
    fn seek_worker_singleflight_coalesces_latest() {
        let probe = Probe::new(|player, target| {
            let _ = player.try_seek(target);
        });
        let player = probe.player(1);
        probe.worker.submit(player.clone(), Duration::from_secs(1));
        assert_eq!(probe.event(), (1, Duration::from_secs(1)));
        for seconds in [2, 3, 4] {
            probe
                .worker
                .submit(player.clone(), Duration::from_secs(seconds));
        }
        assert!(probe.worker.state.lock().in_flight);
        assert_eq!(
            probe.worker.state.lock().pending_target,
            Some(Duration::from_secs(4))
        );
        probe.gate.release();
        assert_eq!(probe.event(), (1, Duration::from_secs(4)));
        probe.idle();
        assert!(probe.events.try_recv().is_err());
        assert!(probe.worker.state.lock().pending_target.is_none());
    }

    #[test]
    fn seek_worker_invalidate_drops_pending_and_preserves_new_intent() {
        let probe = Probe::new(|player, target| {
            let _ = player.try_seek(target);
        });
        let old = probe.player(1);
        probe.worker.submit(old.clone(), Duration::from_secs(1));
        assert_eq!(probe.event(), (1, Duration::from_secs(1)));
        probe.worker.submit(old.clone(), Duration::from_secs(2));
        // stop/change/rebuild all use this invalidation before replacing queues.
        probe.worker.invalidate();
        assert!(probe.worker.state.lock().pending_target.is_none());
        old.stop();
        let new = probe.player(2);
        probe.worker.submit(new, Duration::from_secs(3));
        probe.gate.release();
        assert_eq!(probe.event(), (2, Duration::from_secs(3)));
        probe.idle();
        assert!(probe.events.try_recv().is_err());
    }

    #[test]
    fn seek_worker_admitted_old_request_cannot_seek_fresh_player() {
        let before_call = Arc::new(Gate::default());
        let frozen = before_call.clone();
        let (entered_tx, entered_rx) = mpsc::channel();
        let probe = Probe::new(move |player, target| {
            entered_tx.send(target).unwrap();
            frozen.wait();
            let _ = player.try_seek(target);
        });
        struct ReleaseOnDrop(Arc<Gate>);
        impl Drop for ReleaseOnDrop {
            fn drop(&mut self) {
                self.0.release();
            }
        }
        // Declared after Probe so assertion unwinding opens the admission gate
        // before Probe joins its worker.
        let _release = ReleaseOnDrop(before_call.clone());
        let old = probe.player(1);
        probe.gate.release();
        probe.worker.submit(old.clone(), Duration::from_secs(1));
        assert_eq!(
            entered_rx.recv_timeout(Duration::from_secs(3)).unwrap(),
            Duration::from_secs(1)
        );
        probe.worker.invalidate();
        old.stop();
        let new = probe.player(2);
        probe.worker.submit(new, Duration::from_secs(4));
        before_call.release();
        let first = probe.event();
        if first == (1, Duration::from_secs(1)) {
            assert_eq!(probe.event(), (2, Duration::from_secs(4)));
        } else {
            assert_eq!(first, (2, Duration::from_secs(4)));
        }
        probe.idle();
        assert!(probe.events.try_recv().is_err());
    }
}
