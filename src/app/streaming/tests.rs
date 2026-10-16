use super::super::streaming::StreamingReader;
use super::super::streaming::StreamingState;
use super::super::streaming::checked_seek;
use std::fs::File;
use std::io::ErrorKind;
use std::io::Read;
use std::sync::Arc;
use std::sync::atomic::Ordering;

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
