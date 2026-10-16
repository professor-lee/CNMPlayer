use super::*;
use crate::ui::theme::{ColorCapability, Theme, ThemePalette};

const RATE: u32 = 48_000;

fn sine(freq: f32, phase: f32, len: usize) -> PcmSnapshot {
    let mut snapshot = PcmSnapshot::default();
    for i in 0..len {
        let t = i as f32 / RATE as f32;
        snapshot.left[i] = (std::f32::consts::TAU * freq * t + phase).sin();
        snapshot.right[i] = snapshot.left[i];
    }
    snapshot.len = len;
    snapshot.sample_rate = RATE;
    snapshot
}

#[test]
fn trigger_locks_onto_a_rising_zero_crossing_at_any_phase() {
    let window = (RATE as f32 * WINDOW_MS / 1000.0) as usize;
    for step in 0..8 {
        let phase = step as f32 * std::f32::consts::TAU / 8.0;
        let snapshot = sine(440.0, phase, window * 4);
        let start = trigger_offset(&snapshot, window);

        // 触发点必须真的落在上升沿上：本身刚过零、前一个为负、后一个为正。
        assert!(snapshot.left[start] >= 0.0 && snapshot.left[start] < 0.07);
        assert!(snapshot.left[start - 1] < 0.0);
        assert!(snapshot.left[start + 1] > 0.0);
    }
}

#[test]
fn silence_falls_back_to_the_newest_window() {
    let snapshot = PcmSnapshot {
        len: 4096,
        sample_rate: RATE,
        ..Default::default()
    };

    let window = window_frames(&snapshot);
    assert_eq!(trigger_offset(&snapshot, window), snapshot.len - window);
}

#[test]
fn full_scale_trace_is_continuous_and_spans_the_grid() {
    let (w_cells, h_cells) = (60usize, 8usize);
    let window = (RATE as f32 * WINDOW_MS / 1000.0) as usize;
    let snapshot = sine(440.0, 0.0, window * 4);
    let start = trigger_offset(&snapshot, window);

    let mut grid = vec![0u8; w_cells * h_cells];
    draw_channel(
        &mut grid,
        w_cells,
        h_cells,
        &snapshot.left[start..start + window],
        1.0,
    );

    // 每一列都得有点亮的格子：包络带连续，接合逻辑没漏掉断点。
    for col in 0..w_cells {
        assert!(
            (0..h_cells).any(|row| grid[row * w_cells + col] != 0),
            "column {col} is empty"
        );
    }
    // 绝对映射：满幅信号必须触到顶行与底行，不被归一化压扁。
    assert!((0..w_cells).any(|col| grid[col] != 0), "top row untouched");
    assert!(
        (0..w_cells).any(|col| grid[(h_cells - 1) * w_cells + col] != 0),
        "bottom row untouched"
    );
}

#[test]
fn quiet_signal_stays_near_zero_level() {
    let (w_cells, h_cells) = (60usize, 8usize);
    let window = (RATE as f32 * WINDOW_MS / 1000.0) as usize;
    let mut snapshot = sine(440.0, 0.0, window * 4);
    for v in snapshot.left.iter_mut() {
        *v *= 0.02;
    }

    let mut grid = vec![0u8; w_cells * h_cells];
    draw_channel(&mut grid, w_cells, h_cells, &snapshot.left[0..window], 1.0);

    // 顶行与底行必须是空的：响度动态没有被 AGC 抹掉。
    assert!((0..w_cells).all(|col| grid[col] == 0));
    assert!((0..w_cells).all(|col| grid[(h_cells - 1) * w_cells + col] == 0));
}

#[test]
fn fewer_samples_than_subcolumns_still_draws_every_column() {
    let (w_cells, h_cells) = (60usize, 6usize);
    let samples: Vec<f32> = (0..17).map(|i| ((i % 5) as f32 - 2.0) / 2.0).collect();

    let mut grid = vec![0u8; w_cells * h_cells];
    draw_channel(&mut grid, w_cells, h_cells, &samples, 1.0);

    for col in 0..w_cells {
        assert!(
            (0..h_cells).any(|row| grid[row * w_cells + col] != 0),
            "column {col} is empty"
        );
    }
}

#[test]
fn gain_envelope_collapses_the_trace_toward_zero_level() {
    let (w_cells, h_cells) = (60usize, 8usize);
    let window = (RATE as f32 * WINDOW_MS / 1000.0) as usize;
    let snapshot = sine(440.0, 0.0, window * 4);

    let span = |gain: f32| {
        let mut grid = vec![0u8; w_cells * h_cells];
        draw_channel(&mut grid, w_cells, h_cells, &snapshot.left[0..window], gain);
        let rows: Vec<usize> = (0..h_cells)
            .filter(|row| (0..w_cells).any(|col| grid[row * w_cells + col] != 0))
            .collect();
        rows.last().map(|hi| hi - rows[0] + 1).unwrap_or(0)
    };

    // 幅度包络越小，波形占的行数越少 —— 暂停后就是这样落回零电平的。
    let full = span(1.0);
    let half = span(0.5);
    assert_eq!(full, h_cells, "满幅应撑满面板");
    assert!(half < full, "half={half} full={full}");
    assert_eq!(span(0.0), 1, "只剩零电平那一行");
}

/// 落平后波形退化成居中直线：由 `draw_channel` 自己画出来，不是外加的中线。
#[test]
fn settled_scope_rasterizes_a_centred_flat_line() {
    let (w_cells, h_cells) = (60usize, 8usize);
    let mut scope = ScopeScratch {
        snapshot: sine(440.0, 0.0, 8192),
        grid: Vec::new(),
    };

    rasterize(&mut scope, w_cells, h_cells, 0.0);

    // 恰好一行被点亮，且落在零电平所在的那一行。
    let lit: Vec<usize> = (0..h_cells)
        .filter(|row| (0..w_cells).any(|col| scope.grid[row * w_cells + col] != 0))
        .collect();
    let zero_row = sample_row(0.0, (h_cells * 4) as i32) as usize / 4;
    assert_eq!(lit, vec![zero_row], "应只点亮零电平那一行");

    // 且贯穿全宽：每一格都有点，才是一条不断的直线。
    assert!(
        (0..w_cells).all(|col| scope.grid[zero_row * w_cells + col] != 0),
        "直线有断点"
    );
}

/// 环里没有样本时（刚启动）画的必须与落平后完全一致 —— 否则启动
/// 瞬间会闪一下空白，或停止后画面与启动态不符。
#[test]
fn no_samples_matches_settled_frame() {
    let (w_cells, h_cells) = (60usize, 8usize);

    let mut empty = ScopeScratch::default();
    rasterize(&mut empty, w_cells, h_cells, 1.0);

    let mut settled = ScopeScratch {
        snapshot: sine(440.0, 0.0, 8192),
        grid: Vec::new(),
    };
    rasterize(&mut settled, w_cells, h_cells, 0.0);

    assert_eq!(empty.grid, settled.grid);
}

fn test_theme() -> Theme {
    Theme {
        name: "frappe".to_string(),
        palette: ThemePalette {
            text: (198, 208, 245),
            subtext: (165, 173, 206),
            base: (48, 52, 70),
            surface: (65, 69, 89),
            buff: (81, 87, 109),
            accent: (140, 170, 238),
            accent2: (133, 193, 220),
            accent3: (202, 158, 230),
        },
        capability: ColorCapability::TrueColor,
    }
}

/// 离屏渲染一帧，返回 (点亮的单元格数, 出现过的前景色集合)。
fn paint_frame(gain: f32) -> (usize, Vec<Color>) {
    let (w_cells, h_cells) = (60u16, 8u16);
    let area = Rect::new(0, 0, w_cells, h_cells);
    let mut scope = ScopeScratch {
        snapshot: sine(440.0, 0.0, 8192),
        grid: Vec::new(),
    };
    rasterize(&mut scope, w_cells as usize, h_cells as usize, gain);

    let mut buf = Buffer::empty(area);
    paint(
        &mut buf,
        area,
        &scope.grid,
        &test_theme(),
        w_cells as usize,
        h_cells as usize,
    );

    let mut colours = Vec::new();
    let mut lit = 0;
    for y in 0..h_cells {
        for x in 0..w_cells {
            let cell = &buf[(x, y)];
            if cell.symbol() == " " {
                continue;
            }
            lit += 1;
            if !colours.contains(&cell.fg) {
                colours.push(cell.fg);
            }
        }
    }
    (lit, colours)
}

#[test]
fn settled_frame_is_a_single_centred_waveform_row() {
    let (w_cells, _) = (60usize, 8usize);
    let (playing, _) = paint_frame(1.0);
    let (settled, colours) = paint_frame(0.0);

    assert!(playing > w_cells, "播放中应画出波形");
    // 静止态正好一整行：波形自己收成的直线，不多不少。
    assert_eq!(settled, w_cells, "静止态应只剩居中那一行");
    // 且用波形的渐变色，不是任何专供中线的颜色 —— 没有独立的中线元素。
    assert_eq!(colours.len(), 1, "一行只有一个颜色: {colours:?}");
    assert_ne!(colours[0], test_theme().color_surface());
}

/// 静止那条线必须落在垂直居中位置。
#[test]
fn settled_line_sits_vertically_centred() {
    for h_cells in [4usize, 6, 8, 12] {
        let w_cells = 8usize;
        let area = Rect::new(0, 0, w_cells as u16, h_cells as u16);
        let mut scope = ScopeScratch::default();
        rasterize(&mut scope, w_cells, h_cells, 0.0);

        let mut buf = Buffer::empty(area);
        paint(&mut buf, area, &scope.grid, &test_theme(), w_cells, h_cells);

        // 收集点亮的子行，取其中点，与面板正中比较。
        let mut subrows = Vec::new();
        for row in 0..h_cells {
            let bits = scope.grid[row * w_cells];
            for dy in 0..4 {
                if bits & braille_bit(0, dy) != 0 {
                    subrows.push(row * 4 + dy);
                }
            }
        }
        assert_eq!(subrows.len(), 1, "h={h_cells} 应只有一条线");

        let centre = (h_cells * 4 - 1) as f32 / 2.0;
        let offset = (subrows[0] as f32 - centre).abs();
        assert!(offset <= 0.5, "h={h_cells} 偏离中心 {offset} 子行");
    }
}
