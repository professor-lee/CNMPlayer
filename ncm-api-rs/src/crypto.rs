/// 加密模块 - 对应 Node.js 版本的 util/crypto.js
///
/// 实现四种加密方式：
/// - weapi: 双层 AES-128-CBC + RSA
/// - eapi: MD5 签名 + AES-128-ECB
/// - linuxapi: AES-128-ECB
/// - xeapi: AES-256-ECB 静态密钥 + 动态密钥，X25519/AES-128-GCM 交换动态密钥
use crate::error::{NcmError, Result};
use aes::cipher::{BlockModeDecrypt, BlockModeEncrypt, KeyInit, KeyIvInit, block_padding::Pkcs7};
use aes_gcm::{Aes128Gcm, aead::Aead};
use base64::Engine;
use base64::engine::general_purpose::{STANDARD as BASE64, URL_SAFE_NO_PAD as BASE64_URL};
use hmac::{Hmac, Mac};
use md5::{Digest, Md5};
use rand::RngExt;
use rsa::traits::PublicKeyParts;
use rsa::{BoxedUint, RsaPublicKey};
use sha2::Sha256;
use std::collections::HashMap;
use std::io::Read;

type Aes128CbcEnc = cbc::Encryptor<aes::Aes128>;
type Aes128EcbEnc = ecb::Encryptor<aes::Aes128>;
type Aes128EcbDec = ecb::Decryptor<aes::Aes128>;
type HmacSha256 = Hmac<Sha256>;

/// xeapi 静态 AES-256 密钥（上游 xeapiStaticKey）
const XEAPI_STATIC_KEY: [u8; 32] = [
    0xab, 0x1d, 0x5a, 0x43, 0x0f, 0x6b, 0xb0, 0x4a, 0x3f, 0x01, 0xe8, 0x1d, 0xdd, 0x72, 0xbd, 0x91,
    0x6d, 0x5c, 0xe5, 0x91, 0x24, 0x8a, 0xc1, 0x28, 0x71, 0x48, 0x06, 0xd7, 0xf8, 0xfb, 0x1b, 0x84,
];
/// xeapi 签名 HMAC 密钥：上游直接用这段 base64 文本的字节作 key
const XEAPI_SIGN_KEY: &[u8] =
    b"mUHCwVNWJbunMqAHf5MImuirT6plvs6VSFW62MGHstFQxhBGdEoIhLItH3djc4+FB/OKty3+lL2rGeoFBpVe5g==";

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

/// 服务端下发的 xeapi 公钥状态（上游 register_xeapikey 回包解密后的 JSON）
#[derive(Debug, Clone)]
pub struct XeapiPublicKey {
    /// 服务端 X25519 公钥（32 字节原始格式）
    pub public_key: [u8; 32],
    pub version: String,
    pub sk: String,
}

/// 按密钥长度选择 AES-128/192/256-ECB（PKCS#7）
fn aes_ecb_encrypt_with(key: &[u8], plaintext: &[u8]) -> Result<Vec<u8>> {
    let invalid = |_| NcmError::Crypto(format!("Invalid AES-ECB key length {}", key.len()));
    Ok(match key.len() {
        16 => ecb::Encryptor::<aes::Aes128>::new_from_slice(key)
            .map_err(invalid)?
            .encrypt_padded_vec::<Pkcs7>(plaintext),
        24 => ecb::Encryptor::<aes::Aes192>::new_from_slice(key)
            .map_err(invalid)?
            .encrypt_padded_vec::<Pkcs7>(plaintext),
        32 => ecb::Encryptor::<aes::Aes256>::new_from_slice(key)
            .map_err(invalid)?
            .encrypt_padded_vec::<Pkcs7>(plaintext),
        len => {
            return Err(NcmError::Crypto(format!(
                "Invalid AES-ECB key length {len}"
            )));
        }
    })
}

/// xeapi 签名：HMAC-SHA256(timestamp + nonce)，输出 base64
pub fn xeapi_sign(timestamp: &str, nonce: &str) -> String {
    let mut mac = HmacSha256::new_from_slice(XEAPI_SIGN_KEY).expect("HMAC accepts any key length");
    mac.update(timestamp.as_bytes());
    mac.update(nonce.as_bytes());
    BASE64.encode(mac.finalize().into_bytes())
}

/// HKDF-SHA256（全零 salt、info = 临时公钥）取前 16 字节作 AES-128-GCM 密钥
fn xeapi_derive_aes_key(shared_secret: &[u8], ephemeral_public: &[u8]) -> [u8; 16] {
    let mut mac = HmacSha256::new_from_slice(&[0u8; 32]).expect("HMAC accepts any key length");
    mac.update(shared_secret);
    let prk = mac.finalize().into_bytes();
    let mut mac = HmacSha256::new_from_slice(&prk).expect("HMAC accepts any key length");
    mac.update(ephemeral_public);
    mac.update(&[1]);
    let okm = mac.finalize().into_bytes();
    let mut key = [0u8; 16];
    key.copy_from_slice(&okm[..16]);
    key
}

/// 随机 16 字节掩码异或密文并循环左移，掩码前置
fn xeapi_mid_transform(ciphertext: &[u8], mask: [u8; 16]) -> Vec<u8> {
    let xored: Vec<u8> = ciphertext
        .iter()
        .enumerate()
        .map(|(index, byte)| byte ^ mask[index & 0x0f])
        .collect();
    let rot = if xored.is_empty() {
        0
    } else {
        usize::from(mask[0] & 0x0f) % xored.len()
    };
    let mut out = Vec::with_capacity(16 + xored.len());
    out.extend_from_slice(&mask);
    out.extend_from_slice(&xored[rot..]);
    out.extend_from_slice(&xored[..rot]);
    out
}

/// S 段：临时 X25519 公钥 ‖ GCM nonce ‖ AES-128-GCM(动态密钥|os|sk) ‖ tag
fn xeapi_encrypt_s(dynamic_key: &[u8], public_key: &XeapiPublicKey, os: &str) -> Result<Vec<u8>> {
    let secret = x25519_dalek::StaticSecret::from(rand::random::<[u8; 32]>());
    let ephemeral_public = x25519_dalek::PublicKey::from(&secret);
    let shared = secret.diffie_hellman(&x25519_dalek::PublicKey::from(public_key.public_key));
    let aes_key = xeapi_derive_aes_key(shared.as_bytes(), ephemeral_public.as_bytes());
    let iv = rand::random::<[u8; 12]>();
    let plaintext = format!("{}|{}|{}", BASE64.encode(dynamic_key), os, public_key.sk);
    let sealed = Aes128Gcm::new(&aes_key.into())
        .encrypt(&iv.into(), plaintext.as_bytes())
        .map_err(|_| NcmError::Crypto("xeapi AES-GCM encryption failed".to_string()))?;
    let mut out = Vec::with_capacity(32 + 12 + sealed.len());
    out.extend_from_slice(ephemeral_public.as_bytes());
    out.extend_from_slice(&iv);
    out.extend_from_slice(&sealed);
    Ok(out)
}

/// 上游 buildXeapiPlaintext：表单体放 content，固定追加 e_r=true
fn xeapi_plaintext(data: &serde_json::Value) -> Result<String> {
    let pairs: Vec<(&str, String)> = data
        .as_object()
        .into_iter()
        .flatten()
        .filter(|(key, _)| key.as_str() != "e_r")
        .map(|(key, value)| {
            let value = match value {
                serde_json::Value::String(text) => text.clone(),
                other => other.to_string(),
            };
            (key.as_str(), value)
        })
        .collect();
    let content =
        serde_urlencoded::to_string(&pairs).map_err(|error| NcmError::Crypto(error.to_string()))?;
    let mut fields = serde_json::Map::new();
    if !content.is_empty() {
        fields.insert("content".to_string(), content.into());
    }
    fields.insert("queryString".to_string(), "e_r=true".into());
    Ok(serde_json::Value::Object(fields).to_string())
}

/// xeapi 加密，返回表单字段 C / S / R。
///
/// `session` 为服务端下发的 (ssid, sskey)；存在时 sskey 直接作动态密钥。
pub fn xeapi(
    data: &serde_json::Value,
    public_key: &XeapiPublicKey,
    session: Option<(&str, &str)>,
    os: &str,
) -> Result<HashMap<String, String>> {
    let session = session.filter(|(_, key)| matches!(key.len(), 16 | 24 | 32));
    let dynamic_key = match session {
        Some((_, key)) => key.as_bytes().to_vec(),
        None => rand::random::<[u8; 16]>().to_vec(),
    };
    let plaintext = xeapi_plaintext(data)?;
    let inner = aes_ecb_encrypt_with(&XEAPI_STATIC_KEY, plaintext.as_bytes())?;
    let c = aes_ecb_encrypt_with(
        &dynamic_key,
        &xeapi_mid_transform(&inner, rand::random::<[u8; 16]>()),
    )?;
    let s = xeapi_encrypt_s(&dynamic_key, public_key, os)?;
    let session_id = session.map(|(id, _)| id).unwrap_or("");
    let r = aes_ecb_encrypt_with(
        &XEAPI_STATIC_KEY,
        format!("{}|{}", public_key.version, session_id).as_bytes(),
    )?;

    let mut result = HashMap::new();
    result.insert("C".to_string(), BASE64_URL.encode(c));
    result.insert("S".to_string(), BASE64.encode(s));
    result.insert("R".to_string(), BASE64.encode(r));
    Ok(result)
}

fn aes256_ecb_decrypt(ciphertext: &[u8], key: &[u8; 32]) -> Result<Vec<u8>> {
    ecb::Decryptor::<aes::Aes256>::new(key.into())
        .decrypt_padded_vec::<Pkcs7>(ciphertext)
        .map_err(|_| {
            NcmError::Crypto("Invalid AES-256-ECB ciphertext or PKCS#7 padding".to_string())
        })
}

/// xeapi 回包：AES-128-ECB(eapi 密钥) 解密，gzip 魔数开头时再解压
pub fn xeapi_res_decrypt(body: &[u8]) -> Result<serde_json::Value> {
    let decrypted = aes_ecb_decrypt(body, EAPI_KEY)?;
    let plaintext = if decrypted.starts_with(&[0x1f, 0x8b]) {
        let mut inflated = Vec::new();
        flate2::read::GzDecoder::new(decrypted.as_slice())
            .read_to_end(&mut inflated)
            .map_err(|error| NcmError::Crypto(format!("Invalid xeapi gzip body: {error}")))?;
        inflated
    } else {
        decrypted
    };
    serde_json::from_slice(&plaintext)
        .map_err(|source| NcmError::response_decode("xeapi", plaintext.len(), source))
}

/// 解密 /api/bsr/sk/get 回包里的 encryptedData（AES-256-ECB 静态密钥）
pub fn xeapi_decrypt_public_key(encrypted_data: &str) -> Result<XeapiPublicKey> {
    let ciphertext = BASE64
        .decode(encrypted_data)
        .map_err(|error| NcmError::Crypto(format!("Invalid xeapi key base64: {error}")))?;
    let decrypted = aes256_ecb_decrypt(&ciphertext, &XEAPI_STATIC_KEY)?;
    let state: serde_json::Value = serde_json::from_slice(&decrypted)
        .map_err(|source| NcmError::response_decode("xeapi key", decrypted.len(), source))?;
    let public_key = state
        .get("publicKey")
        .and_then(|value| value.as_str())
        .and_then(|value| BASE64.decode(value).ok())
        .and_then(|value| <[u8; 32]>::try_from(value).ok())
        .ok_or_else(|| NcmError::Crypto("xeapi key response missing publicKey".to_string()))?;
    let version = match state.get("version") {
        Some(serde_json::Value::String(text)) => text.clone(),
        Some(serde_json::Value::Number(number)) => number.to_string(),
        _ => String::new(),
    };
    let sk = state
        .get("sk")
        .and_then(|value| value.as_str())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| NcmError::Crypto("xeapi key response missing sk".to_string()))?
        .to_string();
    Ok(XeapiPublicKey {
        public_key,
        version,
        sk,
    })
}

#[cfg(test)]
mod tests;
