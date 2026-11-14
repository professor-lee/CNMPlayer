use super::Query;
use crate::error::Result;
/// 听歌打卡
/// 对应 Node.js module/scrobble.js
use crate::request::{ApiClient, ApiResponse, CryptoType};
use serde_json::json;

impl ApiClient {
    /// 听歌打卡
    /// 对应 /scrobble
    pub async fn scrobble(&self, query: &Query) -> Result<ApiResponse> {
        let data = json!({
            "logs": serde_json::to_string(&json!([{
                "action": "play",
                "json": {
                    "download": 0,
                    "end": query.get_or("end", "playend"),
                    "id": query.get_or("id", "0"),
                    "sourceId": query.get_or("sourceid", ""),
                    "time": query.get_or("time", "0").parse::<i64>().unwrap_or(0),
                    "type": "song",
                    "wifi": 0,
                    "source": "list",
                    "mainsite": 1,
                    "content": ""
                }
            }])).unwrap_or_default()
        });
        self.request(
            "/api/feedback/weblog",
            data,
            query.to_option(CryptoType::Weapi),
        )
        .await
    }
}
