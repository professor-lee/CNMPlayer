//! macOS 音频输出：可以换设备流而不丢当前曲目的输出端。
//!
//! 系统里已没有任何输出设备时（拔掉耳机且没有扬声器等其他输出），cpal 的 CoreAudio 后端会
//! 停掉音频流并报 `DeviceNotAvailable`；之后插回设备只报 `DeviceChanged`，不会自己恢复。
//! rodio 又不暴露底层 `cpal::Stream`，流没法重新 play。所以 macOS 版在 `Player` 与设备流之间
//! 垫一层常驻混音器：`Player` 接在常驻混音器上，设备流只负责从中拉样本。恢复播放时换一条
//! 新的设备流接上同一个混音器，曲目、播放位置、音量都留在 `Player` 里不受影响。
//!
//! 设备流停掉期间没人拉样本，rodio 的跳转、停止都要等音频线程处理才会生效（跳转甚至会一直阻塞），
//! 所以这段时间由空转线程按实时速率拉取并丢弃（此时曲目已暂停，拉到的是静音），效果与 PipeWire
//! 上暂停后仍在运转的流相同。
//!
//! Linux 上 PipeWire 会让同一条流在设备回来后自动重新连接，不需要这一层。

use parking_lot::{Condvar, Mutex, MutexGuard};
use rodio::mixer::{Mixer, MixerSource};
use rodio::{ChannelCount, MixerDeviceSink, Sample, SampleRate, Source};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

/// 每次加锁从常驻混音器取多少帧。按整帧取，换流时新流不会从半帧开始而左右声道错位。
const FRAMES_PER_PULL: usize = 256;
/// 空转线程每次拉取的帧数（约 20 ms）；rodio 每 5 ms 输出检查一次跳转与停止。
const IDLE_FRAMES_PER_PULL: usize = 1024;

/// 当前设备流是否已被系统停掉。cpal 的设备监视线程置位，打开新流时清除。
#[derive(Clone, Default)]
pub(crate) struct OutputLoss(Arc<LossState>);

#[derive(Default)]
struct LossState {
    lost: Mutex<bool>,
    changed: Condvar,
}

impl OutputLoss {
    pub(crate) fn mark_lost(&self) {
        *self.0.lost.lock() = true;
        self.0.changed.notify_all();
    }

    /// 清除标记，返回之前是否处于失效状态。
    pub(crate) fn restore(&self) -> bool {
        std::mem::replace(&mut *self.0.lost.lock(), false)
    }

    pub(crate) fn is_lost(&self) -> bool {
        *self.0.lost.lock()
    }
}

pub(crate) struct OutputSink {
    device: MixerDeviceSink,
    mixer: Mixer,
    source: Arc<Mutex<MixerSource>>,
    _idle: IdlePump,
}

impl OutputSink {
    pub(crate) fn new(device: MixerDeviceSink, loss: OutputLoss) -> Self {
        let config = device.config();
        let (mixer, source) = rodio::mixer::mixer(config.channel_count(), config.sample_rate());
        let source = Arc::new(Mutex::new(source));
        device.mixer().add(SharedSource::new(source.clone()));
        Self {
            device,
            mixer,
            _idle: IdlePump::spawn(loss, source.clone()),
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

/// 设备流失效期间代替它拉取常驻混音器的线程；其余时间挂起等待。
struct IdlePump {
    loss: OutputLoss,
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl IdlePump {
    fn spawn(loss: OutputLoss, source: Arc<Mutex<MixerSource>>) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let handle = {
            let loss = loss.clone();
            let stop = stop.clone();
            std::thread::Builder::new()
                .name("cnmplayer-idle-output".into())
                .spawn(move || idle_pull(&loss, &stop, &source))
                .inspect_err(|err| log::warn!("idle output thread spawn failed: {err}"))
                .ok()
        };
        Self { loss, stop, handle }
    }
}

impl Drop for IdlePump {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        {
            // 持锁通知：线程要么在等待前看到 stop，要么被这次通知唤醒。
            let _lost = self.loss.0.lost.lock();
            self.loss.0.changed.notify_all();
        }
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

fn idle_pull(loss: &OutputLoss, stop: &AtomicBool, source: &Mutex<MixerSource>) {
    let (samples, period) = {
        let source = source.lock();
        let channels = usize::from(source.channels().get());
        let rate = f64::from(source.sample_rate().get());
        (
            IDLE_FRAMES_PER_PULL * channels,
            Duration::from_secs_f64(IDLE_FRAMES_PER_PULL as f64 / rate),
        )
    };
    let mut lost = loss.0.lost.lock();
    loop {
        if stop.load(Ordering::Acquire) {
            return;
        }
        if !*lost {
            loss.0.changed.wait(&mut lost);
            continue;
        }
        MutexGuard::unlocked(&mut lost, || {
            let mut source = source.lock();
            for _ in 0..samples {
                source.next();
            }
        });
        loss.0.changed.wait_for(&mut lost, period);
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
    use rodio::Player;
    use rodio::buffer::SamplesBuffer;
    use std::num::NonZero;
    use std::sync::mpsc;

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

    /// 设备流被系统停掉、曲目暂停时跳转仍会完成；否则跳转线程永远阻塞，退出时卡在 join 上。
    #[test]
    fn seek_completes_while_device_stream_is_lost() {
        let channels = NonZero::new(2).unwrap();
        let rate = NonZero::new(48_000).unwrap();
        let (mixer, source) = rodio::mixer::mixer(channels, rate);
        let source = Arc::new(Mutex::new(source));
        let loss = OutputLoss::default();
        let _idle = IdlePump::spawn(loss.clone(), source);

        let player = Arc::new(Player::connect_new(&mixer));
        player.append(SamplesBuffer::new(
            channels,
            rate,
            vec![0.0; 48_000 * 2 * 10],
        ));
        player.pause();
        loss.mark_lost();

        let (done_tx, done_rx) = mpsc::channel();
        let seeking = player.clone();
        std::thread::spawn(move || {
            let _ = done_tx.send(seeking.try_seek(Duration::from_secs(3)));
        });
        let result = done_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("seek never completed");
        assert!(result.is_ok());
        assert_eq!(player.get_pos(), Duration::from_secs(3));
    }
}
