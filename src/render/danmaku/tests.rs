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
    for text in CORPUS.iter().chain(KAOMOJI) {
        assert!(
            text.width() <= MAX_TEXT_WIDTH,
            "{text:?} 宽 {} 列，超过 {MAX_TEXT_WIDTH}",
            text.width()
        );
    }
}

/// 换成小字不改变列宽：排布按原文宽度算，变宽就会压到下一条弹幕。
#[test]
fn small_script_keeps_column_width() {
    for text in CORPUS {
        for script in [Script::Super, Script::Sub] {
            let mapped: String = text.chars().map(|c| script.apply(c)).collect();
            assert_eq!(mapped.width(), text.width(), "{text:?} → {mapped:?}");
        }
    }
}

/// 上标覆盖全部数字和字母（大写缺上标时换小写上标），只有 q/Q 没有；
/// 选中下标的弹幕里每个数字/字母都真的变成了下标，不会半大半小。
#[test]
fn small_script_covers_letters_and_whole_bullets() {
    for c in ('0'..='9').chain('a'..='z').chain('A'..='Z') {
        let mapped = Script::Super.apply(c);
        assert_eq!(mapped == c, c.eq_ignore_ascii_case(&'q'), "{c} → {mapped}");
    }
    assert_eq!(Script::Super.apply('L'), 'ᴸ');
    assert_eq!(Script::Super.apply('X'), 'ˣ');

    for text in CORPUS {
        for roll in 0..10 {
            if pick_script(text, roll) == Script::Sub {
                assert!(
                    text.chars()
                        .filter(char::is_ascii_alphanumeric)
                        .all(|c| Script::Sub.apply(c) != c),
                    "{text:?} 下标后仍有大字"
                );
            }
        }
    }
    assert_eq!(pick_script("GG", 9), Script::Normal, "g 没有下标，退回大字");
    assert_eq!(pick_script("23333", 9), Script::Sub);
    assert_eq!(pick_script("太好听了", 6), Script::Normal, "没有可换的字符");
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

/// 弹幕自右向左，每条车道速度落在 10..=18 列/秒。
#[test]
fn bullets_move_right_to_left_at_lane_speed() {
    for seed in [3u64, 99, 0xC0FFEE] {
        for lane in 0..20u16 {
            let speed = lane_speed(seed, lane) as i64;
            assert!((10..=18).contains(&speed), "车道 {lane} 速度 {speed}");

            let at = |ms| visible_bullets(seed, Duration::from_millis(ms), 68, lane + 1);
            let before: Vec<Bullet> = at(10_000).into_iter().filter(|b| b.lane == lane).collect();
            let after: Vec<Bullet> = at(11_000).into_iter().filter(|b| b.lane == lane).collect();
            // 同一条弹幕 1 秒后正好左移 speed 列（文字、字号都不变）。
            // 只比 1 秒后仍在屏幕内的：更左的可能已随槽位滑出可见范围。
            for b in before.iter().filter(|b| b.x - speed >= 0) {
                assert!(
                    after
                        .iter()
                        .any(|a| a.x == b.x - speed && a.text == b.text && a.script == b.script),
                    "seed {seed} 车道 {lane}: {b:?} 没有出现在左边 {speed} 列处"
                );
            }
        }
    }
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
