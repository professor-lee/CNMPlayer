/// 加密模块 - 对应 Node.js 版本的 util/crypto.js
///
/// 实现三种加密方式：
/// - weapi: 双层 AES-128-CBC + RSA
/// - eapi: MD5 签名 + AES-128-ECB
/// - linuxapi: AES-128-ECB
use crate::error::{NcmError, Result};
use aes::cipher::{block_padding::Pkcs7, BlockModeDecrypt, BlockModeEncrypt, KeyInit, KeyIvInit};
use md5::{Digest, Md5};
use rand::RngExt;
use rsa::traits::PublicKeyParts;
use rsa::{BoxedUint, RsaPublicKey};
use std::collections::HashMap;

type Aes128CbcEnc = cbc::Encryptor<aes::Aes128>;
type Aes128EcbEnc = ecb::Encryptor<aes::Aes128>;
type Aes128EcbDec = ecb::Decryptor<aes::Aes128>;

const IV: &[u8; 16] = b"0102030405060708";
const PRESET_KEY: &[u8; 16] = b"0CoJUm6Qyw8W8jud";
const LINUXAPI_KEY: &[u8; 16] = b"rFgB&h#%2?^eDg:Q";
const EAPI_KEY: &[u8; 16] = b"e82ckenh8dichen8";
const BASE62: &[u8] = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";

// RSA 公钥 Base64 编码的 DER 数据（PKCS#8 / SubjectPublicKeyInfo 格式）
const PUBLIC_KEY_DER_B64: &str = "MIGfMA0GCSqGSIb3DQEBAQUAA4GNADCBiQKBgQDgtQn2JZ34ZC28NWYpAUd98iZ37BUrX/aKzmFbt7clFSs6sXqHauqKWqdtLkF2KexO40H1YTX8z2lSgBBOAxLsvaklV8k4cBFK9snQXE9/DDaFt6Rr7iVZMldczhC0JNgTz+SHXT6CBHuX3e9SdB1Ua44oncaTWz7OBGLbCiK45wIDAQAB";

/// AES-128-CBC 加密，输出 Base64
fn aes_cbc_encrypt_base64(plaintext: &[u8], key: &[u8; 16], iv: &[u8; 16]) -> String {
    let cipher = Aes128CbcEnc::new(key.into(), iv.into());
    let ciphertext = cipher.encrypt_padded_vec::<Pkcs7>(plaintext);
    base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &ciphertext)
}

/// AES-128-ECB 加密，输出大写 Hex
fn aes_ecb_encrypt_hex(plaintext: &[u8], key: &[u8; 16]) -> String {
    let cipher = Aes128EcbEnc::new(key.into());
    let ciphertext = cipher.encrypt_padded_vec::<Pkcs7>(plaintext);
    hex::encode_upper(&ciphertext)
}

/// AES-128-ECB 解密（输入大写 Hex）
fn aes_ecb_decrypt_hex(ciphertext_hex: &str, key: &[u8; 16]) -> Result<Vec<u8>> {
    let ciphertext = hex::decode(ciphertext_hex)
        .map_err(|error| NcmError::Crypto(format!("Invalid ciphertext hex: {error}")))?;
    aes_ecb_decrypt(&ciphertext, key)
}

fn aes_ecb_decrypt(ciphertext: &[u8], key: &[u8; 16]) -> Result<Vec<u8>> {
    Aes128EcbDec::new(key.into())
        .decrypt_padded_vec::<Pkcs7>(ciphertext)
        .map_err(|_| {
            NcmError::Crypto("Invalid AES-128-ECB ciphertext or PKCS#7 padding".to_string())
        })
}

/// RSA 加密（NONE / raw / textbook RSA，无 padding）
/// 对应 Node.js: forge.publicKey.encrypt(str, 'NONE')
fn rsa_encrypt_no_padding(plaintext: &[u8; 16]) -> String {
    use base64::Engine;
    use rsa::pkcs8::DecodePublicKey;

    let der_bytes = base64::engine::general_purpose::STANDARD
        .decode(PUBLIC_KEY_DER_B64)
        .expect("Failed to decode RSA public key base64");

    let public_key = RsaPublicKey::from_public_key_der(&der_bytes).unwrap();

    let n = public_key.n().to_odd().unwrap();

    // Textbook RSA: c = m^e mod n
    let m = BoxedUint::from_be_slice(plaintext, public_key.n_bits_precision()).unwrap();
    let c = m.pow_mod(public_key.e(), &n);

    // 输出固定长度 hex（与模数等长，256 hex chars for 1024-bit key）
    let n_bytes = n.bits() / 8;
    let c_bytes = c.to_be_bytes();

    // 左侧填充 0
    let mut padded = vec![0u8; n_bytes as usize - c_bytes.len()];
    padded.extend_from_slice(&c_bytes);
    hex::encode(&padded)
}

/// weapi 加密
/// 双层 AES-128-CBC + RSA 加密随机密钥
pub fn weapi(object: &serde_json::Value) -> HashMap<String, String> {
    let text = serde_json::to_string(object).unwrap();
    let mut rng = rand::rng();

    // 生成 16 位随机密钥
    let mut secret_key: [u8; 16] = std::array::from_fn(|_| BASE62[rng.random_range(0..62)]);

    // 第一层 AES-CBC：preset_key + iv
    let first_encrypt = aes_cbc_encrypt_base64(text.as_bytes(), PRESET_KEY, IV);
    // 第二层 AES-CBC：secret_key + iv
    let params = aes_cbc_encrypt_base64(first_encrypt.as_bytes(), &secret_key, IV);

    // RSA 加密反转后的 secret_key
    secret_key.reverse();
    let enc_sec_key = rsa_encrypt_no_padding(&secret_key);

    let mut result = HashMap::new();
    result.insert("params".to_string(), params);
    result.insert("encSecKey".to_string(), enc_sec_key);
    result
}

/// linuxapi 加密
/// 单层 AES-128-ECB
pub fn linuxapi(object: &serde_json::Value) -> HashMap<String, String> {
    let text = serde_json::to_string(object).unwrap();
    let mut result = HashMap::new();
    result.insert(
        "eparams".to_string(),
        aes_ecb_encrypt_hex(text.as_bytes(), LINUXAPI_KEY),
    );
    result
}

/// eapi 加密
/// MD5 签名 + AES-128-ECB
pub fn eapi(url: &str, object: &serde_json::Value) -> HashMap<String, String> {
    let text = serde_json::to_string(object).unwrap();
    let message = format!("nobody{}use{}md5forencrypt", url, text);
    let digest = hex::encode(Md5::digest(message.as_bytes()));
    let data = format!("{}-36cd479b6b5-{}-36cd479b6b5-{}", url, text, digest);

    let mut result = HashMap::new();
    result.insert(
        "params".to_string(),
        aes_ecb_encrypt_hex(data.as_bytes(), EAPI_KEY),
    );
    result
}

/// eapi 响应解密；无效密文、UTF-8 或 JSON 返回明确错误
pub fn eapi_res_decrypt(encrypted_hex: &str) -> Result<serde_json::Value> {
    let ciphertext = hex::decode(encrypted_hex)
        .map_err(|error| NcmError::Crypto(format!("Invalid ciphertext hex: {error}")))?;
    eapi_res_decrypt_bytes(&ciphertext)
}

pub(crate) fn eapi_res_decrypt_bytes(ciphertext: &[u8]) -> Result<serde_json::Value> {
    let decrypted = aes_ecb_decrypt(ciphertext, EAPI_KEY)?;
    let text = std::str::from_utf8(&decrypted)
        .map_err(|error| NcmError::Crypto(format!("Invalid decrypted response UTF-8: {error}")))?;
    serde_json::from_str(text)
        .map_err(|source| NcmError::response_decode("eapi", decrypted.len(), source))
}

/// eapi 请求解密（调试用）；保留请求 URL 和 JSON 参数
pub fn eapi_req_decrypt(encrypted_hex: &str) -> Result<(String, serde_json::Value)> {
    let decrypted = aes_ecb_decrypt_hex(encrypted_hex, EAPI_KEY)?;
    let text = std::str::from_utf8(&decrypted)
        .map_err(|error| NcmError::Crypto(format!("Invalid decrypted request UTF-8: {error}")))?;

    let mut parts = text.splitn(3, "-36cd479b6b5-");
    let url = parts.next().unwrap_or_default();
    let json = parts
        .next()
        .ok_or_else(|| NcmError::Crypto("Missing eapi request envelope delimiter".to_string()))?;
    let data = serde_json::from_str(json)
        .map_err(|source| NcmError::response_decode("eapi request", json.len(), source))?;
    Ok((url.to_string(), data))
}

#[cfg(test)]
mod tests;
