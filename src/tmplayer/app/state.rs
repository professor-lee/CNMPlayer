use crate::app::{SIDEBAR_ANIM_DURATION, cubic_bezier_y};
use crate::data::config::Language;
use crate::tmplayer::audio::smoother::Ema;
use crate::tmplayer::data::config::Config;
use crate::tmplayer::data::playlist::Playlist;
use crate::tmplayer::playback::remote_fetch::TrackKey;
use crate::tmplayer::render::cover_cache::CoverCache;
use crate::tmplayer::render::cover_cache::CoverKey;
use crate::tmplayer::render::cover_renderer::render_cover_ascii;
use crate::tmplayer::ui::theme::Theme;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc;
use std::sync::mpsc::{Receiver, Sender, TryRecvError};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlayMode {
    Idle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaybackState {
    Playing,
    Paused,
    Stopped,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RepeatMode {
    Sequence,
    Shuffle,
    LoopAll,
    LoopOne,
}

impl RepeatMode {
    pub fn next(self) -> Self {
        match self {
            RepeatMode::Sequence => RepeatMode::Shuffle,
            RepeatMode::Shuffle => RepeatMode::LoopAll,
            RepeatMode::LoopAll => RepeatMode::LoopOne,
            RepeatMode::LoopOne => RepeatMode::Sequence,
        }
    }

    pub fn symbol(self) -> &'static str {
        match self {
            // 需求：使用 Nerd Font 图标
            // 顺序播放 ，随机播放 ，列表循环 ，单曲循环 
            RepeatMode::Sequence => "",
            RepeatMode::Shuffle => "",
            RepeatMode::LoopAll => "",
            RepeatMode::LoopOne => "",
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct EqSettings {
    pub bands_db: [f32; EQ_BANDS],
}

pub const EQ_BANDS: usize = 10;
pub const EQ_FREQS_HZ: [f32; EQ_BANDS] = [
    31.0, 62.0, 125.0, 250.0, 500.0, 1000.0, 2000.0, 4000.0, 8000.0, 16000.0,
];

impl Default for EqSettings {
    fn default() -> Self {
        Self {
            bands_db: [0.0; EQ_BANDS],
        }
    }
}

impl EqSettings {
    pub fn clamp(self) -> Self {
        let mut out = self;
        for v in &mut out.bands_db {
            *v = v.clamp(-12.0, 12.0);
        }
        out
    }
}

#[derive(Debug, Clone)]
pub struct LyricLine {
    pub start_ms: u64,
    pub text: String,
}

#[derive(Debug, Clone)]
pub struct TrackMetadata {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub duration: Duration,
    pub cover: Option<Vec<u8>>,
    pub cover_hash: Option<u64>,
    pub cover_folder: Option<PathBuf>,
    pub lyrics: Option<Vec<LyricLine>>,
}

#[derive(Debug, Clone)]
pub struct CoverSnapshot {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub cover: Option<Vec<u8>>,
    pub cover_hash: Option<u64>,
    pub cover_folder: Option<PathBuf>,
}

impl From<&TrackMetadata> for CoverSnapshot {
    fn from(t: &TrackMetadata) -> Self {
        Self {
            title: t.title.clone(),
            artist: t.artist.clone(),
            album: t.album.clone(),
            cover: t.cover.clone(),
            cover_hash: t.cover_hash,
            cover_folder: t.cover_folder.clone(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct CoverAnim {
    pub from: CoverSnapshot,
    pub to: CoverSnapshot,
    // -1 => slide left (next), +1 => slide right (prev)
    pub dir: i8,
    pub started_at: Instant,
    pub duration: Duration,
}

#[derive(Debug, Clone)]
pub struct PlaylistAlbumAnim {
    pub from_cover: Option<Vec<u8>>,
    pub from_hash: Option<u64>,
    pub from_folder: Option<PathBuf>,
    pub to_cover: Option<Vec<u8>>,
    pub to_hash: Option<u64>,
    pub to_folder: Option<PathBuf>,
    // -1 => slide left (next), +1 => slide right (prev)
    pub dir: i8,
    pub started_at: Instant,
    pub duration: Duration,
}

impl Default for TrackMetadata {
    fn default() -> Self {
        Self {
            title: "Unknown".to_string(),
            artist: "Unknown".to_string(),
            album: "Unknown".to_string(),
            duration: Duration::from_secs(0),
            cover: None,
            cover_hash: None,
            cover_folder: None,
            lyrics: None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct SpectrumData {
    pub bars: Vec<f32>,
    pub bars_left: Vec<f32>,
    pub bars_right: Vec<f32>,
}

impl Default for SpectrumData {
    fn default() -> Self {
        Self {
            bars: vec![0.0; 64],
            bars_left: vec![0.0; 64],
            bars_right: vec![0.0; 64],
        }
    }
}

/// cava 的平滑基准帧率，即其 `framerate_mod = 66 / framerate` 里的 66。
const CAVA_REFERENCE_HZ: f32 = 66.0;

/// cava 默认的 `noise_reduction`（`config.c` 取 77 后除以 100）。
const CAVA_NOISE_REDUCTION: f32 = 0.77;

/// cava 每帧给 `cava_fall` 的增量（`cavacore.c`: `p->cava_fall[n] += 0.028`）。
const CAVA_FALL_STEP: f32 = 0.028;

/// 包络推进所依据的帧率。实际帧间隔用 `dt` 换算到这个基准，因此掉帧时动画仍
/// 按时间走，不会随帧率变快变慢。
const SCOPE_REFERENCE_HZ: f32 = 60.0;

/// 视作已经到位的残差。指数逼近永远到不了端点，差到这一步就直接钳住。
///
/// 1/255 ≈ 一个 8 位色阶，也小于半个盲文子行在任何实际面板高度下的占比，
/// 因此这一步钳位在视觉上不可见。
const SCOPE_SETTLED_EPSILON: f32 = 1.0 / 255.0;

/// 示波器的幅度包络：波形样本在绘制前统一乘上它。
///
/// rodio 暂停时直接产出静音、不再拉取解码器，PCM 环因此停更，波形会僵在最后
/// 一帧。这里把 cava 的两级平滑（`cavacore.c` 的 `process [smoothing]`）搬过来
/// 接管两端 —— 频谱条走的就是这套，观感因此同源：
///
/// - **起振**（cava 的 `integral`）：`out = mem·nr/integral_mod + raw`。输入恒定
///   时这是个几何级数，等价于一阶滞后：每帧朝目标前进固定比例。
/// - **回落**（cava 的 `falloff`）：`out = peak·(1 − fall²·gravity_mod)`，`fall`
///   随时间线性增长 —— 自由落体，位移正比于时间平方。
///
/// 为什么回落不能用指数衰减：指数首帧就掉 35%、三帧只剩 27%，在几百毫秒的尺度
/// 上看起来就是瞬间归零，根本看不出动画。自由落体前几帧几乎不动
/// （1.0 → 0.96），跌落集中在后半段，这才是"回落"该有的样子。
///
/// 包络是**全局**的，不按列分开：所有盲文列同乘一个系数，于是归零发生在同一
/// 瞬间，全部列同时落到垂直居中位置。按列各自衰减会让它们先后到位。
///
/// 归零不代表不画：幅度为 0 时波形退化成居中那条直线，那条线就是波形本身，
/// 没有独立的中线元素。
#[derive(Debug, Default)]
pub struct ScopeGain {
    current: f32,
    /// 回落起点的幅度，对应 cava 的 `cava_peak`。
    peak: f32,
    /// 已下落的时长，对应 cava 逐帧累加的 `cava_fall`。累的是时间而不是帧数，
    /// 因此掉帧时落点不变。
    fallen: Duration,
    animating: bool,
}

impl ScopeGain {
    /// cava `integral` 的记忆保留系数 `noise_reduction / integral_mod`。
    fn integral_retention() -> f32 {
        let framerate_mod = CAVA_REFERENCE_HZ / SCOPE_REFERENCE_HZ;
        CAVA_NOISE_REDUCTION / framerate_mod.powf(0.1)
    }

    /// cava `falloff` 的 `gravity_mod`。
    fn gravity() -> f32 {
        let framerate_mod = CAVA_REFERENCE_HZ / SCOPE_REFERENCE_HZ;
        framerate_mod.powf(2.5) * 2.0 / CAVA_NOISE_REDUCTION
    }

    fn tick(&mut self, playing: bool, dt: Duration) {
        if playing {
            self.rise(dt);
        } else {
            self.fall(dt);
        }
    }

    /// 起振：cava 的 integral。几何级数的归一化形式即一阶滞后，残差每帧乘
    /// `retention`；按 `dt` 取幂，掉帧时到位时刻不变。
    fn rise(&mut self, dt: Duration) {
        // 重新起振即取消下落：下次下落从当时的幅度重新起算。
        self.peak = 0.0;
        self.fallen = Duration::ZERO;

        if self.current >= 1.0 {
            self.animating = false;
            return;
        }
        self.animating = true;

        let frames = dt.as_secs_f32() * SCOPE_REFERENCE_HZ;
        let remaining = (1.0 - self.current) * Self::integral_retention().powf(frames);
        self.current = 1.0 - remaining;

        if remaining <= SCOPE_SETTLED_EPSILON {
            self.current = 1.0;
            self.animating = false;
        }
    }

    /// 回落：cava 的 falloff，位移正比于已下落时长的平方。
    fn fall(&mut self, dt: Duration) {
        if self.current <= 0.0 {
            self.animating = false;
            return;
        }

        if self.peak <= 0.0 {
            // 下落的第一帧：锚定峰值，本帧仍按当前幅度画 —— cava 同样是先记
            // `cava_peak`、下一帧才开始扣。
            self.peak = self.current;
        }
        self.animating = true;

        // cava 每帧 `fall += FALL_STEP`，故 fall = 帧数 × step。这里用时间换算
        // 帧数，得到与帧率无关的同一条抛物线。
        self.fallen += dt;
        let fall = self.fallen.as_secs_f32() * SCOPE_REFERENCE_HZ * CAVA_FALL_STEP;
        self.current = self.peak * (1.0 - fall * fall * Self::gravity());

        if self.current < SCOPE_SETTLED_EPSILON {
            // 落平：波形收成居中的直线，动画到此结束。
            self.current = 0.0;
            self.peak = 0.0;
            self.fallen = Duration::ZERO;
            self.animating = false;
        }
    }

    /// 当前幅度系数，渲染时乘在样本上。
    pub fn value(&self) -> f32 {
        self.current
    }

    fn is_animating(&self) -> bool {
        self.animating
    }
}

#[derive(Debug)]
pub struct PlayerState {
    pub mode: PlayMode,
    pub playback: PlaybackState,
    pub position: Duration,
    pub volume: f32,
    pub repeat_mode: RepeatMode,
    pub liked: bool,
    /// 宿主正在后台加载跳转目标（进度条显示脉冲加载动画）
    pub seeking: bool,
    pub track: TrackMetadata,
}

impl Default for PlayerState {
    fn default() -> Self {
        Self {
            mode: PlayMode::Idle,
            playback: PlaybackState::Stopped,
            position: Duration::from_secs(0),
            volume: 0.0,
            repeat_mode: RepeatMode::Sequence,
            liked: false,
            seeking: false,
            track: TrackMetadata::default(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Overlay {
    None,
    Playlist,
    SettingsModal,
    BarSettingsModal,
    LocalAudioSettingsModal,
    AboutModal,
    AcoustIdModal,
    HelpModal,
    EqModal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalFolderKind {
    Plain,
    Album,
    MultiAlbum,
}

#[derive(Debug)]
pub struct AppState {
    pub config: Config,
    pub theme: Theme,
    pub language: Language,

    pub player: PlayerState,
    pub api_tracks: Vec<TrackMetadata>,
    pub playlist: Playlist,

    // Playlist overlay browsing list.
    // For MultiAlbum, this can differ from `playlist` (playback queue).
    pub playlist_view: Playlist,
    pub spectrum: SpectrumData,
    pub spectrum_bar_smoother: Ema,
    pub spectrum_render_grid: Vec<Vec<char>>,

    /// 宿主播放链路上的 PCM 抽头环；无宿主（独立全屏）时为 None。
    pub pcm_ring: Option<Arc<crate::tmplayer::audio::pcm_tap::PcmRing>>,
    /// 示波器的复用缓冲，渲染路径因此零分配。
    pub scope: crate::tmplayer::render::oscilloscope_renderer::ScopeScratch,
    /// 波形幅度包络，暂停/停止后驱动波形收回中线。
    pub scope_gain: ScopeGain,

    pub cover_cache: RefCell<CoverCache>,
    pub cover_dominant_rgb_cache: RefCell<HashMap<u64, (u8, u8, u8)>>,

    cover_render_tx: Sender<CoverRenderRequest>,
    cover_render_rx: Receiver<CoverRenderResult>,
    cover_render_inflight: RefCell<HashSet<CoverKey>>,
    remote_last_sent: Option<TrackKey>,

    pub overlay: Overlay,

    pub settings_selected: usize,
    pub bar_settings_selected: usize,
    pub local_audio_settings_selected: usize,
    pub help_keybind_selected: usize,
    pub vip_audio_unlocked: bool,

    pub eq: EqSettings,
    pub eq_selected: usize,

    pub acoustid_input: String,

    // Folder that backs the *current playback queue* (contains audio files).
    pub local_folder: Option<PathBuf>,

    pub local_folder_kind: LocalFolderKind,

    // For MultiAlbum: all album folders under `local_root_folder`.
    pub local_album_folders: Vec<PathBuf>,
    // Which album folder is currently being *viewed* in the playlist overlay.
    pub local_view_album_index: usize,
    pub local_view_album_folder: Option<PathBuf>,

    // Album cover shown in the playlist overlay's top area.
    pub local_view_album_cover: Option<Vec<u8>>,
    pub local_view_album_cover_hash: Option<u64>,
    pub ncm_cover_cache_dir: Option<PathBuf>,

    pub playlist_album_anim: Option<PlaylistAlbumAnim>,

    pub cover_anim: Option<CoverAnim>,
    pub pending_system_cover_anim: Option<(CoverSnapshot, i8, Instant)>,

    pub toast: Option<(String, Instant)>,

    // Ask host CNMPlayer to open its settings after exiting fullscreen.
    pub request_host_settings_open: bool,

    pub last_mouse_click: Option<(Instant, u16, u16)>,

    // playlist slide animation（time-based，与帧率解耦；与主页侧边栏共用时长/缓动）
    pub playlist_slide_x: i16,
    pub playlist_slide_target_x: i16,
    playlist_slide_from_x: i16,
    playlist_slide_started_at: Option<Instant>,

    pub last_frame: Instant,
}

#[derive(Debug)]
struct CoverRenderRequest {
    key: CoverKey,
    bytes: Vec<u8>,
    placeholder: char,
    persist_folder: Option<PathBuf>,
}

#[derive(Debug)]
struct CoverRenderResult {
    key: CoverKey,
    ascii: String,
}

fn fill_ascii(width: u16, height: u16, ch: char) -> String {
    let row = ch.to_string().repeat(width as usize);
    let mut s = String::new();
    for _ in 0..height {
        s.push_str(&row);
        s.push('\n');
    }
    s
}

impl AppState {
    pub fn new(config: Config, theme: Theme, language: Language) -> Self {
        let (cover_render_tx, cover_render_req_rx) = mpsc::channel::<CoverRenderRequest>();
        let (cover_render_res_tx, cover_render_rx) = mpsc::channel::<CoverRenderResult>();

        std::thread::spawn(move || {
            while let Ok(req) = cover_render_req_rx.recv() {
                let ascii = render_cover_ascii(&req.bytes, req.key.width, req.key.height)
                    .unwrap_or_else(|| fill_ascii(req.key.width, req.key.height, req.placeholder));

                if let Some(folder) = req.persist_folder.as_deref() {
                    let _ = crate::tmplayer::playback::local_player::write_cover_ascii_cache(
                        folder,
                        req.key.hash,
                        req.key.width,
                        req.key.height,
                        &ascii,
                    );
                }
                let _ = cover_render_res_tx.send(CoverRenderResult {
                    key: req.key,
                    ascii,
                });
            }
        });

        Self {
            config,
            theme,
            language,
            player: PlayerState::default(),
            api_tracks: Vec::new(),
            playlist: Playlist::default(),
            playlist_view: Playlist::default(),
            spectrum: SpectrumData::default(),
            spectrum_bar_smoother: Ema::new(0.35, 64),
            spectrum_render_grid: Vec::new(),
            pcm_ring: None,
            scope: Default::default(),
            scope_gain: ScopeGain::default(),
            cover_cache: RefCell::new(CoverCache::new(20)),
            cover_dominant_rgb_cache: RefCell::new(HashMap::new()),
            cover_render_tx,
            cover_render_rx,
            cover_render_inflight: RefCell::new(HashSet::new()),
            remote_last_sent: None,
            overlay: Overlay::None,
            settings_selected: 0,
            bar_settings_selected: 0,
            local_audio_settings_selected: 0,
            help_keybind_selected: 0,
            vip_audio_unlocked: false,

            eq: EqSettings::default(),
            eq_selected: 0,

            acoustid_input: String::new(),

            local_folder: None,
            local_folder_kind: LocalFolderKind::Plain,
            local_album_folders: Vec::new(),
            local_view_album_index: 0,
            local_view_album_folder: None,
            local_view_album_cover: None,
            local_view_album_cover_hash: None,
            ncm_cover_cache_dir: None,

            playlist_album_anim: None,

            cover_anim: None,
            pending_system_cover_anim: None,
            toast: None,
            request_host_settings_open: false,
            last_mouse_click: None,
            playlist_slide_x: 0,
            playlist_slide_target_x: 0,
            playlist_slide_from_x: 0,
            playlist_slide_started_at: None,
            last_frame: Instant::now(),
        }
    }

    pub fn reset_remote_fetch_state(&mut self) {
        self.remote_last_sent = None;
    }

    pub fn cover_dominant_rgb(&self, hash: u64, bytes: &[u8]) -> Option<(u8, u8, u8)> {
        if let Some(rgb) = self.cover_dominant_rgb_cache.borrow().get(&hash).copied() {
            return Some(rgb);
        }
        let rgb = crate::tmplayer::render::dominant_color::dominant_rgb_from_image_bytes(bytes)?;
        self.cover_dominant_rgb_cache.borrow_mut().insert(hash, rgb);
        Some(rgb)
    }

    pub fn set_toast(&mut self, msg: impl Into<String>) {
        self.toast = Some((msg.into(), Instant::now()));
    }

    pub fn queue_cover_ascii_render(
        &self,
        key: CoverKey,
        bytes: &[u8],
        placeholder: char,
        persist_folder: Option<PathBuf>,
    ) {
        if self.cover_cache.borrow().contains(key) {
            return;
        }
        if self.cover_render_inflight.borrow().contains(&key) {
            return;
        }
        self.cover_render_inflight.borrow_mut().insert(key);
        let _ = self.cover_render_tx.send(CoverRenderRequest {
            key,
            bytes: bytes.to_vec(),
            placeholder,
            persist_folder,
        });
    }

    pub fn tick(&mut self, now: Instant) {
        // 必须在覆盖 last_frame 之前取，否则帧间隔恒为 0。
        let dt = now.saturating_duration_since(self.last_frame);
        self.last_frame = now;

        if !self.cover_render_inflight.borrow().is_empty() {
            loop {
                match self.cover_render_rx.try_recv() {
                    Ok(msg) => {
                        self.cover_render_inflight.borrow_mut().remove(&msg.key);
                        self.cover_cache.borrow_mut().put(msg.key, msg.ascii);
                    }
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => break,
                }
            }
        }

        if let Some(anim) = &self.cover_anim {
            if now.duration_since(anim.started_at) >= anim.duration {
                self.cover_anim = None;
            }
        }

        if let Some(anim) = &self.playlist_album_anim {
            if now.duration_since(anim.started_at) >= anim.duration {
                self.playlist_album_anim = None;
            }
        }

        if let Some((_, _, at)) = &self.pending_system_cover_anim {
            if now.duration_since(*at) > Duration::from_secs(2) {
                self.pending_system_cover_anim = None;
            }
        }

        if let Some((_, at)) = &self.toast {
            if now.duration_since(*at) > Duration::from_millis(1500) {
                self.toast = None;
            }
        }

        self.tick_playlist_slide(now);
        self.scope_gain
            .tick(self.player.playback == PlaybackState::Playing, dt);
    }

    /// 启动一次侧边栏滑入/滑出。记录当前位置作为起点，因此支持动画中途反向。
    pub fn start_playlist_slide(&mut self, target_x: i16) {
        if self.playlist_slide_x == target_x && self.playlist_slide_target_x == target_x {
            return;
        }
        self.playlist_slide_from_x = self.playlist_slide_x;
        self.playlist_slide_target_x = target_x;
        // 起始时刻留给下一次 tick 填，避免这里再取一次 Instant::now()。
        self.playlist_slide_started_at = None;
    }

    fn tick_playlist_slide(&mut self, now: Instant) {
        if self.playlist_slide_x == self.playlist_slide_target_x {
            self.playlist_slide_started_at = None;
            return;
        }

        let started_at = *self.playlist_slide_started_at.get_or_insert(now);
        let elapsed = now.saturating_duration_since(started_at);
        let t = if elapsed >= SIDEBAR_ANIM_DURATION {
            1.0
        } else {
            elapsed.as_secs_f32() / SIDEBAR_ANIM_DURATION.as_secs_f32()
        };

        let eased = cubic_bezier_y(t, 0.0, 0.7);
        let from = f32::from(self.playlist_slide_from_x);
        let target = f32::from(self.playlist_slide_target_x);
        self.playlist_slide_x = (from + (target - from) * eased).round() as i16;

        if t >= 1.0 {
            self.playlist_slide_x = self.playlist_slide_target_x;
            self.playlist_slide_started_at = None;
        }
    }

    pub fn should_continuous_redraw(&self) -> bool {
        if self.player.playback == PlaybackState::Playing {
            return true;
        }

        // 后台加载跳转目标时保持重绘，驱动进度条脉冲动画
        if self.player.seeking {
            return true;
        }

        if self.player.playback == PlaybackState::Paused && self.has_spectrum_tail_motion() {
            return true;
        }

        if self.scope_is_animating() {
            return true;
        }

        if self.cover_anim.is_some()
            || self.playlist_album_anim.is_some()
            || self.pending_system_cover_anim.is_some()
        {
            return true;
        }

        if self.toast.is_some() {
            return true;
        }

        if self.playlist_slide_x != self.playlist_slide_target_x {
            return true;
        }

        false
    }

    pub fn active_render_fps(&self) -> u32 {
        use crate::tmplayer::data::config::VisualizeMode;

        let base = self.config.ui_fps.clamp(10, 60);
        // 频谱靠 cava 的拖尾衰减，示波器靠自己的收尾动画：暂停后两者都还在动。
        let visual_active = match self.config.visualize {
            VisualizeMode::Off => false,
            VisualizeMode::Bars => {
                self.player.playback == PlaybackState::Playing
                    || (self.player.playback == PlaybackState::Paused
                        && self.has_spectrum_tail_motion())
            }
            VisualizeMode::Oscilloscope => {
                self.player.playback == PlaybackState::Playing || self.scope_gain.is_animating()
            }
        };

        if visual_active {
            return self.config.spectrum_hz.clamp(base, 60);
        }
        base
    }

    pub fn idle_render_fps(&self) -> u32 {
        self.config.ui_fps.clamp(4, 12)
    }

    fn has_spectrum_tail_motion(&self) -> bool {
        const TAIL_EPS: f32 = 0.003;
        self.spectrum.bars.iter().any(|&v| v > TAIL_EPS)
            || self.spectrum.bars_left.iter().any(|&v| v > TAIL_EPS)
            || self.spectrum.bars_right.iter().any(|&v| v > TAIL_EPS)
    }

    /// 示波器的包络动画（起振或回落）正在进行，需要持续重绘把它推完。
    fn scope_is_animating(&self) -> bool {
        matches!(
            self.config.visualize,
            crate::tmplayer::data::config::VisualizeMode::Oscilloscope
        ) && self.scope_gain.is_animating()
    }

    pub fn start_cover_anim(
        &mut self,
        from: CoverSnapshot,
        to: CoverSnapshot,
        dir: i8,
        now: Instant,
    ) {
        self.cover_anim = Some(CoverAnim {
            from,
            to,
            dir,
            started_at: now,
            duration: Duration::from_millis(220),
        });
    }

    pub fn close_overlay(&mut self) {
        self.overlay = Overlay::None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FRAME: Duration = Duration::from_millis(1000 / 60);

    /// 一帧一帧跑 cava `cavacore.c` 的两级平滑，作为对照实现。
    ///
    /// `raw` 恒定输入；返回归一化到稳态的逐帧输出。cava 的 integral 是个不收敛到
    /// 1 的几何级数（稳态 `1/(1-k)`），归一化后才能和包络比。
    fn cava_reference(raw: f32, frames: usize) -> Vec<f32> {
        let framerate_mod = CAVA_REFERENCE_HZ / SCOPE_REFERENCE_HZ;
        let gravity_mod = framerate_mod.powf(2.5) * 2.0 / CAVA_NOISE_REDUCTION;
        let integral_mod = framerate_mod.powf(0.1);
        let k = CAVA_NOISE_REDUCTION / integral_mod;

        let (mut mem, mut prev, mut peak, mut fall) = (0.0f32, 0.0f32, 0.0f32, 0.0f32);
        let mut out_series = Vec::with_capacity(frames);
        for _ in 0..frames {
            let mut out = raw;
            if out < prev && CAVA_NOISE_REDUCTION > 0.1 {
                out = (peak * (1.0 - fall * fall * gravity_mod)).max(0.0);
                fall += CAVA_FALL_STEP;
            } else {
                peak = out;
                fall = 0.0;
            }
            prev = out;
            out = mem * k + out;
            mem = out;
            out_series.push(out * (1.0 - k) / raw);
        }
        out_series
    }

    /// 起振逐帧对齐 cava 的 integral（归一化后）。
    #[test]
    fn rise_matches_cava_integral() {
        let reference = cava_reference(1.0, 12);
        let mut gain = ScopeGain::default();

        for (frame, expected) in reference.iter().enumerate() {
            gain.tick(true, Duration::from_secs_f32(1.0 / SCOPE_REFERENCE_HZ));
            assert!(
                (gain.value() - expected).abs() < 1.0e-3,
                "第 {frame} 帧不一致: scope={} cava={expected}",
                gain.value()
            );
        }
    }

    /// 起振是一阶滞后：首帧就走掉可观一段，随后逐帧放缓。
    #[test]
    fn rise_is_front_loaded_and_decelerates() {
        let mut gain = ScopeGain::default();
        assert_eq!(gain.value(), 0.0, "静止态包络为 0，波形是居中直线");

        let mut steps = Vec::new();
        let mut prev = 0.0;
        for _ in 0..8 {
            gain.tick(true, FRAME);
            steps.push(gain.value() - prev);
            prev = gain.value();
        }

        assert!(steps[0] > 0.2, "首帧应明显张开: {}", steps[0]);
        assert!(
            steps.windows(2).all(|pair| pair[1] < pair[0]),
            "步长应逐帧变小: {steps:?}"
        );
    }

    /// 回落是自由落体：**前段几乎不动**，跌落集中在后半段。
    ///
    /// 这条是"看得见回落动画"的判据。换成指数衰减会立刻挂：指数首帧就掉 35%。
    #[test]
    fn fall_is_gravity_shaped_not_exponential() {
        let mut gain = ScopeGain::default();
        gain.tick(true, Duration::from_secs(1));
        assert_eq!(gain.value(), 1.0);

        let mut values = Vec::new();
        loop {
            gain.tick(false, FRAME);
            values.push(gain.value());
            if gain.value() == 0.0 || values.len() > 120 {
                break;
            }
        }

        // 起手极慢：第 3 帧还留着九成以上。指数衰减此时只剩 0.27。
        assert!(values[2] > 0.9, "起手应几乎不动: {}", values[2]);
        // 过半时间才掉一半左右，而不是早早贴底。
        let half = values[values.len() / 2];
        assert!(half > 0.4 && half < 0.85, "中点幅度 {half} 不像自由落体");
        // 单调下落，且确实落到零。
        assert!(
            values.windows(2).all(|pair| pair[1] <= pair[0]),
            "幅度不得回升: {values:?}"
        );
        assert_eq!(*values.last().unwrap(), 0.0);
    }

    /// 回落逐帧对齐 cava 的 falloff（同一条抛物线）。
    #[test]
    fn fall_matches_cava_falloff() {
        let one_frame = Duration::from_secs_f32(1.0 / SCOPE_REFERENCE_HZ);
        let framerate_mod = CAVA_REFERENCE_HZ / SCOPE_REFERENCE_HZ;
        let gravity_mod = framerate_mod.powf(2.5) * 2.0 / CAVA_NOISE_REDUCTION;

        let mut gain = ScopeGain::default();
        gain.tick(true, Duration::from_secs(1));

        // cava 侧：peak 已锚定为 1.0，fall 从 0 起累加。
        let mut fall = 0.0f32;
        for frame in 0..25 {
            gain.tick(false, one_frame);
            fall += CAVA_FALL_STEP;
            let expected = (1.0 - fall * fall * gravity_mod).max(0.0);
            if expected < SCOPE_SETTLED_EPSILON {
                assert_eq!(gain.value(), 0.0, "第 {frame} 帧应已落平");
                break;
            }
            assert!(
                (gain.value() - expected).abs() < 1.0e-3,
                "第 {frame} 帧不一致: scope={} cava={expected}",
                gain.value()
            );
        }
    }

    #[test]
    fn fall_returns_to_flat_and_stops_requesting_redraw() {
        let mut gain = ScopeGain::default();
        gain.tick(true, Duration::from_secs(1));

        // 停止：全程标记为动画中——否则重绘会被停掉，回落只推进一帧就卡住。
        for frame in 0..40 {
            gain.tick(false, FRAME);
            if gain.value() == 0.0 {
                break;
            }
            assert!(gain.is_animating(), "第 {frame} 帧应仍在动画中");
        }

        assert_eq!(gain.value(), 0.0, "应已落平");
        assert!(!gain.is_animating());

        // 落平后继续 tick 不该再请求重绘。
        gain.tick(false, FRAME);
        assert!(!gain.is_animating());
        assert_eq!(gain.value(), 0.0);
    }

    #[test]
    fn resuming_mid_fall_ramps_up_from_the_residual_value() {
        let mut gain = ScopeGain::default();
        gain.tick(true, Duration::from_secs(1));

        for _ in 0..8 {
            gain.tick(false, FRAME);
        }
        let mid = gain.value();
        assert!(mid > 0.0 && mid < 1.0, "应处于回落途中: {mid}");

        // 恢复播放从残值继续张开，不跳变、也不从 0 重来。
        gain.tick(true, FRAME);
        assert!(
            gain.value() > mid,
            "应从残值继续增长: {} vs {mid}",
            gain.value()
        );

        // 再次停止：抛物线从这一刻重新起算，而不是接着上一轮的 fallen。
        gain.tick(false, FRAME);
        assert!(
            gain.value() > mid,
            "重新起算应高于上一轮同期: {} vs {mid}",
            gain.value()
        );
    }

    /// 两端动画都必须与帧率无关：掉帧时按时间补足。
    #[test]
    fn envelope_is_frame_rate_independent() {
        let elapsed = Duration::from_millis(150);

        for playing in [true, false] {
            let mut fast = ScopeGain::default();
            let mut slow = ScopeGain::default();
            if !playing {
                fast.tick(true, Duration::from_secs(1));
                slow.tick(true, Duration::from_secs(1));
            }

            let step = elapsed / 20;
            for _ in 0..20 {
                fast.tick(playing, step);
            }
            slow.tick(playing, elapsed);

            assert!(
                (fast.value() - slow.value()).abs() < 1.0e-3,
                "playing={playing} fast={} slow={}",
                fast.value(),
                slow.value()
            );
        }
    }
}
