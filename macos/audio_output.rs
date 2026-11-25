//! macOS 音频输出：可以换设备流而不丢当前曲目的输出端。
//!
//! 系统里已没有任何输出设备时（拔掉耳机且没有扬声器等其他输出），cpal 的 CoreAudio 后端会
//! 停掉音频流并报 `DeviceNotAvailable`；之后插回设备只报 `DeviceChanged`，不会自己恢复。
//! rodio 又不暴露底层 `cpal::Stream`，流没法重新 play。所以 macOS 版在 `Player` 与设备流之间
//! 垫一层常驻混音器：`Player` 接在常驻混音器上，设备流只负责从中拉样本。恢复播放时换一条
//! 新的设备流接上同一个混音器，曲目、播放位置、音量都留在 `Player` 里不受影响。
//!
//! Linux 上 PipeWire 会让同一条流在设备回来后自动重新连接，不需要这一层。

use parking_lot::Mutex;
use rodio::mixer::{Mixer, MixerSource};
use rodio::{ChannelCount, MixerDeviceSink, Sample, SampleRate, Source};
use std::sync::Arc;
use std::time::Duration;

/// 每次加锁从常驻混音器取多少帧。按整帧取，换流时新流不会从半帧开始而左右声道错位。
const FRAMES_PER_PULL: usize = 256;

pub(crate) struct OutputSink {
    device: MixerDeviceSink,
    mixer: Mixer,
    source: Arc<Mutex<MixerSource>>,
}

impl OutputSink {
    pub(crate) fn new(device: MixerDeviceSink) -> Self {
        let config = device.config();
        let (mixer, source) = rodio::mixer::mixer(config.channel_count(), config.sample_rate());
        let source = Arc::new(Mutex::new(source));
        device.mixer().add(SharedSource::new(source.clone()));
        Self {
            device,
            mixer,
            source,
        }
    }

    /// `Player` 要接的混音器；换设备流后保持不变。
    pub(crate) fn mixer(&self) -> &Mixer {
        &self.mixer
    }

    /// 让新的设备流接管输出。旧流（已被 cpal 停掉）随之释放。
    pub(crate) fn reattach(&mut self, device: MixerDeviceSink) {
        device.mixer().add(SharedSource::new(self.source.clone()));
        self.device = device;
    }
}

/// 设备流里的一路输入：从常驻混音器成块取样本。混音器空了也输出静音而不是结束，
/// 否则设备流会把这一路移除，之后再接上的 `Player` 就没有声音。
struct SharedSource {
    inner: Arc<Mutex<MixerSource>>,
    channels: ChannelCount,
    sample_rate: SampleRate,
    /// 整帧数的样本块；`pos` 到末尾时整块重取。
    buffer: Box<[Sample]>,
    pos: usize,
}

impl SharedSource {
    fn new(inner: Arc<Mutex<MixerSource>>) -> Self {
        let (channels, sample_rate) = {
            let source = inner.lock();
            (source.channels(), source.sample_rate())
        };
        let len = FRAMES_PER_PULL * usize::from(channels.get());
        Self {
            inner,
            channels,
            sample_rate,
            buffer: vec![0.0; len].into_boxed_slice(),
            pos: len,
        }
    }

    fn refill(&mut self) {
        let mut source = self.inner.lock();
        for slot in self.buffer.iter_mut() {
            *slot = source.next().unwrap_or(0.0);
        }
        self.pos = 0;
    }
}

impl Iterator for SharedSource {
    type Item = Sample;

    fn next(&mut self) -> Option<Sample> {
        if self.pos == self.buffer.len() {
            self.refill();
        }
        let sample = self.buffer[self.pos];
        self.pos += 1;
        Some(sample)
    }
}

impl Source for SharedSource {
    fn current_span_len(&self) -> Option<usize> {
        None
    }

    fn channels(&self) -> ChannelCount {
        self.channels
    }

    fn sample_rate(&self) -> SampleRate {
        self.sample_rate
    }

    fn total_duration(&self) -> Option<Duration> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rodio::buffer::SamplesBuffer;
    use std::num::NonZero;

    fn shared_mixer() -> (Mixer, Arc<Mutex<MixerSource>>) {
        let (mixer, source) =
            rodio::mixer::mixer(NonZero::new(2).unwrap(), NonZero::new(48_000).unwrap());
        (mixer, Arc::new(Mutex::new(source)))
    }

    /// 混音器里暂时没有 Player 时输出静音而不结束，设备流才不会把这一路移除。
    #[test]
    fn empty_mixer_yields_silence_instead_of_ending() {
        let (_mixer, source) = shared_mixer();
        let mut shared = SharedSource::new(source);
        assert!((0..FRAMES_PER_PULL * 4).all(|_| shared.next() == Some(0.0)));
    }

    /// 旧流停在半帧上，新流接管后左右声道仍然对齐。
    #[test]
    fn reattached_stream_keeps_channel_order() {
        let (mixer, source) = shared_mixer();
        let frames = FRAMES_PER_PULL * 4;
        let samples: Vec<Sample> = (0..frames).flat_map(|_| [1.0, -1.0]).collect();
        mixer.add(SamplesBuffer::new(
            NonZero::new(2).unwrap(),
            NonZero::new(48_000).unwrap(),
            samples,
        ));

        let mut old = SharedSource::new(source.clone());
        for _ in 0..3 {
            old.next();
        }
        drop(old);

        let mut new = SharedSource::new(source);
        for _ in 0..FRAMES_PER_PULL {
            assert_eq!(new.next(), Some(1.0));
            assert_eq!(new.next(), Some(-1.0));
        }
    }
}
