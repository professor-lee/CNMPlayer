//! Coalesced completion notifications shared by the active UI and background workers.
use futures::future::poll_fn;
use futures::task::AtomicWaker;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::task::Poll;

#[derive(Debug, Default)]
struct State {
    pending: AtomicBool,
    waker: AtomicWaker,
}

/// One active UI consumer; cloned producers may notify from any thread.
#[derive(Debug, Clone, Default)]
pub(crate) struct WakeSignal(Arc<State>);

impl WakeSignal {
    pub fn notify(&self) {
        self.0.pending.store(true, Ordering::Release);
        self.0.waker.wake();
    }

    pub fn take(&self) -> bool {
        self.0.pending.swap(false, Ordering::AcqRel)
    }

    /// Register before the second check so a completion cannot fall between them.
    /// Dropping a pending waiter does not acknowledge the notification.
    pub async fn wait(&self) {
        poll_fn(|cx| {
            if self.take() {
                return Poll::Ready(());
            }
            self.0.waker.register(cx.waker());
            if self.take() {
                Poll::Ready(())
            } else {
                Poll::Pending
            }
        })
        .await
    }
}

#[cfg(test)]
mod tests;
