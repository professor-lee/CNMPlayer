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
mod tests {
    use super::*;
    use futures::{FutureExt, pin_mut};

    #[test]
    fn early_notifications_coalesce_and_pending_cancellation_keeps_completion() {
        let wake = WakeSignal::default();
        wake.notify();
        wake.notify();
        assert_eq!(wake.wait().now_or_never(), Some(()));
        assert!(wake.wait().now_or_never().is_none());
        wake.notify();
        assert_eq!(wake.wait().now_or_never(), Some(()));
        assert!(!wake.take());
    }

    #[test]
    fn completion_after_registration_wakes_the_consumer() {
        let wake = WakeSignal::default();
        let waiter = wake.wait();
        pin_mut!(waiter);
        assert!(waiter.as_mut().now_or_never().is_none());
        let producer = wake.clone();
        std::thread::spawn(move || producer.notify())
            .join()
            .unwrap();
        assert_eq!(waiter.as_mut().now_or_never(), Some(()));
    }
}
