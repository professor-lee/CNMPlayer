use super::Query;
use crate::error::Result;
/// 听歌打卡
/// 对应 Node.js module/scrobble.js
use crate::request::{ApiClient, ApiResponse, CryptoType};
use serde_json::json;

/// 听歌日志上报网关（上游 APP_CONF.clDomian）
const CLIENT_LOG_DOMAIN: &str = "https://clientlog.music.163.com";

impl ApiClient {
    /// 听歌打卡：与上游一致以 os=osx 走 clientlog 的 eapi，先上报 startplay
    /// （进「最近播放」），再上报 play（涨「听歌排行」计数）。
    /// 对应 /scrobble
    ///
    /// `end` 为结束原因，缺省 `playend`（上游固定值）。
    pub async fn scrobble(&self, query: &Query) -> Result<ApiResponse> {
        let id = query.get_or("id", "0");
        let source_id = query.get_or("sourceid", "");
        let content = format!("id={source_id}");
        let logs = |log: serde_json::Value| json!({ "logs": json!([log]).to_string() });

        let startplay = logs(json!({
            "action": "startplay",
            "json": {
                "id": id,
                "type": "song",
                "mainsite": "1",
                "mainsiteWeb": "1",
                "content": content
            }
        }));
        let play = logs(json!({
            "action": "play",
            "json": {
                "download": 0,
                "end": query.get_or("end", "playend"),
                "id": id,
                "sourceId": source_id,
                "time": query.get_or("time", "0"),
                "type": "song",
                "wifi": 0,
                "source": "list",
                "mainsite": "1",
                "mainsiteWeb": "1",
                "content": content
            }
        }));

        let option = || {
            let mut option = query.to_option(CryptoType::Eapi);
            option.domain = Some(CLIENT_LOG_DOMAIN.to_string());
            option.cookie_overrides = vec![("os", "osx")];
            option
        };
        let startplay = self
            .request("/api/feedback/weblog", startplay, option())
            .await?;
        let play = self.request("/api/feedback/weblog", play, option()).await?;

        let mut cookie = startplay.cookie;
        cookie.extend(play.cookie);
        Ok(ApiResponse {
            status: 200,
            body: json!({
                "code": 200,
                "data": "success",
                "details": { "startplay": startplay.body, "play": play.body }
            }),
            cookie,
        })
    }
}
