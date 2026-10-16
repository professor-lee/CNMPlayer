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
