use super::Query;
use crate::error::Result;
/// 私人 FM 模式
/// 对应 Node.js module/personal_fm_mode.js
use crate::request::{ApiClient, ApiResponse, CryptoType};
use serde_json::json;

impl ApiClient {
    /// 私人 FM 模式
    /// 对应 /personal/fm/mode
    pub async fn personal_fm_mode(&self, query: &Query) -> Result<ApiResponse> {
        // 上游只发送调用方给出的 mode / subMode，缺省时由服务端选默认模式。
        let mut data = json!({
            "limit": query.get_i64("limit", 3)
        });
        if let Some(mode) = query.get("mode") {
            data["mode"] = json!(mode);
        }
        if let Some(sub_mode) = query.get("submode") {
            data["subMode"] = json!(sub_mode);
        }
        self.request(
            "/api/v1/radio/get",
            data,
            query.to_option(CryptoType::default()),
        )
        .await
    }
}
