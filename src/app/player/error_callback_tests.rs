use super::error_cb;
use parking_lot::Mutex;
use rodio::cpal::{Error, ErrorKind};
use std::sync::Arc;

#[test]
fn default_device_reroutes_do_not_request_playback_restart() {
    let failure = Arc::new(Mutex::new(None));
    let callback = error_cb(failure.clone());

    callback(Error::new(ErrorKind::DeviceChanged));
    callback(Error::new(ErrorKind::DeviceChanged));

    assert!(failure.lock().is_none());
}

#[test]
fn reroute_does_not_mask_a_pending_stream_failure() {
    for kind in [
        ErrorKind::DeviceNotAvailable,
        ErrorKind::StreamInvalidated,
        ErrorKind::UnsupportedConfig,
    ] {
        let failure = Arc::new(Mutex::new(None));
        let callback = error_cb(failure.clone());
        let fatal = Error::with_message(kind, "output stream failed");

        callback(fatal.clone());
        callback(Error::new(ErrorKind::DeviceChanged));

        assert_eq!(*failure.lock(), Some(fatal));
    }
}

#[test]
fn stream_failure_after_a_reroute_still_requests_failure_handling() {
    let failure = Arc::new(Mutex::new(None));
    let callback = error_cb(failure.clone());
    let fatal = Error::new(ErrorKind::DeviceNotAvailable);

    callback(Error::new(ErrorKind::DeviceChanged));
    callback(fatal.clone());

    assert_eq!(*failure.lock(), Some(fatal));
}
