//! about 彩蛋的背景弹幕：内容区每行一条车道，弹幕自右向左无限滚动。
//!
//! 无状态：每条弹幕的文字、位置、颜色都由「种子 + 车道号 + 槽位号」散列得出，
//! 按经过时间直接算出当前位置。不维护弹幕列表，`AboutEasterEgg` 因此保持 `Copy`。
//!
//! 车道被切成等宽槽位，每个槽位至多放一条弹幕；弹幕在槽位内随机错位但不越过
//! 槽位右侧的最小间隙，同车道速度恒定，所以相邻弹幕永不重叠。

use crate::render::mascot::{self, hash3};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Color;
use std::time::Duration;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// 文字弹幕语料。数字与拉丁字母弹出时随机以大字、上标或下标出现（见 [`Script`]），
/// 这里只存大字原文。刻意不收 emoji、从右往左书写的文字和带组合符的文字：
/// 它们在终端里的列宽不稳定，会让整行错位。
pub const CORPUS: &[&str] = &[
    // B站 / 中文
    "2333333",
    "23333",
    "23333333333",
    "6666666",
    "66666",
    "666666",
    "awsl",
    "yyds",
    "太好听了",
    "太牛了！！",
    "太厉害了",
    "厉害了",
    "UP牛批",
    "下次一定",
    "前方高能",
    "名场面",
    "泪目",
    "爷青回",
    "单曲循环",
    "耳朵怀孕了",
    "神曲",
    "哈哈哈哈",
    "绝了",
    "xswl",
    // Niconico
    "wwwwww",
    "草",
    "8888888",
    "kksk",
    "ここすき",
    "うぽつ",
    "エモい",
    // 英语 / Twitch
    "lol",
    "LMAO",
    "W",
    "GG",
    "Pog",
    "KEKW",
    "F",
    "based",
    "banger",
    "on repeat",
    "10/10",
    "+1",
    // 韩语
    "ㅋㅋㅋㅋㅋ",
    "ㅎㅎㅎ",
    "대박",
    "미쳤다",
    // 其他语言的“哈哈哈”
    "jajaja",
    "kkkkk",
    "mdr",
    "555",
    "xD",
    "ахаха",
    "hahaha",
];

/// 颜文字语料：原样显示，不做上下标变换（会把五官变形）。
pub const KAOMOJI: &[&str] = &[
    // 古早梗
    "(╯°□°）╯︵ ┻━┻",
    "┬─┬ノ( º _ ºノ)",
    "¯\\_(ツ)_/¯",
    "orz",
    "OTL",
    "m(_ _)m",
    "_(:3」∠)_",
    "(´・ω・`)",
    "(・∀・)",
    "( ﾟ∀ﾟ)",
    "(　ﾟ∀ﾟ)o彡ﾟ",
    "Σ(ﾟДﾟ)",
    "(ﾟДﾟ≡ﾟДﾟ)",
    "(｀・ω・´)",
    "(*´∀`)~♥",
    "(￣▽￣)",
    "o(*￣▽￣*)ブ",
    "ヽ(✿ﾟ▽ﾟ)ノ",
    "(=^･ω･^=)",
    "(*/ω＼*)",
    "(っ´ω`c)",
    "(°ー°〃)",
    "(⊙o⊙)",
    "(ง •_•)ง",
    "ಠ_ಠ",
    "\\(^o^)/",
    "^_^",
    "T_T",
    "QAQ",
    "TAT",
    "=_=",
    "-_-||",
    ">_<",
    "(づ｡◕‿‿◕｡)づ",
    "ʕ•ᴥ•ʔ",
    // 与形象同款的小字系，形象自己也来发一条
    mascot::MASCOT_CAPTION,
    "₍ᐢ..ᐢ₎",
    "₍^. .^₎⟆",
    "૮₍ ˃ ⤙ ˂ ₎ა",
    "૮ ˶ᵔ ᵕ ᵔ˶ ა",
    "(˶ᵔ ᵕ ᵔ˶)",
    "(ᵔᴥᵔ)",
    "(⸝⸝⸝>﹏<⸝⸝⸝)",
    "ᐠ( ᐛ )ᐟ",
    "(˃ᴗ˂)",
    "(*ˊᵕˋ*)",
    "⸜(｡˃ ᵕ ˂ )⸝♡",
    "₍₍ ◝(●˙꒳˙●)◜ ₎₎",
    "(ᗒᗨᗕ)",
    "(っ˘ω˘ς)",
    "(ﾉ>ω<)ﾉ",
    "(｡>﹏<)",
    "(≧∇≦)ﾉ",
    "(〃＞＿＜;〃)",
    "ᕙ(`▿´)ᕗ",
];

/// 抽中颜文字的概率（百分比），其余抽文字弹幕。
const KAOMOJI_PERCENT: u64 = 30;

/// 文字弹幕的字号：大字原样，或整条换成上标/下标小字。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Script {
    Normal,
    Super,
    Sub,
}

impl Script {
    /// 把一个字符换成该字号；没有对应小字的字符原样返回。
    pub fn apply(self, ch: char) -> char {
        match self {
            Script::Normal => ch,
            Script::Super => superscript(ch).unwrap_or(ch),
            Script::Sub => subscript(ch).unwrap_or(ch),
        }
    }
}

/// 上标小字。大写字母没有上标形式时换成对应的小写上标（C→ᶜ、X→ˣ…）；
/// 只有 q/Q 两种都没有。
fn superscript(ch: char) -> Option<char> {
    const DIGITS: [char; 10] = ['⁰', '¹', '²', '³', '⁴', '⁵', '⁶', '⁷', '⁸', '⁹'];
    const LOWER: [char; 26] = [
        'ᵃ', 'ᵇ', 'ᶜ', 'ᵈ', 'ᵉ', 'ᶠ', 'ᵍ', 'ʰ', 'ⁱ', 'ʲ', 'ᵏ', 'ˡ', 'ᵐ', 'ⁿ', 'ᵒ', 'ᵖ', '\0', 'ʳ',
        'ˢ', 'ᵗ', 'ᵘ', 'ᵛ', 'ʷ', 'ˣ', 'ʸ', 'ᶻ',
    ];
    const UPPER: [char; 26] = [
        'ᴬ', 'ᴮ', '\0', 'ᴰ', 'ᴱ', '\0', 'ᴳ', 'ᴴ', 'ᴵ', 'ᴶ', 'ᴷ', 'ᴸ', 'ᴹ', 'ᴺ', 'ᴼ', 'ᴾ', '\0',
        'ᴿ', '\0', 'ᵀ', 'ᵁ', 'ⱽ', 'ᵂ', '\0', '\0', '\0',
    ];
    let mapped = match ch {
        '0'..='9' => DIGITS[ch as usize - '0' as usize],
        'a'..='z' => LOWER[ch as usize - 'a' as usize],
        'A'..='Z' => match UPPER[ch as usize - 'A' as usize] {
            '\0' => LOWER[ch as usize - 'A' as usize],
            c => c,
        },
        '+' => '⁺',
        '-' => '⁻',
        '=' => '⁼',
        '(' => '⁽',
        ')' => '⁾',
        _ => return None,
    };
    (mapped != '\0').then_some(mapped)
}

/// 下标小字。Unicode 只给了部分小写字母，大写字母按小写查。
fn subscript(ch: char) -> Option<char> {
    const DIGITS: [char; 10] = ['₀', '₁', '₂', '₃', '₄', '₅', '₆', '₇', '₈', '₉'];
    let mapped = match ch.to_ascii_lowercase() {
        d @ '0'..='9' => DIGITS[d as usize - '0' as usize],
        'a' => 'ₐ',
        'e' => 'ₑ',
        'h' => 'ₕ',
        'i' => 'ᵢ',
        'j' => 'ⱼ',
        'k' => 'ₖ',
        'l' => 'ₗ',
        'm' => 'ₘ',
        'n' => 'ₙ',
        'o' => 'ₒ',
        'p' => 'ₚ',
        'r' => 'ᵣ',
        's' => 'ₛ',
        't' => 'ₜ',
        'u' => 'ᵤ',
        'v' => 'ᵥ',
        'x' => 'ₓ',
        '+' => '₊',
        '-' => '₋',
        '=' => '₌',
        '(' => '₍',
        ')' => '₎',
        _ => return None,
    };
    Some(mapped)
}

/// 为一条文字弹幕挑字号（大字 50%、上标 30%、下标 20%）。
///
/// 整条要么全换要么不换：上标要求至少一个字符有上标形式；下标要求每个数字/字母都有
/// 下标形式（否则会出现半大半小）。条件不满足就退回大字。
fn pick_script(text: &str, roll: u64) -> Script {
    let wanted = match roll % 10 {
        0..=4 => return Script::Normal,
        5..=7 => Script::Super,
        _ => Script::Sub,
    };
    let ok = match wanted {
        Script::Super => text.chars().any(|c| superscript(c).is_some()),
        _ => {
            text.chars().any(|c| subscript(c).is_some())
                && text
                    .chars()
                    .filter(char::is_ascii_alphanumeric)
                    .all(|c| subscript(c).is_some())
        }
    };
    if ok { wanted } else { Script::Normal }
}

/// 每个槽位的宽度（列）。
const SLOT: i64 = 24;
/// 同车道相邻弹幕之间至少留出的空列。
const MIN_GAP: i64 = 3;
/// 同一密度段包含的槽位数：密度按段随机，弹幕因此时密时疏。
const DENSITY_BLOCK: i64 = 4;

const LANE_SALT: u64 = 0xDA_4A_C0;
const DENSITY_SALT: u64 = 0xDE_45_17;
const TEXT_SALT: u64 = 0x7E_47;
const SCRIPT_SALT: u64 = 0x5C_21;

/// 弹幕配色：大部分用暗色，少量点缀主题强调色，作为背景不抢形象。
#[derive(Debug, Clone, Copy)]
pub struct Palette {
    pub base: Color,
    pub accents: [Color; 3],
}

/// 一条弹幕在当前时刻的位置：`x` 是相对内容区左缘的列，可能为负或超出右缘。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Bullet {
    pub lane: u16,
    pub x: i64,
    pub text: &'static str,
    /// 文字弹幕的字号；颜文字恒为 [`Script::Normal`]。
    pub script: Script,
    /// 0 表示用基础色，1..=3 表示第几种强调色。
    pub accent: u8,
}

/// 车道的滚动速度（列/秒），10..=18。
fn lane_speed(seed: u64, lane: u16) -> u64 {
    10 + hash3(lane as u64, 0, seed ^ LANE_SALT) % 9
}

/// 槽位里放哪条弹幕、用什么字号。
fn pick_text(seed: u64, lane: u16, slot: i64) -> (&'static str, Script) {
    let h = hash3(lane as u64, slot as u64, seed ^ SCRIPT_SALT);
    if h % 100 < KAOMOJI_PERCENT {
        let text = KAOMOJI[((h >> 8) % KAOMOJI.len() as u64) as usize];
        (text, Script::Normal)
    } else {
        let text = CORPUS[((h >> 8) % CORPUS.len() as u64) as usize];
        (text, pick_script(text, h >> 32))
    }
}

/// 某个槽位是否有弹幕。概率按密度段随机（15%..=90%），弹幕因此时密时疏。
fn slot_occupied(seed: u64, lane: u16, slot: i64) -> bool {
    let block = slot.div_euclid(DENSITY_BLOCK) as u64;
    let density = 15 + hash3(lane as u64, block, seed ^ DENSITY_SALT) % 76;
    hash3(lane as u64, slot as u64, seed) % 100 < density
}

/// 当前时刻内容区（`width` 列 × `lanes` 行）里可见的全部弹幕。
///
/// 第 `k` 个槽位在「世界坐标」上从 `k * SLOT` 列起，屏幕位置 = 世界坐标 − 已滚动列数。
/// 世界坐标从 0 起铺满，所以彩蛋一出现屏幕上就已经有弹幕，不会从空屏开始。
pub fn visible_bullets(seed: u64, elapsed: Duration, width: u16, lanes: u16) -> Vec<Bullet> {
    let mut out = Vec::new();
    let width = width as i64;
    let millis = elapsed.as_millis() as u64;
    for lane in 0..lanes {
        let scrolled = (millis * lane_speed(seed, lane) / 1000) as i64;
        // 从右缘还在屏幕内的第一个槽位起，到左缘越过屏幕右缘为止。
        let mut slot = scrolled.div_euclid(SLOT);
        loop {
            let slot_left = slot * SLOT - scrolled;
            if slot_left >= width {
                break;
            }
            if slot_occupied(seed, lane, slot) {
                let h = hash3(lane as u64, slot as u64, seed ^ TEXT_SALT);
                let (text, script) = pick_text(seed, lane, slot);
                let room = (SLOT - MIN_GAP - text.width() as i64).max(0);
                let jitter = ((h >> 20) % (room as u64 + 1)) as i64;
                let accent = if (h >> 40).is_multiple_of(7) {
                    1 + ((h >> 44) % 3) as u8
                } else {
                    0
                };
                out.push(Bullet {
                    lane,
                    x: slot_left + jitter,
                    text,
                    script,
                    accent,
                });
            }
            slot += 1;
        }
    }
    out
}

/// 把当前时刻的弹幕写进 `buf` 的 `area`，只改字形与前景色，背景保留弹窗底色。
///
/// 宽字符（CJK、韩文占 2 列）只在两列都落在 `area` 内时才画：
/// 左右边缘只露出半个字时整字跳过，避免残影与压到边框。
pub fn draw(buf: &mut Buffer, area: Rect, seed: u64, elapsed: Duration, palette: Palette) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let width = area.width as i64;
    for bullet in visible_bullets(seed, elapsed, area.width, area.height) {
        let color = match bullet.accent {
            0 => palette.base,
            n => palette.accents[(n - 1) as usize],
        };
        let y = area.y + bullet.lane;
        let mut col = bullet.x;
        for ch in bullet.text.chars().map(|c| bullet.script.apply(c)) {
            let w = ch.width().unwrap_or(0) as i64;
            if w == 0 {
                continue;
            }
            if col >= 0 && col + w <= width {
                let x = area.x + col as u16;
                let mut sym = [0u8; 4];
                if let Some(cell) = buf.cell_mut((x, y)) {
                    cell.set_symbol(ch.encode_utf8(&mut sym)).set_fg(color);
                }
                // 宽字符的后续列由终端占用；与 ratatui 自身约定一致，写成空格。
                for extra in 1..w {
                    if let Some(cell) = buf.cell_mut((x + extra as u16, y)) {
                        cell.set_symbol(" ");
                    }
                }
            }
            col += w;
        }
    }
}

#[cfg(test)]
mod tests;
