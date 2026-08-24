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
    pub fn from_str_or_system(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "latte" => Self::Latte,
            "frappe" => Self::Frappe,
            "macchiato" => Self::Macchiato,
            "mocha" => Self::Mocha,
            _ => Self::System,
        }
    }

    #[allow(dead_code)]
    pub fn as_label(&self) -> &'static str {
        match self {
            ThemeName::System => "System",
            ThemeName::Latte => "Latte",
            ThemeName::Frappe => "Frappe",
            ThemeName::Macchiato => "Macchiato",
            ThemeName::Mocha => "Mocha",
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ThemePalette {
    pub text: (u8, u8, u8),
    pub subtext: (u8, u8, u8),
    pub base: (u8, u8, u8),
    pub surface: (u8, u8, u8),
    #[allow(dead_code)]
    pub buff: (u8, u8, u8),
    pub accent: (u8, u8, u8),
    pub accent2: (u8, u8, u8),
    pub accent3: (u8, u8, u8),
}

#[derive(Debug, Clone, Copy)]
pub struct Theme {
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
    #[allow(dead_code)]
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

    /// 将主题色向白色方向提亮 amount∈[0,1]：脉冲动画“稍浅一点”的效果，
    /// 基础颜色来自主题，明暗主题均自适应（不硬编码颜色）。
    pub fn lighten(&self, color: (u8, u8, u8), amount: f32) -> Color {
        let amount = amount.clamp(0.0, 1.0);
        let mix = |x: u8| (x as f32 + (255.0 - x as f32) * amount).round() as u8;
        map_color(self.capability, (mix(color.0), mix(color.1), mix(color.2)))
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

fn map_color(cap: ColorCapability, t: (u8, u8, u8)) -> Color {
    match cap {
        ColorCapability::TrueColor => Color::Rgb(t.0, t.1, t.2),
        ColorCapability::Ansi256 => Color::Indexed(rgb_to_ansi256(t.0, t.1, t.2)),
        ColorCapability::NoColor => Color::Reset,
    }
}

fn rgb_to_ansi256(r: u8, g: u8, b: u8) -> u8 {
    // 6x6x6 color cube, 16..231
    let r6 = (r as u16 * 5 / 255) as u8;
    let g6 = (g as u16 * 5 / 255) as u8;
    let b6 = (b as u16 * 5 / 255) as u8;
    16 + 36 * r6 + 6 * g6 + b6
}
