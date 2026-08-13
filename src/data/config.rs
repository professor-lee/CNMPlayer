use crate::data::assets;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

const DEFAULT_EQ_BANDS_DB: [f32; crate::tmplayer::app::state::EQ_BANDS] =
    [0.0; crate::tmplayer::app::state::EQ_BANDS];
const LEGACY_STARTUP_FOLDER_KEY: &str = concat!("default", "_opening", "_folder");
const LEGACY_STARTUP_FOLDER_KEY_KEBAB: &str = concat!("default", "-opening", "-folder");

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum GraphicsProtocol {
    Off,
    #[default]
    #[serde(alias = "auto")]
    #[serde(alias = "sixel")]
    #[serde(alias = "kitty")]
    #[serde(alias = "iterm2")]
    Halfblocks,
}

impl GraphicsProtocol {
    const ALL: [Self; 2] = [Self::Off, Self::Halfblocks];

    pub fn to_ratatui_protocol(self) -> Option<ratatui_image::picker::ProtocolType> {
        match self {
            GraphicsProtocol::Off => None,
            GraphicsProtocol::Halfblocks => Some(ratatui_image::picker::ProtocolType::Halfblocks),
        }
    }

    pub fn cycle(self, delta: i32) -> Self {
        if delta == 0 {
            return self;
        }

        let current = match self {
            GraphicsProtocol::Off => 0,
            GraphicsProtocol::Halfblocks => 1,
        };
        let next = (current as i32 + delta).rem_euclid(Self::ALL.len() as i32) as usize;
        Self::ALL[next]
    }

    pub fn display_name(self) -> &'static str {
        match self {
            GraphicsProtocol::Off => "off",
            GraphicsProtocol::Halfblocks => "Halfblocks",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub theme: String,
    pub ui_fps: u32,
    pub spectrum_hz: u32,
    pub mpris_poll_ms: u64,

    #[serde(default = "default_visualize")]
    pub visualize: VisualizeMode,

    #[serde(default = "default_eq_bands_db")]
    pub eq_bands_db: [f32; crate::tmplayer::app::state::EQ_BANDS],

    #[serde(default)]
    pub transparent_background: bool,

    #[serde(default = "default_album_border")]
    pub album_border: bool,

    #[serde(default)]
    pub graphics_protocol: GraphicsProtocol,

    #[serde(default = "default_kitty_cover_scale_percent")]
    pub kitty_cover_scale_percent: u8,

    #[serde(default)]
    pub super_smooth_bar: bool,

    #[serde(default)]
    pub bars_gap: bool,

    #[serde(default = "default_bar_number")]
    pub bar_number: BarNumber,

    #[serde(default = "default_bar_channels")]
    pub bar_channels: BarChannels,

    #[serde(default)]
    pub bar_channel_reverse: bool,

    #[serde(default)]
    pub lyrics_cover_fetch: bool,

    #[serde(default)]
    pub lyrics_cover_download: bool,

    #[serde(default)]
    pub audio_fingerprint: bool,

    #[serde(default)]
    pub acoustid_api_key: String,

    #[serde(default)]
    pub resume_last_position: bool,

    #[serde(default)]
    pub default_opening_title: String,

    #[serde(default = "default_language")]
    pub language: Language,

    #[serde(default = "default_page_lyrics")]
    pub page_lyrics: bool,

    #[serde(default = "default_audio_quality")]
    pub audio_quality: AudioQuality,

    #[serde(default)]
    pub playback_memory: bool,

    #[serde(default = "default_show_hints")]
    pub show_hints: bool,

    #[serde(default)]
    pub home_more_recommend: bool,

    #[serde(default)]
    pub cache: CacheConfig,

    #[serde(default = "default_keybind_search_box")]
    pub keybind_search_box: String,

    #[serde(default = "default_keybind_fullscreen")]
    pub keybind_fullscreen: String,

    #[serde(default = "default_keybind_settings")]
    pub keybind_settings: String,

    #[serde(default = "default_keybind_sidebar")]
    pub keybind_sidebar: String,

    #[serde(default = "default_keybind_quit")]
    pub keybind_quit: String,

    #[serde(default = "default_keybind_page_up")]
    pub keybind_page_up: String,

    #[serde(default = "default_keybind_page_down")]
    pub keybind_page_down: String,

    #[serde(default = "default_keybind_prev")]
    pub keybind_prev: String,

    #[serde(default = "default_keybind_next")]
    pub keybind_next: String,

    #[serde(default = "default_keybind_toggle_play_pause")]
    pub keybind_toggle_play_pause: String,

    #[serde(default = "default_keybind_toggle_mode")]
    pub keybind_toggle_mode: String,

    #[serde(default = "default_keybind_fullscreen_prev")]
    pub keybind_fullscreen_prev: String,

    #[serde(default = "default_keybind_fullscreen_next")]
    pub keybind_fullscreen_next: String,

    #[serde(default = "default_keybind_fullscreen_toggle_play_pause")]
    pub keybind_fullscreen_toggle_play_pause: String,

    #[serde(default = "default_keybind_fullscreen_toggle_mode")]
    pub keybind_fullscreen_toggle_mode: String,

    #[serde(default = "default_keybind_fullscreen_eq")]
    pub keybind_fullscreen_eq: String,

    #[serde(default = "default_keybind_fullscreen_eq_reset")]
    pub keybind_fullscreen_eq_reset: String,

    #[serde(default = "default_keybind_toggle_like_fullscreen")]
    pub keybind_toggle_like_fullscreen: String,

    #[serde(default = "default_keybind_toggle_like_collapsed")]
    pub keybind_toggle_like_collapsed: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[derive(Default)]
pub enum CacheCleanStrategy {
    Size,
    Age,
    #[default]
    Both,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheConfig {
    #[serde(default)]
    pub path: Option<String>,

    #[serde(default)]
    pub clean_strategy: CacheCleanStrategy,

    #[serde(default = "default_cache_max_size_mb")]
    pub max_size_mb: u64,

    #[serde(default = "default_cache_max_age_days")]
    pub max_age_days: u64,

    #[serde(default = "default_cache_clean_on_startup")]
    pub clean_on_startup: bool,
}

impl Default for CacheConfig {
    fn default() -> Self {
        Self {
            path: None,
            clean_strategy: CacheCleanStrategy::default(),
            max_size_mb: default_cache_max_size_mb(),
            max_age_days: default_cache_max_age_days(),
            clean_on_startup: default_cache_clean_on_startup(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum VisualizeMode {
    Off,
    Bars,
    Oscilloscope,
}

impl VisualizeMode {
    pub fn cycle(self, delta: i32) -> Self {
        const MODES: [VisualizeMode; 3] = [
            VisualizeMode::Off,
            VisualizeMode::Bars,
            VisualizeMode::Oscilloscope,
        ];

        let index = MODES.iter().position(|mode| *mode == self).unwrap_or(1) as i32;
        let next = (index + delta).rem_euclid(MODES.len() as i32) as usize;
        MODES[next]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BarChannels {
    Stereo,
    Mono,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BarNumber {
    #[serde(rename = "auto")]
    Auto,
    #[serde(rename = "16")]
    N16,
    #[serde(rename = "32")]
    N32,
    #[serde(rename = "48")]
    N48,
    #[serde(rename = "64")]
    N64,
    #[serde(rename = "80")]
    N80,
    #[serde(rename = "96")]
    N96,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Language {
    Zh,
    En,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AudioQuality {
    #[serde(rename = "standard")]
    Standard,
    #[serde(rename = "higher")]
    Higher,
    #[serde(rename = "exhigh")]
    Exhigh,
    #[serde(rename = "lossless")]
    Lossless,
    #[serde(rename = "hires")]
    Hires,
    #[serde(rename = "jyeffect")]
    Jyeffect,
    #[serde(rename = "sky")]
    Sky,
    #[serde(rename = "dolby")]
    Dolby,
    #[serde(rename = "jymaster")]
    Jymaster,
}

impl AudioQuality {
    pub const FREE_LEVELS: [Self; 3] = [Self::Standard, Self::Higher, Self::Exhigh];
    pub const ALL_LEVELS: [Self; 9] = [
        Self::Standard,
        Self::Higher,
        Self::Exhigh,
        Self::Lossless,
        Self::Hires,
        Self::Jyeffect,
        Self::Sky,
        Self::Dolby,
        Self::Jymaster,
    ];

    pub fn as_api_level(self) -> &'static str {
        match self {
            Self::Standard => "standard",
            Self::Higher => "higher",
            Self::Exhigh => "exhigh",
            Self::Lossless => "lossless",
            Self::Hires => "hires",
            Self::Jyeffect => "jyeffect",
            Self::Sky => "sky",
            Self::Dolby => "dolby",
            Self::Jymaster => "jymaster",
        }
    }

    pub fn clamp_for_vip(self, vip_unlocked: bool) -> Self {
        if vip_unlocked {
            self
        } else {
            match self {
                Self::Standard | Self::Higher | Self::Exhigh => self,
                _ => Self::Exhigh,
            }
        }
    }

    pub fn cycle(self, delta: i32, vip_unlocked: bool) -> Self {
        let options: &[Self] = if vip_unlocked {
            &Self::ALL_LEVELS
        } else {
            &Self::FREE_LEVELS
        };

        let current = self.clamp_for_vip(vip_unlocked);
        let index = options
            .iter()
            .position(|item| *item == current)
            .unwrap_or(0) as i32;
        let next = (index + delta).rem_euclid(options.len() as i32) as usize;
        options[next]
    }
}

fn default_visualize() -> VisualizeMode {
    if crate::tmplayer::audio::cava::is_available() {
        VisualizeMode::Bars
    } else {
        VisualizeMode::Off
    }
}

fn default_eq_bands_db() -> [f32; crate::tmplayer::app::state::EQ_BANDS] {
    DEFAULT_EQ_BANDS_DB
}

fn default_album_border() -> bool {
    true
}

fn default_kitty_cover_scale_percent() -> u8 {
    100
}

fn default_bar_number() -> BarNumber {
    BarNumber::Auto
}

fn default_bar_channels() -> BarChannels {
    BarChannels::Mono
}

fn default_language() -> Language {
    Language::Zh
}

fn default_page_lyrics() -> bool {
    false
}

fn default_audio_quality() -> AudioQuality {
    AudioQuality::Exhigh
}

fn default_show_hints() -> bool {
    true
}

fn default_cache_max_size_mb() -> u64 {
    500
}

fn default_cache_max_age_days() -> u64 {
    7
}

fn default_cache_clean_on_startup() -> bool {
    true
}

fn default_keybind_search_box() -> String {
    "Ctrl+S".to_string()
}

fn default_keybind_fullscreen() -> String {
    "Ctrl+F".to_string()
}

fn default_keybind_settings() -> String {
    "T".to_string()
}

fn default_keybind_sidebar() -> String {
    "P".to_string()
}

fn is_legacy_sidebar_default(value: &str) -> bool {
    let normalized = value.trim().to_ascii_lowercase().replace(' ', "");
    normalized == "alt+b"
}

fn default_keybind_quit() -> String {
    "Q".to_string()
}

fn default_keybind_page_up() -> String {
    "pageUP".to_string()
}

fn default_keybind_page_down() -> String {
    "pageDown".to_string()
}

fn default_keybind_prev() -> String {
    "Alt+Left".to_string()
}

fn default_keybind_next() -> String {
    "Alt+Right".to_string()
}

fn default_keybind_toggle_play_pause() -> String {
    "Alt+Space".to_string()
}

fn default_keybind_toggle_mode() -> String {
    "Alt+M".to_string()
}

fn default_keybind_fullscreen_prev() -> String {
    "Left".to_string()
}

fn default_keybind_fullscreen_next() -> String {
    "Right".to_string()
}

fn default_keybind_fullscreen_toggle_play_pause() -> String {
    "Space".to_string()
}

fn default_keybind_fullscreen_toggle_mode() -> String {
    "M".to_string()
}

fn default_keybind_fullscreen_eq() -> String {
    "E".to_string()
}

fn default_keybind_fullscreen_eq_reset() -> String {
    "Alt+R".to_string()
}

fn default_keybind_toggle_like_fullscreen() -> String {
    "L".to_string()
}

fn default_keybind_toggle_like_collapsed() -> String {
    "Alt+L".to_string()
}

impl Default for Config {
    fn default() -> Self {
        Self {
            theme: "frappe".to_string(),
            ui_fps: 30,
            spectrum_hz: 30,
            mpris_poll_ms: 100,
            visualize: default_visualize(),
            eq_bands_db: default_eq_bands_db(),
            transparent_background: true,
            album_border: default_album_border(),
            graphics_protocol: GraphicsProtocol::default(),
            kitty_cover_scale_percent: default_kitty_cover_scale_percent(),
            super_smooth_bar: false,
            bars_gap: false,
            bar_number: default_bar_number(),
            bar_channels: default_bar_channels(),
            bar_channel_reverse: false,
            lyrics_cover_fetch: false,
            lyrics_cover_download: false,
            audio_fingerprint: false,
            acoustid_api_key: String::new(),
            resume_last_position: false,
            default_opening_title: String::new(),
            language: default_language(),
            page_lyrics: default_page_lyrics(),
            audio_quality: default_audio_quality(),
            playback_memory: false,
            show_hints: default_show_hints(),
            home_more_recommend: false,
            cache: CacheConfig::default(),
            keybind_search_box: default_keybind_search_box(),
            keybind_fullscreen: default_keybind_fullscreen(),
            keybind_settings: default_keybind_settings(),
            keybind_sidebar: default_keybind_sidebar(),
            keybind_quit: default_keybind_quit(),
            keybind_page_up: default_keybind_page_up(),
            keybind_page_down: default_keybind_page_down(),
            keybind_prev: default_keybind_prev(),
            keybind_next: default_keybind_next(),
            keybind_toggle_play_pause: default_keybind_toggle_play_pause(),
            keybind_toggle_mode: default_keybind_toggle_mode(),
            keybind_fullscreen_prev: default_keybind_fullscreen_prev(),
            keybind_fullscreen_next: default_keybind_fullscreen_next(),
            keybind_fullscreen_toggle_play_pause: default_keybind_fullscreen_toggle_play_pause(),
            keybind_fullscreen_toggle_mode: default_keybind_fullscreen_toggle_mode(),
            keybind_fullscreen_eq: default_keybind_fullscreen_eq(),
            keybind_fullscreen_eq_reset: default_keybind_fullscreen_eq_reset(),
            keybind_toggle_like_fullscreen: default_keybind_toggle_like_fullscreen(),
            keybind_toggle_like_collapsed: default_keybind_toggle_like_collapsed(),
        }
    }
}

impl Config {
    pub fn load_or_default() -> Result<Self> {
        let _ = assets::ensure_assets_ready();
        let path = Self::default_path();
        if !path.exists() {
            let cfg = Self::default();
            let _ = cfg.save();
            return Ok(cfg);
        }

        let raw = fs::read_to_string(path)?;
        let legacy_startup_folder_key_present = raw.contains(LEGACY_STARTUP_FOLDER_KEY_KEBAB)
            || raw.contains(LEGACY_STARTUP_FOLDER_KEY);
        let graphics_protocol_needs_save = graphics_protocol_needs_save(&raw);
        let mut cfg: Config = toml::from_str(&raw).unwrap_or_default();

        if cfg.ui_fps == 0 {
            cfg.ui_fps = 30;
        }
        if cfg.spectrum_hz == 0 {
            cfg.spectrum_hz = 30;
        }

        let mut forced_visualize_off = false;
        if !crate::tmplayer::audio::cava::is_available() && cfg.visualize != VisualizeMode::Off {
            cfg.visualize = VisualizeMode::Off;
            forced_visualize_off = true;
        }

        let mut migrated_legacy_sidebar = false;
        if is_legacy_sidebar_default(&cfg.keybind_sidebar) {
            cfg.keybind_sidebar = default_keybind_sidebar();
            migrated_legacy_sidebar = true;
        }

        if !raw.contains("default_opening_title")
            || !raw.contains("language")
            || !raw.contains("page_lyrics")
            || !raw.contains("eq_bands_db")
            || !raw.contains("audio_quality")
            || !raw.contains("playback_memory")
            || !raw.contains("show_hints")
            || !raw.contains("home_more_recommend")
            || !raw.contains("[cache]")
            || !raw.contains("bar_number")
            || !raw.contains("bar_channels")
            || !raw.contains("bar_channel_reverse")
            || graphics_protocol_needs_save
            || !raw.contains("keybind_search_box")
            || !raw.contains("keybind_fullscreen")
            || !raw.contains("keybind_settings")
            || !raw.contains("keybind_sidebar")
            || !raw.contains("keybind_quit")
            || !raw.contains("keybind_page_up")
            || !raw.contains("keybind_page_down")
            || !raw.contains("keybind_prev")
            || forced_visualize_off
            || !raw.contains("keybind_next")
            || !raw.contains("keybind_toggle_play_pause")
            || !raw.contains("keybind_toggle_mode")
            || !raw.contains("keybind_fullscreen_prev")
            || !raw.contains("keybind_fullscreen_next")
            || !raw.contains("keybind_fullscreen_toggle_play_pause")
            || !raw.contains("keybind_fullscreen_toggle_mode")
            || !raw.contains("keybind_fullscreen_eq")
            || !raw.contains("keybind_fullscreen_eq_reset")
            || !raw.contains("keybind_toggle_like_fullscreen")
            || !raw.contains("keybind_toggle_like_collapsed")
            || legacy_startup_folder_key_present
            || migrated_legacy_sidebar
        {
            let _ = cfg.save();
        }

        Ok(cfg)
    }

    pub fn save(&self) -> Result<()> {
        let _ = assets::ensure_assets_ready();
        let path = Self::default_path();
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let raw = toml::to_string_pretty(self).unwrap_or_default();
        fs::write(path, raw)?;
        Ok(())
    }

    fn default_path() -> PathBuf {
        assets::resolve_config_path()
    }
}

fn graphics_protocol_needs_save(raw: &str) -> bool {
    let Some(value) = raw.lines().map(str::trim).find_map(|line| {
        if line.starts_with('#') || !line.starts_with("graphics_protocol") {
            return None;
        }

        let (_, value) = line.split_once('=')?;
        let value = value.split('#').next()?.trim().trim_matches('"');
        Some(value)
    }) else {
        return true;
    };

    matches!(value, "auto" | "sixel" | "kitty" | "iterm2")
}

#[cfg(test)]
mod tests {
    use super::GraphicsProtocol;
    use serde::Deserialize;

    #[derive(Debug, Deserialize)]
    struct GraphicsProtocolWrapper {
        protocol: GraphicsProtocol,
    }

    #[test]
    fn graphics_protocol_keeps_legacy_values_loadable() {
        let cases = [
            ("off", GraphicsProtocol::Off),
            ("halfblocks", GraphicsProtocol::Halfblocks),
            ("auto", GraphicsProtocol::Halfblocks),
            ("sixel", GraphicsProtocol::Halfblocks),
            ("kitty", GraphicsProtocol::Halfblocks),
            ("iterm2", GraphicsProtocol::Halfblocks),
        ];

        for (raw, expected) in cases {
            let parsed: GraphicsProtocolWrapper =
                toml::from_str(&format!("protocol = \"{}\"", raw)).unwrap();
            assert_eq!(parsed.protocol, expected);
        }
    }
}
