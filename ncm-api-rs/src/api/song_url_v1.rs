use super::Query;
use crate::error::Result;
/// 歌曲播放链接 v1
/// 对应 Node.js module/song_url_v1.js
use crate::request::{ApiClient, ApiResponse, CryptoType};
use crate::util::config::VIVID_COOKIE_OVERRIDES;
use serde_json::json;

impl ApiClient {
    /// 歌曲播放链接（上游已改走 xeapi）
    /// 对应 /song/url/v1
    ///
    /// `level=sky` 时可用 `immerseType` 选沉浸声类型（默认 c51）；
    /// `level=vivid` 时按上游改用 mp3 载荷并以安卓客户端身份请求。
    pub async fn song_url_v1(&self, query: &Query) -> Result<ApiResponse> {
        let id = query.get_or("id", "0");
        let level = query.get_or("level", "standard");
        let mut data = json!({
            "ids": format!("[{}]", id),
            "level": level,
            "encodeType": "flac"
        });
        let mut option = query.to_option(CryptoType::Xeapi);
        match level.as_str() {
            "sky" => data["immerseType"] = json!(query.get_or("immerseType", "c51")),
            "vivid" => {
                data["encodeType"] = json!("mp3");
                option.cookie_overrides = VIVID_COOKIE_OVERRIDES.to_vec();
            }
            _ => {}
        }
        self.request("/api/song/enhance/player/url/v1", data, option)
            .await
    }
}
