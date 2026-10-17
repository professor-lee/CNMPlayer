use super::*;

// Fixed vectors generated independently with OpenSSL AES-128-ECB/PKCS#7.
#[test]
fn eapi_response_decodes_fixed_vector() {
    let body = eapi_res_decrypt("51B05E35C69B2F9FF4967735DED68881").unwrap();
    assert_eq!(body, serde_json::json!({"code": 200}));
}

#[test]
fn eapi_response_rejects_invalid_hex_and_padding() {
    assert!(matches!(
        eapi_res_decrypt("not hex"),
        Err(NcmError::Crypto(_))
    ));
    assert!(matches!(eapi_res_decrypt("00"), Err(NcmError::Crypto(_))));
    assert!(matches!(eapi_res_decrypt(""), Err(NcmError::Crypto(_))));
}

#[test]
fn eapi_request_rejects_missing_envelope() {
    assert!(matches!(
        eapi_req_decrypt("51B05E35C69B2F9FF4967735DED68881"),
        Err(NcmError::Crypto(_))
    ));
}

#[test]
fn eapi_response_rejects_invalid_utf8() {
    assert!(matches!(
        eapi_res_decrypt("9DF6853B4BD91D9D60D42478618B540C"),
        Err(NcmError::Crypto(_))
    ));
}

#[test]
fn eapi_response_rejects_malformed_json() {
    assert!(matches!(
        eapi_res_decrypt("67E8E46C291AD4030FDF54CF200490C6"),
        Err(NcmError::ResponseDecode { body_len: 4, .. })
    ));
    assert!(matches!(
        eapi_res_decrypt("6AA3B102FBE7296AB0DB9EA5C46AD12B"),
        Err(NcmError::ResponseDecode { body_len: 0, .. })
    ));
}

#[test]
fn test_aes_ecb_roundtrip() {
    let plaintext = b"hello world test";
    let encrypted = aes_ecb_encrypt_hex(plaintext, EAPI_KEY);
    let decrypted = aes_ecb_decrypt_hex(&encrypted, EAPI_KEY).unwrap();
    assert_eq!(decrypted, plaintext);
}

#[test]
fn test_weapi_produces_params_and_encseckey() {
    let obj = serde_json::json!({"id": 123});
    let result = weapi(&obj);
    assert!(result.contains_key("params"));
    assert!(result.contains_key("encSecKey"));
    assert!(!result["params"].is_empty());
    // RSA 输出 256 hex chars (1024-bit key)
    assert_eq!(result["encSecKey"].len(), 256);
}

#[test]
fn test_linuxapi_produces_eparams() {
    let obj = serde_json::json!({"method": "POST", "url": "https://music.163.com/api/test"});
    let result = linuxapi(&obj);
    assert!(result.contains_key("eparams"));
    assert!(!result["eparams"].is_empty());
}

#[test]
fn test_eapi_encrypt_decrypt() {
    let url = "/api/song/detail";
    let obj = serde_json::json!({"id": 123});
    let encrypted = eapi(url, &obj);
    let params = &encrypted["params"];

    // 验证能解密回来
    let (dec_url, dec_data) = eapi_req_decrypt(params).unwrap();
    assert_eq!(dec_url, url);
    assert_eq!(dec_data, obj);
}
