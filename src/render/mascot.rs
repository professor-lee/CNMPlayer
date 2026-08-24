use crate::render::mascot_frames::{self, MascotFrame};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use std::time::Duration;

/// 蓄力：版本行背景的彩色噪点由稀疏到密集，体现正在积蓄。
pub const CHARGE_DURATION: Duration = Duration::from_millis(1200);
/// 迸发：噪点带自下向上划过弹窗内容区并离开。
pub const BURST_DURATION: Duration = Duration::from_millis(600);
/// 果冻：点击形象后的一轮 squash & stretch。
pub const JELLY_DURATION: Duration = Duration::from_millis(500);

/// 触发彩蛋所需的版本行点击次数。
pub const TRIGGER_CLICKS: u8 = 10;

/// 彩蛋激活后版本行显示的文字。
pub const MASCOT_CAPTION: &str = "₍^ >ヮ<^₎ .ᐟ.ᐟ";

/// 噪点以半个字符高度为一个像素：上下半格各自取色，由 `▀` 的前景/背景呈现。
const HALF_BLOCK: &str = "▀";

/// 形象静止时的帧。
pub fn idle_frame() -> &'static MascotFrame {
    &mascot_frames::FRAMES[0]
}

/// 果冻动画进行到 `elapsed` 时应显示的帧；超出时长则回到静止帧。
pub fn jelly_frame(elapsed: Duration) -> &'static MascotFrame {
    if elapsed >= JELLY_DURATION {
        return idle_frame();
    }

    let t = elapsed.as_secs_f32() / JELLY_DURATION.as_secs_f32();
    let last = mascot_frames::FRAMES.len().saturating_sub(1);
    let idx = ((t * mascot_frames::FRAMES.len() as f32) as usize).min(last);
    &mascot_frames::FRAMES[idx]
}

/// 把一帧转成可直接交给 `Paragraph` 的文本行。
///
/// 帧里的颜色是 256 色索引，交由终端按自身调色板呈现；[`mascot_frames::NO_COLOR`]
/// 表示该槽位不着色，此时回退到调用方给的 `fallback_bg`（弹窗底色）。
pub fn frame_lines(frame: &MascotFrame, fallback_bg: Color) -> Vec<Line<'static>> {
    let width = frame.width as usize;
    if width == 0 {
        return Vec::new();
    }

    let mut lines = Vec::with_capacity(frame.height as usize);
    let mut spans: Vec<Span<'static>> = Vec::with_capacity(width);

    for (idx, ch) in frame.chars.chars().enumerate() {
        let fg = frame
            .fg
            .get(idx)
            .copied()
            .unwrap_or(mascot_frames::NO_COLOR);
        let bg = frame
            .bg
            .get(idx)
            .copied()
            .unwrap_or(mascot_frames::NO_COLOR);

        let mut style = Style::default().bg(indexed_or(bg, fallback_bg));
        if fg != mascot_frames::NO_COLOR {
            style = style.fg(Color::Indexed(fg as u8));
        }
        spans.push(Span::styled(ch.to_string(), style));

        if spans.len() == width {
            lines.push(Line::from(std::mem::take(&mut spans)));
            spans.reserve(width);
        }
    }

    if !spans.is_empty() {
        lines.push(Line::from(spans));
    }
    lines
}

/// 一行彩色噪点。
///
/// `density` 为 0..=1：越大越密，用来体现蓄力过程；`seed` 随时间推进使噪点流动。
/// 每个字符单元含上下两个半格像素，因此纵向分辨率是半个字符高度。
pub fn noise_line(width: u16, seed: u64, density: f32, fallback_bg: Color) -> Line<'static> {
    let density = density.clamp(0.0, 1.0);
    let mut spans = Vec::with_capacity(width as usize);

    for x in 0..width {
        let upper = noise_color(x as u64, 0, seed, density);
        let lower = noise_color(x as u64, 1, seed, density);
        spans.push(Span::styled(
            HALF_BLOCK.to_string(),
            Style::default()
                .fg(upper.unwrap_or(fallback_bg))
                .bg(lower.unwrap_or(fallback_bg)),
        ));
    }

    Line::from(spans)
}

fn indexed_or(code: u16, fallback: Color) -> Color {
    if code == mascot_frames::NO_COLOR {
        fallback
    } else {
        Color::Indexed(code as u8)
    }
}

/// 取某个半格像素的噪点颜色；返回 `None` 表示该像素本轮不点亮。
fn noise_color(x: u64, half: u64, seed: u64, density: f32) -> Option<Color> {
    let h = hash3(x, half, seed);

    // 低位决定是否点亮，与取色位错开，避免密度和颜色相关。
    let lit = (h & 0xFFFF) as f32 / 65535.0;
    if lit > density {
        return None;
    }

    // 取 256 色立方体（16..=231）中的鲜艳色，避开首尾的暗色与灰阶。
    let idx = 16 + ((h >> 16) % 216) as u8;
    Some(Color::Indexed(idx))
}

/// 整数散列：噪点每帧要算数千次，避免浮点噪声函数的开销。
fn hash3(x: u64, y: u64, seed: u64) -> u64 {
    let mut h = x
        .wrapping_mul(0x9E37_79B9_7F4A_7C15)
        .wrapping_add(y.wrapping_mul(0xC2B2_AE3D_27D4_EB4F))
        .wrapping_add(seed.wrapping_mul(0x1656_67B1_9E37_79F9));
    h ^= h >> 33;
    h = h.wrapping_mul(0xFF51_AFD7_ED55_8CCD);
    h ^= h >> 33;
    h
}
