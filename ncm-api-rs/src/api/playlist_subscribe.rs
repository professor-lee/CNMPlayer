use super::Query;
use crate::error::Result;
/// 收藏/取消收藏歌单
/// 对应 Node.js module/playlist_subscribe.js
use crate::request::{ApiClient, ApiResponse, CryptoType};
use serde_json::json;

impl ApiClient {
    /// 收藏/取消收藏歌单；与上游一致两种操作都带实时反作弊 token
    /// 对应 /playlist/subscribe
    pub async fn playlist_subscribe(&self, query: &Query) -> Result<ApiResponse> {
        let t = query.get_or("t", "1");
        let path = if t == "1" { "subscribe" } else { "unsubscribe" };
        let data = json!({
            "id": query.get_or("id", "0")
        });
        let mut option = query.to_option(CryptoType::Eapi);
        option.check_token = true;
        self.request(&format!("/api/playlist/{}", path), data, option)
            .await
    }
}
