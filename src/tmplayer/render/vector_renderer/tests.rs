use super::*;

const SR: u32 = 44_100;
const FRAME: Duration = Duration::from_millis(33);

/// 生成一个合成快照：`sample_at(i)` 给出第 i 帧的 (左, 右)。
fn synth(len: usize, sample_at: impl Fn(usize) -> (f32, f32)) -> PcmSnapshot {
    let mut s = PcmSnapshot::default();
    for i in 0..len {
        let (l, r) = sample_at(i);
        s.left[i] = l;
        s.right[i] = r;
    }
    s.len = len;
    s.sample_rate = SR;
    s.stereo = true;
    s
}

fn circle(amp: f32, freq_hz: f32, phase: f32) -> impl Fn(usize) -> (f32, f32) {
    move |i| {
        let t = i as f32 / SR as f32;
        let w = std::f32::consts::TAU * freq_hz * t;
        (amp * w.sin(), amp * (w + phase).sin())
    }
}

/// 确定性伪随机噪声：相邻样本大幅跳变，轨迹布满面板，用于粒子上限测试。
fn noise() -> impl Fn(usize) -> (f32, f32) {
    move |i| {
        let s = i as u32;
        (
            jitter(s) * 2.0 - 1.0,
            jitter(s.rotate_left(13) ^ 0x5BD1) * 2.0 - 1.0,
        )
    }
}

/// 收集光栅里全部点亮点的子像素坐标（与示波器测试同一解码方式）。
fn lit_dots(state: &VectorState) -> Vec<(i32, i32)> {
    let mut dots = Vec::new();
    for (cell, &bits) in state.grid.iter().enumerate() {
        if bits == 0 {
            continue;
        }
        let (cx, cy) = (cell % state.w_cells, cell / state.w_cells);
        for dy in 0..4 {
            for dx in 0..2 {
                if bits & braille_bit(dx, dy) != 0 {
                    dots.push(((cx * 2 + dx) as i32, (cy * 4 + dy) as i32));
                }
            }
        }
    }
    dots
}

fn bbox(dots: &[(i32, i32)]) -> (i32, i32, i32, i32) {
    let xs = dots.iter().map(|d| d.0);
    let ys = dots.iter().map(|d| d.1);
    (
        xs.clone().min().unwrap(),
        xs.max().unwrap(),
        ys.clone().min().unwrap(),
        ys.max().unwrap(),
    )
}

/// 走一帧完整循环（与事件循环的 observe→tick→draw 顺序一致到一帧以内）。
fn circle_state(amp: f32) -> VectorState {
    let mut st = VectorState {
        snapshot: synth(882, circle(amp, 110.0, std::f32::consts::FRAC_PI_2)),
        ..Default::default()
    };
    st.observe(window_level(&st.snapshot), 40, 20);
    st.rasterize();
    st.tick(true, true, Duration::from_secs(1));
    st.rasterize();
    st
}

/// 110 Hz 双声道相位差 90°：以 0.8 幅度画出一个居中的大圆。
#[test]
fn lissajous_draws_centered_figure() {
    let st = circle_state(0.8);
    let dots = lit_dots(&st);
    assert!(
        dots.len() > 100,
        "应有可观密度的轨迹，实得 {} 点",
        dots.len()
    );

    let (min_x, max_x, min_y, max_y) = bbox(&dots);
    let (w_px, h_px) = (80.0, 80.0);
    let (cx, cy) = ((w_px - 1.0) * 0.5, (h_px - 1.0) * 0.5);
    assert!(
        (max_x - min_x) as f32 >= w_px * 0.6,
        "图形宽度应占面板多数：{min_x}..{max_x}"
    );
    assert!(
        (max_y - min_y) as f32 >= h_px * 0.6,
        "图形高度应占面板多数：{min_y}..{max_y}"
    );
    let mid_x = (min_x + max_x) as f32 / 2.0;
    let mid_y = (min_y + max_y) as f32 / 2.0;
    assert!((mid_x - cx).abs() <= 3.0, "水平居中：中点 {mid_x} vs {cx}");
    assert!((mid_y - cy).abs() <= 3.0, "垂直居中：中点 {mid_y} vs {cy}");
}

/// 高于下限的任何电平都缩放到同一尺寸；低于可见下限则图形消失。
#[test]
fn autoscale_fills_panel_and_fades_to_nothing() {
    for amp in [1.0f32, 0.01] {
        let st = circle_state(amp);
        let (min_x, max_x, _, _) = bbox(&lit_dots(&st));
        assert!(
            (max_x - min_x) as f32 >= 80.0 * 0.6,
            "幅度 {amp} 应缩放到接近满幅"
        );
    }

    let mut st = circle_state(1.0e-5);
    st.observe(window_level(&st.snapshot), 40, 20);
    st.tick(true, true, Duration::from_secs(1));
    st.rasterize();
    assert!(
        lit_dots(&st).is_empty(),
        "低于可见下限应完全消失（渐弱的最终）"
    );
}

/// 单声道两声道相同：全部点落在右上 45° 对角线上（x−cx = −(y−cy)）。
#[test]
fn mono_source_renders_up_right_diagonal() {
    let snap = synth(882, |i| {
        let t = i as f32 / SR as f32;
        let v = (std::f32::consts::TAU * 70.0 * t).sin() * 0.6;
        (v, v)
    });
    let mut st = VectorState {
        snapshot: snap,
        ..Default::default()
    };
    st.observe(window_level(&st.snapshot), 40, 20);
    st.tick(true, true, Duration::from_secs(1));
    st.rasterize();

    let (cx, cy) = (39.5f32, 39.5f32);
    for (x, y) in lit_dots(&st) {
        let off = (x as f32 - cx) + (y as f32 - cy);
        assert!(
            off.abs() <= 2.0,
            "点 ({x},{y}) 偏离右上对角线 {off:.2}（连线步进与取整允差内）"
        );
    }
}

#[test]
fn first_vector_frame_uses_observed_peak_without_a_prior_tick() {
    for amplitude in [0.01_f32, 0.8] {
        let mut immediate = VectorState {
            snapshot: synth(882, circle(amplitude, 110.0, std::f32::consts::FRAC_PI_2)),
            ..Default::default()
        };
        immediate.observe(window_level(&immediate.snapshot), 40, 20);
        immediate.rasterize();
        let initialized = circle_state(amplitude);
        assert_eq!(
            lit_dots(&immediate),
            lit_dots(&initialized),
            "first frame must use the same calibrated scale as normal playback"
        );
        immediate.observe(None, 40, 20);
        immediate.snapshot = synth(882, circle(0.3, 110.0, std::f32::consts::FRAC_PI_2));
        immediate.observe(window_level(&immediate.snapshot), 40, 20);
        immediate.rasterize();
        assert!(
            (immediate.scale_peak - 0.3).abs() < 1.0e-3,
            "new PCM samples after reset recalibrate before drawing"
        );
    }
}

/// 缩放基准 = 本曲开播以来的最大峰值：只增不减；更响的段落把基准
/// 上调、更安静的段落不拉低；环重置→样本重现（切歌）后从零重新累积。
#[test]
fn scale_tracks_song_maximum_and_resets_on_track_change() {
    let mut st = VectorState::default();

    st.observe(Some(0.5), 40, 20);
    st.tick(true, true, Duration::from_millis(16));
    assert!((st.scale_peak - 0.5).abs() < 1e-6, "首帧即以当前电平为基准");

    // 更安静的段落不拉低基准（最响段仍恰好撑满面板）。
    st.observe(Some(0.2), 40, 20);
    st.tick(true, true, Duration::from_secs(1));
    assert_eq!(st.scale_peak, 0.5, "基准只增不减");

    // 更响的段落一次性上调基准。
    st.observe(Some(0.9), 40, 20);
    st.tick(true, true, Duration::from_millis(16));
    assert!((st.scale_peak - 0.9).abs() < 1e-6, "更响段落上调基准");

    // 静音不重置（环未重置，非切歌）。
    st.observe(Some(0.0), 40, 20);
    st.tick(true, true, Duration::from_secs(1));
    assert_eq!(st.scale_peak, 0.9);

    // 切歌：环重置（None）→ 样本重现后从零重新累积。
    st.observe(None, 40, 20);
    st.tick(true, true, Duration::from_millis(16));
    st.observe(Some(0.3), 40, 20);
    st.tick(true, true, Duration::from_millis(16));
    assert!((st.scale_peak - 0.3).abs() < 1e-6, "切歌后重新累积");

    st.tick(false, true, Duration::from_secs(1));
    assert_eq!(st.scale_peak, 0.0, "关档复位");
}

/// 暂停打断：点亮点化为粒子（有上限，超出按步长抽样直接消失），
/// 飞向随机落点并在限时内停稳；停稳后位置静止、亮度按 Sparkle 明灭。
#[test]
fn pause_disperses_settles_and_floats() {
    let mut st = circle_state(0.8);
    let dots_before = lit_dots(&st).len();
    assert!(dots_before > 100);

    // 暂停 → 分散。
    st.tick(true, false, FRAME);
    assert_eq!(st.phase, Phase::Dispersing, "暂停立即分散");
    assert!(
        !st.particles.is_empty() && st.particles.len() <= MAX_PARTICLES,
        "粒子数受上限约束：{}",
        st.particles.len()
    );

    let starts: Vec<(f32, f32)> = st.particles.iter().map(|p| (p.x, p.y)).collect();
    for _ in 0..40 {
        st.tick(true, false, FRAME);
    }
    assert_eq!(st.phase, Phase::Floating, "全部停稳进入悬浮");
    for (i, p) in st.particles.iter().enumerate() {
        assert_eq!(p.vx, 0.0, "粒子 {i} 应停稳");
        assert_eq!(p.vy, 0.0);
        // 落点在可视化区域内（随机落点均匀采样，飞不出去）。
        assert!((0.0..=79.0).contains(&p.x), "粒子 {i} 越界 x={}", p.x);
        assert!((0.0..=79.0).contains(&p.y), "粒子 {i} 越界 y={}", p.y);
    }
    // 随机落点：停稳后不应仍聚在原图形附近（散布为随机尘埃）。
    let moved_avg = st
        .particles
        .iter()
        .zip(&starts)
        .map(|(p, s)| (p.x - s.0).hypot(p.y - s.1))
        .sum::<f32>()
        / st.particles.len() as f32;
    assert!(
        moved_avg >= 15.0,
        "平均位移 {moved_avg:.1} 过小：仍能看出原图形"
    );

    // 停稳后：位置与数量静止，亮度按 Astra Sparkle 公式明灭。
    let settled_count = st.particles.len();
    let settled_pos: Vec<(f32, f32)> = st.particles.iter().map(|p| (p.x, p.y)).collect();
    let mut alpha_snapshots = Vec::new();
    for f in 0..240 {
        st.tick(true, false, Duration::from_millis(50)); // 共 12 s
        if f % 24 == 0 {
            st.rasterize();
            alpha_snapshots.push(st.cell_alpha.clone());
        }
    }
    st.rasterize();
    assert_eq!(st.particles.len(), settled_count, "静止不增减粒子");
    for (i, p) in st.particles.iter().enumerate() {
        assert_eq!((p.x, p.y), settled_pos[i], "粒子 {i} 不得改变位置");
    }
    assert!(st.is_floating(), "明灭持续，需要持续重绘");
    assert!(
        alpha_snapshots.windows(2).any(|w| w[0] != w[1]),
        "12 s 内亮度分布必须变化（星点在明灭）"
    );
}

/// 停稳后的明灭曲线与 codex Astra Sparkle（sparkle_field.rs）一致：
/// 同一停稳点在任何时刻的透明度恒等于公式值，熄灭帧整点不画；
/// Dying 的淡出在明灭之上继续收敛。
#[test]
fn settled_dust_twinkles_with_astra_sparkle_formula() {
    let mut st = VectorState {
        phase: Phase::Floating,
        w_cells: 40,
        h_cells: 20,
        ..Default::default()
    };
    // 两颗隔离的粒子：一颗 Solid，一颗淡出到一半的 Dying（×0.5）。
    st.particles = vec![
        Particle {
            x: 21.0,
            y: 43.0,
            twinkle: Twinkle::Solid,
            ..new_scatter_particle((10, 10), 1, 80.0, 80.0)
        },
        Particle {
            x: 31.0,
            y: 43.0,
            twinkle: Twinkle::Dying {
                left: 1.0,
                total: 2.0,
            },
            ..new_scatter_particle((11, 10), 1, 80.0, 80.0)
        },
    ];
    let solid = ((21 / 2) + (43 / 4) * 40, star_hash(21, 43), 1.0);
    let dying = ((31 / 2) + (43 / 4) * 40, star_hash(31, 43), 0.5);

    let mut saw_visible = false;
    let mut saw_extinguished = false;
    for f in 0..=1400 {
        let t = f as f32 * 0.05; // 0~70 s：覆盖最长周期十余轮
        st.float_elapsed = Duration::from_secs_f32(t);
        st.rasterize();
        for (cell, hash, factor) in [solid, dying] {
            let expect = sparkle_brightness(hash, t) * factor;
            let got = st.cell_alpha[cell];
            assert!(
                (got - expect).abs() < 1e-5,
                "t={t} cell={cell}：透明度 {got} 应等于公式值 {expect}"
            );
            assert_ne!(st.grid[cell], 0, "t={t} cell={cell}：粒子位必须持续保留");
            if expect > 0.04 {
                saw_visible = true;
            } else {
                saw_extinguished = true;
            }
        }
    }
    assert!(saw_visible, "70 s 内应观测到可见相位");
    assert!(saw_extinguished, "70 s 内应观测到熄灭相位");
}

#[test]
fn settled_particles_in_one_braille_cell_keep_independent_alpha() {
    let mut st = VectorState {
        phase: Phase::Floating,
        w_cells: 1,
        h_cells: 1,
        ..Default::default()
    };
    st.particles = vec![
        Particle {
            x: 0.0,
            y: 0.0,
            twinkle: Twinkle::Solid,
            ..new_scatter_particle((0, 0), 1, 2.0, 4.0)
        },
        Particle {
            x: 1.0,
            y: 0.0,
            twinkle: Twinkle::Solid,
            ..new_scatter_particle((1, 0), 1, 2.0, 4.0)
        },
    ];
    let mut found_different = false;
    for frame in 0..2000 {
        let time = frame as f32 * 0.01;
        st.float_elapsed = Duration::from_secs_f32(time);
        st.rasterize();
        let left = st.pixel_alpha[0];
        // (x=1,y=0) 的盲文位是 0x08 → 槽 3；槽 1（0x02 位）属于 (x=0,y=1)。
        let right = st.pixel_alpha[3];
        if (left - right).abs() > 1.0e-4 {
            found_different = true;
            assert_ne!(st.grid[0] & braille_bit(0, 0), 0);
            assert_ne!(st.grid[0] & braille_bit(1, 0), 0);
            break;
        }
    }
    assert!(found_different, "同一盲文格的两个像素应有不同亮灭曲线");
}

/// 回归：粒子 alpha 必须落在它自己的盲文位槽上。曾经用线性序
/// `dy*2+dx` 写槽，(1,0)/(0,1)/(0,2)/(1,1) 四个子像素写错位置，
/// paint_with_alpha 读到 0 就把盲文位清掉——约半数尘埃点不显示。
#[test]
fn particle_alpha_lands_on_its_braille_dot_slot() {
    let mut st = VectorState {
        phase: Phase::Floating,
        w_cells: 1,
        h_cells: 1,
        ..Default::default()
    };
    st.particles = vec![Particle {
        x: 1.0,
        y: 0.0,
        twinkle: Twinkle::Solid,
        ..new_scatter_particle((1, 0), 1, 2.0, 4.0)
    }];

    let mut checked = 0;
    for f in 0..=800 {
        let t = f as f32 * 0.05;
        st.float_elapsed = Duration::from_secs_f32(t);
        st.rasterize();
        let expect = sparkle_brightness(star_hash(1, 0), t);
        if expect > 0.05 {
            checked += 1;
            assert!(
                (st.pixel_alpha[3] - expect).abs() < 1e-5,
                "t={t}：槽 3（0x08 位）应等于公式值 {expect}，实得 {}",
                st.pixel_alpha[3]
            );
        }
        assert_eq!(st.pixel_alpha[1], 0.0, "t={t}：槽 1（0x02 位）不应被占用");
        assert_ne!(st.grid[0] & braille_bit(1, 0), 0);
    }
    assert!(checked > 0, "40 s 内应观测到可见相位");
}

/// 回归：就近锚定按 Chebyshev 环序找**最近**的点（周界扫描重写后
/// 语义不变），且越界点被忽略。
#[test]
fn nearest_lit_dot_walks_ring_perimeter_in_order() {
    let (w, h) = (10, 6); // 20×24 点阵
    let mut grid = vec![0u8; w * h];
    // 两个亮点：(4,4)（与查询点同格）与 (9,9)（第 5 环）。
    set_pixel(&mut grid, w, h, 4, 4);
    set_pixel(&mut grid, w, h, 9, 9);
    assert_eq!(nearest_lit_dot(&grid, w, h, 4, 4, 16), Some((4.0, 4.0)));
    assert_eq!(nearest_lit_dot(&grid, w, h, 8, 8, 16), Some((9.0, 9.0)));
    // 半径收不进任何点时返回 None；越界方向不 panic。
    assert_eq!(nearest_lit_dot(&grid, w, h, 19, 23, 4), None);
    assert_eq!(nearest_lit_dot(&grid, w, h, 0, 0, 1), None);
}

/// 回归：恢复期没有可见轨迹（恢复进静音段）时不做锚定搜索，
/// 粒子在聚集超时后释放，不冻结界面。
#[test]
fn recovering_without_trace_releases_particles_on_timeout() {
    let mut st = VectorState {
        phase: Phase::Recovering,
        w_cells: 40,
        h_cells: 20,
        ..Default::default()
    };
    st.particles = vec![Particle {
        x: 5.0,
        y: 5.0,
        twinkle: Twinkle::Solid,
        ..new_scatter_particle((2, 2), 1, 80.0, 80.0)
    }];
    st.last_drawn = false;
    st.gather_elapsed = Duration::ZERO;

    // 动画时钟每 tick 最多推进 100 ms：7 次推进 700 ms 仍滞留。
    for _ in 0..7 {
        st.tick(true, true, Duration::from_secs(1));
    }
    assert_eq!(st.particles.len(), 1, "超时前粒子应滞留");
    // 第 8 次到达 800 ms 超时：无处可归，释放并回到 Active。
    st.tick(true, true, Duration::from_secs(1));
    assert!(st.particles.is_empty(), "超时后粒子应释放");
    assert_eq!(st.phase, Phase::Active);
}
#[test]
fn active_trace_populates_pixel_alpha_for_rendering() {
    let st = circle_state(0.8);
    assert_eq!(st.phase, Phase::Active);
    assert!(st.grid.iter().any(|&bits| bits != 0));
    assert!(st.pixel_alpha.contains(&1.0));
}

/// 粒子上限：高密度图形孵化时按步长抽样，粒子数不超过上限。
#[test]
fn scatter_caps_particle_count() {
    // 伪随机噪声轨迹布满 80×80 点面板（远超粒子上限）。
    let mut st = VectorState {
        snapshot: synth(882, noise()),
        ..Default::default()
    };
    st.observe(window_level(&st.snapshot), 40, 20);
    st.rasterize();
    st.tick(true, true, Duration::from_secs(1));
    st.rasterize();
    let dots = lit_dots(&st).len();
    st.tick(true, false, FRAME);
    assert!(
        dots > MAX_PARTICLES,
        "前置条件：密集图形点数 {dots} 应超过上限"
    );
    // 全部点都化作粒子：没有一个点瞬间消失（多余的以 Dying 淡出）。
    assert_eq!(
        st.particles.len(),
        dots,
        "每个点亮点都应有粒子（多余者走淡出退役）"
    );
    let kept = st
        .particles
        .iter()
        .filter(|p| !matches!(p.twinkle, Twinkle::Dying { .. }))
        .count();
    assert!(
        (MAX_PARTICLES * 9 / 10..=MAX_PARTICLES).contains(&kept),
        "抽样保留 {kept} 应接近且不超过上限"
    );
    assert_eq!(
        st.particles.len() - kept,
        dots - kept,
        "多余点应处于 Dying 相位"
    );
    // 淡出全部完成后，参与轮换的粒子数收敛到上限以内。
    for _ in 0..80 {
        st.tick(true, false, FRAME);
    }
    assert!(
        st.particles.len() <= MAX_PARTICLES,
        "退役后粒子数 {} 超过上限 {}",
        st.particles.len(),
        MAX_PARTICLES
    );
    assert!(
        st.particles
            .iter()
            .all(|p| !matches!(p.twinkle, Twinkle::Dying { .. })),
        "不应再有 Dying 粒子"
    );
}

/// 帧间隔尖峰（单线程运行时被宿主桥/下载阻塞数秒）不得跳过 Dying
/// 淡出：动画时钟钳步后，dt=10 s 的 tick 也只推进 100 ms。
#[test]
fn dt_spike_does_not_skip_dying_fade() {
    let mut st = VectorState {
        phase: Phase::Floating,
        w_cells: 40,
        h_cells: 20,
        ..Default::default()
    };
    st.particles = vec![Particle {
        x: 20.0,
        y: 40.0,
        twinkle: Twinkle::Dying {
            left: 2.0,
            total: 2.0,
        },
        ..new_scatter_particle((10, 10), 1, 80.0, 80.0)
    }];
    st.observe(Some(1.0e-6), 40, 20);

    // 一个 10 s 的尖峰 tick：淡出只推进 100 ms，而不是整段跳过。
    st.tick(true, false, Duration::from_secs(10));
    assert!(
        matches!(st.particles[0].twinkle, Twinkle::Dying { .. }),
        "尖峰 dt 不应瞬间移除 Dying 粒子"
    );
    let a = st.particles[0].twinkle.alpha();
    assert!(
        (0.9..=1.0).contains(&a),
        "尖峰后透明度应只下降约 100 ms 的量：{a}"
    );
}

/// 播放中突然静音：持续超过 SILENCE_SUSTAIN 判打断；短于则不打断。
#[test]
fn sudden_silence_disperses_after_sustain() {
    let mut st = circle_state(0.8);
    st.observe(Some(1.0e-5), 40, 20);
    st.rasterize(); // 静音帧的轨迹已不可见
    st.tick(true, true, Duration::from_millis(40));
    assert_eq!(st.phase, Phase::Active, "40 ms 低于持续阈值不打断");
    st.tick(true, true, Duration::from_millis(60));
    assert_eq!(st.phase, Phase::Dispersing, "80 ms 起判突然静音");
}

/// 渐弱不触发分散：−20 dB/s 的滑落（快于多数真实淡出）紧贴近期电平，
/// 差值判据永不满足；图形按幅度缩到消失。
#[test]
fn gradual_fade_disappears_without_dispersing() {
    let mut st = circle_state(0.8);
    // 3 s、共 −60 dB 的指数渐弱，尾接静音并远超持续阈值。
    for f in 0..90 {
        let level = (0.8 * 10.0_f32.powf(-3.0 * f as f32 / 90.0)).max(1.0e-5);
        st.observe(Some(level), 40, 20);
        st.rasterize();
        st.tick(true, true, Duration::from_millis(33));
        assert_eq!(st.phase, Phase::Active, "渐弱帧 {f} 不得分散");
    }
    for _ in 0..10 {
        st.observe(Some(1.0e-5), 40, 20);
        st.rasterize();
        st.tick(true, true, Duration::from_millis(33));
    }
    assert_eq!(st.phase, Phase::Active, "静音持续后仍不分散（差值判据）");
    assert!(st.particles.is_empty());
    assert!(lit_dots(&st).is_empty(), "渐弱的最终是图形消失");
}

/// 渐弱消失超过宽限期后的暂停：最后一幅可见轨迹已退役，不得散出幽灵。
#[test]
fn pause_long_after_fade_scatters_nothing() {
    let mut st = circle_state(0.8);
    // 3 s 渐弱到不可见（差值判据不触发），随后 2 s 完全静音。
    for f in 0..90 {
        let level = (0.8 * 10.0_f32.powf(-3.0 * f as f32 / 90.0)).max(1.0e-6);
        st.observe(Some(level), 40, 20);
        st.rasterize();
        st.tick(true, true, Duration::from_millis(33));
        assert_eq!(st.phase, Phase::Active, "渐弱帧 {f} 不得分散");
    }
    for _ in 0..60 {
        st.observe(Some(1.0e-6), 40, 20);
        st.rasterize();
        st.tick(true, true, Duration::from_millis(33));
    }
    assert_eq!(st.phase, Phase::Active);
    assert!(st.last_trace_grid.is_empty(), "宽限期后幽灵轨迹应退役");

    st.tick(true, false, Duration::from_millis(33));
    assert_eq!(st.phase, Phase::Active, "无可散开的图形");
    assert!(st.particles.is_empty());
}

/// 恢复播放：粒子**聚集回归** —— 带点火延迟与可见的指数逼近，
/// 0.8 s 内全部归位吸收，图形恢复。
#[test]
fn resume_gathers_all_particles_visibly() {
    let mut st = circle_state(0.8);
    st.tick(true, false, FRAME); // 暂停分散
    for _ in 0..40 {
        st.tick(true, false, FRAME);
    }
    assert_eq!(st.phase, Phase::Floating);
    let initial = st.particles.len();
    assert!(initial > 100);

    // 恢复播放且有声：进入聚集（轨迹与粒子同画）。
    st.observe(Some(0.8), 40, 20);
    st.rasterize();
    st.tick(true, true, FRAME);
    assert_eq!(st.phase, Phase::Recovering, "聚集是可见动画而非瞬切");

    // 点火期内（≤0.12 s）粒子原地待命，随后陆续被吸收：
    // 中途应观测到「部分归位、部分仍在途」的中间态。
    let mut seen_partial = false;
    for _ in 0..25 {
        st.rasterize();
        st.tick(true, true, FRAME);
        let left = st.particles.len();
        if left > 0 && left < initial {
            seen_partial = true;
        }
    }
    assert!(seen_partial, "聚集应有可见的渐进过程（观测到中间态）");
    assert!(st.particles.is_empty(), "粒子应全部归位吸收");
    assert_eq!(st.phase, Phase::Active);
    st.rasterize();
    assert!(!lit_dots(&st).is_empty(), "聚集完成后图形恢复");
}

/// 图形本已消失（渐弱末段 / 无样本）时打断：无可散开，维持 Active。
#[test]
fn disperse_with_nothing_lit_stays_active() {
    let mut st = VectorState::default();
    st.observe(None, 40, 20);
    st.rasterize();
    st.tick(true, false, FRAME);
    assert_eq!(st.phase, Phase::Active);
    assert!(st.particles.is_empty());
}

/// 淡入淡出的颜色（透明度）渐变：半透明粒子的前景色恰为波形色与
/// 面板底色的中点插值，全可见粒子与示波器同色。
#[test]
fn fading_particles_blend_colors_toward_background() {
    use crate::ui::theme::{ColorCapability, Theme, ThemePalette};
    use ratatui::buffer::Buffer;

    let theme = Theme {
        name: "system".to_string(),
        capability: ColorCapability::TrueColor,
        palette: ThemePalette {
            text: (200, 200, 200),
            subtext: (170, 170, 170),
            base: (36, 36, 48),
            surface: (48, 48, 64),
            buff: (60, 60, 80),
            accent: (140, 170, 220),
            accent2: (160, 200, 240),
            accent3: (130, 220, 200),
        },
    };

    let mut st = VectorState {
        // Dispersing：粒子按自身状态机透明度渲染（Floating 会叠加
        // Sparkle 明灭，颜色插值的基准就不纯了）。
        phase: Phase::Dispersing,
        w_cells: 40,
        h_cells: 20,
        ..Default::default()
    };
    // 两颗隔离的粒子：一颗半透明（淡出到一半），一颗全可见。
    st.particles = vec![
        Particle {
            x: 20.0,
            y: 40.0,
            twinkle: Twinkle::Dying {
                left: 1.0,
                total: 2.0,
            },
            ..new_scatter_particle((10, 10), 1, 80.0, 80.0)
        },
        Particle {
            x: 30.0,
            y: 40.0,
            twinkle: Twinkle::Solid,
            ..new_scatter_particle((11, 10), 1, 80.0, 80.0)
        },
    ];
    st.rasterize();

    let area = Rect::new(0, 0, 40, 20);
    let mut buf = Buffer::empty(area);
    paint_with_alpha(&mut buf, area, &st.grid, &st.pixel_alpha, &theme, 40, 20);

    // 半透明粒子：y=40 子像素 → 第 10 行；期望色 = 行渐变色与底色按 0.5 插值。
    let row = 10usize;
    let t = row as f32 / 19.0;
    let wave = vertical_gradient_color(&theme, t);
    let expect_half = mix_colors(wave, theme.color_base(), 0.5);
    assert_eq!(buf.cell((10, 10)).unwrap().fg, expect_half);

    // 全可见粒子：与示波器同色（纯行渐变）。
    assert_eq!(buf.cell((15, 10)).unwrap().fg, wave);
}

/// 聚集（Recovering）时轨迹格恒全亮：不再被画成底色（曾导致整幅
/// 图形隐形、聚集完成一帧全亮瞬现），也不被暗粒子压暗；
/// 纯粒子格仍按自身透明度渐变。
#[test]
fn recovering_trace_cells_stay_full_bright() {
    use crate::ui::theme::{ColorCapability, Theme, ThemePalette};
    use ratatui::buffer::Buffer;

    let theme = Theme {
        name: "system".to_string(),
        capability: ColorCapability::TrueColor,
        palette: ThemePalette {
            text: (200, 200, 200),
            subtext: (170, 170, 170),
            base: (36, 36, 48),
            surface: (48, 48, 64),
            buff: (60, 60, 80),
            accent: (140, 170, 220),
            accent2: (160, 200, 240),
            accent3: (130, 220, 200),
        },
    };

    let mut st = VectorState {
        phase: Phase::Recovering,
        w_cells: 40,
        h_cells: 20,
        ..Default::default()
    };
    // 轨迹点在单元格 (10, 10)；复合光栅 = 轨迹 | 粒子。
    st.trace_grid = vec![0; 40 * 20];
    st.trace_grid[10 * 40 + 10] = braille_bit(0, 0);
    st.grid = st.trace_grid.clone();
    // 一颗暗粒子（alpha=0.1）叠在轨迹格，另一颗落在空白格 (15, 10)。
    let dim = Twinkle::Dying {
        left: 0.1,
        total: 1.0,
    };
    st.particles = vec![
        Particle {
            x: 20.0,
            y: 40.0,
            twinkle: dim,
            ..new_scatter_particle((10, 10), 1, 80.0, 80.0)
        },
        Particle {
            x: 30.0,
            y: 40.0,
            twinkle: dim,
            ..new_scatter_particle((11, 10), 1, 80.0, 80.0)
        },
    ];
    st.stamp_particles();

    let area = Rect::new(0, 0, 40, 20);
    let mut buf = Buffer::empty(area);
    paint_with_alpha(&mut buf, area, &st.grid, &st.pixel_alpha, &theme, 40, 20);

    let row = 10usize;
    let t = row as f32 / 19.0;
    let wave = vertical_gradient_color(&theme, t);
    // 轨迹格（即便压着暗粒子）恒为波形色，不隐形也不被压暗。
    assert_eq!(buf.cell((10, 10)).unwrap().fg, wave);
    // 纯粒子格按自身透明度（0.1）向底色插值。
    let expect_dim = mix_colors(wave, theme.color_base(), 0.1);
    assert_eq!(buf.cell((15, 10)).unwrap().fg, expect_dim);
}
