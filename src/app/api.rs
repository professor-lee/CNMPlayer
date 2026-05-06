use anyhow::{Context, Result, anyhow, bail};
use cyper::{Client, Response};
use futures::StreamExt;
use ncm_api::{ApiClient, ApiResponse, Query};

const MAX_COVER_IMAGE_BYTES: usize = 5 * 1024 * 1024;

#[derive(Clone)]
pub struct ApiState {
    client: ApiClient,
    cookie: Option<String>,
    http: Client,
}

impl ApiState {
    pub fn new(cookie: Option<String>, http: Client) -> Result<Self> {
        let client = ApiClient::new(cookie.clone(), http.clone());

        Ok(Self {
            client,
            cookie,
            http,
        })
    }

    pub fn session_cookie(&self) -> Option<&str> {
        self.cookie.as_deref()
    }

    /// Get the HTTP client for streaming playback.
    pub fn http_client(&self) -> &Client {
        &self.http
    }

    pub fn set_cookie(&mut self, cookie: String) {
        self.cookie = Some(cookie.clone());
        self.client.set_cookie(cookie);
    }

    pub fn clear_cookie(&mut self) {
        self.cookie = None;
        self.client.set_cookie(String::new());
    }

    pub async fn validate_cookie(&mut self, cookie: &str) -> Result<bool> {
        let query = Query::new().cookie(cookie);
        let response = self.client.login_status(&query).await?;
        let code = response
            .body
            .get("code")
            .and_then(|value| value.as_i64())
            .unwrap_or(response.status);

        if code == 200 {
            self.set_cookie(cookie.to_string());
            self.capture_cookie(&response);
            return Ok(true);
        }

        Ok(false)
    }

    pub async fn login_status(&mut self) -> Result<ApiResponse> {
        let query = self.query_with_cookie();
        let response = self.client.login_status(&query).await?;
        self.capture_cookie(&response);
        Ok(response)
    }

    pub async fn user_account(&mut self) -> Result<ApiResponse> {
        let query = self.query_with_cookie();
        let response = self.client.user_account(&query).await?;
        self.capture_cookie(&response);
        Ok(response)
    }

    pub async fn user_playlist_create(
        &mut self,
        uid: &str,
        limit: usize,
        offset: usize,
    ) -> Result<ApiResponse> {
        let query = self
            .query_with_cookie()
            .param("uid", uid)
            .param("limit", &limit.max(1).to_string())
            .param("offset", &offset.to_string());
        let response = self.client.user_playlist_create(&query).await?;
        Ok(response)
    }

    pub async fn user_playlist_collect(
        &mut self,
        uid: &str,
        limit: usize,
        offset: usize,
    ) -> Result<ApiResponse> {
        let query = self
            .query_with_cookie()
            .param("uid", uid)
            .param("limit", &limit.max(1).to_string())
            .param("offset", &offset.to_string());
        let response = self.client.user_playlist_collect(&query).await?;
        Ok(response)
    }

    pub async fn login_email(&mut self, email: &str, password: &str) -> Result<ApiResponse> {
        let query = Query::new()
            .param("email", email)
            .param("password", password);
        let response = self.client.login(&query).await?;
        self.capture_cookie(&response);
        Ok(response)
    }

    pub async fn captcha_sent(&mut self, phone: &str) -> Result<ApiResponse> {
        let query = Query::new().param("phone", phone);
        let response = self.client.captcha_sent(&query).await?;
        Ok(response)
    }

    pub async fn login_phone_captcha(&mut self, phone: &str, captcha: &str) -> Result<ApiResponse> {
        let query = Query::new().param("phone", phone).param("captcha", captcha);
        let response = self.client.login_cellphone(&query).await?;
        self.capture_cookie(&response);
        Ok(response)
    }

    pub async fn login_qr_key(&mut self) -> Result<ApiResponse> {
        let query = Query::new();
        let response = self.client.login_qr_key(&query).await?;
        Ok(response)
    }

    pub async fn login_qr_create(&mut self, key: &str) -> Result<ApiResponse> {
        let query = Query::new().param("key", key);
        let response = self.client.login_qr_create(&query).await?;
        Ok(response)
    }

    pub async fn login_qr_check(&mut self, key: &str) -> Result<ApiResponse> {
        let query = Query::new().param("key", key);
        let response = self.client.login_qr_check(&query).await?;
        self.capture_cookie(&response);
        Ok(response)
    }

    pub async fn recommend_resource(&mut self) -> Result<ApiResponse> {
        let query = self.query_with_cookie();
        let response = self.client.recommend_resource(&query).await?;
        Ok(response)
    }

    pub async fn recommend_songs(&mut self) -> Result<ApiResponse> {
        let query = self.query_with_cookie();
        let response = self.client.recommend_songs(&query).await?;
        Ok(response)
    }

    pub async fn personalized(&mut self, limit: usize) -> Result<ApiResponse> {
        let limit = limit.max(1).to_string();
        let query = self.query_with_cookie().param("limit", &limit);
        let response = self.client.personalized(&query).await?;
        Ok(response)
    }

    pub async fn playlist_detail(&mut self, id: &str) -> Result<ApiResponse> {
        let query = self.query_with_cookie().param("id", id);
        let response = self.client.playlist_detail(&query).await?;
        Ok(response)
    }

    pub async fn album(&mut self, id: &str) -> Result<ApiResponse> {
        let query = self.query_with_cookie().param("id", id);
        let response = self.client.album(&query).await?;
        Ok(response)
    }

    pub async fn artist_detail(&mut self, id: &str) -> Result<ApiResponse> {
        let query = self.query_with_cookie().param("id", id);
        let response = self.client.artist_detail(&query).await?;
        Ok(response)
    }

    pub async fn artist_desc(&mut self, id: &str) -> Result<ApiResponse> {
        let query = self.query_with_cookie().param("id", id);
        let response = self.client.artist_desc(&query).await?;
        Ok(response)
    }

    pub async fn artist_top_song(&mut self, id: &str) -> Result<ApiResponse> {
        let query = self.query_with_cookie().param("id", id);
        let response = self.client.artist_top_song(&query).await?;
        Ok(response)
    }

    pub async fn artist_album(
        &mut self,
        id: &str,
        limit: usize,
        offset: usize,
    ) -> Result<ApiResponse> {
        let query = self
            .query_with_cookie()
            .param("id", id)
            .param("limit", &limit.max(1).to_string())
            .param("offset", &offset.to_string());
        let response = self.client.artist_album(&query).await?;
        Ok(response)
    }

    pub async fn artist_sublist(&mut self, limit: usize, offset: usize) -> Result<ApiResponse> {
        let query = self
            .query_with_cookie()
            .param("limit", &limit.max(1).to_string())
            .param("offset", &offset.to_string());
        let response = self.client.artist_sublist(&query).await?;
        Ok(response)
    }

    pub async fn song_detail(&mut self, song_id: &str) -> Result<ApiResponse> {
        let query = self.query_with_cookie().param("ids", song_id);
        let response = self.client.song_detail(&query).await?;
        Ok(response)
    }

    pub async fn lyric(&mut self, song_id: &str) -> Result<ApiResponse> {
        let query = self.query_with_cookie().param("id", song_id);
        let response = self.client.lyric(&query).await?;
        Ok(response)
    }

    pub async fn like_song(&mut self, song_id: &str, like: bool) -> Result<ApiResponse> {
        let query = self
            .query_with_cookie()
            .param("id", song_id)
            .param("like", if like { "true" } else { "false" });
        let response = self.client.like(&query).await?;
        Ok(response)
    }

    pub async fn likelist(&mut self, uid: &str) -> Result<ApiResponse> {
        let query = self.query_with_cookie().param("uid", uid);
        let response = self.client.likelist(&query).await?;
        Ok(response)
    }

    pub async fn song_like_check(&mut self, ids_json: &str) -> Result<ApiResponse> {
        let query = self.query_with_cookie().param("ids", ids_json);
        let response = self.client.song_like_check(&query).await?;
        Ok(response)
    }

    pub async fn song_url(&mut self, song_id: &str) -> Result<ApiResponse> {
        let query = self
            .query_with_cookie()
            .param("id", song_id)
            .param("br", "320000");
        let response = self.client.song_url(&query).await?;
        Ok(response)
    }

    pub async fn song_url_v1(&mut self, song_id: &str, level: &str) -> Result<ApiResponse> {
        let query = self
            .query_with_cookie()
            .param("id", song_id)
            .param("level", level);
        let response = self.client.song_url_v1(&query).await?;
        Ok(response)
    }

    pub async fn song_stream_url_with_quality(
        &mut self,
        song_id: &str,
        level: &str,
    ) -> Result<String> {
        let response = self.song_url_v1(song_id, level).await?;
        if let Some(url) = response
            .body
            .pointer("/data/0/url")
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            return Ok(url.to_string());
        }

        // Keep backward compatibility for songs that only expose legacy stream URLs.
        let fallback = self.song_url(song_id).await?;
        if let Some(url) = fallback
            .body
            .pointer("/data/0/url")
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            return Ok(url.to_string());
        }

        Err(anyhow!(
            "song stream url not found for id {} at level {}",
            song_id,
            level
        ))
    }

    pub async fn vip_info(&mut self) -> Result<ApiResponse> {
        let query = self.query_with_cookie();
        let response = self.client.vip_info(&query).await?;
        Ok(response)
    }

    pub async fn vip_info_v2(&mut self) -> Result<ApiResponse> {
        let query = self.query_with_cookie();
        let response = self.client.vip_info_v2(&query).await?;
        Ok(response)
    }

    pub async fn search(
        &mut self,
        keywords: &str,
        search_type: i32,
        limit: usize,
        offset: usize,
    ) -> Result<ApiResponse> {
        let query = self
            .query_with_cookie()
            .param("keywords", keywords)
            .param("type", &search_type.to_string())
            .param("limit", &limit.max(1).to_string())
            .param("offset", &offset.to_string());
        let response = self.client.search(&query).await?;
        Ok(response)
    }

    pub async fn fetch_cover_bytes(&self, url: &str) -> Result<Vec<u8>> {
        let url = url.trim();
        if url.is_empty() {
            return Ok(Vec::new());
        }

        let response = self.http.get(url)?.send().await?;
        let response = error_for_status(response)?;

        if let Some(content_len) = response.content_length() {
            if content_len > MAX_COVER_IMAGE_BYTES as u64 {
                return Err(anyhow!(
                    "cover image exceeds {} byte limit",
                    MAX_COVER_IMAGE_BYTES
                ));
            }
        }

        let mut bytes = Vec::with_capacity(64 * 1024);
        let mut stream = response.bytes_stream();
        while let Some(Ok(chunk)) = stream.next().await {
            if chunk.is_empty() {
                continue;
            }

            if bytes.len().saturating_add(chunk.len()) > MAX_COVER_IMAGE_BYTES {
                return Err(anyhow!(
                    "cover image exceeds {} byte limit",
                    MAX_COVER_IMAGE_BYTES
                ));
            }

            bytes.extend_from_slice(&chunk);
        }

        let bytes = Ok::<Vec<u8>, anyhow::Error>(bytes)
            .with_context(|| format!("download cover image failed: {}", url))?;

        Ok(bytes)
    }

    fn query_with_cookie(&self) -> Query {
        if let Some(cookie) = self.cookie.as_deref() {
            return Query::new().cookie(cookie);
        }
        Query::new()
    }

    fn capture_cookie(&mut self, response: &ApiResponse) {
        if response.cookie.is_empty() {
            return;
        }

        let merged = response.cookie.join("; ");
        self.cookie = Some(merged.clone());
        self.client.set_cookie(merged);
    }
}

pub fn error_for_status(resp: Response) -> Result<Response> {
    let status = resp.status();
    let url = resp.url();
    let reason = status.canonical_reason().unwrap_or_default();
    if status.is_client_error() || status.is_server_error() {
        bail!("{url} {status} {reason}");
    } else {
        Ok(resp)
    }
}
