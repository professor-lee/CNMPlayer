use crate::data::{assets, atomic_file};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use std::sync::LazyLock;
const LEGACY_STARTUP_FOLDER_KEY: &str = concat!("default", "_opening", "_folder");
const LEGACY_STARTUP_FOLDER_KEY_KEBAB: &str = concat!("default", "-opening", "-folder");

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GraphicsProtocol {
    Off,
    #[serde(alias = "auto")]
    #[serde(alias = "sixel")]
    #[serde(alias = "kitty")]
    #[serde(alias = "iterm2")]
    Halfblocks,
}

impl Default for GraphicsProtocol {
    fn default() -> Self {
        DEFAULT_CONFIG.config.graphics_protocol
    }
}

impl GraphicsProtocol {
    const ALL: [Self; 2] = [Self::Off, Self::Halfblocks];

    pub fn cycle(self, delta: i32) -> Self {
        if delta == 0 {
            return self;
        }

        let current = match self {
            GraphicsProtocol::Off => 0,
            GraphicsProtocol::Halfblocks => 1,
        };
        let next = (current + delta).rem_euclid(Self::ALL.len() as i32) as usize;
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
    pub visualize: VisualizeMode,
    pub eq_bands_db: [f32; crate::tmplayer::app::state::EQ_BANDS],
    pub transparent_background: bool,
    pub album_border: bool,
    pub graphics_protocol: GraphicsProtocol,
    /// Smooth fractional cells for the narrow-window VU meter only.
    pub super_smooth_bar: bool,
    pub bars_gap: bool,
    pub bar_number: BarNumber,
    pub bar_channels: BarChannels,
    pub bar_channel_reverse: bool,
    pub default_opening_title: String,
    pub language: Language,
    pub page_lyrics: bool,
    /// 歌词浮窗是否允许鼠标拖动。
    pub page_lyrics_drag: bool,
    /// 拖动结束后是否吸附到最近的边。
    pub page_lyrics_snap: bool,
    /// 歌词浮窗左上角在内容区内的归一化位置（0..=1）。
    pub page_lyrics_pos_x: f32,
    pub page_lyrics_pos_y: f32,
    pub audio_quality: AudioQuality,
    /// 下载音频档位，默认值由内嵌配置模板提供。
    pub download_audio_quality: AudioQuality,
    /// None 使用系统音乐目录下的 cnmplayer/，无音乐目录时回退 ~/Music/。
    pub download_path: Option<String>,
    pub playback_memory: bool,
    pub show_hints: bool,
    pub small_window_display: bool,
    pub home_more_recommend: bool,
    pub cache: CacheConfig,
    pub keybind_search_box: String,
    pub keybind_fullscreen: String,
    pub keybind_settings: String,
    pub keybind_sidebar: String,
    pub keybind_quit: String,
    pub keybind_page_up: String,
    pub keybind_page_down: String,
    pub keybind_prev: String,
    pub keybind_next: String,
    pub keybind_toggle_play_pause: String,
    pub keybind_toggle_mode: String,
    pub keybind_fullscreen_prev: String,
    pub keybind_fullscreen_next: String,
    pub keybind_fullscreen_toggle_play_pause: String,
    pub keybind_fullscreen_toggle_mode: String,
    pub keybind_fullscreen_eq: String,
    pub keybind_fullscreen_eq_reset: String,
    pub keybind_toggle_like_fullscreen: String,
    pub keybind_toggle_like_collapsed: String,
    pub keybind_small_window_toggle: String,
    /// 主应用：下载当前聚焦的单曲。
    pub keybind_download: String,
    /// 全屏页：下载当前播放的单曲。
    pub keybind_download_fullscreen: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CacheCleanStrategy {
    Size,
    Age,
    Both,
}

impl Default for CacheCleanStrategy {
    fn default() -> Self {
        DEFAULT_CONFIG.config.cache.clean_strategy
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheConfig {
    pub path: Option<String>,
    pub clean_strategy: CacheCleanStrategy,
    pub max_size_mb: u64,
    pub max_age_days: u64,
    pub clean_on_startup: bool,
}

impl Default for CacheConfig {
    fn default() -> Self {
        DEFAULT_CONFIG.config.cache.clone()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum VisualizeMode {
    /// 只显示歌词，不画可视化。旧配置里写的就是 `off`，故保留该别名。
    #[serde(alias = "off")]
    Lyrics,
    /// 右侧（可视化 + 歌词）整块收起，全屏页只留歌曲信息区，并摊满整宽。
    Hidden,
    Bars,
    Oscilloscope,
    /// 左右声道作 X/Y 的李萨如图（矢量模式），直接读播放链路上的 PCM 抽头。
    Vector,
}

impl VisualizeMode {
    /// 按「显示内容由少到多」在所有五档之间循环。
    pub fn cycle(self, delta: i32) -> Self {
        const MODES: [VisualizeMode; 5] = [
            VisualizeMode::Hidden,
            VisualizeMode::Lyrics,
            VisualizeMode::Bars,
            VisualizeMode::Oscilloscope,
            VisualizeMode::Vector,
        ];

        let index = MODES.iter().position(|mode| *mode == self).unwrap_or(1) as i32;
        MODES[((index as i64 + delta as i64).rem_euclid(MODES.len() as i64)) as usize]
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

fn is_legacy_sidebar_default(value: &str) -> bool {
    let normalized = value.trim().to_ascii_lowercase().replace(' ', "");
    normalized == "alt+b"
}

struct DefaultConfig {
    config: Config,
    table: toml::Table,
}

static DEFAULT_CONFIG: LazyLock<DefaultConfig> = LazyLock::new(|| {
    let table: toml::Table = toml::from_str(assets::DEFAULT_CONFIG_TOML)
        .expect("invalid embedded config/default.toml: malformed TOML");
    // Config and CacheConfig have no serde defaults: an incomplete template is
    // a development error, never a recursive request for Config::default().
    let config: Config = toml::Value::Table(table.clone())
        .try_into()
        .expect("invalid embedded config/default.toml: incomplete or invalid configuration");
    assert!(
        config.ui_fps != 0,
        "invalid embedded config/default.toml: ui_fps must be greater than zero"
    );
    DefaultConfig { config, table }
});

impl Default for Config {
    fn default() -> Self {
        DEFAULT_CONFIG.config.clone()
    }
}

/// Fill only absent keys, recursively; explicit values (including false, empty
/// strings and invalid types) always win and are validated by deserialization.
fn complete_missing_fields(user: &mut toml::Table, defaults: &toml::Table) -> bool {
    let mut changed = false;
    for (key, default) in defaults {
        match user.get_mut(key) {
            None => {
                user.insert(key.clone(), default.clone());
                changed = true;
            }
            Some(toml::Value::Table(table)) => {
                if let toml::Value::Table(default_table) = default {
                    changed |= complete_missing_fields(table, default_table);
                }
            }
            Some(_) => {}
        }
    }
    changed
}

impl Config {
    pub fn load_or_default() -> Result<Self> {
        LazyLock::force(&DEFAULT_CONFIG);
        assets::ensure_assets_ready()?;
        Self::load_from_path(&Self::default_path())
    }

    fn load_from_path(path: &std::path::Path) -> Result<Self> {
        if !path.exists() {
            let cfg = Self::default();
            atomic_file::write_atomic(path, assets::DEFAULT_CONFIG_TOML.as_bytes())?;
            return Ok(cfg);
        }

        let raw = fs::read_to_string(path)?;
        let mut user: toml::Table =
            toml::from_str(&raw).with_context(|| format!("parse {}", path.display()))?;
        let legacy_startup_folder_key_present = user.contains_key(LEGACY_STARTUP_FOLDER_KEY_KEBAB)
            || user.contains_key(LEGACY_STARTUP_FOLDER_KEY);
        let graphics_protocol_needs_save = matches!(
            user.get("graphics_protocol").and_then(toml::Value::as_str),
            Some("auto" | "sixel" | "kitty" | "iterm2")
        );
        let completed = complete_missing_fields(&mut user, &DEFAULT_CONFIG.table);
        let mut cfg: Config = toml::Value::Table(user)
            .try_into()
            .with_context(|| format!("parse {}", path.display()))?;

        anyhow::ensure!(
            cfg.ui_fps != 0,
            "invalid ui_fps in {}: must be greater than zero",
            path.display()
        );

        cfg.page_lyrics_pos_x = cfg.page_lyrics_pos_x.clamp(0.0, 1.0);
        cfg.page_lyrics_pos_y = cfg.page_lyrics_pos_y.clamp(0.0, 1.0);

        let mut migrated_legacy_sidebar = false;
        if is_legacy_sidebar_default(&cfg.keybind_sidebar) {
            cfg.keybind_sidebar
                .clone_from(&DEFAULT_CONFIG.config.keybind_sidebar);
            migrated_legacy_sidebar = true;
        }

        if completed
            || graphics_protocol_needs_save
            || legacy_startup_folder_key_present
            || migrated_legacy_sidebar
        {
            cfg.save_to_path(path)?;
        }

        Ok(cfg)
    }

    pub fn save(&self) -> Result<()> {
        assets::ensure_assets_ready()?;
        self.save_to_path(&Self::default_path())
    }

    fn save_to_path(&self, path: &std::path::Path) -> Result<()> {
        let raw = toml::to_string_pretty(self).context("serialize configuration")?;
        atomic_file::write_atomic(path, raw.as_bytes())
    }

    fn default_path() -> PathBuf {
        assets::resolve_config_path()
    }
}

#[cfg(test)]
mod tests {
    use super::{GraphicsProtocol, VisualizeMode};
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

    /// 「仅歌词」曾经写作 `off`；手改或沿用旧配置的用户不能因为改名而丢档位。
    #[test]
    fn visualize_keeps_legacy_off_value_loadable() {
        #[derive(Debug, Deserialize)]
        struct VisualizeWrapper {
            visualize: VisualizeMode,
        }

        let cases = [
            ("lyrics", VisualizeMode::Lyrics),
            ("off", VisualizeMode::Lyrics),
            ("hidden", VisualizeMode::Hidden),
            ("bars", VisualizeMode::Bars),
            ("oscilloscope", VisualizeMode::Oscilloscope),
            ("vector", VisualizeMode::Vector),
        ];

        for (raw, expected) in cases {
            let parsed: VisualizeWrapper =
                toml::from_str(&format!("visualize = \"{}\"", raw)).unwrap();
            assert_eq!(parsed.visualize, expected);
        }
    }

    #[test]
    fn visualize_cycles_all_modes_without_external_dependencies() {
        let modes = [
            VisualizeMode::Hidden,
            VisualizeMode::Lyrics,
            VisualizeMode::Bars,
            VisualizeMode::Oscilloscope,
            VisualizeMode::Vector,
        ];
        for (index, mode) in modes.iter().copied().enumerate() {
            assert_eq!(mode.cycle(1), modes[(index + 1) % modes.len()]);
            assert_eq!(
                mode.cycle(-1),
                modes[(index + modes.len() - 1) % modes.len()]
            );
            assert_eq!(mode.cycle(0), mode);
        }
    }

    #[test]
    fn fps_loading_rejects_zero_and_preserves_every_positive_u32() {
        let dir =
            std::env::temp_dir().join(format!("cnmplayer-fps-config-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("default.toml");
        for fps in [0, 2, 144, u32::MAX] {
            let cfg = super::Config {
                ui_fps: fps,
                ..super::Config::default()
            };
            let raw = toml::to_string_pretty(&cfg).unwrap();
            std::fs::write(&path, &raw).unwrap();
            let loaded = super::Config::load_from_path(&path);
            if fps == 0 {
                assert!(loaded.unwrap_err().to_string().contains("ui_fps"));
                assert_eq!(std::fs::read_to_string(&path).unwrap(), raw);
            } else {
                assert_eq!(loaded.unwrap().ui_fps, fps);
            }
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn partial_user_configuration_preserves_overrides_and_completes_nested_fields() {
        let dir = std::env::temp_dir().join(format!(
            "cnmplayer-partial-config-test-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("default.toml");
        std::fs::write(
            &path,
            "ui_fps = 144\ntransparent_background = false\n[cache]\nmax_age_days = 11\n",
        )
        .unwrap();
        let loaded = super::Config::load_from_path(&path).unwrap();
        assert_eq!(loaded.ui_fps, 144);
        assert!(!loaded.transparent_background);
        assert_eq!(loaded.cache.max_age_days, 11);
        let saved = std::fs::read_to_string(&path).unwrap();
        let reparsed: super::Config = toml::from_str(&saved).unwrap();
        assert_eq!(reparsed.ui_fps, 144);
        assert_eq!(reparsed.cache.max_age_days, 11);
        assert_eq!(
            toml::to_string(&loaded).unwrap(),
            toml::to_string(&reparsed).unwrap()
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn invalid_user_values_are_rejected_without_defaulting_or_rewriting() {
        let dir = std::env::temp_dir().join(format!(
            "cnmplayer-invalid-values-test-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("default.toml");
        for raw in [
            "ui_fps = 'fast'\n",
            "cache = false\n",
            "[cache]\nmax_age_days = -1\n",
        ] {
            std::fs::write(&path, raw).unwrap();
            assert!(super::Config::load_from_path(&path).is_err());
            assert_eq!(std::fs::read_to_string(&path).unwrap(), raw);
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn corrupt_config_is_rejected_without_replacement() {
        let dir =
            std::env::temp_dir().join(format!("cnmplayer-config-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("default.toml");
        std::fs::write(&path, "not = [valid").unwrap();
        assert!(super::Config::load_from_path(&path).is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "not = [valid");
        let _ = std::fs::remove_dir_all(dir);
    }
}
