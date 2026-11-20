use super::{OutputEvents, error_cb};
use crate::render::wake::WakeSignal;
use rodio::cpal::{Error, ErrorKind, HostId};
use std::sync::Arc;
use std::sync::atomic::Ordering;

fn events() -> Arc<OutputEvents> {
    Arc::new(OutputEvents::new(WakeSignal::default()))
}

fn pause_requested(output: &OutputEvents) -> bool {
    output.pause_requested.load(Ordering::Acquire)
}

#[test]
fn route_changes_request_pause_without_failing_playback() {
    for kind in [ErrorKind::DeviceChanged, ErrorKind::DeviceNotAvailable] {
        let output = events();
        let wake = output.wake.clone();
        let callback = error_cb(output.clone(), HostId::PipeWire);

        callback(Error::new(kind));

        assert!(pause_requested(&output), "{kind:?} must pause");
        assert!(output.failure.lock().is_none(), "{kind:?} must not stop");
        assert!(wake.take(), "{kind:?} must wake the UI");
    }
}

#[test]
fn self_recovering_warnings_neither_pause_nor_fail() {
    for kind in [
        ErrorKind::Xrun,
        ErrorKind::BackendError,
        ErrorKind::RealtimeDenied,
    ] {
        let output = events();
        let callback = error_cb(output.clone(), HostId::PipeWire);

        callback(Error::new(kind));

        assert!(!pause_requested(&output), "{kind:?} must not pause");
        assert!(output.failure.lock().is_none(), "{kind:?} must not stop");
    }
}

#[test]
fn dead_streams_still_request_failure_handling() {
    for kind in [
        ErrorKind::StreamInvalidated,
        ErrorKind::UnsupportedConfig,
        ErrorKind::Other,
    ] {
        let output = events();
        let callback = error_cb(output.clone(), HostId::PipeWire);
        let fatal = Error::with_message(kind, "output stream failed");

        callback(fatal.clone());

        assert_eq!(*output.failure.lock(), Some(fatal));
        assert!(!pause_requested(&output));
    }
}

#[test]
fn route_change_does_not_mask_a_pending_stream_failure() {
    let output = events();
    let callback = error_cb(output.clone(), HostId::PipeWire);
    let fatal = Error::new(ErrorKind::StreamInvalidated);

    callback(fatal.clone());
    callback(Error::new(ErrorKind::DeviceChanged));

    assert_eq!(*output.failure.lock(), Some(fatal));
}

#[cfg(target_os = "linux")]
#[test]
fn alsa_device_loss_is_a_failure_because_its_stream_worker_exits() {
    let output = events();
    let callback = error_cb(output.clone(), HostId::Alsa);
    let fatal = Error::new(ErrorKind::DeviceNotAvailable);

    callback(fatal.clone());

    assert_eq!(*output.failure.lock(), Some(fatal));
    assert!(!pause_requested(&output));
}
