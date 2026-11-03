use anyhow::{Result, anyhow};
use parking_lot::{Condvar, Mutex};
use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::mpsc::{self, Sender};
use std::thread::{self, JoinHandle};

const ORDINARY_CAPACITY: usize = 32;
const KEYED_CAPACITY: usize = 4;

type Job = Box<dyn FnOnce() -> Result<()> + Send + 'static>;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum PersistenceKey {
    Config,
    Session,
    Playback,
    PrivateRoam,
}

enum Item {
    Ordinary {
        epoch: u64,
        job: Job,
    },
    Keyed {
        key: PersistenceKey,
        epoch: u64,
        job: Job,
    },
    Flush {
        epoch: u64,
        done: Sender<Option<String>>,
    },
    Shutdown(Sender<()>),
}

struct State {
    accepting: bool,
    stopped: bool,
    epoch: u64,
    queue: VecDeque<Item>,
    errors: Vec<(u64, String)>,
}

struct Shared {
    state: Mutex<State>,
    wake: Condvar,
    barrier: Mutex<()>,
}

/// Cloneable submission endpoint for asynchronous producers. The worker owner
/// remains responsible for flushing and shutting down the shared queue.
#[derive(Clone)]
pub struct PersistenceHandle {
    shared: Arc<Shared>,
}

impl PersistenceHandle {
    pub fn enqueue<F>(&self, job: F) -> Result<()>
    where
        F: FnOnce() -> Result<()> + Send + 'static,
    {
        submit_ordinary(&self.shared, Box::new(job))
    }
}

/// Ordered, single-flight persistence worker. Ordinary jobs are bounded FIFO;
/// keyed jobs retain only the newest pending snapshot for each key.
pub struct PersistenceWorker {
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
}

impl PersistenceWorker {
    pub fn spawn(thread_name: &'static str) -> Result<Self> {
        let shared = Arc::new(Shared {
            state: Mutex::new(State {
                accepting: true,
                stopped: false,
                epoch: 0,
                queue: VecDeque::new(),
                errors: Vec::new(),
            }),
            barrier: Mutex::new(()),
            wake: Condvar::new(),
        });
        let worker_shared = shared.clone();
        let thread = thread::Builder::new()
            .name(thread_name.to_string())
            .spawn(move || worker_main(worker_shared))?;
        Ok(Self {
            shared,
            thread: Some(thread),
        })
    }

    pub fn handle(&self) -> PersistenceHandle {
        PersistenceHandle {
            shared: self.shared.clone(),
        }
    }

    /// Observe the first background failure without consuming a flush barrier.
    pub fn pending_error(&self) -> Option<String> {
        self.shared
            .state
            .lock()
            .errors
            .first()
            .map(|(_, error)| error.clone())
    }

    pub fn enqueue<F>(&self, job: F) -> Result<()>
    where
        F: FnOnce() -> Result<()> + Send + 'static,
    {
        submit_ordinary(&self.shared, Box::new(job))
    }

    pub fn enqueue_latest<F>(&self, key: PersistenceKey, job: F) -> Result<()>
    where
        F: FnOnce() -> Result<()> + Send + 'static,
    {
        submit_latest(&self.shared, key, Box::new(job))
    }

    pub fn flush(&self) -> Result<()> {
        let _barrier_guard = self.shared.barrier.lock();
        let (done_tx, done_rx) = mpsc::channel();
        {
            let mut state = self.shared.state.lock();
            if !state.accepting || state.stopped {
                return Err(worker_stopped_error(&state));
            }
            // Advancing the epoch makes a keyed submission on either side of
            // this marker ineligible to replace the other side's snapshot.
            let barrier_epoch = state.epoch;
            state.epoch = state.epoch.wrapping_add(1);
            state.queue.push_back(Item::Flush {
                epoch: barrier_epoch,
                done: done_tx,
            });
            self.shared.wake.notify_one();
        }
        let error = done_rx
            .recv()
            .map_err(|_| anyhow!("persistence worker stopped"))?;
        error.map_or(Ok(()), |error| Err(anyhow!("{error}")))
    }
}

impl Drop for PersistenceWorker {
    fn drop(&mut self) {
        let (done_tx, done_rx) = mpsc::channel();
        let should_wait = {
            let mut state = self.shared.state.lock();
            if state.accepting {
                // Closing admission before appending Shutdown makes every
                // cloned handle reject, while the marker drains all prior work.
                state.accepting = false;
                state.queue.push_back(Item::Shutdown(done_tx));
                self.shared.wake.notify_one();
                true
            } else {
                false
            }
        };
        if should_wait {
            let _ = done_rx.recv();
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn submit_ordinary(shared: &Arc<Shared>, job: Job) -> Result<()> {
    let mut state = shared.state.lock();
    if !state.accepting || state.stopped {
        return Err(worker_stopped_error(&state));
    }
    let ordinary_count = state
        .queue
        .iter()
        .filter(|item| matches!(item, Item::Ordinary { .. }))
        .count();
    if ordinary_count >= ORDINARY_CAPACITY {
        return Err(anyhow!(
            "persistence ordinary queue full (capacity {ORDINARY_CAPACITY})"
        ));
    }
    let epoch = state.epoch;
    state.queue.push_back(Item::Ordinary { epoch, job });
    shared.wake.notify_one();
    Ok(())
}

fn submit_latest(shared: &Arc<Shared>, key: PersistenceKey, job: Job) -> Result<()> {
    let mut state = shared.state.lock();
    if !state.accepting || state.stopped {
        return Err(worker_stopped_error(&state));
    }

    // A replacement is deliberately removed and appended: updates to another
    // key already in the queue retain their relative submission order.
    if let Some(index) = state.queue.iter().position(|item| {
        matches!(
            item,
            Item::Keyed {
                key: queued_key,
                epoch,
                ..
            } if *queued_key == key && *epoch == state.epoch
        )
    }) {
        let _ = state.queue.remove(index);
    } else {
        let keyed_count = state
            .queue
            .iter()
            .filter(|item| matches!(item, Item::Keyed { .. }))
            .count();
        if keyed_count >= KEYED_CAPACITY {
            let message = format!("persistence keyed queue full (capacity {KEYED_CAPACITY})");
            let epoch = state.epoch;
            record_error(&mut state, epoch, message.clone());
            return Err(anyhow!(message));
        }
    }
    let epoch = state.epoch;
    state.queue.push_back(Item::Keyed { key, epoch, job });
    shared.wake.notify_one();
    Ok(())
}

fn record_error(state: &mut State, epoch: u64, error: String) {
    if state.errors.iter().any(|(existing, _)| *existing == epoch) {
        return;
    }
    let index = state
        .errors
        .iter()
        .position(|(existing, _)| *existing > epoch)
        .unwrap_or(state.errors.len());
    state.errors.insert(index, (epoch, error));
}

fn worker_main(shared: Arc<Shared>) {
    worker_loop(&shared);
}

fn worker_loop(shared: &Arc<Shared>) {
    loop {
        let item = {
            let mut state = shared.state.lock();
            loop {
                if let Some(item) = state.queue.pop_front() {
                    break item;
                }
                if !state.accepting {
                    state.stopped = true;
                    shared.wake.notify_all();
                    return;
                }
                shared.wake.wait(&mut state);
            }
        };

        match item {
            Item::Ordinary { epoch, job } => run_job(shared, epoch, job),
            Item::Keyed { epoch, job, .. } => run_job(shared, epoch, job),
            Item::Flush { epoch, done } => {
                let error = {
                    let mut state = shared.state.lock();
                    state
                        .errors
                        .iter()
                        .position(|(error_epoch, _)| *error_epoch == epoch)
                        .map(|index| state.errors.remove(index).1)
                };
                let _ = done.send(error);
            }
            Item::Shutdown(done) => {
                let _ = done.send(());
                let mut state = shared.state.lock();
                state.stopped = true;
                shared.wake.notify_all();
                return;
            }
        }
    }
}

fn run_job(shared: &Arc<Shared>, epoch: u64, job: Job) {
    let error = match job() {
        Ok(()) => return,
        Err(error) => format!("{error:#}"),
    };
    log::error!("persistence worker job failed: {error}");
    let mut state = shared.state.lock();
    record_error(&mut state, epoch, error);
}

fn worker_stopped_error(state: &State) -> anyhow::Error {
    if state.stopped {
        anyhow!("persistence worker stopped")
    } else {
        anyhow!("persistence worker shutting down")
    }
}

#[cfg(test)]
mod tests;
