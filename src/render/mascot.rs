use crate::render::mascot_frames::{self, MascotFrame};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Color;
use std::time::Duration;

/// 蓄力：内容区被半格噪点逐个填满。
pub const CHARGE_DURATION: Duration = Duration::from_millis(1200);
/// 迸发：填满的噪点自下向上扫过并消失，扫过处即已是彩蛋内容。
pub const BURST_DURATION: Duration = Duration::from_millis(600);
/// 果冻：点击形象后的一轮 squash & stretch。
pub const JELLY_DURATION: Duration = Duration::from_millis(500);

/// 触发彩蛋所需的版本行点击次数。
pub const TRIGGER_CLICKS: u8 = 10;

/// 彩蛋激活后版本行显示的文字。
pub const MASCOT_CAPTION: &str = "₍^ >ヮ<^₎ .ᐟ.ᐟ";

/// 噪点以半个字符高度为一个像素：上下半格各自取色，由 `▀` 的前景/背景呈现。
pub const HALF_BLOCK: &str = "▀";

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

/// 把一帧直接写进 `buf` 的 `area`（左上对齐，超出部分裁掉）。
///
/// 帧里的颜色是 256 色索引，交由终端按自身调色板呈现；[`mascot_frames::NO_COLOR`]
/// 表示该槽位不着色，此时回退到调用方给的 `fallback_fg` / `fallback_bg`（弹窗配色）。
///
/// 唯一例外：背景不着色的空格视为透明、整格不写，下层（背景弹幕）因此能从
/// 形象的空隙里透出来。有字形的格照常整格覆盖，形象本身不会被弹幕穿透。
pub fn draw_frame(
    frame: &MascotFrame,
    buf: &mut Buffer,
    area: Rect,
    fallback_fg: Color,
    fallback_bg: Color,
) {
    let width = frame.width as usize;
    if width == 0 {
        return;
    }

    for (idx, ch) in frame.chars.chars().enumerate() {
        let col = (idx % width) as u16;
        let row = (idx / width) as u16;
        if col >= area.width || row >= area.height {
            continue;
        }
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
        if ch == ' ' && bg == mascot_frames::NO_COLOR {
            continue;
        }

        let Some(cell) = buf.cell_mut((area.x + col, area.y + row)) else {
            continue;
        };
        let mut sym = [0u8; 4];
        cell.set_symbol(ch.encode_utf8(&mut sym))
            .set_fg(indexed_or(fg, fallback_fg))
            .set_bg(indexed_or(bg, fallback_bg));
    }
}

/// 判断某格在蓄力进度 `progress`（0..=1）时是否已被填充。
///
/// 每格由散列得到一个固定的“入场时刻”，因此填充顺序看似随机但每次一致，
/// 且无需维护乱序表 —— 只要比较该格的入场时刻是否已被进度越过。
pub fn cell_filled(x: u16, y: u16, progress: f32) -> bool {
    let h = hash3(x as u64, y as u64, 0xF11);
    let threshold = (h & 0xFFFF) as f32 / 65535.0;
    threshold < progress.clamp(0.0, 1.0)
}

/// 单个噪点字符的上下半格取色，两半都必然点亮。
///
/// 用于蓄力填充与迸发扫过：那里需要的是实心噪点，稀疏与否由填充进度体现，
/// 而不是由密度体现。
pub fn noise_cell_colors(x: u16, y: u16, seed: u64) -> (Color, Color) {
    let upper = vivid_color(hash3(x as u64, y as u64 * 2, seed));
    let lower = vivid_color(hash3(x as u64, y as u64 * 2 + 1, seed));
    (upper, lower)
}

/// 逆时针沿边框环绕的噪点取色。
///
/// `perimeter_pos` 是该格在边框环上的位置（0 起，沿逆时针递增），`phase` 随时间
/// 推进使颜色沿环流动。
pub fn border_flow_color(perimeter_pos: u16, phase: u64) -> Color {
    // 取色索引沿环递进：相邻格颜色相近，整体呈现流动而非闪烁。
    let step = (perimeter_pos as u64).wrapping_add(phase);
    vivid_color(hash3(step, 0, 0x5EED))
}

fn indexed_or(code: u16, fallback: Color) -> Color {
    if code == mascot_frames::NO_COLOR {
        fallback
    } else {
        Color::Indexed(code as u8)
    }
}

/// 取 256 色立方体（16..=231）中的鲜艳色，避开首尾的暗色与灰阶。
fn vivid_color(h: u64) -> Color {
    Color::Indexed(16 + ((h >> 16) % 216) as u8)
}

/// 整数散列：噪点每帧要算数千次，避免浮点噪声函数的开销。
pub(crate) fn hash3(x: u64, y: u64, seed: u64) -> u64 {
    let mut h = x
        .wrapping_mul(0x9E37_79B9_7F4A_7C15)
        .wrapping_add(y.wrapping_mul(0xC2B2_AE3D_27D4_EB4F))
        .wrapping_add(seed.wrapping_mul(0x1656_67B1_9E37_79F9));
    h ^= h >> 33;
    h = h.wrapping_mul(0xFF51_AFD7_ED55_8CCD);
    h ^= h >> 33;
    h
}
