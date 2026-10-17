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
//! 快照允许丢批；增量频谱必须观察缺口，避免把丢批两侧当作连续 PCM。

use crate::tmplayer::audio::lufs_meter::LufsMeter;
use parking_lot::Mutex;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

/// 环容量（帧）。取 2 的幂以便用掩码回绕。
///
/// 65536 帧可容纳 384 kHz 的低帧率读取与 Cava 初始窗口；示波器、矢量仍
/// 按自身的时间窗口读取快照尾部，快照本身保留全部有效帧。
pub const CAPACITY: usize = 65536;
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
    /// 每次 [`PcmRing::reset`] 自增；两侧发现代号变化即视环为空。
    /// 这让 `reset` 只是一次原子自增，可以从任意线程（含音频线程）调用。
    generation: AtomicU64,
    /// 锁竞争时累计丢弃的非空批次；不占用环锁，读者也能观察无新帧的缺口。
    dropped_batches: AtomicU64,
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
    /// 与样本在同一把锁内发布，丢弃批次不能改变已存样本的采样率。
    sample_rate: u32,
    /// 成功提交的累计帧号，跨 reset 单调递增，而非环位置差值。
    written: u64,
    generation_start: u64,
    /// 起点之前的帧仍供 snapshot 使用，但不能跨丢批拼进一次增量读取。
    contiguous_start: u64,
    seen_drops: u64,
    /// Monotonic timestamp of the last successful non-empty push in this generation.
    updated_at: Option<Instant>,
}

/// Current PCM activity, published atomically with the ring's sample metadata.
#[derive(Debug, Clone, Copy)]
pub(crate) struct PcmActivity {
    pub generation: u64,
    pub frames: usize,
    pub updated_at: Option<Instant>,
}

impl Ring {
    /// 代号变化说明期间发生过 `reset`：丢弃全部内容。
    /// 无需清零样本数组 —— `filled` 已界定有效范围。
    fn sync_generation(&mut self, generation: u64) {
        if self.generation != generation {
            self.generation = generation;
            self.pos = 0;
            self.filled = 0;
            self.sample_rate = 0;
            self.updated_at = None;
            self.generation_start = self.written;
            self.contiguous_start = self.written;
        }
    }

    fn sync_drops(&mut self, dropped_batches: u64) {
        if self.seen_drops != dropped_batches {
            self.seen_drops = dropped_batches;
            self.contiguous_start = self.written;
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

/// 每个增量消费者独立持有，不清空共享环，也不影响 snapshot。
#[derive(Debug, Default)]
pub(crate) struct PcmReader {
    cursor: Option<ReadCursor>,
}

#[derive(Debug)]
struct ReadCursor {
    generation: u64,
    written: u64,
    dropped_batches: u64,
}

/// 增量复制结果，最旧帧在前，只有 `[..len]` 有效；复用实例时零分配。
#[derive(Debug)]
pub(crate) struct PcmBatch {
    pub left: Vec<f32>,
    pub right: Vec<f32>,
    pub len: usize,
    pub sample_rate: u32,
    /// 首次读取或源代号变化，须清窗口、autosens 和滤波状态。
    pub reset: bool,
    /// 写侧丢批或未读帧已被覆盖，仅须清窗口；无新帧时也可能为 true。
    pub discontinuity: bool,
}

impl Default for PcmBatch {
    fn default() -> Self {
        Self {
            left: vec![0.0; CAPACITY],
            right: vec![0.0; CAPACITY],
            len: 0,
            sample_rate: 0,
            reset: false,
            discontinuity: false,
        }
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
                sample_rate: 0,
                written: 0,
                generation_start: 0,
                contiguous_start: 0,
                seen_drops: 0,
                updated_at: None,
            }),
            generation: AtomicU64::new(0),
            dropped_batches: AtomicU64::new(0),
        }
    }

    /// 丢弃环内全部样本。切歌与跳转后必须调用，否则示波器会画出上一段音频。
    pub fn reset(&self) {
        self.generation.fetch_add(1, Ordering::Release);
    }
    /// Return the current sample count and last successful write time without consuming samples.
    pub(crate) fn activity(&self) -> PcmActivity {
        let mut ring = self.inner.lock();
        ring.sync_generation(self.generation.load(Ordering::Acquire));
        PcmActivity {
            generation: ring.generation,
            frames: ring.filled,
            updated_at: ring.updated_at,
        }
    }

    /// 写入一批帧，`left` 与 `right` 按较短者对齐。抢不到锁则整批丢弃。
    pub fn push(&self, left: &[f32], right: &[f32], sample_rate: u32) {
        let count = left.len().min(right.len());
        if count == 0 {
            return;
        }
        let Some(mut ring) = self.inner.try_lock() else {
            self.dropped_batches.fetch_add(1, Ordering::Release);
            return;
        };
        ring.sync_generation(self.generation.load(Ordering::Acquire));
        if ring.sample_rate != 0 && ring.sample_rate != sample_rate {
            // 不能把不同速率的样本放进同一代；采样率变更也重建频谱核心。
            let generation = self.generation.fetch_add(1, Ordering::AcqRel) + 1;
            ring.sync_generation(generation);
        }
        ring.sample_rate = sample_rate;
        ring.sync_drops(self.dropped_batches.load(Ordering::Acquire));

        // 单批超过容量时只保留最新一段：更早的部分本就会被同一批覆盖。
        let skip = count.saturating_sub(CAPACITY);
        let pos = ring.pos;
        write_wrapping(&mut ring.left, pos, &left[skip..count]);
        write_wrapping(&mut ring.right, pos, &right[skip..count]);

        let written = count - skip;
        ring.pos = (pos + written) & MASK;
        ring.filled = (ring.filled + written).min(CAPACITY);
        ring.written += count as u64;
        ring.updated_at = Some(Instant::now());
    }

    #[allow(dead_code)]
    /// Full non-destructive snapshot retained for diagnostics and PCM tests.
    pub fn snapshot(&self, out: &mut PcmSnapshot) {
        self.snapshot_inner(out, None);
    }

    /// 只复制指定时长的最新尾部。显示消费者须计入触发搜索区间；采样率与
    /// 样本在同一把锁内读取，避免切歌时以旧采样率截取新流。
    pub(crate) fn snapshot_tail_ms(&self, out: &mut PcmSnapshot, milliseconds: u32) {
        self.snapshot_inner(out, Some(milliseconds));
    }

    fn snapshot_inner(&self, out: &mut PcmSnapshot, milliseconds: Option<u32>) {
        let mut ring = self.inner.lock();
        // 代号必须在锁内读：锁外读会漏掉"读代号之后、取到锁之前"落地的 reset，
        // 那一帧就会画出跳转前的波形。
        ring.sync_generation(self.generation.load(Ordering::Acquire));
        out.sample_rate = ring.sample_rate;

        let len = milliseconds.map_or(ring.filled, |milliseconds| {
            let frames = u64::from(ring.sample_rate) * u64::from(milliseconds) / 1000;
            frames.min(ring.filled as u64) as usize
        });
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

    /// 非破坏性复制尚未读取的成功提交帧，之后立即释放锁，调用方再做 FFT。
    /// 全环回绕由累计帧号识别；覆盖仅保留现存尾部，并报告 discontinuity。
    /// 标志只描述本次读取，读者必须处理 len == 0 时的 reset/缺口。
    /// 每批只返回最近一段连续 PCM；丢批之前未读的帧留在 snapshot 中，
    /// 但不能与丢批后的帧拼接。丢批后尚无成功写入时返回 len == 0。
    pub(crate) fn read_since(&self, reader: &mut PcmReader, out: &mut PcmBatch) {
        out.len = 0;
        out.reset = false;
        out.discontinuity = false;
        let mut ring = self.inner.lock();
        ring.sync_generation(self.generation.load(Ordering::Acquire));
        out.sample_rate = ring.sample_rate;
        let dropped_batches = self.dropped_batches.load(Ordering::Acquire);
        ring.sync_drops(dropped_batches);
        let cursor = reader.cursor.as_ref();
        out.reset = cursor.is_none_or(|cursor| cursor.generation != ring.generation);
        let last_written = if out.reset {
            ring.generation_start
        } else {
            cursor.expect("an initialized reader has a cursor").written
        };
        let unread = ring.written - last_written;
        let continuous = ring.written - last_written.max(ring.contiguous_start);
        let len = continuous.min(ring.filled as u64) as usize;
        out.discontinuity = unread > ring.filled as u64
            || last_written < ring.contiguous_start
            || cursor.map_or(0, |cursor| cursor.dropped_batches) != dropped_batches;
        if len != 0 {
            let start = (ring.pos + CAPACITY - len) & MASK;
            read_wrapping(&ring.left, start, &mut out.left[..len]);
            read_wrapping(&ring.right, start, &mut out.right[..len]);
        }
        out.len = len;
        reader.cursor = Some(ReadCursor {
            generation: ring.generation,
            written: ring.written,
            dropped_batches,
        });
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
    lufs_meter: Option<Arc<LufsMeter>>,
    left: Box<[f32]>,
    right: Box<[f32]>,
    len: usize,
    channels: usize,
    sample_rate: u32,
}

impl PcmTap {
    pub fn new(
        ring: std::sync::Arc<PcmRing>,
        channels: usize,
        sample_rate: u32,
        lufs_meter: Option<Arc<LufsMeter>>,
    ) -> Self {
        if let Some(meter) = &lufs_meter {
            meter.configure_and_reset(channels, sample_rate);
        }
        Self {
            ring,
            lufs_meter,
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
        if let Some(meter) = &self.lufs_meter {
            meter.reset();
        }
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
        if let Some(meter) = &self.lufs_meter {
            meter.push_batch(&self.left[..self.len], &self.right[..self.len]);
        }
        self.len = 0;
    }
}

#[cfg(test)]
mod tests;
