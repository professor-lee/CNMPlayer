use crate::STORAGE;
use crate::app::streaming::{StreamingReader, StreamingReaderHandle};
use crate::data::config::{CacheCleanStrategy, Config};
use crate::tmplayer::app::state::{EQ_BANDS, EQ_FREQS_HZ, EqSettings};
use crate::tmplayer::audio::pcm_tap::{PcmRing, PcmTap};
use anyhow::{Context, Result};
use rodio::cpal::Error;
use rodio::decoder::DecoderBuilder;
use rodio::source::SeekError;
use rodio::{DeviceSinkBuilder, MixerDeviceSink, Player, Source};
use see::sync::Receiver;
use std::borrow::Cow;
use std::fs;
use std::fs::File;
use std::io::BufReader;
use std::num::NonZero;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioPlayerState {
    Playing,
    Paused,
    Stopped,
}

type MaybeError = Arc<Mutex<Option<Error>>>;

pub struct AudioPlayer {
    _device_sink: MixerDeviceSink,
    player: Arc<Player>,
    error: MaybeError,
    cache_dir: PathBuf,
    total_duration: Option<Duration>,
    eq_params: Arc<EqParams>,
    progress_rx: Option<Receiver<(u64, u64)>>,
    seek_state: Arc<Mutex<SeekState>>,
    stream_handle: Option<StreamingReaderHandle>,
    /// PCM 时域抽头：`EqSource` 逐样本写入，全屏页示波器读取。
    /// 环的寿命长于单个 `EqSource`，故切歌与跳转时必须显式重置。
    pcm_ring: Arc<PcmRing>,
    /// 切歌时若有未完成的跳转，下次 clear_and_play 需要用归零 seek 使其失效
    invalidate_seek_on_next_play: bool,
}

#[derive(Default)]
struct SeekState {
    /// 跳转代数：用于丢弃过期的后台 seek 完成通知
    generation: u64,
    /// Some(target) 表示一次后台跳转/加载正在进行
    pending_target: Option<Duration>,
}

fn error_cb(error: MaybeError) -> impl Fn(Error) {
    move |e| {
        let mut error = error.lock().unwrap();
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
        let mut error = self.error.lock().unwrap();
        if error.is_some() {
            let (player, sink) = build_player(self.error.clone())?;
            self._device_sink = sink;
            self.player = Arc::new(player);
            *error = None;
        };
        Ok(())
    }

    fn clear_and_play(&mut self, src: impl Source + Send + 'static) -> Result<()> {
        self.rebuild_on_error()?;
        // 切歌前先取消旧流：音频线程可能正阻塞在旧 StreamingReader 的
        // read/seek 等待上，取消后立即释放，append 内部的 sleep_until_end
        // 才不会与下载互相等待形成死锁。
        if let Some(handle) = self.stream_handle.take() {
            handle.cancel();
        }
        let had_pending_seek = self.invalidate_seek_on_next_play
            || self.seek_state.lock().unwrap().pending_target.is_some();
        self.player.stop();
        // 环内还是上一首的样本；不清掉的话示波器会先画一段前一首的波形。
        self.pcm_ring.reset();
        self.player.append(src);
        self.player.play();
        if had_pending_seek {
            // 让可能残留的旧 seek 指令失效：换成归零指令（新源上瞬时完成），
            // 同时旧 seek 的后台线程会因反馈通道关闭而立即退出。
            let mut state = self.seek_state.lock().unwrap();
            state.generation = state.generation.wrapping_add(1);
            state.pending_target = None;
            drop(state);
            self.invalidate_seek_on_next_play = false;
            let _ = self.player.try_seek(Duration::ZERO);
        }
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

        if config.cache.clean_on_startup {
            let _ = cleanup_cache_dir(&cache_dir, &config.cache);
        }
        let _ = fs::create_dir_all(&cache_dir);

        let player = Self {
            _device_sink: sink,
            player: Arc::new(player),
            error,
            cache_dir,
            total_duration: None,
            eq_params,
            progress_rx: None,
            seek_state: Arc::new(Mutex::new(SeekState::default())),
            stream_handle: None,
            pcm_ring: Arc::new(PcmRing::new()),
            invalidate_seek_on_next_play: false,
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

    pub fn play_from_file(&mut self, file_path: &PathBuf) -> Result<()> {
        let file = File::open(file_path)?;
        let builder = DecoderBuilder::new().with_byte_len(file.metadata()?.len());
        let decoder = builder.with_data(BufReader::new(file)).build()?;
        let total_duration = decoder.total_duration();
        let source = EqSource::new(decoder, self.eq_params.clone(), self.pcm_ring.clone());
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
        let stream_handle = StreamingReaderHandle::from(&reader);
        let builder = DecoderBuilder::new().with_byte_len(reader.total());
        let f = move || builder.with_data(BufReader::new(reader)).build();
        let decoder = compio::runtime::spawn_blocking(f).await.unwrap()?;
        let total_duration = decoder.total_duration();
        let source = EqSource::new(decoder, self.eq_params.clone(), self.pcm_ring.clone());
        self.clear_and_play(source)?;
        self.stream_handle = Some(stream_handle);
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
        self.player.stop();
        // 这里刻意不清 PCM 环：停止后示波器要靠这段残留把波形缓动收回中线。
        // 换歌不会漏看上一首——所有播放入口都经 clear_and_play，那里会清。
        self.progress_rx = None;
        self.total_duration = None;
        // 丢弃未完成的跳转：切歌后旧的 seek 结果不再有意义
        let had_pending = self.seek_state.lock().unwrap().pending_target.is_some();
        self.invalidate_seek_on_next_play |= had_pending;
        let mut state = self.seek_state.lock().unwrap();
        state.generation = state.generation.wrapping_add(1);
        state.pending_target = None;
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

        // 记录跳转意图：UI 立即把进度条显示到目标位置，并进入“加载中”状态。
        let generation = {
            let mut state = self.seek_state.lock().unwrap();
            state.generation = state.generation.wrapping_add(1);
            state.pending_target = Some(target);
            state.generation
        };

        // 后台执行一次阻塞 seek（不重试）：目标位置尚未下载时，音频线程会在
        // StreamingReader::seek 中等待（仅播放冻结，无死锁——下载任务在主
        // runtime 上持续推进），下载追到目标后 seek 完成、自动继续播放。
        // 用 spawn_blocking 跑在共享阻塞线程池上（不新建 OS 线程），
        // detach 不等待结果：seek 完成的收尾由 seek_state 的代数机制处理。
        let player = self.player.clone();
        let seek_state = self.seek_state.clone();
        compio::runtime::spawn_blocking(move || {
            let _ = player.try_seek(target);
            let mut state = seek_state.lock().unwrap();
            if state.generation == generation {
                state.pending_target = None;
            }
        })
        .detach();

        Ok(())
    }

    /// 是否正在后台加载跳转目标（UI 据此显示加载动画）。
    pub fn is_seeking(&self) -> bool {
        self.seek_state.lock().unwrap().pending_target.is_some()
    }

    /// 用于界面显示的播放位置：后台加载期间直接显示跳转目标。
    pub fn display_position(&self) -> Duration {
        self.seek_state
            .lock()
            .unwrap()
            .pending_target
            .unwrap_or_else(|| self.player.get_pos())
    }

    pub fn position(&self) -> Duration {
        self.player.get_pos()
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
    fn new(inner: S, params: Arc<EqParams>, pcm_ring: Arc<PcmRing>) -> Self {
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
            tap: PcmTap::new(pcm_ring, channels.get() as usize, sample_rate),
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

pub fn is_nonempty_file(path: &Path) -> bool {
    if !path.is_file() {
        return false;
    }
    fs::metadata(path).map(|meta| meta.len()).unwrap_or(0) > 0
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

pub(crate) fn resolve_cache_root(config: &Config) -> Cow<'static, PathBuf> {
    if let Some(custom) = config
        .cache
        .path
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        return Cow::Owned(PathBuf::from(custom));
    }

    Cow::Borrowed(&STORAGE.cache)
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
        if !path.is_file() {
            continue;
        }

        let metadata = entry
            .metadata()
            .with_context(|| format!("read cache metadata failed: {}", path.display()))?;
        let modified = metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH);

        out.push(CacheEntry {
            path,
            size: metadata.len(),
            modified,
        });
    }

    Ok(out)
}
