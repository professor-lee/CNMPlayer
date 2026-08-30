//! 实时 Momentary LUFS 计量器（EBU R128 / ITU-R BS.1770 的 K 计权 + 400ms 窗口）。
//!
//! 计量发生在音频链路内：`EqSource` 通过 `PcmTap` 每 512 帧把一批样本送入
//! 这里，因此不依赖 UI 线程对 PCM 环做全窗重滤波，也不受界面掉帧影响。
//!
//! 窗口实现为 8 个 50ms 子块，子块能量就绪时发布一次最近 400ms 的均方能量。
//! UI 只读取线性均方值；是否换算 LUFS、功率平均、显示端 attack/release
//! 都由读取方决定。

use std::sync::Mutex;

/// 每个子块的时长。Momentary 窗口 = 8 * 50ms = 400ms。
const SUB_BLOCK_SECS: f64 = 0.050;
const WINDOW_BLOCKS: usize = 8;
/// 数字原型采样率。EBU R128 给出的滤波器系数以 48kHz 为原型，
/// 其他采样率通过该原型反演模拟滤波器后重新双线性变换得到。
const PROTOTYPE_RATE: f64 = 48_000.0;

#[derive(Debug, Clone, Copy)]
struct Biquad {
    b0: f64,
    b1: f64,
    b2: f64,
    a1: f64,
    a2: f64,
}

#[derive(Debug, Clone, Copy)]
struct Prototype {
    b0: f64,
    b1: f64,
    b2: f64,
    a1: f64,
    a2: f64,
}

const SHELF_PROTO: Prototype = Prototype {
    b0: 1.535_124_859_586_97,
    b1: -2.691_696_189_406_38,
    b2: 1.198_392_810_852_85,
    a1: -1.690_659_293_182_41,
    a2: 0.732_480_774_215_85,
};

const HIGH_PASS_PROTO: Prototype = Prototype {
    b0: 1.0,
    b1: -2.0,
    b2: 1.0,
    a1: -1.990_047_454_833_98,
    a2: 0.990_072_250_366_21,
};

#[derive(Debug, Clone, Copy, Default)]
pub struct LufsReading {
    pub left_mean_square: f32,
    pub right_mean_square: f32,
    pub generation: u64,
}

#[derive(Debug)]
struct FilterState {
    x1: f64,
    x2: f64,
    y1: f64,
    y2: f64,
}

impl FilterState {
    fn reset(&mut self) {
        self.x1 = 0.0;
        self.x2 = 0.0;
        self.y1 = 0.0;
        self.y2 = 0.0;
    }
}

impl Default for FilterState {
    fn default() -> Self {
        Self {
            x1: 0.0,
            x2: 0.0,
            y1: 0.0,
            y2: 0.0,
        }
    }
}

#[derive(Debug)]
struct ChannelMeter {
    filters: [Biquad; 2],
    state: [FilterState; 2],
    acc: f64,
    count: usize,
}

impl ChannelMeter {
    fn new(filters: [Biquad; 2]) -> Self {
        Self {
            filters,
            state: Default::default(),
            acc: 0.0,
            count: 0,
        }
    }

    fn reset(&mut self) {
        for state in &mut self.state {
            state.reset();
        }
        self.acc = 0.0;
        self.count = 0;
    }

    fn push(&mut self, sample: f64) -> f64 {
        let x = sample;
        let stage0 = process_biquad(&self.filters[0], &mut self.state[0], x);
        let y = process_biquad(&self.filters[1], &mut self.state[1], stage0);
        self.acc += y * y;
        self.count = self.count.saturating_add(1);
        y
    }
}

#[derive(Debug)]
struct State {
    sample_rate: u32,
    channels: usize,
    sub_frames: usize,
    left: ChannelMeter,
    right: ChannelMeter,
    window_left: [f64; WINDOW_BLOCKS],
    window_right: [f64; WINDOW_BLOCKS],
    window_pos: usize,
    valid_blocks: usize,
    generation: u64,
    published: LufsReading,
}

#[derive(Debug)]
pub struct LufsMeter {
    inner: Mutex<State>,
}

impl Default for LufsMeter {
    fn default() -> Self {
        Self::new()
    }
}

impl LufsMeter {
    pub fn new() -> Self {
        let filters = k_weighting_filters(PROTOTYPE_RATE);
        Self {
            inner: Mutex::new(State {
                sample_rate: 0,
                channels: 2,
                sub_frames: 1,
                left: ChannelMeter::new(filters),
                right: ChannelMeter::new(filters),
                window_left: [0.0; WINDOW_BLOCKS],
                window_right: [0.0; WINDOW_BLOCKS],
                window_pos: 0,
                valid_blocks: 0,
                generation: 0,
                published: LufsReading::default(),
            }),
        }
    }

    /// 在音频源建立时配置采样率与声道数，并清空旧计量状态。
    pub fn configure_and_reset(&self, channels: usize, sample_rate: u32) {
        let channels = channels.clamp(1, 2);
        let sample_rate = sample_rate.max(1);
        let filters = k_weighting_filters(sample_rate as f64);
        let sub_frames = ((sample_rate as f64 * SUB_BLOCK_SECS).round() as usize).max(1);

        if let Ok(mut state) = self.inner.lock() {
            state.sample_rate = sample_rate;
            state.channels = channels;
            state.sub_frames = sub_frames;
            state.left = ChannelMeter::new(filters);
            state.right = ChannelMeter::new(filters);
            state.window_left = [0.0; WINDOW_BLOCKS];
            state.window_right = [0.0; WINDOW_BLOCKS];
            state.window_pos = 0;
            state.valid_blocks = 0;
            state.generation = state.generation.wrapping_add(1);
            state.published = LufsReading::default();
        }
    }

    /// 跳转/换歌时清空滤波器与子块窗口，避免把上一段音频计入当前响度。
    ///
    /// 可能被音频链路调用：与批量写入一样不等待锁。
    pub fn reset(&self) {
        if let Ok(mut state) = self.inner.try_lock() {
            state.left.reset();
            state.right.reset();
            state.window_left = [0.0; WINDOW_BLOCKS];
            state.window_right = [0.0; WINDOW_BLOCKS];
            state.window_pos = 0;
            state.valid_blocks = 0;
            state.generation = state.generation.wrapping_add(1);
            state.published = LufsReading::default();
        }
    }

    /// 音频线程批量写入。抢不到锁就整批丢弃：LUFS 只是显示计量，
    /// 缺一个 50ms 子块对 400ms 窗口影响可忽略，音频线程绝不阻塞。
    pub fn push_batch(&self, left: &[f32], right: &[f32]) {
        let count = left.len().min(right.len());
        if count == 0 {
            return;
        }
        let Ok(mut state) = self.inner.try_lock() else {
            return;
        };
        if state.sample_rate == 0 {
            return;
        }

        for index in 0..count {
            let l = f64::from(left[index]);
            let r = f64::from(right[index]);
            let ly = state.left.push(l);
            if state.channels == 1 {
                state.right.acc += ly * ly;
                state.right.count = state.right.count.saturating_add(1);
            } else {
                let _ = state.right.push(r);
            }

            if state.left.count >= state.sub_frames && state.right.count >= state.sub_frames {
                finalize_block(&mut state);
            }
        }
    }

    /// 读取最近一次 400ms 窗口的左右声道均方能量。
    pub fn latest(&self) -> LufsReading {
        self.inner
            .lock()
            .map(|state| state.published)
            .unwrap_or_default()
    }
}

fn finalize_block(state: &mut State) {
    let left_ms = state.left.acc / state.left.count.max(1) as f64;
    let right_ms = state.right.acc / state.right.count.max(1) as f64;
    state.left.acc = 0.0;
    state.left.count = 0;
    state.right.acc = 0.0;
    state.right.count = 0;

    state.window_left[state.window_pos] = left_ms;
    state.window_right[state.window_pos] = right_ms;
    state.window_pos = (state.window_pos + 1) % WINDOW_BLOCKS;
    state.valid_blocks = (state.valid_blocks + 1).min(WINDOW_BLOCKS);

    let blocks = state.valid_blocks.max(1);
    let left_sum = state.window_left.iter().take(blocks).sum::<f64>();
    let right_sum = state.window_right.iter().take(blocks).sum::<f64>();
    state.generation = state.generation.wrapping_add(1);
    state.published = LufsReading {
        left_mean_square: (left_sum / blocks as f64) as f32,
        right_mean_square: (right_sum / blocks as f64) as f32,
        generation: state.generation,
    };
}

fn process_biquad(filter: &Biquad, state: &mut FilterState, x: f64) -> f64 {
    let y = filter.b0 * x + filter.b1 * state.x1 + filter.b2 * state.x2
        - filter.a1 * state.y1
        - filter.a2 * state.y2;
    state.x2 = state.x1;
    state.x1 = x;
    state.y2 = state.y1;
    state.y1 = y;
    y
}

/// 由 48kHz 数字原型反演模拟滤波器，再按目标采样率做双线性变换。
///
/// EBU R128 的 K 计权包含两级：
/// 1. +4dB high shelf；
/// 2. 高通（RLB）级。
fn k_weighting_filters(sample_rate: f64) -> [Biquad; 2] {
    let mut filters = [
        prototype_to_biquad(&SHELF_PROTO, sample_rate),
        prototype_to_biquad(&HIGH_PASS_PROTO, sample_rate),
    ];

    // 标准 44.1/48kHz 高通级分子都写成 [1, -2, 1]；反演变换只保证比例，
    // 这里恢复标准写法，避免引入接近 1 但非 1 的增益差异。
    let hp = &mut filters[1];
    let scale = hp.b0;
    if scale.abs() > 1.0e-12 {
        hp.b0 /= scale;
        hp.b1 /= scale;
        hp.b2 /= scale;
    }
    filters
}

fn prototype_to_biquad(proto: &Prototype, sample_rate: f64) -> Biquad {
    // 数字 H(z) = (b0 + b1 z^-1 + b2 z^-2)/(1 + a1 z^-1 + a2 z^-2)
    // 在 48kHz 下反演双线性变换，得到模拟 H(s) 的分子/分母系数：
    // A2 s^2 + A1 s + A0。
    let fs0 = PROTOTYPE_RATE;
    let a2 = proto.b0 - proto.b1 + proto.b2;
    let a1 = 4.0 * fs0 * (proto.b0 - proto.b2);
    let a0 = 4.0 * fs0 * fs0 * (proto.b0 + proto.b1 + proto.b2);

    let b2 = 1.0 - proto.a1 + proto.a2;
    let b1 = 4.0 * fs0 * (1.0 - proto.a2);
    let b0 = 4.0 * fs0 * fs0 * (1.0 + proto.a1 + proto.a2);

    // 再按目标采样率双线性变换 s = 2fs (z-1)/(z+1)：
    // N(z) = A2*4fs^2*(z-1)^2 + A1*2fs*(z-1)(z+1) + A0*(z+1)^2。
    let fs = sample_rate.max(1.0);
    let fs2 = fs * fs;
    let n2 = a2 * 4.0 * fs2 + a1 * 2.0 * fs + a0;
    let n1 = -a2 * 8.0 * fs2 + 2.0 * a0;
    let n0 = a2 * 4.0 * fs2 - a1 * 2.0 * fs + a0;

    let d2 = b2 * 4.0 * fs2 + b1 * 2.0 * fs + b0;
    let d1 = -b2 * 8.0 * fs2 + 2.0 * b0;
    let d0 = b2 * 4.0 * fs2 - b1 * 2.0 * fs + b0;

    Biquad {
        b0: n2 / d2,
        b1: n1 / d2,
        b2: n0 / d2,
        a1: d1 / d2,
        a2: d0 / d2,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shelf_filter_matches_48k_prototype() {
        let filter = prototype_to_biquad(&SHELF_PROTO, 48_000.0);
        assert!((filter.b0 - SHELF_PROTO.b0).abs() < 1.0e-9);
        assert!((filter.b1 - SHELF_PROTO.b1).abs() < 1.0e-9);
        assert!((filter.b2 - SHELF_PROTO.b2).abs() < 1.0e-9);
        assert!((filter.a1 - SHELF_PROTO.a1).abs() < 1.0e-9);
        assert!((filter.a2 - SHELF_PROTO.a2).abs() < 1.0e-9);
    }

    #[test]
    fn shelf_filter_matches_known_44_1k_values() {
        let filter = prototype_to_biquad(&SHELF_PROTO, 44_100.0);
        assert!((filter.b0 - 1.530_880_324_135_5).abs() < 1.0e-6);
        assert!((filter.b1 - -2.651_351_292_867_2).abs() < 1.0e-6);
        assert!((filter.b2 - 1.169_344_026_329_0).abs() < 1.0e-6);
        assert!((filter.a1 - -1.663_901_800_496_3).abs() < 1.0e-6);
        assert!((filter.a2 - 0.712_774_858_093_6).abs() < 1.0e-6);
    }

    #[test]
    fn high_pass_filter_keeps_standard_b_coefficients() {
        let filters = k_weighting_filters(44_100.0);
        assert!((filters[1].b0 - 1.0).abs() < 1.0e-12);
        assert!((filters[1].b1 + 2.0).abs() < 1.0e-12);
        assert!((filters[1].b2 - 1.0).abs() < 1.0e-12);
    }

    #[test]
    fn silence_publishes_zero_mean_square() {
        let meter = LufsMeter::new();
        meter.configure_and_reset(2, 48_000);
        let samples = vec![0.0f32; 4800];
        meter.push_batch(&samples, &samples);
        let reading = meter.latest();
        assert_eq!(reading.left_mean_square, 0.0);
        assert_eq!(reading.right_mean_square, 0.0);
        assert!(reading.generation > 0);
    }
}
