use crate::render::motion::{Curve, Transition};
use ratatui::style::Color;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorCapability {
    TrueColor,
    Ansi256,
    NoColor,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ThemePalette {
    pub text: (u8, u8, u8),
    pub subtext: (u8, u8, u8),
    pub base: (u8, u8, u8),
    pub surface: (u8, u8, u8),
    pub buff: (u8, u8, u8),
    pub accent: (u8, u8, u8),
    pub accent2: (u8, u8, u8),
    pub accent3: (u8, u8, u8),
}

/// 主题不再由枚举穷举：`name` 是 `themes/<key>.toml` 的 key（加载失败的
/// 调用方回退默认主题）。
#[derive(Debug, Clone)]
pub struct Theme {
    #[allow(dead_code)]
    pub name: String,
    pub palette: ThemePalette,
    pub capability: ColorCapability,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct ThemeTransition {
    from: ThemePalette,
    to: ThemePalette,
    motion: Transition,
}

impl ThemeTransition {
    pub(crate) fn new(from: ThemePalette, to: ThemePalette, now: Instant) -> Self {
        let mut motion = Transition::new(0.0);
        motion.retarget(1.0, now, Duration::from_millis(200), Curve::EaseInOut);
        Self { from, to, motion }
    }

    pub(crate) fn tick(&mut self, now: Instant) -> ThemePalette {
        self.motion.tick(now);
        ThemePalette::mix(self.from, self.to, self.motion.value())
    }

    pub(crate) fn is_running(&self) -> bool {
        self.motion.is_running()
    }

    pub(crate) fn target(&self) -> ThemePalette {
        self.to
    }
}

pub(crate) fn start_theme_transition(
    current: &mut Theme,
    transition: &mut Option<ThemeTransition>,
    next: Theme,
    now: Instant,
) {
    let from = current.palette;
    if next.capability == ColorCapability::NoColor || from == next.palette {
        *current = next;
        *transition = None;
        return;
    }

    let to = next.palette;
    *current = next;
    current.palette = from;
    *transition = Some(ThemeTransition::new(from, to, now));
}

impl ThemePalette {
    fn mix(from: Self, to: Self, amount: f32) -> Self {
        Self {
            text: mix_rgb(from.text, to.text, amount),
            subtext: mix_rgb(from.subtext, to.subtext, amount),
            base: mix_rgb(from.base, to.base, amount),
            surface: mix_rgb(from.surface, to.surface, amount),
            buff: mix_rgb(from.buff, to.buff, amount),
            accent: mix_rgb(from.accent, to.accent, amount),
            accent2: mix_rgb(from.accent2, to.accent2, amount),
            accent3: mix_rgb(from.accent3, to.accent3, amount),
        }
    }
}

fn mix_rgb(from: (u8, u8, u8), to: (u8, u8, u8), amount: f32) -> (u8, u8, u8) {
    let amount = amount.clamp(0.0, 1.0);
    let mix = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * amount).round() as u8;
    (mix(from.0, to.0), mix(from.1, to.1), mix(from.2, to.2))
}

impl Theme {
    pub fn color_text(&self) -> Color {
        map_color(self.capability, self.palette.text)
    }

    pub fn color_subtext(&self) -> Color {
        map_color(self.capability, self.palette.subtext)
    }

    pub fn color_base(&self) -> Color {
        map_color(self.capability, self.palette.base)
    }

    pub fn color_surface(&self) -> Color {
        map_color(self.capability, self.palette.surface)
    }

    pub fn color_buff(&self) -> Color {
        map_color(self.capability, self.palette.buff)
    }

    pub fn color_accent(&self) -> Color {
        map_color(self.capability, self.palette.accent)
    }

    pub fn color_accent2(&self) -> Color {
        map_color(self.capability, self.palette.accent2)
    }

    pub fn color_accent3(&self) -> Color {
        map_color(self.capability, self.palette.accent3)
    }

    pub fn lighten(&self, color: (u8, u8, u8), amount: f32) -> Color {
        let amount = amount.clamp(0.0, 1.0);
        let mix = |x: u8| (x as f32 + (255.0 - x as f32) * amount).round() as u8;
        map_color(self.capability, (mix(color.0), mix(color.1), mix(color.2)))
    }

    pub(crate) fn mix(&self, from: (u8, u8, u8), to: (u8, u8, u8), amount: f32) -> Color {
        map_color(self.capability, mix_rgb(from, to, amount))
    }
}

impl Default for Theme {
    fn default() -> Self {
        Self {
            name: "system".to_string(),
            capability: detect_color_capability(),
            palette: ThemePalette {
                text: (255, 255, 255),
                subtext: (170, 170, 170),
                base: (0, 0, 0),
                surface: (32, 32, 32),
                buff: (42, 42, 42),
                accent: (255, 255, 255),
                accent2: (255, 255, 255),
                accent3: (255, 255, 255),
            },
        }
    }
}

pub fn detect_color_capability() -> ColorCapability {
    let colorterm = std::env::var("COLORTERM")
        .unwrap_or_default()
        .to_lowercase();
    if colorterm.contains("truecolor") || colorterm.contains("24bit") {
        return ColorCapability::TrueColor;
    }

    let term = std::env::var("TERM").unwrap_or_default().to_lowercase();
    if term.contains("256color") {
        return ColorCapability::Ansi256;
    }

    ColorCapability::NoColor
}

fn map_color(cap: ColorCapability, rgb: (u8, u8, u8)) -> Color {
    match cap {
        ColorCapability::TrueColor => Color::Rgb(rgb.0, rgb.1, rgb.2),
        ColorCapability::Ansi256 => Color::Indexed(rgb_to_ansi256(rgb.0, rgb.1, rgb.2)),
        ColorCapability::NoColor => Color::Reset,
    }
}

fn rgb_to_ansi256(r: u8, g: u8, b: u8) -> u8 {
    let r6 = (r as u16 * 5 / 255) as u8;
    let g6 = (g as u16 * 5 / 255) as u8;
    let b6 = (b as u16 * 5 / 255) as u8;
    16 + 36 * r6 + 6 * g6 + b6
}
