use super::*;
use crate::app::download::{DownloadState, state_glyph};

/// 收藏爱心（Nerd Font PUA）：实心 = 已收藏，空心 = 未收藏。
/// 用码位转义书写，免得复制粘贴时被编辑器换成别的字形。
const HEART_LIKED: &str = "\u{f004}";
const HEART_UNLIKED: &str = "\u{f08a}";

/// 爱心命中区：爱心贴左列右端（与 `compose_left_right_line` 的右对齐同源），
/// 宽度不足时不登记，避免留下点不动的隐形按钮。
///
/// 渲染路径现在直接调 `right_suffix_hits`（要顺带登记下载按钮），
/// 这个包装只留给单测。
fn heart_hit_rect(left_rect: Rect, heart: &str) -> Option<HitRect> {
    right_suffix_hits(left_rect, heart, None).1
}

fn row(x: u16, width: u16) -> Rect {
    Rect {
        x,
        y: 3,
        width,
        height: 1,
    }
}

/// 爱心命中区必须正好落在 `compose_left_right_line` 画出的那一格上。
#[test]
fn heart_hit_rect_matches_the_right_aligned_glyph() {
    for width in 1..14u16 {
        let area = row(7, width);
        for heart in [HEART_LIKED, HEART_UNLIKED] {
            let line = compose_left_right_line("a fairly long title", heart, width as usize);
            let hit = heart_hit_rect(area, heart).expect("宽度够时应登记命中区");

            assert!(line.ends_with(heart), "爱心贴右端，实际是 {line:?}");
            assert_eq!(display_width(&line), width as usize, "整行宽度不变");
            assert_eq!(hit.x, area.x + width - 1, "命中区落在串尾那一格");
            assert_eq!(hit.width, 1);
        }
    }
}

/// 左列宽度为 0（极窄窗口）时不登记，避免出现点不动的隐形按钮。
#[test]
fn heart_hit_rect_is_absent_when_the_column_is_blank() {
    assert!(heart_hit_rect(row(0, 0), HEART_LIKED).is_none());
}

/// 下载按钮在爱心左侧隔一格；同一行里两者互不重叠。
#[test]
fn download_hit_rect_sits_one_gap_left_of_the_heart() {
    let area = row(5, 20);
    let (download, heart) = right_suffix_hits(area, HEART_UNLIKED, Some('\u{f019}'));
    let download = download.expect("宽度够时应登记下载按钮");
    let heart = heart.expect("爱心格");

    assert_eq!(heart.x, area.x + area.width - 1);
    assert_eq!(
        download.x + download.width + 1,
        heart.x,
        "下载图标与爱心之间正好隔一格"
    );
    assert_eq!(download.width, 1);
    assert_eq!(download.y, area.y);
}

/// 左列放不下「图标 + 空格 + 爱心」时，只登记爱心（也不该画下载图标）。
#[test]
fn download_hit_rect_is_absent_when_the_gap_does_not_fit() {
    // 宽度 1：只够爱心；宽度 2：够爱心 + 一格空格，但放不下图标。
    for width in [1u16, 2] {
        let (download, heart) = right_suffix_hits(row(0, width), HEART_LIKED, Some('⠋'));
        assert!(download.is_none(), "宽度 {width} 不该登记下载按钮");
        assert!(heart.is_some(), "宽度 {width} 仍应有爱心");
    }
}

/// 下载图标本身必须是 1 格宽（否则左列排版会漂）。
#[test]
fn download_glyphs_are_single_cell() {
    use crate::data::icons::UiIcons;
    let icons = UiIcons::new();
    let phase = Duration::from_millis(0);
    for state in [
        DownloadState::NotDownloaded,
        DownloadState::Downloading,
        DownloadState::Done,
    ] {
        let glyph = state_glyph(state, phase, icons);
        assert_eq!(
            display_width(&glyph.to_string()),
            1,
            "{glyph:?} 应为 1 格宽"
        );
    }
}

/// 符号被吞成空串时播放栏会错位并留下点不到的按钮——这条直接兜住。
#[test]
fn player_bar_glyphs_are_single_cell() {
    for heart in [HEART_LIKED, HEART_UNLIKED] {
        assert_eq!(display_width(heart), 1, "爱心应为 1 格宽：{heart:?}");
    }
    let icons = UiIcons::new();
    for symbol in [
        icons.sequence(),
        icons.shuffle(),
        icons.loop_all(),
        icons.loop_one(),
    ] {
        assert_eq!(display_width(symbol), 1, "模式符号应为 1 格宽：{symbol:?}");
    }
}

/// 控制行命中区与 render 的居中串同源：各段起点 = 串首 + 前缀显示宽度，
/// 末段右端 = 整串右端。
#[test]
fn control_hit_rects_track_the_centered_label_row() {
    let labels = ["[<]", "[>]", "[>]", "M"];
    let area = row(4, 30);
    let hits = control_hit_rects(area, labels);

    let joined = labels.join(" ");
    let row_w = display_width(&joined) as u16;
    let start = area.x + (area.width - row_w) / 2;

    let prev = hits.prev.expect("prev 命中区");
    let play = hits.play_pause.expect("play 命中区");
    let next = hits.next.expect("next 命中区");
    let mode = hits.mode.expect("mode 命中区");

    assert_eq!(prev.x, start);
    assert_eq!(play.x, start + display_width("[<] ") as u16);
    assert_eq!(next.x, start + display_width("[<] [>] ") as u16);
    assert_eq!(mode.x, start + display_width("[<] [>] [>] ") as u16);
    assert_eq!(
        mode.x + mode.width,
        start + row_w,
        "命中区不多不少覆盖控制串"
    );
    for hit in [prev, play, next, mode] {
        assert_eq!(hit.y, area.y);
        assert_eq!(hit.height, 1);
        assert!(hit.width > 0);
    }
}

/// 窗口比控制串窄时整段越界的 token 不登记（否则会在频谱列上留下幽灵按钮）。
#[test]
fn control_hit_rects_skip_tokens_clipped_by_a_narrow_row() {
    let labels = ["[<]", "[>]", "[>]", "M"];
    let row_w = display_width(&labels.join(" ")) as u16;

    let fits = control_hit_rects(row(0, row_w), labels);
    assert!(fits.mode.is_some(), "刚好放得下时应登记模式命中区");

    let clipped = control_hit_rects(row(0, row_w - 1), labels);
    assert!(clipped.mode.is_none(), "末段越界时不登记");
    assert!(clipped.prev.is_some(), "首段仍在区域内");
}

/// 尾段必须按「图标 / 空格 / 爱心」拆开，图标不能留在头段里
/// （留在头段就会被当成标题上色——图标颜色静默回归的根因）。
#[test]
fn left_tail_peels_the_glyph_out_of_the_head() {
    let glyph = '\u{f019}';
    for heart in [HEART_LIKED, HEART_UNLIKED] {
        let suffix = format!("{glyph} {heart}");
        let line = compose_left_right_line("Title", &suffix, 20);
        let split = split_left_tail(&line, heart, Some(glyph));

        assert_eq!(split.glyph, Some(glyph), "图标自成一段：{line:?}");
        assert_eq!(split.heart, Some(heart), "爱心自成一段：{line:?}");
        assert!(
            !split.head.contains(glyph),
            "头段里不该再留图标：{:?}",
            split.head
        );
        assert_eq!(
            format!(
                "{}{}{}{}",
                split.head,
                split.glyph.unwrap(),
                ' ',
                split.heart.unwrap()
            ),
            line,
            "拆出的三段必须原样拼回整串"
        );
    }
}

/// 图标被裁掉（只剩爱心）时不产生图标段；没有爱心时整串就是头段。
#[test]
fn left_tail_handles_missing_glyph_and_missing_heart() {
    let heart = HEART_UNLIKED;
    let glyph = '\u{f019}';

    let heart_only = compose_left_right_line("Title", heart, 20);
    let split = split_left_tail(&heart_only, heart, Some(glyph));
    assert_eq!(split.glyph, None, "画不下的图标不该凭空出现");
    assert_eq!(split.heart, Some(heart));
    assert_eq!(split.head, heart_only.strip_suffix(heart).unwrap());

    let plain = "Title";
    let split = split_left_tail(plain, heart, Some(glyph));
    assert_eq!(split.head, plain, "没有爱心尾巴时原样返回");
    assert_eq!(split.glyph, None);
    assert_eq!(split.heart, None);
}
