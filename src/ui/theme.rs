use ratatui::style::Color;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorCapability {
    TrueColor,
    Ansi256,
    NoColor,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThemeName {
    System,
    Latte,
    Frappe,
    Macchiato,
    Mocha,
}

impl ThemeName {
    pub fn from_str_or_system(raw: &str) -> Self {
        match raw.to_lowercase().as_str() {
            "latte" => Self::Latte,
            "frappe" => Self::Frappe,
            "macchiato" => Self::Macchiato,
            "mocha" => Self::Mocha,
            _ => Self::System,
        }
    }
}

#[derive(Debug, Clone, Copy)]
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

#[derive(Debug, Clone, Copy)]
pub struct Theme {
    #[allow(dead_code)]
    pub name: ThemeName,
    pub palette: ThemePalette,
    pub capability: ColorCapability,
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

    /// 按 t∈[0,1] 在两种主题色之间插值（脉冲动画等动态高亮，颜色全部来自主题）。
    pub fn blend(&self, a: (u8, u8, u8), b: (u8, u8, u8), t: f32) -> Color {
        let t = t.clamp(0.0, 1.0);
        let mix = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
        map_color(
            self.capability,
            (mix(a.0, b.0), mix(a.1, b.1), mix(a.2, b.2)),
        )
    }
}

impl Default for Theme {
    fn default() -> Self {
        Self {
            name: ThemeName::System,
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
