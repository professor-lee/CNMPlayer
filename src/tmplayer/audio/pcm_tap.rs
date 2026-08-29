//! 播放链路上的 PCM 时域抽头：示波器据此绘制真实波形，而非由频谱反推。
//!
//! 线程模型是本模块唯一的设计约束。写侧跑在 cpal 的实时音频回调线程上
//! （rodio 把整条 `Source` 链逐样本拉取），由此得到三条规则：
//!
//! - 写侧**绝不阻塞**：`push` 用 `try_lock`，抢不到锁就整批丢弃。
//! - 写侧**绝不逐样本取锁**：44.1 kHz 立体声等于每秒 88200 次加锁。调用方
//!   （`EqSource`）经 [`PcmTap`] 攒满一批再落环，锁频率降到每秒约 86 次。
//! - 读侧（渲染线程）用阻塞的 `lock`：丢一帧快照会让画面闪空，而多等几微秒
//!   无人察觉。这个不对称是有意的。
//!
//! 丢一批约 12 ms，在可视化上不可感知，故写侧的丢弃策略是安全的。

use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

/// 环容量（帧）。取 2 的幂以便用掩码回绕。
///
/// 8192 帧 ≈ 186 ms @ 44.1 kHz，须同时容纳显示窗口与触发搜索区间
/// （见 `render::oscilloscope_renderer` 的 `WINDOW_MS` / `TRIGGER_SEARCH_MS`）。
pub const CAPACITY: usize = 8192;
const MASK: usize = CAPACITY - 1;

/// 单批帧数：[`PcmTap`] 的暂存容量，也是环的写入粒度。
///
/// 512 帧 @ 44.1 kHz ≈ 11.6 ms，远小于显示窗口，故波形"最新端"的滞后不可见；
/// 又足够大，使加锁频率相对逐样本降低三个数量级。
const FLUSH_FRAMES: usize = 512;

/// 音频线程与渲染线程之间的共享环形缓冲。
#[derive(Debug)]
pub struct PcmRing {
    inner: Mutex<Ring>,
    sample_rate: AtomicU32,
    /// 每次 [`PcmRing::reset`] 自增；两侧发现代号变化即视环为空。
    /// 这让 `reset` 只是一次原子自增，可以从任意线程（含音频线程）调用。
    generation: AtomicU64,
}

#[derive(Debug)]
struct Ring {
    left: Box<[f32]>,
    right: Box<[f32]>,
    /// 下一个写入下标
    pos: usize,
    /// 有效帧数，上限 [`CAPACITY`]
    filled: usize,
    generation: u64,
}

impl Ring {
    /// 代号变化说明期间发生过 `reset`：丢弃全部内容。
    /// 无需清零样本数组 —— `filled` 已界定有效范围。
    fn sync_generation(&mut self, generation: u64) {
        if self.generation != generation {
            self.generation = generation;
            self.pos = 0;
            self.filled = 0;
        }
    }
}

/// 线性化后的快照，最旧样本在前。调用方复用同一实例，渲染路径因此零分配。
#[derive(Debug)]
pub struct PcmSnapshot {
    /// 仅前 `len` 个元素有效
    pub left: Vec<f32>,
    pub right: Vec<f32>,
    pub len: usize,
    pub sample_rate: u32,
    /// 左右声道确有差异。单声道音源为 `false`，渲染侧据此跳过右声道，
    /// 避免把同一条曲线画两遍。
    pub stereo: bool,
}

impl Default for PcmSnapshot {
    fn default() -> Self {
        Self {
            left: vec![0.0; CAPACITY],
            right: vec![0.0; CAPACITY],
            len: 0,
            sample_rate: 0,
            stereo: false,
        }
    }
}

impl PcmSnapshot {
    pub fn clear(&mut self) {
        self.len = 0;
        self.stereo = false;
    }
}

impl PcmRing {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(Ring {
                left: vec![0.0; CAPACITY].into_boxed_slice(),
                right: vec![0.0; CAPACITY].into_boxed_slice(),
                pos: 0,
                filled: 0,
                generation: 0,
            }),
            sample_rate: AtomicU32::new(0),
            generation: AtomicU64::new(0),
        }
    }

    /// 丢弃环内全部样本。切歌与跳转后必须调用，否则示波器会画出上一段音频。
    pub fn reset(&self) {
        self.generation.fetch_add(1, Ordering::Release);
    }

    /// 写入一批帧，`left` 与 `right` 按较短者对齐。抢不到锁则整批丢弃。
    pub fn push(&self, left: &[f32], right: &[f32], sample_rate: u32) {
        let count = left.len().min(right.len());
        if count == 0 {
            return;
        }
        self.sample_rate.store(sample_rate, Ordering::Relaxed);

        let Ok(mut ring) = self.inner.try_lock() else {
            return;
        };
        ring.sync_generation(self.generation.load(Ordering::Acquire));

        // 单批超过容量时只保留最新一段：更早的部分本就会被同一批覆盖。
        let skip = count.saturating_sub(CAPACITY);
        let pos = ring.pos;
        write_wrapping(&mut ring.left, pos, &left[skip..count]);
        write_wrapping(&mut ring.right, pos, &right[skip..count]);

        let written = count - skip;
        ring.pos = (pos + written) & MASK;
        ring.filled = (ring.filled + written).min(CAPACITY);
    }

    /// 把环线性化进 `out`。渲染线程调用，允许短暂阻塞。
    pub fn snapshot(&self, out: &mut PcmSnapshot) {
        out.sample_rate = self.sample_rate.load(Ordering::Relaxed);

        let generation = self.generation.load(Ordering::Acquire);
        let Ok(mut ring) = self.inner.lock() else {
            out.clear();
            return;
        };
        ring.sync_generation(generation);

        let len = ring.filled;
        out.len = len;
        if len == 0 {
            out.stereo = false;
            return;
        }

        let start = (ring.pos + CAPACITY - len) & MASK;
        read_wrapping(&ring.left, start, &mut out.left[..len]);
        read_wrapping(&ring.right, start, &mut out.right[..len]);
        drop(ring);

        // 单声道音源的左右样本由 PcmTap 逐位复制而来，故精确比较即可，无需 epsilon。
        out.stereo = out.left[..len] != out.right[..len];
    }
}

impl Default for PcmRing {
    fn default() -> Self {
        Self::new()
    }
}

/// 把 `src` 写进环形数组 `dst` 的 `pos` 处，跨越末端时回绕。要求 `src.len() <= dst.len()`。
fn write_wrapping(dst: &mut [f32], pos: usize, src: &[f32]) {
    let head = (dst.len() - pos).min(src.len());
    dst[pos..pos + head].copy_from_slice(&src[..head]);
    dst[..src.len() - head].copy_from_slice(&src[head..]);
}

/// 从环形数组 `src` 的 `pos` 处连续读满 `dst`，跨越末端时回绕。
fn read_wrapping(src: &[f32], pos: usize, dst: &mut [f32]) {
    let head = (src.len() - pos).min(dst.len());
    dst[..head].copy_from_slice(&src[pos..pos + head]);
    let tail = dst.len() - head;
    dst[head..].copy_from_slice(&src[..tail]);
}

/// 抽头的写入端：每样本调用 [`PcmTap::push_sample`]，攒满一批自动落环。
///
/// 暂存数组私有且无任何同步，因此每样本的开销只是一次数组写入 —— 相对
/// `EqSource` 本身每样本 10 次原子读加 10 级 biquad，属噪声级。
#[derive(Debug)]
pub struct PcmTap {
    ring: std::sync::Arc<PcmRing>,
    left: Box<[f32]>,
    right: Box<[f32]>,
    len: usize,
    channels: usize,
    sample_rate: u32,
}

impl PcmTap {
    pub fn new(ring: std::sync::Arc<PcmRing>, channels: usize, sample_rate: u32) -> Self {
        Self {
            ring,
            left: vec![0.0; FLUSH_FRAMES].into_boxed_slice(),
            right: vec![0.0; FLUSH_FRAMES].into_boxed_slice(),
            len: 0,
            channels: channels.max(1),
            sample_rate,
        }
    }

    /// 送入一个样本，`channel` 为其在当前帧内的声道序号。
    pub fn push_sample(&mut self, channel: usize, sample: f32) {
        match channel {
            0 => self.left[self.len] = sample,
            1 => self.right[self.len] = sample,
            // 多声道音源只取前两路：示波器画的是立体声像，其余声道无处安放。
            _ => {}
        }

        if channel + 1 < self.channels {
            return;
        }
        if self.channels == 1 {
            self.right[self.len] = self.left[self.len];
        }

        self.len += 1;
        if self.len == self.left.len() {
            self.flush();
        }
    }

    /// 跳转后调用：暂存与环内的样本都属于跳转前的位置。
    pub fn reset(&mut self) {
        self.len = 0;
        self.ring.reset();
    }

    fn flush(&mut self) {
        if self.len == 0 {
            return;
        }
        self.ring.push(
            &self.left[..self.len],
            &self.right[..self.len],
            self.sample_rate,
        );
        self.len = 0;
    }
}
