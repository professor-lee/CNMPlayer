use crate::app::SIDEBAR_ANIM_DURATION;
use crate::data::config::Language;
use crate::data::config::{Config, VisualizeMode};
use crate::render::motion::{Curve, Transition};
use crate::tmplayer::app::scope::ScopeGain;
use crate::tmplayer::audio::spectrum::Spectrum;
use crate::tmplayer::data::playlist::Playlist;
use crate::tmplayer::render::cover_cache::CoverCache;
use crate::tmplayer::render::cover_cache::CoverKey;
use crate::tmplayer::render::cover_renderer::render_cover_ascii;
use crate::ui::theme::Theme;
use std::cell::RefCell;
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc;
use std::sync::mpsc::{Receiver, Sender, TryRecvError};
use std::time::{Duration, Instant};

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
    pub lyrics: Option<Vec<LyricLine>>,
}

#[derive(Debug, Clone)]
pub struct CoverSnapshot {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub cover: Option<Vec<u8>>,
    pub cover_hash: Option<u64>,
}

impl From<&TrackMetadata> for CoverSnapshot {
    fn from(t: &TrackMetadata) -> Self {
        Self {
            title: t.title.clone(),
            artist: t.artist.clone(),
            album: t.album.clone(),
            cover: t.cover.clone(),
            cover_hash: t.cover_hash,
        }
    }
}

#[derive(Debug, Clone)]
pub struct CoverAnim {
    pub from: CoverSnapshot,
    pub to: CoverSnapshot,
    pub dir: i8,
    pub motion: Transition,
}
impl CoverAnim {
    pub fn slide_offsets(&self, width: u16, now: Instant) -> (i16, i16) {
        let width = width.min(i16::MAX as u16) as i16;
        let offset = (self.motion.sample(now) * f32::from(width)).round() as i16;
        if self.dir < 0 {
            (-offset, width - offset)
        } else {
            (offset, -width + offset)
        }
    }
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

#[derive(Debug)]
pub struct PlayerState {
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
    LyricsSettingsModal,
    /// 「下载设置」页：音质 / 路径 / 恢复默认。
    DownloadSettingsModal,
    /// 下载路径的行内编辑（独立 overlay，字符按键因此直接进输入框）。
    DownloadPathEditModal,
    AboutModal,
    HelpModal,
    EqModal,
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
    pub playlist_view: Playlist,
    pub spectrum: SpectrumData,
    pub spectrum_engine: Spectrum,

    /// 宿主播放链路上的 PCM 抽头环；进入全屏事件循环时绑定。
    pub pcm_ring: Option<Arc<crate::tmplayer::audio::pcm_tap::PcmRing>>,
    /// 示波器的复用缓冲，渲染路径因此零分配。
    pub scope: crate::tmplayer::render::oscilloscope_renderer::ScopeScratch,
    /// 波形幅度包络，暂停/停止后驱动波形收回中线。
    pub scope_gain: ScopeGain,
    /// 矢量模式的可变状态（李萨如光栅 / 自动缩放 / 打断动画）。
    pub vector: crate::tmplayer::render::vector_renderer::VectorState,

    pub cover_cache: RefCell<CoverCache>,

    cover_render_tx: Sender<CoverRenderRequest>,
    cover_render_rx: Receiver<CoverRenderResult>,
    cover_render_inflight: RefCell<HashSet<CoverKey>>,

    pub overlay: Overlay,

    pub settings_selected: usize,
    pub bar_settings_selected: usize,
    pub lyrics_settings_selected: usize,
    pub help_keybind_selected: usize,
    /// 按键提示弹窗的滚动偏移：与主应用/应用内列表一致，仅当焦点行越过
    /// 可视窗口边界时才挪动。
    pub help_keybind_scroll: usize,
    pub vip_audio_unlocked: bool,

    /// 信息区下载图标状态（宿主每帧同步）。
    pub download_state: crate::tmplayer::DownloadIconState,
    /// 下载图标旋转帧的相位基准（time-based）。
    pub download_phase_start: Instant,
    /// 「下载设置」页的选中行 / 待确认态 / 路径行编辑状态。
    pub download_settings_selected: usize,
    pub download_reset_armed: bool,
    pub download_path_edit: Option<crate::app::DownloadPathEdit>,
    /// 解析后的下载目录（`None` = 不可用，设置页除路径行外全部灰置）。
    pub download_root: Option<PathBuf>,

    pub eq: EqSettings,
    pub eq_selected: usize,

    // Host-provided playlist cover shown in the playlist overlay.
    pub playlist_cover: Option<Vec<u8>>,
    pub playlist_cover_hash: Option<u64>,

    pub cover_anim: Option<CoverAnim>,
    pub pending_system_cover_anim: Option<(CoverSnapshot, i8, Instant)>,

    pub toast: Option<(String, Instant)>,

    // Ask host CNMPlayer to open its settings after exiting fullscreen.
    pub request_host_settings_open: bool,
    /// 全屏页里点了作者名/专辑名这类"交给宿主接着做"的请求：
    /// `Some` 即请求退出，落点由 `FullscreenExit` 说明（`Tui::draw` 据此置 `should_quit`）。
    pub exit_request: Option<crate::tmplayer::FullscreenExit>,

    pub last_mouse_click: Option<(Instant, u16, u16)>,
    /// 播放列表上一次点击的条目序号（双击切歌判定）。
    pub last_playlist_click: Option<(Instant, usize)>,
    /// 播放列表上一帧的虚拟滚动窗口（起始条目 + 可见行数），命中区据此换算。
    pub playlist_list_scroll: usize,
    pub playlist_list_rows: usize,
    /// 正在按住拖动全屏页音量条。
    pub volume_drag: bool,

    // Legacy coordinates remain for hit-test/layout compatibility; motion owns the curve.
    pub playlist_slide_x: i16,
    pub playlist_slide_target_x: i16,
    playlist_slide_from_x: i16,
    playlist_slide_motion: Transition,

    pub last_frame: Instant,
}

#[derive(Debug)]
struct CoverRenderRequest {
    key: CoverKey,
    bytes: Vec<u8>,
    placeholder: char,
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
            spectrum_engine: Spectrum::new(64),
            pcm_ring: None,
            scope: Default::default(),
            scope_gain: ScopeGain::default(),
            vector: Default::default(),
            cover_cache: RefCell::new(CoverCache::new(20)),
            cover_render_tx,
            cover_render_rx,
            cover_render_inflight: RefCell::new(HashSet::new()),
            overlay: Overlay::None,
            settings_selected: 0,
            bar_settings_selected: 0,
            lyrics_settings_selected: 0,
            help_keybind_selected: 0,
            help_keybind_scroll: 0,
            vip_audio_unlocked: false,
            download_state: crate::tmplayer::DownloadIconState::Hidden,
            download_phase_start: Instant::now(),
            download_settings_selected: 0,
            download_reset_armed: false,
            download_path_edit: None,
            download_root: None,

            eq: EqSettings::default(),
            eq_selected: 0,
            playlist_cover: None,
            playlist_cover_hash: None,

            cover_anim: None,
            pending_system_cover_anim: None,
            toast: None,
            request_host_settings_open: false,
            exit_request: None,
            last_mouse_click: None,
            last_playlist_click: None,
            playlist_list_scroll: 0,
            playlist_list_rows: 0,
            volume_drag: false,
            playlist_slide_x: 0,
            playlist_slide_target_x: 0,
            playlist_slide_from_x: 0,
            playlist_slide_motion: Transition::new(0.0),
            last_frame: Instant::now(),
        }
    }

    pub fn set_toast(&mut self, msg: impl Into<String>) {
        self.toast = Some((msg.into(), Instant::now()));
    }

    /// 下载图标旋转帧的相位（time-based）。
    pub fn download_phase(&self) -> Duration {
        self.download_phase_start.elapsed()
    }

    /// 下载是否整体可用（宿主解析出的下载目录存在）。
    pub fn download_enabled(&self) -> bool {
        self.download_root.is_some()
    }

    /// 设置弹窗里显示的下载路径（`Null` = 不可用）。
    pub fn download_display_path(&self) -> String {
        self.download_root
            .as_ref()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| crate::app::download::DOWNLOAD_PATH_NULL.to_string())
    }

    /// 重新解析下载根目录（宿主同步回来、或本页改了路径后调用）。
    pub fn refresh_download_root(&mut self) {
        self.download_root =
            crate::app::download::resolve_download_root(self.config.download_path.as_deref());
    }

    pub fn queue_cover_ascii_render(&self, key: CoverKey, bytes: &[u8], placeholder: char) {
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

        if let Some(anim) = &mut self.cover_anim {
            anim.motion.tick(now);
            if !anim.motion.is_running() {
                self.cover_anim = None;
            }
        }

        if let Some((_, _, at)) = &self.pending_system_cover_anim
            && now.duration_since(*at) > Duration::from_secs(2)
        {
            self.pending_system_cover_anim = None;
        }

        if let Some((_, at)) = &self.toast
            && now.duration_since(*at) > Duration::from_millis(1500)
        {
            self.toast = None;
        }

        self.tick_playlist_slide(now);
        if self.config.visualize == VisualizeMode::Oscilloscope {
            let activity = self.pcm_ring.as_ref().map(|ring| ring.activity());
            self.scope_gain.tick(
                self.player.playback == PlaybackState::Playing,
                activity,
                now,
            );
        } else {
            self.scope_gain.reset();
        }

        self.vector.tick(
            self.config.visualize == VisualizeMode::Vector,
            self.player.playback == PlaybackState::Playing,
            dt,
        );
    }

    /// 启动一次侧边栏滑入/滑出。记录当前位置作为起点，因此支持动画中途反向。
    pub fn start_playlist_slide(&mut self, target_x: i16) {
        if self.playlist_slide_x == target_x && self.playlist_slide_target_x == target_x {
            return;
        }
        self.playlist_slide_from_x = self.playlist_slide_x;
        self.playlist_slide_target_x = target_x;
        let mut motion = Transition::new(f32::from(self.playlist_slide_x));
        motion.retarget(
            f32::from(target_x),
            Instant::now(),
            SIDEBAR_ANIM_DURATION,
            Curve::EaseOut,
        );
        self.playlist_slide_motion = motion;
    }

    fn tick_playlist_slide(&mut self, now: Instant) {
        if self.playlist_slide_x == self.playlist_slide_target_x
            && !self.playlist_slide_motion.is_running()
        {
            return;
        }
        if !self.playlist_slide_motion.is_running() {
            self.playlist_slide_motion.retarget(
                f32::from(self.playlist_slide_target_x),
                now,
                SIDEBAR_ANIM_DURATION,
                Curve::EaseOut,
            );
        }
        self.playlist_slide_motion.tick(now);
        self.playlist_slide_x = self.playlist_slide_motion.sample(now).round() as i16;
        if !self.playlist_slide_motion.is_running() {
            self.playlist_slide_x = self.playlist_slide_target_x;
        }
    }

    pub fn should_continuous_redraw(&self) -> bool {
        if self.player.playback == PlaybackState::Playing
            && !matches!(
                self.config.visualize,
                VisualizeMode::Hidden | VisualizeMode::Lyrics
            )
        {
            return true;
        }

        // 后台加载跳转目标时保持重绘，驱动进度条脉冲动画
        if self.player.seeking {
            return true;
        }

        if self.config.visualize == VisualizeMode::Bars && self.spectrum_engine.has_tail() {
            return true;
        }

        if self.scope_is_animating() {
            return true;
        }

        if self.vector_is_animating() || self.vector_is_floating() {
            return true;
        }

        if self.cover_anim.is_some() || self.pending_system_cover_anim.is_some() {
            return true;
        }

        if self.toast.is_some() {
            return true;
        }

        // 下载中：图标要一直转（time-based 帧）。
        if self.download_state == crate::tmplayer::DownloadIconState::Downloading {
            return true;
        }

        if self.playlist_slide_x != self.playlist_slide_target_x
            || self.playlist_slide_motion.is_running()
        {
            return true;
        }

        false
    }

    pub fn render_fps(&self) -> u32 {
        self.config.ui_fps
    }

    /// 示波器的包络动画（起振或回落）正在进行，需要持续重绘把它推完。
    fn scope_is_animating(&self) -> bool {
        matches!(self.config.visualize, VisualizeMode::Oscilloscope)
            && self.scope_gain.is_animating()
    }

    /// 矢量模式的快动画（分散 / 回位）进行中，需要持续重绘把它推完。
    fn vector_is_animating(&self) -> bool {
        self.config.visualize == VisualizeMode::Vector && self.vector.is_animating()
    }

    /// 矢量模式停稳后的尘埃按 Astra Sparkle 持续明灭，暂停下也要维持基础帧率重绘。
    fn vector_is_floating(&self) -> bool {
        self.config.visualize == VisualizeMode::Vector && self.vector.is_floating()
    }

    pub fn start_cover_anim(
        &mut self,
        from: CoverSnapshot,
        to: CoverSnapshot,
        dir: i8,
        now: Instant,
    ) {
        let mut motion = Transition::new(0.0);
        motion.retarget(1.0, now, Duration::from_millis(220), Curve::EaseInOut);
        self.cover_anim = Some(CoverAnim {
            from,
            to,
            dir,
            motion,
        });
    }

    pub fn toggle_help_modal(&mut self) {
        if self.overlay == Overlay::HelpModal {
            self.close_overlay();
        } else {
            self.help_keybind_selected = self
                .help_keybind_selected
                .min(crate::tmplayer::ui::tui::help_item_count(self).saturating_sub(1));
            self.help_keybind_scroll = 0;
            self.overlay = Overlay::HelpModal;
        }
    }

    pub fn close_overlay(&mut self) {
        self.overlay = Overlay::None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_close_releases_overlay_immediately() {
        let mut app = AppState::new(
            Config::default(),
            crate::ui::theme::Theme::default(),
            Language::En,
        );
        for overlay in [
            Overlay::SettingsModal,
            Overlay::BarSettingsModal,
            Overlay::LyricsSettingsModal,
            Overlay::DownloadSettingsModal,
            Overlay::DownloadPathEditModal,
            Overlay::HelpModal,
            Overlay::AboutModal,
            Overlay::EqModal,
        ] {
            app.overlay = overlay;
            app.close_overlay();
            assert_eq!(
                app.overlay,
                Overlay::None,
                "{overlay:?} must close without a tick"
            );
        }
    }

    #[test]
    fn help_toggle_closes_the_whole_modal_instead_of_opening_settings() {
        let mut app = AppState::new(
            Config::default(),
            crate::ui::theme::Theme::default(),
            Language::En,
        );
        for parent in [Overlay::None, Overlay::SettingsModal] {
            app.overlay = parent;
            app.toggle_help_modal();
            assert_eq!(app.overlay, Overlay::HelpModal);

            app.toggle_help_modal();
            assert_eq!(app.overlay, Overlay::None);
        }
    }
}
