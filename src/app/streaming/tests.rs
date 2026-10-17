use super::super::streaming::StreamingReader;
use super::super::streaming::StreamingState;
use super::super::streaming::checked_seek;
use std::fs::File;
use std::io::ErrorKind;
use std::io::Read;
use std::sync::Arc;
use std::sync::atomic::Ordering;

#[test]
fn streaming_temp_names_are_distinct_from_download_parts() {
    use super::super::streaming::parse_streaming_temp_pid;
    use std::path::Path;

    assert_eq!(
        parse_streaming_temp_pid(Path::new("song__exhigh.1234-7.part")),
        Some(1234)
    );
    for unrelated in [
        "song.flac.part",
        "song.audio.part",
        "song.audio.1234.part",
        "song.audio.1234-x.part",
        "song.audio.1234-7.tmp",
        "song__exhigh.part",
    ] {
        assert_eq!(parse_streaming_temp_pid(Path::new(unrelated)), None);
    }
}

#[test]
fn orphan_reclamation_rejects_live_and_unrelated_files() {
    use super::super::streaming::{ProcessLiveness, should_reclaim_streaming_temp};

    assert!(!should_reclaim_streaming_temp(
        42,
        42,
        true,
        ProcessLiveness::Live
    ));
    // A same-PID file not registered by this process belongs to the previous
    // incarnation after PID reuse and is safe to reclaim.
    assert!(should_reclaim_streaming_temp(
        42,
        42,
        false,
        ProcessLiveness::Live
    ));
    assert!(should_reclaim_streaming_temp(
        7,
        42,
        false,
        ProcessLiveness::Dead
    ));
    assert!(!should_reclaim_streaming_temp(
        7,
        42,
        false,
        ProcessLiveness::Live
    ));
    assert!(!should_reclaim_streaming_temp(
        7,
        42,
        false,
        ProcessLiveness::Unknown
    ));
}

#[test]
fn pid_zero_never_proves_an_owner() {
    use super::super::streaming::{ProcessLiveness, process_liveness};

    assert_eq!(process_liveness(0), ProcessLiveness::Unknown);
}

#[test]
fn cache_maintenance_reclaims_orphans_but_keeps_active_and_unrelated_parts() {
    use super::{StreamingTempRegistration, reserve_streaming_temp};
    use crate::app::player::cleanup_orphan_streaming_temps;

    let directory = std::env::temp_dir().join(format!(
        "cnmplayer-orphan-maintenance-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&directory).unwrap();
    let pid = std::process::id();
    let active = directory.join(format!("123__exhigh.{pid}-0.part"));
    let stale = directory.join(format!("456__exhigh.{pid}-1.part"));
    let unrelated = directory.join("track.flac.part");
    let unknown = directory.join("789__exhigh.0-0.part");
    for path in [&active, &stale, &unrelated, &unknown] {
        std::fs::write(path, b"partial").unwrap();
    }
    reserve_streaming_temp(&active);
    let registration = StreamingTempRegistration(active.clone());
    cleanup_orphan_streaming_temps(&directory).unwrap();
    assert!(active.is_file());
    assert!(!stale.exists());
    assert!(unrelated.is_file());
    assert!(unknown.is_file());
    drop(registration);
    cleanup_orphan_streaming_temps(&directory).unwrap();
    assert!(!active.exists());
    assert_eq!(std::fs::read(unrelated).unwrap(), b"partial");
    std::fs::remove_dir_all(directory).unwrap();
}
#[compio::test]
async fn complete_stream_is_published_but_truncated_stream_is_not() {
    for (index, body) in [b"abcdef".as_slice(), b"abc".as_slice()]
        .into_iter()
        .enumerate()
    {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = compio::runtime::spawn_blocking(move || {
            let (mut socket, _) = listener.accept().unwrap();
            let mut request = [0; 4096];
            let received = socket.read(&mut request).unwrap();
            assert!(received > 0, "client sent its HTTP request");
            std::io::Write::write_all(
                &mut socket,
                b"HTTP/1.1 200 OK\r\nContent-Length: 6\r\nConnection: close\r\n\r\n",
            )
            .unwrap();
            std::io::Write::write_all(&mut socket, body).unwrap();
        });
        let directory = std::env::temp_dir().join(format!(
            "cnmplayer-stream-http-{}-{index}",
            std::process::id()
        ));
        let final_path = directory.join("song.audio");
        let http = cyper::Client::builder().no_proxy().build().unwrap();
        let (tx, _rx) = see::sync::channel((0, 0));
        let reader = StreamingReader::new(
            &http,
            &format!("http://{address}/song"),
            final_path.clone(),
            None,
            tx,
        )
        .await
        .unwrap();
        let result = compio::runtime::spawn_blocking(move || {
            let mut reader = reader;
            let mut bytes = Vec::new();
            reader.read_to_end(&mut bytes).map(|_| bytes)
        })
        .await
        .unwrap();
        server.await.unwrap();
        if index == 0 {
            assert_eq!(result.unwrap(), b"abcdef");
            assert_eq!(compio::fs::read(&final_path).await.unwrap(), b"abcdef");
        } else {
            assert!(result.is_err());
            assert!(compio::fs::metadata(&final_path).await.is_err());
        }
        compio::runtime::spawn_blocking(move || std::fs::remove_dir_all(directory))
            .await
            .unwrap()
            .unwrap();
    }
}

#[test]
fn seek_rejects_negative_positions_and_overflow() {
    assert_eq!(checked_seek(4, -3).unwrap(), 1);
    assert_eq!(
        checked_seek(4, -5).unwrap_err().kind(),
        ErrorKind::InvalidInput
    );
    assert_eq!(
        checked_seek(u64::MAX, 1).unwrap_err().kind(),
        ErrorKind::InvalidInput
    );
}

#[test]
fn reads_only_published_bytes_and_propagates_failure() {
    let path = std::env::temp_dir().join(format!("cnmplayer-stream-test-{}", std::process::id()));
    std::fs::write(&path, b"abcdef").unwrap();
    let state = Arc::new(StreamingState::default());
    state.downloaded.store(3, Ordering::SeqCst);
    state.total.store(6, Ordering::SeqCst);
    let mut reader = StreamingReader {
        state: state.clone(),
        file: File::open(&path).unwrap(),
    };
    let mut buf = [0; 6];
    assert_eq!(reader.read(&mut buf).unwrap(), 3);
    assert_eq!(&buf[..3], b"abc");
    state.finish(&Err(anyhow::anyhow!("truncated response")));
    assert!(
        reader
            .read(&mut buf)
            .unwrap_err()
            .to_string()
            .contains("truncated response")
    );
    drop(reader);
    std::fs::remove_file(path).unwrap();
}

#[test]
fn cancellation_wakes_waiting_reader() {
    let state = Arc::new(StreamingState::default());
    let waiting = state.clone();
    let worker = std::thread::spawn(move || {
        let mut guard = waiting.wait_lock.lock();
        while !waiting.cancelled.load(Ordering::SeqCst) {
            waiting.condvar.wait(&mut guard);
        }
    });
    state.cancel();
    worker.join().unwrap();
}
