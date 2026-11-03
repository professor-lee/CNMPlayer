use super::*;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Color;
use std::time::Duration;
use unicode_width::UnicodeWidthStr;

const PALETTE: Palette = Palette {
    base: Color::Gray,
    accents: [Color::Red, Color::Green, Color::Blue],
};

/// 语料允许的最大显示宽度：再宽就挤掉槽位里的最小间隙。
const MAX_TEXT_WIDTH: usize = (SLOT - MIN_GAP) as usize;

/// 语料超宽会挤掉槽位里的最小间隙，相邻弹幕就可能贴在一起。
#[test]
fn every_corpus_entry_fits_its_slot() {
    for text in CORPUS {
        assert!(
            text.width() <= MAX_TEXT_WIDTH,
            "{text:?} 宽 {} 列，超过 {MAX_TEXT_WIDTH}",
            text.width()
        );
    }
}

/// 同车道相邻弹幕至少隔 MIN_GAP 列，任意时刻、任意种子都成立。
#[test]
fn bullets_in_one_lane_never_overlap() {
    for seed in [0, 1, 42, 0xDEAD_BEEF, u64::MAX] {
        for ms in (0..120_000).step_by(733) {
            let bullets = visible_bullets(seed, Duration::from_millis(ms), 68, 20);
            for lane in 0..20 {
                let mut spans: Vec<(i64, i64)> = bullets
                    .iter()
                    .filter(|b| b.lane == lane)
                    .map(|b| (b.x, b.x + b.text.width() as i64))
                    .collect();
                spans.sort();
                for pair in spans.windows(2) {
                    assert!(
                        pair[1].0 - pair[0].1 >= MIN_GAP,
                        "seed {seed} t {ms}ms 车道 {lane}: {pair:?}"
                    );
                }
            }
        }
    }
}

/// 弹幕无限：很久以后屏幕上仍然有弹幕，且一开始就不是空屏。
#[test]
fn danmaku_never_runs_out() {
    for ms in [0, 60_000, 3_600_000, 86_400_000] {
        let bullets = visible_bullets(7, Duration::from_millis(ms), 68, 20);
        let on_screen = bullets
            .iter()
            .filter(|b| b.x < 68 && b.x + b.text.width() as i64 > 0)
            .count();
        assert!(on_screen >= 5, "t={ms}ms 只有 {on_screen} 条");
    }
}

/// 弹幕自右向左：同一条弹幕过一会儿 x 变小。
#[test]
fn bullets_move_right_to_left() {
    let lane0 = |ms| -> Vec<i64> {
        visible_bullets(3, Duration::from_millis(ms), 68, 1)
            .iter()
            .map(|b| b.x)
            .collect()
    };
    let (before, after) = (lane0(10_000), lane0(10_500));
    // 0.5s 内至少移动 3 列，不到一个槽位，因此按序比对的是同一批弹幕的后继位置。
    let moved = before
        .iter()
        .any(|x| after.iter().any(|y| (3..=7).contains(&(x - y))));
    assert!(moved, "before {before:?} after {after:?}");
}

/// 左右边缘只露出半个宽字符时整字跳过，不留下半个字、不写到区域外。
#[test]
fn wide_chars_cut_by_edges_are_skipped() {
    let area = Rect::new(1, 0, 30, 20);
    let outer = Rect::new(0, 0, 32, 20);
    for seed in 0..40u64 {
        for ms in (0..20_000).step_by(97) {
            let mut buf = Buffer::filled(outer, ratatui::buffer::Cell::new("|"));
            for y in 0..20 {
                for x in area.x..area.x + area.width {
                    buf[(x, y)].set_symbol(" ");
                }
            }
            draw(&mut buf, area, seed, Duration::from_millis(ms), PALETTE);
            for y in 0..20 {
                assert_eq!(buf[(0, y)].symbol(), "|", "左边框被写");
                assert_eq!(buf[(31, y)].symbol(), "|", "右边框被写");
                // 最右一列若是宽字符，它的第二列就落在区域外。
                assert!(buf[(30, y)].symbol().width() <= 1, "右缘留下半个宽字符");
            }
        }
    }
}
