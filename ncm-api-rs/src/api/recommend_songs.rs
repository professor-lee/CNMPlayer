use super::Query;
use crate::error::Result;
/// 每日推荐歌曲
/// 对应 Node.js module/recommend_songs.js
use crate::request::{ApiClient, ApiResponse, CryptoType};
use serde_json::json;

impl ApiClient {
    /// 每日推荐歌曲（需要登录）；`afresh` 存在时透传，用于刷新每日推荐
    /// 对应 /recommend/songs
    pub async fn recommend_songs(&self, query: &Query) -> Result<ApiResponse> {
        let mut data = json!({});
        if let Some(afresh) = query.get("afresh") {
            data["afresh"] = json!(afresh);
        }
        self.request(
            "/api/v3/discovery/recommend/songs",
            data,
            query.to_option(CryptoType::Weapi),
        )
        .await
    }
}
