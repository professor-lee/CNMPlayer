use super::Query;
use crate::error::Result;
/// 歌曲下载链接 v1
/// 对应 Node.js module/song_download_url_v1.js
use crate::request::{ApiClient, ApiResponse, CryptoType};
use crate::util::config::VIVID_COOKIE_OVERRIDES;
use serde_json::json;

impl ApiClient {
    /// 歌曲下载链接 v1
    /// 对应 /song/download/url/v1
    pub async fn song_download_url_v1(&self, query: &Query) -> Result<ApiResponse> {
        let level = query.get_or("level", "standard");
        let mut option = query.to_option(CryptoType::default());
        if level == "vivid" {
            option.cookie_overrides = VIVID_COOKIE_OVERRIDES.to_vec();
        }
        let data = json!({
            "id": query.get_or("id", "0"),
            "immerseType": "c51",
            "level": level
        });
        self.request("/api/song/enhance/download/url/v1", data, option)
            .await
    }
}
