use super::Query;
use crate::error::Result;
/// 听歌识曲
/// 对应 Node.js module/audio_match.js
///
/// 注意: Node.js 版本直接通过 axios GET 请求外部接口，
/// Rust 版本同样直接发送 HTTP GET 请求。
use crate::request::{ApiClient, ApiResponse};
use serde_json::json;

impl ApiClient {
    /// 听歌识曲
    /// 对应 /audio/match
    pub async fn audio_match(&self, query: &Query) -> Result<ApiResponse> {
        let duration = query.get_or("duration", "0");
        let audio_fp = query.get_or("audioFP", "");
        let encoded_fp = urlencoding::encode(&audio_fp);
        let url = format!(
            "https://interface.music.163.com/api/music/audio/match?sessionId=0123456789abcdef&algorithmCode=shazam_v2&duration={}&rawdata={}&times=1&decrypt=1",
            duration, encoded_fp
        );

        let client = cyper::Client::new()?;
        let res = client.get(&url)?.send().await?;
        let body: serde_json::Value = res.json().await?;

        Ok(ApiResponse {
            status: 200,
            body: json!({
                "code": 200,
                "data": body.get("data").cloned().unwrap_or(json!(null)),
            }),
            cookie: vec![],
        })
    }
}
