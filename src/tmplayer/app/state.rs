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

/// 暂停或停止后，波形缓动收回中线的时长。
const SCOPE_SETTLE_DURATION: Duration = Duration::from_millis(420);

/// 示波器的幅度包络。
///
/// rodio 暂停时直接产出静音、不再拉取解码器，PCM 环因此停更，波形会僵在最后
/// 一帧。这里给它一条收尾动画：播放中恒为 1，暂停或停止后缓动到 0，波形平滑
/// 收拢到中线。恢复播放立即回满——环里随即就有新样本，渐入只会显得迟钝。
#[derive(Debug, Default)]
pub struct ScopeGain {
    current: f32,
    settle_started_at: Option<Instant>,
}

impl ScopeGain {
    fn tick(&mut self, playing: bool, now: Instant) {
        if playing {
            self.current = 1.0;
            self.settle_started_at = None;
            return;
        }

        match self.settle_started_at {
            // 归零那一帧已经画出去了，现在才停掉重绘。若和归零放在同一帧，
            // 重绘会先一步停掉，屏幕就停在收尾前的最后一丝残影上。
            Some(_) if self.current <= 0.0 => self.settle_started_at = None,
            Some(started_at) => {
                let elapsed = now.saturating_duration_since(started_at);
                let t = elapsed.as_secs_f32() / SCOPE_SETTLE_DURATION.as_secs_f32();
                self.current = if t >= 1.0 {
                    0.0
                } else {
                    // 收尾总是从满幅起步：恢复播放是瞬时的，没有停在半幅的中间态。
                    1.0 - cubic_bezier_y(t, 0.0, 0.7)
                };
            }
            // 刚从播放切过来：锚定起点，本帧仍按满幅画。
            None if self.current > 0.0 => self.settle_started_at = Some(now),
            None => {}
        }
    }

    /// 当前幅度系数，渲染时乘在样本上。
    pub fn value(&self) -> f32 {
        self.current
    }

    fn is_settling(&self) -> bool {
        self.settle_started_at.is_some()
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
            .tick(self.player.playback == PlaybackState::Playing, now);
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

        if self.scope_is_settling() {
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
                self.player.playback == PlaybackState::Playing || self.scope_gain.is_settling()
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

    /// 示波器正在把波形收回中线，需要持续重绘把这段动画推完。
    fn scope_is_settling(&self) -> bool {
        matches!(
            self.config.visualize,
            crate::tmplayer::data::config::VisualizeMode::Oscilloscope
        ) && self.scope_gain.is_settling()
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

    #[test]
    fn scope_gain_settles_to_zero_and_snaps_back_on_resume() {
        let mut gain = ScopeGain::default();
        let t0 = Instant::now();

        gain.tick(true, t0);
        assert_eq!(gain.value(), 1.0);
        assert!(!gain.is_settling());

        // 暂停后的第一帧只锚定起点，幅度仍是满的；从下一帧起才开始收。
        gain.tick(false, t0);
        assert_eq!(gain.value(), 1.0);
        assert!(gain.is_settling());

        // 之后必须单调收敛，且全程标记为动画中——否则重绘会被停掉，
        // 收尾动画只推进一帧就卡住。
        let mut prev = 1.0;
        for ms in [1u64, 100, 200, 300, 419] {
            gain.tick(false, t0 + Duration::from_millis(ms));
            assert!(gain.value() < prev, "gain must shrink by {ms}ms");
            assert!(gain.is_settling(), "must stay animating at {ms}ms");
            prev = gain.value();
        }

        gain.tick(false, t0 + SCOPE_SETTLE_DURATION);
        assert_eq!(gain.value(), 0.0);
        // 归零这一帧本身还得再画一次，所以此刻仍要求重绘；否则屏幕会停在
        // 收尾前的最后一丝残影上，直到别处偶然触发重绘才补上。
        assert!(gain.is_settling());

        gain.tick(
            false,
            t0 + SCOPE_SETTLE_DURATION + Duration::from_millis(16),
        );
        assert!(!gain.is_settling());

        // 之后继续 tick 不该再请求重绘。
        gain.tick(false, t0 + Duration::from_secs(5));
        assert!(!gain.is_settling());

        gain.tick(true, t0 + Duration::from_secs(6));
        assert_eq!(gain.value(), 1.0);
    }

    #[test]
    fn resuming_mid_settle_restarts_a_full_length_settle() {
        let mut gain = ScopeGain::default();
        let t0 = Instant::now();

        gain.tick(true, t0);
        gain.tick(false, t0 + Duration::from_millis(300));
        gain.tick(false, t0 + Duration::from_millis(400));
        assert!(gain.value() < 1.0);

        gain.tick(true, t0 + Duration::from_millis(410));
        assert_eq!(gain.value(), 1.0);

        // 再次暂停：计时从这一刻重新起算。若沿用上一轮起点，下面这帧的
        // elapsed 会超过整个时长，幅度提前归零。
        let pause_at = t0 + Duration::from_millis(420);
        gain.tick(false, pause_at);
        gain.tick(
            false,
            pause_at + SCOPE_SETTLE_DURATION - Duration::from_millis(1),
        );
        assert!(
            gain.value() > 0.0,
            "settle must restart from the resume point"
        );

        gain.tick(false, pause_at + SCOPE_SETTLE_DURATION);
        assert_eq!(gain.value(), 0.0);
    }
}
