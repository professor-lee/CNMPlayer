use crate::state;
use anyhow::{Context as _, Result};
use compio::time::{Interval, interval};
use futures::stream::unfold;
use futures::{Stream, StreamExt};
use see::unsync::Receiver;
use std::fs;
use std::io::{BufRead, BufReader, Read};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicI64, AtomicU8, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CavaChannels {
    Stereo,
    Mono,
}

pub struct MiniCavaState {
    pub event: Receiver<[f32; 20]>,
}

fn interval_stream(interval: Interval) -> impl Stream<Item = Instant> {
    unfold(interval, async |mut interval| {
        Some((interval.tick().await, interval))
    })
}

impl MiniCavaState {
    pub fn try_new(cfg: CavaConfig) -> Result<Self> {
        let freq = cfg.framerate_hz;
        let runner = CavaRunner::start(cfg)?;
        let period = Duration::from_millis((1000 / freq).into());
        let interval = interval(period);
        let stream = interval_stream(interval).map(move |_| {
            let vec = runner.latest_bars();
            let mut arr = [0.0; 20];
            let len = vec.len().min(20);
            arr[..len].copy_from_slice(&vec[..len]);
            arr
        });
        let event = state(stream);
        Ok(Self { event })
    }

    pub fn bars(&self) -> [f32; 20] {
        *self.event.borrow()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CavaConfig {
    pub framerate_hz: u32,
    pub bars: usize,
    pub channels: CavaChannels,
    pub reverse: bool,
}

pub struct CavaRunner {
    left: Arc<Mutex<Vec<f32>>>,
    right: Arc<Mutex<Vec<f32>>>,
    channels: CavaChannels,
    child: Child,
    _reader: thread::JoinHandle<()>,
    cfg_path: String,
}

pub fn is_available() -> bool {
    static AVAILABLE: OnceLock<bool> = OnceLock::new();
    *AVAILABLE.get_or_init(|| find_cava_executable().is_some())
}

impl CavaRunner {
    pub fn start(cfg: CavaConfig) -> Result<Self> {
        // Minimal config we generate ourselves (do not copy upstream example config).
        // Uses raw ascii output to stdout.
        // We request stereo; depending on cava version/backend, it may emit:
        // - 2 lines per frame (one per channel, each 64 values), OR
        // - 1 line per frame containing 128 values.
        let framerate_hz = cfg.framerate_hz.clamp(10, 120);
        let bars = cfg.bars.clamp(8, 96);
        let channels = cfg.channels;
        let channels_str = match channels {
            CavaChannels::Stereo => "stereo",
            CavaChannels::Mono => "mono",
        };
        let reverse = if cfg.reverse { 1 } else { 0 };
        // Inherit the user's cava [input] section if one exists. On macOS this is
        // required: without a loopback source (e.g. `source = "BlackHole 2ch"`) cava 1.x
        // aborts with "output mix capture still requires a loopback-capable device".
        let user_input = user_cava_input_section();
        let input_block = if user_input.is_empty() {
            "[input]\n# Leave method/source unset: cava will pick the best supported backend (pipewire/pulse/etc).\n\n".to_string()
        } else {
            format!("[input]\n{user_input}\n")
        };
        let cfg = format!(
            "[general]\nframerate = {fr}\nbars = {bars}\nreverse = {reverse}\n\n{input_block}[output]\nmethod = raw\nchannels = {channels}\nraw_target = /dev/stdout\ndata_format = ascii\nascii_max_range = 1000\nbar_delimiter = 59\nframe_delimiter = 10\n",
            fr = framerate_hz,
            bars = bars,
            reverse = reverse,
            input_block = input_block,
            channels = channels_str
        );

        let cfg_path = temp_cfg_path();
        fs::write(&cfg_path, cfg).with_context(|| format!("write cava config: {cfg_path}"))?;

        let cava_exe = resolve_cava_executable()?;
        let mut child = Command::new(&cava_exe)
            .arg("-p")
            .arg(&cfg_path)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .with_context(|| format!("spawn cava: {}", cava_exe.display()))?;

        let stdout = child
            .stdout
            .take()
            .context("failed to capture cava stdout")?;
        let stderr = child
            .stderr
            .take()
            .context("failed to capture cava stderr")?;

        let left: Arc<Mutex<Vec<f32>>> = Arc::new(Mutex::new(vec![0.0; bars]));
        let right: Arc<Mutex<Vec<f32>>> = Arc::new(Mutex::new(vec![0.0; bars]));
        let left_cloned = Arc::clone(&left);
        let right_cloned = Arc::clone(&right);

        let reader = thread::spawn(move || {
            let mut br = BufReader::new(stdout);
            let mut stderr = BufReader::new(stderr);
            let mut line = String::new();
            let mut next_is_left = true;
            loop {
                line.clear();
                match br.read_line(&mut line) {
                    Ok(0) => {
                        // cava exited (or produced nothing). Surface its stderr
                        // instead of failing silently with an empty spectrum.
                        let mut err = String::new();
                        let _ = stderr.read_to_string(&mut err);
                        log::warn!("cava exited early (EOF on stdout); stderr: {}", err.trim());
                        break;
                    }
                    Ok(_) => {
                        let frames = parse_frames_ascii(&line, bars);
                        match channels {
                            CavaChannels::Mono => {
                                if let Some(frame) = frames.first() {
                                    let mut g = left_cloned.lock().unwrap();
                                    *g = frame.clone();
                                    let mut r = right_cloned.lock().unwrap();
                                    *r = frame.clone();
                                }
                            }
                            CavaChannels::Stereo => match frames.len() {
                                1 => {
                                    let frame = frames[0].clone();
                                    if next_is_left {
                                        let mut g = left_cloned.lock().unwrap();
                                        *g = frame;
                                    } else {
                                        let mut g = right_cloned.lock().unwrap();
                                        *g = frame;
                                    }
                                    next_is_left = !next_is_left;
                                }
                                2 => {
                                    {
                                        let mut g = left_cloned.lock().unwrap();
                                        *g = frames[0].clone();
                                    }
                                    {
                                        let mut g = right_cloned.lock().unwrap();
                                        *g = frames[1].clone();
                                    }
                                    next_is_left = true;
                                }
                                _ => {}
                            },
                        }
                    }
                    Err(_) => break,
                }
            }
        });

        Ok(Self {
            left,
            right,
            channels,
            child,
            _reader: reader,
            cfg_path,
        })
    }

    pub fn latest_bars(&self) -> Vec<f32> {
        let l = self.left.lock().unwrap().clone();
        let r = self.right.lock().unwrap().clone();
        if self.channels == CavaChannels::Mono {
            return l;
        }
        let mut out = vec![0.0f32; l.len()];
        for i in 0..l.len().min(r.len()) {
            out[i] = ((l[i] + r[i]) * 0.5).clamp(0.0, 1.0);
        }
        out
    }

    pub fn latest_stereo_bars(&self) -> (Vec<f32>, Vec<f32>) {
        (
            self.left.lock().unwrap().clone(),
            self.right.lock().unwrap().clone(),
        )
    }
}

/// Locate the user's cava config file
/// (`$XDG_CONFIG_HOME/cava/config` or `~/.config/cava/config`).
fn user_cava_config_path() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            std::env::var_os("HOME")
                .map(PathBuf::from)
                .unwrap_or_default()
        })
        .join(".config")
        .join("cava")
        .join("config")
}

/// Extract the `[input]` section from the user's cava config.
/// Lets the app inherit a working input method/source (e.g.
/// `method = coreaudio` + `source = "BlackHole 2ch"` on macOS).
fn user_cava_input_section() -> String {
    let Ok(content) = fs::read_to_string(user_cava_config_path()) else {
        return String::new();
    };

    let mut out = String::new();
    let mut in_input = false;
    for line in content.lines() {
        let t = line.trim();
        if t.starts_with('[') {
            in_input = t == "[input]";
            continue;
        }
        if in_input && !t.is_empty() && !t.starts_with('#') && !t.starts_with(';') {
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

/// Colors inherited from the user's cava config `[color]` section.
///
/// cava convention: `gradient_color_1` is the BOTTOM color and higher
/// numbers go UP the screen. `gradient` is therefore stored bottom→top.
/// On macOS, schemes handed out by [`user_cava_color_scheme`] follow the
/// system appearance: in Dark Mode the gradient order is reversed.
#[derive(Debug, Clone, PartialEq)]
pub struct CavaColorScheme {
    /// Multi-stop gradient, ordered bottom→top. Empty when gradient is off.
    pub gradient: Vec<(u8, u8, u8)>,
    /// Flat foreground color (used when gradient is off / has no stops).
    pub foreground: Option<(u8, u8, u8)>,
}

impl CavaColorScheme {
    /// Resolve the color at height `t` of the spectrum, where `t` follows the
    /// cnmplayer renderer convention: 0.0 = TOP of the bars, 1.0 = bottom.
    /// Returns `None` when the scheme carries no usable color.
    pub fn color_at(&self, t: f32) -> Option<(u8, u8, u8)> {
        if self.gradient.is_empty() {
            return self.foreground;
        }
        // Flip cnmplayer t (0=top) to cava position (0=bottom).
        let pos = (1.0 - t).clamp(0.0, 1.0);
        let n = self.gradient.len();
        if n == 1 {
            return Some(self.gradient[0]);
        }
        let scaled = pos * (n - 1) as f32;
        let idx = (scaled.floor() as usize).min(n - 2);
        let frac = scaled - idx as f32;
        let (a, b) = (self.gradient[idx], self.gradient[idx + 1]);
        Some(mix_rgb(a, b, frac))
    }

    /// Same scheme with the gradient stop order flipped (bottom→top becomes
    /// top→bottom). Used to adapt a Light-Mode scheme to macOS Dark Mode:
    /// a gradient running dark→light upwards becomes light→dark, i.e.
    /// white at the bottom and black at the top.
    #[must_use]
    pub fn reversed(mut self) -> Self {
        self.gradient.reverse();
        self
    }
}

fn mix_rgb(a: (u8, u8, u8), b: (u8, u8, u8), t: f32) -> (u8, u8, u8) {
    let t = t.clamp(0.0, 1.0);
    let lerp = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    (lerp(a.0, b.0), lerp(a.1, b.1), lerp(a.2, b.2))
}

/// Parse a single cava color token: `'#rrggbb'`, `#rrggbb` or a named color.
/// Tolerates trailing inline comments (`; ...` or ` # ...`), so real-world
/// values like `'#4C4C4C'  # 明度 30%` parse as `#4C4C4C`.
/// `default` and unknown tokens yield `None` (keep the caller's fallback).
fn parse_cava_color(raw: &str) -> Option<(u8, u8, u8)> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    // Quoted value: take the content between the first matching quote pair;
    // anything after the closing quote (e.g. an inline comment) is dropped.
    let first = raw.chars().next().unwrap();
    if first == '\'' || first == '"' {
        let s = match raw[1..].find(first) {
            Some(end) => &raw[1..1 + end],
            None => raw.trim_matches(first), // unterminated quote: best effort
        };
        return parse_bare_color(s);
    }
    // Unquoted: cut `;` comments first.
    let s = raw.split(';').next().unwrap_or("").trim();
    // A leading `#` begins a hex token, not a comment.
    if let Some(hex) = s.strip_prefix('#') {
        let head: String = hex.chars().take(6).collect();
        return parse_bare_color(&format!("#{head}"));
    }
    // Otherwise an inline comment after whitespace ends the value; named
    // colors are a single word, so keep only the first token.
    let s = s.split_whitespace().next().unwrap_or("");
    parse_bare_color(s)
}

fn parse_bare_color(s: &str) -> Option<(u8, u8, u8)> {
    let s = s.trim();
    if let Some(hex) = s.strip_prefix('#') {
        if hex.len() == 6 {
            let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
            let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
            let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
            return Some((r, g, b));
        }
        return None;
    }
    match s.to_ascii_lowercase().as_str() {
        "black" => Some((0, 0, 0)),
        "red" => Some((255, 0, 0)),
        "green" => Some((0, 255, 0)),
        "yellow" => Some((255, 255, 0)),
        "blue" => Some((0, 0, 255)),
        "magenta" => Some((255, 0, 255)),
        "cyan" => Some((0, 255, 255)),
        "white" => Some((255, 255, 255)),
        _ => None, // "default" and anything unknown → fall back
    }
}

fn parse_user_cava_color_scheme() -> Option<CavaColorScheme> {
    let content = fs::read_to_string(user_cava_config_path()).ok()?;
    parse_color_scheme_from(&content)
}

fn parse_color_scheme_from(content: &str) -> Option<CavaColorScheme> {
    let mut in_color = false;
    let mut gradient = false;
    let mut foreground: Option<(u8, u8, u8)> = None;
    let mut by_index: std::collections::BTreeMap<usize, (u8, u8, u8)> =
        std::collections::BTreeMap::new();
    for line in content.lines() {
        let t = line.trim();
        if t.starts_with('[') {
            in_color = t == "[color]";
            continue;
        }
        if !in_color || t.is_empty() || t.starts_with('#') || t.starts_with(';') {
            continue;
        }
        let Some((key, val)) = t.split_once('=') else {
            continue;
        };
        let key = key.trim();
        let val = val.trim();
        match key {
            "gradient" => {
                let flag = val
                    .split(';')
                    .next()
                    .unwrap_or("")
                    .split_whitespace()
                    .next()
                    .unwrap_or("");
                gradient = flag == "1" || flag.eq_ignore_ascii_case("true");
            }
            "foreground" => foreground = parse_cava_color(val),
            _ if key.starts_with("gradient_color_") => {
                // A malformed index only skips that one stop.
                if let Ok(n) = key["gradient_color_".len()..].trim().parse::<usize>() {
                    if let Some(c) = parse_cava_color(val) {
                        by_index.insert(n, c);
                    }
                }
            }
            _ => {}
        }
    }
    // Assemble bottom→top gradient (cava: gradient_color_1 = bottom),
    // compacting any gaps between defined stops.
    let gradient_colors: Vec<(u8, u8, u8)> = by_index.into_values().collect();

    if gradient && !gradient_colors.is_empty() {
        return Some(CavaColorScheme {
            gradient: gradient_colors,
            foreground,
        });
    }
    foreground.map(|fg| CavaColorScheme {
        gradient: Vec::new(),
        foreground: Some(fg),
    })
}

/// The user's cava `[color]` scheme, parsed once per process from
/// `~/.config/cava/config`, adapted to the macOS system appearance:
/// in Dark Mode the gradient is reversed (white at the bottom → black at
/// the top). `None` when the config has no usable colors (all defaults) —
/// callers then fall back to their own palette.
pub fn user_cava_color_scheme() -> Option<CavaColorScheme> {
    static CACHE: OnceLock<Option<CavaColorScheme>> = OnceLock::new();
    CACHE
        .get_or_init(parse_user_cava_color_scheme)
        .clone()
        .map(|scheme| adapt_scheme_to_appearance(scheme, system_is_dark_mode()))
}

/// In Dark Mode the gradient is flipped so the light end sits at the bottom
/// and the dark end on top; Light Mode keeps the config as-is. A flat
/// foreground (no gradient) is left untouched.
fn adapt_scheme_to_appearance(scheme: CavaColorScheme, dark: bool) -> CavaColorScheme {
    if dark {
        scheme.reversed()
    } else {
        scheme
    }
}

/// Whether the macOS system appearance is currently Dark Mode. The result
/// is cached for a short interval: renderers call this every frame, but the
/// appearance lookup must not run that often.
#[cfg(target_os = "macos")]
fn system_is_dark_mode() -> bool {
    static DARK: AtomicU8 = AtomicU8::new(0);
    static LAST_CHECK_MS: AtomicI64 = AtomicI64::new(i64::MIN);
    const CHECK_INTERVAL_MS: i64 = 3_000;

    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64;
    let last = LAST_CHECK_MS.load(Ordering::Relaxed);
    // Exactly one caller wins the exchange and refreshes the cache; the
    // rest keep using the value from the previous round.
    if now_ms.saturating_sub(last) >= CHECK_INTERVAL_MS
        && LAST_CHECK_MS
            .compare_exchange(last, now_ms, Ordering::Relaxed, Ordering::Relaxed)
            .is_ok()
    {
        let dark = macos_is_dark_mode();
        DARK.store(u8::from(dark), Ordering::Relaxed);
        return dark;
    }
    DARK.load(Ordering::Relaxed) == 1
}

/// Other platforms have no system Light/Dark appearance to follow.
#[cfg(not(target_os = "macos"))]
fn system_is_dark_mode() -> bool {
    false
}

#[cfg(target_os = "macos")]
fn macos_is_dark_mode() -> bool {
    session_style_is_dark(macos_appearance::apple_interface_style().as_deref())
}

/// Only the literal `Dark` (case-insensitive) means Dark Mode; an absent
/// style means Light Mode (or Auto appearance during daytime).
#[cfg(target_os = "macos")]
fn session_style_is_dark(style: Option<&str>) -> bool {
    style.is_some_and(|s| s.eq_ignore_ascii_case("dark"))
}

#[cfg(target_os = "macos")]
mod macos_appearance {
    //! Minimal CoreFoundation FFI to read the global `AppleInterfaceStyle`
    //! preference — the exact same store `defaults read -g AppleInterfaceStyle`
    //! consults, but in-process (no fork/exec, no TCC permission prompt).
    //!
    //! Semantics match `defaults read`: the key holds `Dark` while Dark Mode
    //! is manually selected and is absent in Light Mode. Note that with
    //! appearance set to Auto the key stays absent (macOS 15 verified), so
    //! Auto's night-time dark is reported as light; manual toggles — the
    //! common case — always update live via cfprefsd.

    use std::ffi::{CStr, CString, c_char, c_void};

    type CFStringRef = *const c_void;
    type CFPropertyListRef = *const c_void;

    /// `kCFStringEncodingUTF8`.
    const K_CF_STRING_ENCODING_UTF8: u32 = 0x0800_0100;

    /// `kCFPreferencesAnyApplication` — the global (`defaults read -g`) domain.
    /// The constant's value equals its symbol name.
    const K_CF_PREFERENCES_ANY_APPLICATION: &str = "kCFPreferencesAnyApplication";

    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFPreferencesCopyAppValue(
            key: CFStringRef,
            application: CFStringRef,
        ) -> CFPropertyListRef;
        fn CFPreferencesAppSynchronize(application: CFStringRef);
        fn CFStringCreateWithCString(
            alloc: *const c_void,
            c_str: *const c_char,
            encoding: u32,
        ) -> CFStringRef;
        fn CFStringGetCString(
            the_string: CFStringRef,
            buffer: *mut c_char,
            buffer_size: isize,
            encoding: u32,
        ) -> bool;
        fn CFGetTypeID(cf: *const c_void) -> u64;
        fn CFStringGetTypeID() -> u64;
        fn CFRelease(cf: *const c_void);
    }

    /// Reads a global-domain string preference via cfprefsd, e.g.
    /// `AppleInterfaceStyle` → `Some("Dark")`. `None` when the key is absent
    /// or not a string. Mirrors `defaults read -g <key>`.
    fn read_global_string(key: &str) -> Option<String> {
        unsafe {
            let key_c = CString::new(key).ok()?;
            let domain_c = CString::new(K_CF_PREFERENCES_ANY_APPLICATION).ok()?;
            let key_ref =
                CFStringCreateWithCString(std::ptr::null(), key_c.as_ptr(), K_CF_STRING_ENCODING_UTF8);
            if key_ref.is_null() {
                return None;
            }
            let domain_ref = CFStringCreateWithCString(
                std::ptr::null(),
                domain_c.as_ptr(),
                K_CF_STRING_ENCODING_UTF8,
            );
            if domain_ref.is_null() {
                CFRelease(key_ref);
                return None;
            }
            // Refresh the in-process cache so preference changes made by
            // System Settings / `defaults` are seen promptly.
            CFPreferencesAppSynchronize(domain_ref);
            let value = CFPreferencesCopyAppValue(key_ref, domain_ref);
            CFRelease(key_ref);
            CFRelease(domain_ref);
            if value.is_null() || CFGetTypeID(value) != CFStringGetTypeID() {
                return None; // absent, or unexpectedly a non-string
            }
            let mut buf: [c_char; 32] = [0; 32];
            let out = if CFStringGetCString(
                value as CFStringRef,
                buf.as_mut_ptr(),
                buf.len() as isize,
                K_CF_STRING_ENCODING_UTF8,
            ) {
                Some(CStr::from_ptr(buf.as_ptr()).to_string_lossy().into_owned())
            } else {
                None
            };
            CFRelease(value);
            out
        }
    }

    /// Current `AppleInterfaceStyle`: `Some("Dark")` in Dark Mode, `None` in
    /// Light Mode (and Auto appearance, see module docs).
    pub fn apple_interface_style() -> Option<String> {
        read_global_string("AppleInterfaceStyle")
    }
}

/// Visual channel mode inherited from the user's cava config `[output]`
/// section (`channels = mono|stereo`), parsed once per process.
///
/// cava semantics match cnmplayer's spectrum exactly: `stereo` mirrors the
/// bars with low frequencies in the center, `mono` draws a single row.
/// `None` when the config doesn't define it (missing / commented out /
/// unknown value) — callers then keep their own setting.
pub fn user_cava_channels() -> Option<CavaChannels> {
    static CACHE: OnceLock<Option<CavaChannels>> = OnceLock::new();
    *CACHE.get_or_init(parse_user_cava_channels)
}

fn parse_user_cava_channels() -> Option<CavaChannels> {
    let content = fs::read_to_string(user_cava_config_path()).ok()?;
    parse_channels_from(&content)
}

fn parse_channels_from(content: &str) -> Option<CavaChannels> {
    let mut in_output = false;
    for line in content.lines() {
        let t = line.trim();
        if t.starts_with('[') {
            in_output = t == "[output]";
            continue;
        }
        if !in_output || t.is_empty() || t.starts_with('#') || t.starts_with(';') {
            continue;
        }
        let Some((key, val)) = t.split_once('=') else {
            continue;
        };
        if key.trim() == "channels" {
            // Cut `;` comments first, then handle quotes like
            // `parse_cava_color` does: `mono`, `'stereo'`, `"Mono"` and
            // `'stereo'  # mirrored` all parse.
            let bare = val.split(';').next().unwrap_or("").trim();
            let flag = match bare.chars().next() {
                Some(q @ ('\'' | '"')) => bare[1..]
                    .find(q)
                    .map(|end| &bare[1..1 + end])
                    .unwrap_or_else(|| bare.trim_matches(q)),
                _ => bare.split_whitespace().next().unwrap_or(""),
            }
            .to_ascii_lowercase();
            return match flag.as_str() {
                "mono" => Some(CavaChannels::Mono),
                "stereo" => Some(CavaChannels::Stereo),
                _ => None,
            };
        }
    }
    None
}

fn find_cava_executable() -> Option<PathBuf> {
    // Resolution order:
    // 1) env var override
    // 2) bundled next to our executable or in ./third_party/cava/
    // 3) PATH fallback
    if let Some(p) = std::env::var_os("TMPLAYER_CAVA") {
        let p = PathBuf::from(p);
        if p.is_file() {
            return Some(p);
        }
    }

    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(exe_dir) = exe.parent() {
            candidates.push(exe_dir.join("cava"));
            candidates.push(exe_dir.join("third_party").join("cava").join("cava"));
        }
    }

    if let Ok(cwd) = std::env::current_dir() {
        candidates.push(cwd.join("third_party").join("cava").join("cava"));
    }

    for p in candidates {
        if p.is_file() {
            return Some(p);
        }
    }

    if which_in_path("cava").is_some() {
        return Some(PathBuf::from("cava"));
    }

    None
}

fn resolve_cava_executable() -> Result<PathBuf> {
    if let Some(p) = find_cava_executable() {
        return Ok(p);
    }

    Err(anyhow::anyhow!("cava not found"))
}

fn which_in_path(bin: &str) -> Option<PathBuf> {
    let paths = std::env::var_os("PATH")?;
    for p in std::env::split_paths(&paths) {
        let cand = p.join(bin);
        if cand.is_file() {
            return Some(cand);
        }
    }
    None
}

impl Drop for CavaRunner {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = fs::remove_file(&self.cfg_path);
    }
}

fn parse_frames_ascii(s: &str, bars: usize) -> Vec<Vec<f32>> {
    // ascii_max_range=1000, bar_delimiter=';'
    // Can be N values (one channel) or 2N values (two channels) on a single line.
    let mut vals: Vec<f32> = Vec::new();
    for part in s.split([';', '\n', '\r', ' ', '\t']) {
        if part.is_empty() {
            continue;
        }
        if let Ok(v) = part.parse::<u32>() {
            vals.push((v as f32 / 1000.0).clamp(0.0, 1.0));
        }
    }

    if bars == 0 {
        return Vec::new();
    }

    let mut out: Vec<Vec<f32>> = Vec::new();
    let mut idx = 0usize;
    while idx + bars <= vals.len() {
        let mut frame = vec![0.0f32; bars];
        for i in 0..bars {
            frame[i] = vals[idx + i];
        }
        out.push(frame);
        idx += bars;
    }

    out
}

fn temp_cfg_path() -> String {
    let pid = std::process::id();
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    format!("/tmp/tmplayer-cava-{pid}-{ts}.conf")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_hex_and_named_colors() {
        assert_eq!(parse_cava_color("'#59cc33'"), Some((0x59, 0xcc, 0x33)));
        assert_eq!(parse_cava_color("#D20F39"), Some((0xd2, 0x0f, 0x39)));
        assert_eq!(parse_cava_color("black"), Some((0, 0, 0)));
        assert_eq!(parse_cava_color("cyan"), Some((0, 255, 255)));
        // "default" / unknown → None so the caller keeps its fallback.
        assert_eq!(parse_cava_color("default"), None);
        assert_eq!(parse_cava_color("hotpink"), None);
    }

    #[test]
    fn gradient_flows_bottom_to_top() {
        // cava: gradient_color_1 = bottom. cnmplayer t: 0.0 = top, 1.0 = bottom.
        let scheme = CavaColorScheme {
            gradient: vec![(0, 0, 0), (255, 255, 255)], // black bottom, white top
            foreground: Some((1, 2, 3)),
        };
        assert_eq!(scheme.color_at(1.0), Some((0, 0, 0))); // bottom → color_1
        assert_eq!(scheme.color_at(0.0), Some((255, 255, 255))); // top → last color
        let mid = scheme.color_at(0.5).unwrap();
        assert!(mid.0 > 100 && mid.0 < 160, "mid gray expected, got {mid:?}");
    }

    #[test]
    fn parses_colors_with_inline_comments() {
        // The real-world format: quoted hex followed by a `# comment`.
        assert_eq!(
            parse_cava_color("'#4C4C4C'  # 明度 30%"),
            Some((0x4c, 0x4c, 0x4c))
        );
        assert_eq!(parse_cava_color("white ; light"), Some((255, 255, 255)));
        assert_eq!(parse_cava_color("cyan	# accent"), Some((0, 255, 255)));
        // Unquoted hex with a trailing comment stays parseable.
        assert_eq!(
            parse_cava_color("#D20F39 ; brand red"),
            Some((0xd2, 0x0f, 0x39))
        );
        assert_eq!(parse_cava_color("'#59cc33"), Some((0x59, 0xcc, 0x33)));
    }

    #[test]
    fn scheme_from_user_style_config() {
        // Mirrors a real user config: 10 stops, quoted values, inline comments.
        let mut cfg = String::from("[general]\nbars = 40\n\n[color]\ngradient = 1\ngradient_count = 10\n\n");
        let stops = [
            "#4C4C4C", "#535353", "#595959", "#666666", "#737373", "#8C8C8C", "#A0A0A0",
            "#B3B3B3", "#C0C0C0", "#CCCCCC",
        ];
        for (i, c) in stops.iter().enumerate() {
            cfg.push_str(&format!("gradient_color_{}  = '{}'  # stop\n", i + 1, c));
        }
        cfg.push_str("\n[smoothing]\n\n");
        let scheme = parse_color_scheme_from(&cfg).expect("scheme should parse");
        assert_eq!(scheme.gradient.len(), 10);
        assert_eq!(scheme.gradient[0], (0x4c, 0x4c, 0x4c)); // bottom
        assert_eq!(scheme.gradient[9], (0xcc, 0xcc, 0xcc)); // top
        // Dark at the bottom (t=1.0), light at the top (t=0.0).
        assert_eq!(scheme.color_at(1.0), Some((0x4c, 0x4c, 0x4c)));
        assert_eq!(scheme.color_at(0.0), Some((0xcc, 0xcc, 0xcc)));
        // A stop that fails to parse only drops that stop, not the scheme.
        cfg.push_str("gradient_color_11 = default\n");
        let scheme = parse_color_scheme_from(&cfg).expect("scheme survives bad stop");
        assert_eq!(scheme.gradient.len(), 10);
    }

    #[test]
    fn flat_foreground_when_no_gradient() {
        let flat = CavaColorScheme {
            gradient: Vec::new(),
            foreground: Some((9, 9, 9)),
        };
        assert_eq!(flat.color_at(0.3), Some((9, 9, 9)));
        let empty = CavaColorScheme {
            gradient: Vec::new(),
            foreground: None,
        };
        assert_eq!(empty.color_at(0.3), None);
    }

    #[test]
    fn parses_output_channels() {
        // The real-world config: plain value in [output].
        let cfg = "[general]\nbars = 40\n\n[output]\nmethod = raw\nchannels = mono\n";
        assert_eq!(parse_channels_from(cfg), Some(CavaChannels::Mono));
        // Quoted value with a trailing inline comment.
        let cfg = "[output]\nchannels = 'stereo'  # mirrored\n";
        assert_eq!(parse_channels_from(cfg), Some(CavaChannels::Stereo));
        // Case-insensitive.
        let cfg = "[output]\nchannels = Stereo\n";
        assert_eq!(parse_channels_from(cfg), Some(CavaChannels::Stereo));
        // Commented out → nothing to inherit.
        let cfg = "[output]\n; channels = stereo\n";
        assert_eq!(parse_channels_from(cfg), None);
        // [input] channels (a different knob) must not leak in.
        let cfg = "[input]\nchannels = mono\n\n[output]\nmethod = raw\n";
        assert_eq!(parse_channels_from(cfg), None);
        // Unknown value → keep the caller's setting.
        let cfg = "[output]\nchannels = auto\n";
        assert_eq!(parse_channels_from(cfg), None);
        // Later sections don't reset the section scan result.
        let cfg = "[output]\nchannels = mono\n\n[color]\nforeground = white\n";
        assert_eq!(parse_channels_from(cfg), Some(CavaChannels::Mono));
    }

    #[test]
    fn dark_mode_reverses_gradient_bottom_white_top_black() {
        // The user's Light-Mode scheme: dark at the bottom → light at the top.
        let stops = [
            "#4C4C4C", "#535353", "#595959", "#666666", "#737373", "#8C8C8C", "#A0A0A0",
            "#B3B3B3", "#C0C0C0", "#CCCCCC",
        ];
        let mut cfg = String::from("[color]\ngradient = 1\n");
        for (i, c) in stops.iter().enumerate() {
            cfg.push_str(&format!("gradient_color_{} = '{c}'\n", i + 1));
        }
        let light = parse_color_scheme_from(&cfg).expect("scheme should parse");

        // Dark Mode: reversed — white at the bottom, black at the top.
        let dark = adapt_scheme_to_appearance(light.clone(), true);
        assert_eq!(dark.color_at(1.0), Some((0xcc, 0xcc, 0xcc))); // bottom
        assert_eq!(dark.color_at(0.0), Some((0x4c, 0x4c, 0x4c))); // top
        // Light Mode keeps the config's own order.
        assert_eq!(light.color_at(1.0), Some((0x4c, 0x4c, 0x4c)));
        assert_eq!(light.color_at(0.0), Some((0xcc, 0xcc, 0xcc)));
    }

    #[test]
    fn dark_mode_leaves_flat_foreground_alone() {
        let flat = CavaColorScheme {
            gradient: Vec::new(),
            foreground: Some((9, 9, 9)),
        };
        let dark = adapt_scheme_to_appearance(flat, true);
        assert_eq!(dark.color_at(0.3), Some((9, 9, 9)));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn only_dark_style_counts_as_dark() {
        assert!(session_style_is_dark(Some("Dark")));
        assert!(session_style_is_dark(Some("dark")));
        // Light Mode and Auto-appearance daytime have no style at all.
        assert!(!session_style_is_dark(None));
        assert!(!session_style_is_dark(Some("Light")));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn session_style_lookup_does_not_panic() {
        // Smoke test for the FFI path: absent in Light Mode, `"Dark"` in
        // Dark Mode. Whatever comes back must be a short style string.
        if let Some(style) = macos_appearance::apple_interface_style() {
            assert!(!style.is_empty() && style.len() < 16, "got {style:?}");
        }
    }
}
