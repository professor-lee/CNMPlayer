use super::*;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::mpsc;
use std::time::Duration;

#[compio::test]
async fn api_network_deadline_covers_headers_and_body() {
    let timeout = Duration::from_millis(200);
    for send_headers in [false, true] {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let (release, wait_for_release) = mpsc::channel();
        let server = compio::runtime::spawn_blocking(move || {
            let (mut socket, _) = listener.accept().unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let mut request = Vec::new();
            let mut buffer = [0; 1024];
            while !request.windows(4).any(|window| window == b"\r\n\r\n") {
                let count = socket.read(&mut buffer).unwrap();
                assert_ne!(count, 0);
                request.extend_from_slice(&buffer[..count]);
            }
            assert!(request.starts_with(b"POST /api/deadline HTTP/1.1\r\n"));
            if send_headers {
                socket
                    .write_all(
                        b"HTTP/1.1 200 OK\r\nContent-Length: 12\r\nConnection: close\r\n\r\n{",
                    )
                    .unwrap();
            }
            // Hold the connection open until the client returns; no busy loop.
            let _ = wait_for_release.recv_timeout(Duration::from_secs(2));
        });
        let http = cyper::Client::builder().no_proxy().build().unwrap();
        let api = ApiClient::new(None, http);
        let result = compio::time::timeout(
            Duration::from_secs(1),
            api.request_with_timeout(
                "/api/deadline",
                serde_json::json!({}),
                RequestOption {
                    crypto: CryptoType::Api,
                    domain: Some(format!("http://{address}")),
                    ..RequestOption::default()
                },
                timeout,
            ),
        )
        .await;
        let _ = release.send(());
        server.await.unwrap();
        let error = result
            .expect("the request deadline must beat the watchdog")
            .unwrap_err();
        assert!(matches!(error, NcmError::Timeout { timeout: elapsed } if elapsed == timeout));
        assert!(error.to_string().contains("complete response"));
    }
}

#[test]
fn successful_response_preserves_status_body_and_cookies() {
    let body = decode_response_body(b"{\"code\":200,\"data\":[1,2]}", false).unwrap();
    let cookies = vec!["MUSIC_U=session; Path=/".to_string()];
    let response = response_from_body(200, body.clone(), cookies.clone()).unwrap();
    assert_eq!(response.status, 200);
    assert_eq!(response.body, body);
    assert_eq!(response.cookie, cookies);
}

#[test]
fn api_uri_requires_nonempty_api_endpoint() {
    assert_eq!(api_endpoint("/api/song/detail").unwrap(), "song/detail");
    assert_eq!(api_endpoint("/api/歌曲").unwrap(), "歌曲");
    for uri in [
        "",
        "/",
        "/api",
        "/api/",
        "😀",
        "/abc/endpoint",
        "/eapi/test",
        "https://music.163.com/api/test",
    ] {
        assert!(matches!(api_endpoint(uri), Err(NcmError::InvalidParam(_))));
    }
}

#[test]
fn malformed_plain_response_is_a_typed_error_with_bounded_context() {
    let mut body = b"not-json SECRET".to_vec();
    body.resize(16 * 1024, b'x');
    let error = decode_response_body(&body, false).unwrap_err();
    assert!(matches!(
        &error,
        NcmError::ResponseDecode {
            format: "plain",
            body_len: 16384,
            ..
        }
    ));
    let message = error.to_string();
    assert!(!message.contains("SECRET"));
    assert!(message.len() < 256);
    assert!(matches!(
        decode_response_body(b"", false),
        Err(NcmError::ResponseDecode { .. })
    ));
}

#[test]
fn encrypted_response_decodes_fixed_vector_or_returns_error() {
    let bytes = hex::decode("51B05E35C69B2F9FF4967735DED68881").unwrap();
    assert_eq!(
        decode_response_body(&bytes, true).unwrap(),
        serde_json::json!({"code": 200})
    );
    assert!(matches!(
        decode_response_body(&[0], true),
        Err(NcmError::Crypto(_))
    ));
    let malformed = hex::decode("67E8E46C291AD4030FDF54CF200490C6").unwrap();
    assert!(matches!(
        decode_response_body(&malformed, true),
        Err(NcmError::ResponseDecode { .. })
    ));
}

#[test]
fn valid_json_body_shapes_are_preserved() {
    for body in ["null", "\"body\"", "[1,2]", "{\"code\":502}"] {
        assert_eq!(
            decode_response_body(body.as_bytes(), false).unwrap(),
            serde_json::from_str::<Value>(body).unwrap()
        );
    }
}

#[test]
fn special_status_codes_preserve_original_response_body() {
    for code in [201, 302, 400, 502, 800, 801, 802, 803] {
        let body = serde_json::json!({"code": code});
        let response = response_from_body(500, body.clone(), vec![]).unwrap();
        assert_eq!(response.status, 200);
        assert_eq!(response.body, body);
    }
    let response = response_from_body(500, serde_json::json!({"code": "800"}), vec![]).unwrap();
    assert_eq!(response.body["code"], "800");
    assert_eq!(response.status, 200);
    assert!(matches!(
        response_from_body(200, serde_json::json!({"code": 301}), vec![]),
        Err(NcmError::AuthRequired(_))
    ));
    assert!(matches!(
        response_from_body(503, serde_json::json!({}), vec![]),
        Err(NcmError::RateLimited(_))
    ));
}
