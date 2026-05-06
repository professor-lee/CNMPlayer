use crate::error::Result;
/// 相关歌单
/// 对应 Node.js module/related_playlist.js
///
/// 注意：此接口通过抓取网页 HTML 获取数据，不使用标准 API 请求。
use crate::request::{ApiClient, ApiResponse};
use regex_lite::Regex;
use serde_json::json;

impl ApiClient {
    /// 相关歌单
    /// 对应 /related/playlist
    pub async fn related_playlist(&self, query: &super::Query) -> Result<ApiResponse> {
        let id = query.get_or("id", "0");
        let url = format!("https://music.163.com/playlist?id={}", id);

        let resp = self.client.get(&url)?.send().await?;
        let status = resp.status().as_u16() as i64;
        let html = resp.text().await?;

        let pattern = Regex::new(
            r#"<div class="cver u-cover u-cover-3">[\s\S]*?<img src="([^"]+)">[\s\S]*?<a class="sname f-fs1 s-fc0" href="([^"]+)"[^>]*>([^<]+?)</a>[\s\S]*?<a class="nm nm f-thide s-fc3" href="([^"]+)"[^>]*>([^<]+?)</a>"#,
        ).map_err(|e| crate::error::NcmError::Unknown(e.to_string()))?;

        let mut playlists = Vec::new();
        for caps in pattern.captures_iter(&html) {
            let cover_img_url = caps.get(1).map(|m| m.as_str()).unwrap_or("");
            let playlist_href = caps.get(2).map(|m| m.as_str()).unwrap_or("");
            let name = caps.get(3).map(|m| m.as_str()).unwrap_or("");
            let user_href = caps.get(4).map(|m| m.as_str()).unwrap_or("");
            let nickname = caps.get(5).map(|m| m.as_str()).unwrap_or("");

            let user_id = user_href
                .strip_prefix("/user/home?id=")
                .unwrap_or(user_href);
            let playlist_id = playlist_href
                .strip_prefix("/playlist?id=")
                .unwrap_or(playlist_href);
            let cover = cover_img_url
                .strip_suffix("?param=50y50")
                .unwrap_or(cover_img_url);

            playlists.push(json!({
                "creator": {
                    "userId": user_id,
                    "nickname": nickname
                },
                "coverImgUrl": cover,
                "name": name,
                "id": playlist_id
            }));
        }

        Ok(ApiResponse {
            status,
            body: json!({ "code": 200, "playlists": playlists }),
            cookie: vec![],
        })
    }
}
