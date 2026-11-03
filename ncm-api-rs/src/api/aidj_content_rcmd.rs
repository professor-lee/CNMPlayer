use super::Query;
use crate::error::Result;
/// 私人 DJ
/// 对应 Node.js module/aidj_content_rcmd.js
use crate::request::{ApiClient, ApiResponse, CryptoType};
use serde_json::json;
use std::time::{SystemTime, UNIX_EPOCH};

impl ApiClient {
    /// 私人 DJ
    /// 对应 /aidj/content/rcmd
    pub async fn aidj_content_rcmd(&self, query: &Query) -> Result<ApiResponse> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default();
        let timestamp_ms = now.as_millis() as u64;
        let timestamp_s = now.as_secs();

        let mut ext_info = json!({
            "noAidjToAidj": false,
            "lastRequestTimestamp": timestamp_ms,
            "listenedTs": false
        });

        if let Some(latitude) = query.get("latitude")
            && let Some(longitude) = query.get("longitude")
        {
            ext_info["lbsInfoList"] = json!([{
                "lat": latitude,
                "lon": longitude,
                "time": timestamp_s
            }]);
        }

        let data = json!({
            "extInfo": ext_info.to_string()
        });
        self.request(
            "/api/aidj/content/rcmd/info",
            data,
            query.to_option(CryptoType::Eapi),
        )
        .await
    }
}
