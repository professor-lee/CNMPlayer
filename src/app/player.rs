use crate::app::streaming::StreamingReader;
use crate::data::assets;
use crate::data::config::{CacheCleanStrategy, Config};
use crate::tmplayer::app::state::{EQ_BANDS, EQ_FREQS_HZ, EqSettings};
use anyhow::{Context, Result};
use directories::BaseDirs;
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
use std::time::{Duration, SystemTime};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioPlayerState {
    Playing,
    Paused,
    Stopped,
}

pub struct AudioPlayer {
    _device_sink: MixerDeviceSink,
    player: Player,
    cache_dir: PathBuf,
    total_duration: Option<Duration>,
    eq_params: Arc<EqParams>,
    progress_rx: Option<Receiver<(u64, u64)>>,
}

impl AudioPlayer {
    pub fn new(config: &Config) -> Result<Self> {
        let sink = DeviceSinkBuilder::open_default_sink()?;
        let player = Player::connect_new(sink.mixer());
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
            player,
            cache_dir,
            total_duration: None,
            eq_params,
            progress_rx: None,
        };

        Ok(player)
    }

    pub fn cached_song_path(&self, song_id: &str, quality_level: &str) -> PathBuf {
        let quality = sanitize_cache_key(quality_level);
        let name = format!("{song_id}__{quality}.audio");
        self.cache_dir.join(name)
    }

    pub fn play_from_file(&mut self, file_path: &PathBuf) -> Result<()> {
        let file = File::open(file_path)?;
        let builder = DecoderBuilder::new().with_byte_len(file.metadata()?.len());
        let decoder = builder.with_data(BufReader::new(file)).build()?;
        let total_duration = decoder.total_duration();
        let source = EqSource::new(decoder, self.eq_params.clone());

        self.player.stop();
        self.player.append(source);
        self.player.play();

        self.total_duration = total_duration;
        self.progress_rx = None;
        Ok(())
    }

    pub async fn play_streaming(
        &mut self,
        reader: StreamingReader,
        progress_rx: Receiver<(u64, u64)>,
    ) -> Result<()> {
        let builder = DecoderBuilder::new().with_byte_len(reader.total());
        let f = move || builder.with_data(BufReader::new(reader)).build();
        let decoder = compio::runtime::spawn_blocking(f).await.unwrap()?;
        let total_duration = decoder.total_duration();
        let source = EqSource::new(decoder, self.eq_params.clone());

        self.player.stop();
        self.player.append(source);
        self.player.play();

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
        self.player.stop();
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

        self.player.try_seek(target)?;
        Ok(())
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
}

impl<S> EqSource<S>
where
    S: Source<Item = f32>,
{
    fn new(inner: S, params: Arc<EqParams>) -> Self {
        let channels = inner.channels();
        let fs = inner.sample_rate().get() as f32;
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

    system_cache_root().unwrap_or_else(|| assets::resolve_asset_path(Path::new("cache")))
}

fn system_cache_root() -> Option<PathBuf> {
    BaseDirs::new().map(|dirs| dirs.cache_dir().join("cnmplayer"))
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
