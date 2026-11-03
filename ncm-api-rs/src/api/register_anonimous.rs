use super::Query;
use crate::error::Result;
/// 匿名注册
/// 对应 Node.js module/register_anonimous.js
use crate::request::{ApiClient, ApiResponse, CryptoType};
use crate::util::device::generate_device_id;
use base64::{Engine, engine::general_purpose::STANDARD};
use md5::{Digest, Md5};
use serde_json::json;

const ID_XOR_KEY_1: &str = "3go8&$8*3*3h0k(2)2";

fn cloudmusic_dll_encode_id(some_id: &str) -> String {
    let xored: Vec<u8> = some_id
        .bytes()
        .enumerate()
        .map(|(i, b)| {
            let key_byte = ID_XOR_KEY_1.as_bytes()[i % ID_XOR_KEY_1.len()];
            b ^ key_byte
        })
        .collect();
    let digest = Md5::digest(&xored);
    STANDARD.encode(digest)
}

impl ApiClient {
    /// 匿名注册
    /// 对应 /register/anonimous
    pub async fn register_anonimous(&self, query: &Query) -> Result<ApiResponse> {
        let device_id = generate_device_id();
        let id_with_hash = format!("{} {}", device_id, cloudmusic_dll_encode_id(&device_id));
        let encoded_id = STANDARD.encode(id_with_hash.as_bytes());
        let data = json!({
            "username": encoded_id
        });
        let result = self
            .request(
                "/api/register/anonimous",
                data,
                query.to_option(CryptoType::Weapi),
            )
            .await?;
        Ok(result)
    }
}
