use super::super::player::SeekState;
use super::super::player::SeekWorker;
use parking_lot::Condvar;
use parking_lot::Mutex;
use rodio::Player;
use rodio::Source;
use rodio::source::SeekError;
use std::num::NonZero;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::sync::mpsc;
use std::sync::mpsc::Receiver as TestReceiver;
use std::sync::mpsc::Sender;
use std::thread::JoinHandle;
use std::time::Duration;

#[derive(Default)]
struct Gate {
    open: Mutex<bool>,
    wake: Condvar,
}

impl Gate {
    fn wait(&self) {
        let mut open = self.open.lock();
        while !*open {
            self.wake.wait(&mut open);
        }
    }

    fn release(&self) {
        *self.open.lock() = true;
        self.wake.notify_all();
    }
}

struct ProbeSource {
    id: u8,
    seeks: Sender<(u8, Duration)>,
    gate: Arc<Gate>,
}

impl Iterator for ProbeSource {
    type Item = f32;

    fn next(&mut self) -> Option<f32> {
        Some(0.0)
    }
}

impl Source for ProbeSource {
    fn current_span_len(&self) -> Option<usize> {
        None
    }

    fn channels(&self) -> rodio::ChannelCount {
        NonZero::new(1).unwrap()
    }

    fn sample_rate(&self) -> rodio::SampleRate {
        NonZero::new(1000).unwrap()
    }

    fn total_duration(&self) -> Option<Duration> {
        Some(Duration::from_secs(10))
    }

    fn try_seek(&mut self, target: Duration) -> std::result::Result<(), SeekError> {
        self.seeks.send((self.id, target)).unwrap();
        self.gate.wait();
        Ok(())
    }
}

struct Probe {
    worker: SeekWorker,
    mixer: rodio::mixer::Mixer,
    events: TestReceiver<(u8, Duration)>,
    sender: Sender<(u8, Duration)>,
    gate: Arc<Gate>,
    render_stop: Arc<AtomicBool>,
    render: Option<JoinHandle<()>>,
}

impl Probe {
    fn new(execute: impl Fn(Arc<Player>, Duration) + Send + 'static) -> Self {
        let (mixer, mut output) =
            rodio::mixer::mixer(NonZero::new(1).unwrap(), NonZero::new(1000).unwrap());
        let render_stop = Arc::new(AtomicBool::new(false));
        let stop = render_stop.clone();
        let render = std::thread::spawn(move || {
            while !stop.load(Ordering::SeqCst) {
                // A clocked software audio callback, not an executor poll.
                for _ in 0..5 {
                    let _ = output.next();
                }
                std::thread::sleep(Duration::from_millis(5));
            }
        });
        let (sender, events) = mpsc::channel();
        Self {
            worker: SeekWorker::with_executor(Arc::new(Mutex::new(SeekState::default())), execute),
            mixer,
            events,
            sender,
            gate: Arc::new(Gate::default()),
            render_stop,
            render: Some(render),
        }
    }

    fn player(&self, id: u8) -> Arc<Player> {
        let player = Arc::new(Player::connect_new(&self.mixer));
        player.append(ProbeSource {
            id,
            seeks: self.sender.clone(),
            gate: self.gate.clone(),
        });
        player
    }

    fn event(&self) -> (u8, Duration) {
        self.events.recv_timeout(Duration::from_secs(3)).unwrap()
    }

    fn idle(&self) {
        let mut state = self.worker.state.lock();
        while state.in_flight || state.pending.is_some() {
            let wait = self
                .worker
                .wake
                .wait_for(&mut state, Duration::from_secs(3));
            assert!(!wait.timed_out(), "seek worker did not become idle");
        }
    }
}

impl Drop for Probe {
    fn drop(&mut self) {
        {
            let mut state = self.worker.state.lock();
            state.shutdown = true;
            state.pending = None;
        }
        self.gate.release();
        self.worker.shutdown();
        self.render_stop.store(true, Ordering::SeqCst);
        if let Some(render) = self.render.take() {
            let _ = render.join();
        }
    }
}

#[test]
fn seek_worker_singleflight_coalesces_latest() {
    let probe = Probe::new(|player, target| {
        let _ = player.try_seek(target);
    });
    let player = probe.player(1);
    probe.worker.submit(player.clone(), Duration::from_secs(1));
    assert_eq!(probe.event(), (1, Duration::from_secs(1)));
    for seconds in [2, 3, 4] {
        probe
            .worker
            .submit(player.clone(), Duration::from_secs(seconds));
    }
    assert!(probe.worker.state.lock().in_flight);
    assert_eq!(
        probe.worker.state.lock().pending_target,
        Some(Duration::from_secs(4))
    );
    probe.gate.release();
    assert_eq!(probe.event(), (1, Duration::from_secs(4)));
    probe.idle();
    assert!(probe.events.try_recv().is_err());
    assert!(probe.worker.state.lock().pending_target.is_none());
}

#[test]
fn seek_worker_invalidate_drops_pending_and_preserves_new_intent() {
    let probe = Probe::new(|player, target| {
        let _ = player.try_seek(target);
    });
    let old = probe.player(1);
    probe.worker.submit(old.clone(), Duration::from_secs(1));
    assert_eq!(probe.event(), (1, Duration::from_secs(1)));
    probe.worker.submit(old.clone(), Duration::from_secs(2));
    // stop/change/rebuild all use this invalidation before replacing queues.
    probe.worker.invalidate();
    assert!(probe.worker.state.lock().pending_target.is_none());
    old.stop();
    let new = probe.player(2);
    probe.worker.submit(new, Duration::from_secs(3));
    probe.gate.release();
    assert_eq!(probe.event(), (2, Duration::from_secs(3)));
    probe.idle();
    assert!(probe.events.try_recv().is_err());
}

#[test]
fn seek_worker_admitted_old_request_cannot_seek_fresh_player() {
    let before_call = Arc::new(Gate::default());
    let frozen = before_call.clone();
    let (entered_tx, entered_rx) = mpsc::channel();
    let probe = Probe::new(move |player, target| {
        entered_tx.send(target).unwrap();
        frozen.wait();
        let _ = player.try_seek(target);
    });
    struct ReleaseOnDrop(Arc<Gate>);
    impl Drop for ReleaseOnDrop {
        fn drop(&mut self) {
            self.0.release();
        }
    }
    // Declared after Probe so assertion unwinding opens the admission gate
    // before Probe joins its worker.
    let _release = ReleaseOnDrop(before_call.clone());
    let old = probe.player(1);
    probe.gate.release();
    probe.worker.submit(old.clone(), Duration::from_secs(1));
    assert_eq!(
        entered_rx.recv_timeout(Duration::from_secs(3)).unwrap(),
        Duration::from_secs(1)
    );
    probe.worker.invalidate();
    old.stop();
    let new = probe.player(2);
    probe.worker.submit(new, Duration::from_secs(4));
    before_call.release();
    let first = probe.event();
    if first == (1, Duration::from_secs(1)) {
        assert_eq!(probe.event(), (2, Duration::from_secs(4)));
    } else {
        assert_eq!(first, (2, Duration::from_secs(4)));
    }
    probe.idle();
    assert!(probe.events.try_recv().is_err());
}
