use super::super::api::ApiState;
use std::io::Read;
use std::io::Write;
use std::net::TcpListener;
use std::sync::mpsc;
use std::time::Duration;

#[compio::test]
async fn busy_cover_decoder_waits_for_capacity_instead_of_losing_image() {
    static GATE: super::CoverValidationGate = super::CoverValidationGate::new();
    let first = GATE.try_acquire().unwrap();
    let second = GATE.try_acquire().unwrap();
    let release = compio::runtime::spawn(async move {
        compio::time::sleep(Duration::from_millis(30)).await;
        drop(first);
    });
    let third = GATE.acquire(Duration::from_secs(1)).await.unwrap();
    assert!(
        GATE.try_acquire().is_err(),
        "waiting did not increase decoder concurrency"
    );
    drop(third);
    drop(second);
    release.await.unwrap();
}

#[compio::test]
async fn cover_network_deadline_is_not_reset_after_headers() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let png = tiny_png();
    let server = compio::runtime::spawn_blocking(move || {
        let (mut socket, _) = listener.accept().unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let mut request = [0; 1024];
        assert_ne!(socket.read(&mut request).unwrap(), 0);
        std::thread::sleep(Duration::from_millis(200));
        write!(
            socket,
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            png.len()
        )
        .unwrap();
        socket.write_all(&png[..8]).unwrap();
        std::thread::sleep(Duration::from_millis(200));
        // A single deadline has expired; a fresh body deadline would accept this PNG.
        let _ = socket.write_all(&png[8..]);
    });
    let http = cyper::Client::builder().no_proxy().build().unwrap();
    let api = ApiState::new(None, http).unwrap();
    let url = format!("http://{address}/slow-cover");
    let result = compio::time::timeout(
        Duration::from_secs(2),
        api.fetch_cover_bytes_with_timeout(&url, Duration::from_millis(300)),
    )
    .await;
    server.await.unwrap();
    let error = result
        .expect("the cover deadline must beat the watchdog")
        .unwrap_err();
    assert!(matches!(
        error.downcast_ref::<ncm_api::NcmError>(),
        Some(ncm_api::NcmError::Timeout { timeout })
            if *timeout == Duration::from_millis(300)
    ));
    let message = format!("{error:#}");
    assert!(message.contains("complete response"));
    assert!(message.contains(&url));
}

#[compio::test]
async fn cover_validation_admission_lives_until_blocking_work_finishes() {
    static GATE: super::CoverValidationGate = super::CoverValidationGate::new();
    let first = GATE.try_acquire().unwrap();
    let second = GATE.try_acquire().unwrap();
    assert!(GATE.try_acquire().unwrap_err().to_string().contains("busy"));
    let (release, wait_for_release) = mpsc::channel();
    let worker = compio::runtime::spawn_blocking(move || {
        let _permit = first;
        let _ = wait_for_release.recv_timeout(Duration::from_secs(2));
    });
    drop(worker);
    // Dropping the caller does not free the running decoder's slot.
    assert!(GATE.try_acquire().is_err());
    let _ = release.send(());
    compio::time::timeout(Duration::from_secs(1), async {
        loop {
            if let Ok(permit) = GATE.try_acquire() {
                drop(permit);
                break;
            }
            compio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .expect("finishing blocking work must release its admission slot");
    drop(second);
    let _first = GATE.try_acquire().unwrap();
    let _second = GATE.try_acquire().unwrap();
}

fn tiny_png() -> Vec<u8> {
    use image::ImageEncoder;
    let mut bytes = Vec::new();
    image::codecs::png::PngEncoder::new(&mut bytes)
        .write_image(&[0, 0, 0, 255], 1, 1, image::ExtendedColorType::Rgba8)
        .unwrap();
    bytes
}

#[compio::test]
async fn cover_download_accepts_only_complete_valid_images() {
    let png = tiny_png();
    let length_header = format!("Content-Length: {}\r\n", png.len());
    let cases: &[(&[u8], &str, bool)] = &[
        (&png, &length_header, true),
        (&png, "", true),
        (b"not-an-image", "Content-Length: 12\r\n", false),
        (&png[..20], &length_header, false),
        (&png[..png.len() - 12], "", false),
        (b"a\r\nabc", "Transfer-Encoding: chunked\r\n", false),
    ];
    for (index, (body, headers, expected_ok)) in cases.iter().enumerate() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let body = body.to_vec();
        let headers = headers.to_string();
        let server = compio::runtime::spawn_blocking(move || {
            let (mut socket, _) = listener.accept().unwrap();
            let mut request = [0; 1024];
            let _ = socket.read(&mut request);
            write!(
                socket,
                "HTTP/1.1 200 OK\r\n{headers}Connection: close\r\n\r\n"
            )
            .unwrap();
            socket.write_all(&body).unwrap();
        });
        let http = cyper::Client::builder().no_proxy().build().unwrap();
        let api = ApiState::new(None, http).unwrap();
        let result = api
            .fetch_cover_bytes(&format!("http://{address}/cover-{index}"))
            .await;
        server.await.unwrap();
        assert_eq!(result.is_ok(), *expected_ok, "case {index}: {result:?}");
        if *expected_ok {
            assert_eq!(result.unwrap(), png);
        }
    }
}

#[compio::test]
async fn cover_download_rejects_declared_oversize_before_body() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = compio::runtime::spawn_blocking(move || {
        let (mut socket, _) = listener.accept().unwrap();
        let mut request = [0; 1024];
        let _ = socket.read(&mut request);
        write!(
            socket,
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            super::MAX_COVER_IMAGE_BYTES + 1
        )
        .unwrap();
    });
    let http = cyper::Client::builder().no_proxy().build().unwrap();
    let api = ApiState::new(None, http).unwrap();
    assert!(
        api.fetch_cover_bytes(&format!("http://{address}/oversize"))
            .await
            .is_err()
    );
    server.await.unwrap();
}

#[compio::test]
async fn cover_download_caps_body_without_content_length() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = compio::runtime::spawn_blocking(move || {
        let (mut socket, _) = listener.accept().unwrap();
        let mut request = [0; 1024];
        let _ = socket.read(&mut request);
        socket
            .write_all(b"HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n")
            .unwrap();
        let block = [0; 8192];
        for _ in 0..=(super::MAX_COVER_IMAGE_BYTES / block.len()) {
            if socket.write_all(&block).is_err() {
                break;
            }
        }
    });
    let http = cyper::Client::builder().no_proxy().build().unwrap();
    let api = ApiState::new(None, http).unwrap();
    let error = api
        .fetch_cover_bytes(&format!("http://{address}/unbounded"))
        .await
        .unwrap_err();
    assert!(error.to_string().contains("byte limit"));
    server.await.unwrap();
}

#[test]
fn display_cover_url_requests_one_target_size_and_keeps_other_params() {
    use super::super::api::cover_url_for_display;

    assert_eq!(
        cover_url_for_display("https://p1.music.126.net/a.jpg"),
        "https://p1.music.126.net/a.jpg?param=500y500"
    );
    assert_eq!(
        cover_url_for_display("https://p1.music.126.net/a.jpg?param=3000y3000"),
        "https://p1.music.126.net/a.jpg?param=500y500"
    );
    assert_eq!(
        cover_url_for_display("https://p1.music.126.net/a.jpg?x=1&param=2y2&y=3#frag"),
        "https://p1.music.126.net/a.jpg?x=1&y=3&param=500y500#frag"
    );
    assert_eq!(cover_url_for_display(""), "");
    assert_eq!(
        cover_url_for_display("data:image/png;base64,abc"),
        "data:image/png;base64,abc"
    );
}

#[compio::test]
async fn original_cover_download_drops_the_display_size_param() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let png = tiny_png();
    let expected = png.clone();
    let server = std::thread::spawn(move || {
        let (mut socket, _) = listener.accept().unwrap();
        let mut request = [0; 2048];
        let read = socket.read(&mut request).unwrap();
        let request = String::from_utf8_lossy(&request[..read]);
        assert!(
            request.starts_with("GET /cover "),
            "size param leaked into original request: {request}"
        );
        write!(
            socket,
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            png.len()
        )
        .unwrap();
        socket.write_all(&png).unwrap();
    });
    let api = ApiState::new(None, cyper::Client::builder().no_proxy().build().unwrap()).unwrap();
    let bytes = api
        .fetch_original_cover_bytes(&format!("http://{address}/cover?param=100y100"))
        .await
        .unwrap();
    server.join().unwrap();
    assert_eq!(bytes, expected);
}

#[test]
fn download_cover_url_strips_size_param_but_keeps_everything_else() {
    use super::super::api::cover_url_for_download;

    assert_eq!(
        cover_url_for_download("https://p1.music.126.net/a.jpg"),
        "https://p1.music.126.net/a.jpg"
    );
    assert_eq!(
        cover_url_for_download("https://p1.music.126.net/a.jpg?param=500y500"),
        "https://p1.music.126.net/a.jpg"
    );
    assert_eq!(
        cover_url_for_download("https://p1.music.126.net/a.jpg?x=1&param=2y2&y=3#frag"),
        "https://p1.music.126.net/a.jpg?x=1&y=3#frag"
    );
}
