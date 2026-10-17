use anyhow::Context;
use anyhow::Result;
use anyhow::anyhow;
use anyhow::bail;
use cyper::Client;
use cyper::Response;
use futures::StreamExt;
use image::load_from_memory;
use ncm_api::ApiClient;
use ncm_api::ApiResponse;
use ncm_api::Query;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
use std::time::Duration;

const MAX_COVER_IMAGE_BYTES: usize = 5 * 1024 * 1024;
const MAX_ORIGINAL_COVER_BYTES: usize = 20 * 1024 * 1024;
const COVER_NETWORK_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_COVER_VALIDATIONS: usize = 2;
const COVER_SIZE_QUERY_KEY: &str = "param";
const DISPLAY_COVER_EDGE: u32 = 500;
static COVER_VALIDATION_GATE: CoverValidationGate = CoverValidationGate::new();

/// Playback/UI covers are small on screen. Ask the CDN for that size instead of
/// downloading the original and relying on a post-decode resize.
pub(super) fn cover_url_for_display(url: &str) -> String {
    let url = url.trim();
    if url.is_empty() || !url.starts_with("http") {
        return url.to_string();
    }
    set_url_query_param(
        url,
        COVER_SIZE_QUERY_KEY,
        &format!("{DISPLAY_COVER_EDGE}y{DISPLAY_COVER_EDGE}"),
    )
}

/// Song downloads embed the original artwork; size params on API URLs would
/// silently replace it with a thumbnail.
pub(super) fn cover_url_for_download(url: &str) -> String {
    let url = url.trim();
    if url.is_empty() {
        return url.to_string();
    }
    remove_url_query_param(url, COVER_SIZE_QUERY_KEY)
}

fn set_url_query_param(url: &str, key: &str, value: &str) -> String {
    let (base, query, fragment) = split_url_query(url);
    let mut pairs = query_pairs_without_key(query, key);
    pairs.push(format!("{key}={value}"));
    format!("{base}?{}{fragment}", pairs.join("&"))
}

fn remove_url_query_param(url: &str, key: &str) -> String {
    let (base, query, fragment) = split_url_query(url);
    let pairs = query_pairs_without_key(query, key);
    if pairs.is_empty() {
        format!("{base}{fragment}")
    } else {
        format!("{base}?{}{fragment}", pairs.join("&"))
    }
}

fn split_url_query(url: &str) -> (&str, &str, &str) {
    let (body, fragment) = match url.find('#') {
        Some(index) => (&url[..index], &url[index..]),
        None => (url, ""),
    };
    match body.find('?') {
        Some(index) => (&body[..index], &body[index + 1..], fragment),
        None => (body, "", fragment),
    }
}

fn query_pairs_without_key(query: &str, key: &str) -> Vec<String> {
    query
        .split('&')
        .filter(|pair| !pair.is_empty())
        .filter(|pair| pair.split('=').next() != Some(key))
        .map(str::to_string)
        .collect()
}

/// Decoders can recover incomplete images. A persisted or embedded cover must
/// at least carry the format's terminal marker; full decoding is optional for
/// byte-only paths that embed the original.
pub(super) fn validate_cover_bytes(bytes: &[u8]) -> Result<()> {
    match image::guess_format(bytes)? {
        image::ImageFormat::Png => {
            if !bytes.ends_with(&[0, 0, 0, 0, 73, 69, 78, 68, 174, 66, 96, 130]) {
                bail!("PNG cover is missing its terminal IEND chunk");
            }
        }
        image::ImageFormat::Jpeg => {
            if !bytes.ends_with(&[0xff, 0xd9]) {
                bail!("JPEG cover is missing its end-of-image marker");
            }
        }
        _ => bail!("unsupported cover image format"),
    }
    Ok(())
}

#[derive(Debug)]
struct CoverValidationGate {
    in_flight: AtomicUsize,
}

impl CoverValidationGate {
    const fn new() -> Self {
        Self {
            in_flight: AtomicUsize::new(0),
        }
    }

    fn try_acquire(&self) -> Result<CoverValidationPermit<'_>> {
        let mut count = self.in_flight.load(Ordering::Acquire);
        loop {
            if count >= MAX_COVER_VALIDATIONS {
                bail!(
                    "cover image validation is busy ({MAX_COVER_VALIDATIONS} jobs already running)"
                );
            }
            match self.in_flight.compare_exchange_weak(
                count,
                count + 1,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return Ok(CoverValidationPermit { gate: self }),
                Err(actual) => count = actual,
            }
        }
    }

    async fn acquire(&self, timeout: Duration) -> Result<CoverValidationPermit<'_>> {
        compio::time::timeout(timeout, async {
            loop {
                if let Ok(permit) = self.try_acquire() {
                    break permit;
                }
                compio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .map_err(|_| anyhow!("cover decoder admission timed out"))
    }
}

#[derive(Debug)]
struct CoverValidationPermit<'a> {
    gate: &'a CoverValidationGate,
}

impl Drop for CoverValidationPermit<'_> {
    fn drop(&mut self) {
        self.gate.in_flight.fetch_sub(1, Ordering::Release);
    }
}

#[derive(Clone)]
pub(crate) struct ApiState {
    client: ApiClient,
    cookie: Option<String>,
    http: Client,
    wake: crate::render::wake::WakeSignal,
}

impl ApiState {
    pub(super) fn new(cookie: Option<String>, http: Client) -> Result<Self> {
        let client = ApiClient::new(cookie.clone(), http.clone());

        Ok(Self {
            client,
            cookie,
            http,
            wake: crate::render::wake::WakeSignal::default(),
        })
    }

    pub(crate) fn wake_signal(&self) -> crate::render::wake::WakeSignal {
        self.wake.clone()
    }

    pub(super) fn session_cookie(&self) -> Option<&str> {
        self.cookie.as_deref()
    }

    /// Get the HTTP client for streaming playback.
    pub(super) fn http_client(&self) -> &Client {
        &self.http
    }

    pub(super) fn set_cookie(&mut self, cookie: String) {
        self.cookie = Some(cookie.clone());
        self.client.set_cookie(cookie);
    }

    pub(super) fn clear_cookie(&mut self) {
        self.cookie = None;
        self.client.set_cookie(String::new());
    }

    pub(super) async fn validate_cookie(&mut self, cookie: &str) -> Result<bool> {
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

    pub(super) async fn login_status(&mut self) -> Result<ApiResponse> {
        let query = self.query_with_cookie();
        let response = self.client.login_status(&query).await?;
        self.capture_cookie(&response);
        Ok(response)
    }

    pub(super) async fn user_account(&mut self) -> Result<ApiResponse> {
        let query = self.query_with_cookie();
        let response = self.client.user_account(&query).await?;
        self.capture_cookie(&response);
        Ok(response)
    }

    pub(super) async fn user_playlist_create(
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

    pub(super) async fn user_playlist_collect(
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

    pub(super) async fn login_email(&mut self, email: &str, password: &str) -> Result<ApiResponse> {
        let query = Query::new()
            .param("email", email)
            .param("password", password);
        let response = self.client.login(&query).await?;
        self.capture_cookie(&response);
        Ok(response)
    }

    pub(super) async fn captcha_sent(&mut self, phone: &str) -> Result<ApiResponse> {
        let query = Query::new().param("phone", phone);
        let response = self.client.captcha_sent(&query).await?;
        Ok(response)
    }

    pub(super) async fn login_phone_captcha(
        &mut self,
        phone: &str,
        captcha: &str,
    ) -> Result<ApiResponse> {
        let query = Query::new().param("phone", phone).param("captcha", captcha);
        let response = self.client.login_cellphone(&query).await?;
        self.capture_cookie(&response);
        Ok(response)
    }

    pub(super) async fn login_qr_key(&mut self) -> Result<ApiResponse> {
        let query = Query::new();
        let response = self.client.login_qr_key(&query).await?;
        Ok(response)
    }

    pub(super) async fn login_qr_create(&mut self, key: &str) -> Result<ApiResponse> {
        let query = Query::new().param("key", key);
        let response = self.client.login_qr_create(&query).await?;
        Ok(response)
    }

    pub(super) async fn login_qr_check(&mut self, key: &str) -> Result<ApiResponse> {
        let query = Query::new().param("key", key);
        let response = self.client.login_qr_check(&query).await?;
        self.capture_cookie(&response);
        Ok(response)
    }

    pub(super) async fn recommend_resource(&mut self) -> Result<ApiResponse> {
        let query = self.query_with_cookie();
        let response = self.client.recommend_resource(&query).await?;
        Ok(response)
    }

    pub(super) async fn recommend_songs(&mut self) -> Result<ApiResponse> {
        let query = self.query_with_cookie();
        let response = self.client.recommend_songs(&query).await?;
        Ok(response)
    }

    pub(super) async fn personalized(&mut self, limit: usize) -> Result<ApiResponse> {
        let limit = limit.max(1).to_string();
        let query = self.query_with_cookie().param("limit", &limit);
        let response = self.client.personalized(&query).await?;
        Ok(response)
    }

    /// 私人漫游（私人 FM 模式选择）
    pub(super) async fn personal_fm_mode(
        &mut self,
        mode: &str,
        limit: usize,
    ) -> Result<ApiResponse> {
        let query = self
            .query_with_cookie()
            .param("mode", mode)
            .param("limit", &limit.max(1).to_string());
        let response = self.client.personal_fm_mode(&query).await?;
        Ok(response)
    }

    pub(super) async fn playlist_detail(&mut self, id: &str) -> Result<ApiResponse> {
        let query = self.query_with_cookie().param("id", id);
        let response = self.client.playlist_detail(&query).await?;
        Ok(response)
    }
    pub(super) async fn playlist_track_all(
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
        let response = self.client.playlist_track_all(&query).await?;
        Ok(response)
    }

    pub(super) async fn album(&mut self, id: &str) -> Result<ApiResponse> {
        let query = self.query_with_cookie().param("id", id);
        let response = self.client.album(&query).await?;
        Ok(response)
    }

    pub(super) async fn artist_detail(&mut self, id: &str) -> Result<ApiResponse> {
        let query = self.query_with_cookie().param("id", id);
        let response = self.client.artist_detail(&query).await?;
        Ok(response)
    }

    pub(super) async fn artist_desc(&mut self, id: &str) -> Result<ApiResponse> {
        let query = self.query_with_cookie().param("id", id);
        let response = self.client.artist_desc(&query).await?;
        Ok(response)
    }

    pub(super) async fn artist_top_song(&mut self, id: &str) -> Result<ApiResponse> {
        let query = self.query_with_cookie().param("id", id);
        let response = self.client.artist_top_song(&query).await?;
        Ok(response)
    }

    pub(super) async fn artist_album(
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

    pub(super) async fn artist_sublist(
        &mut self,
        limit: usize,
        offset: usize,
    ) -> Result<ApiResponse> {
        let query = self
            .query_with_cookie()
            .param("limit", &limit.max(1).to_string())
            .param("offset", &offset.to_string());
        let response = self.client.artist_sublist(&query).await?;
        Ok(response)
    }

    pub(super) async fn song_detail(&mut self, song_id: &str) -> Result<ApiResponse> {
        let query = self.query_with_cookie().param("ids", song_id);
        let response = self.client.song_detail(&query).await?;
        Ok(response)
    }

    pub(super) async fn lyric(&mut self, song_id: &str) -> Result<ApiResponse> {
        let query = self.query_with_cookie().param("id", song_id);
        let response = self.client.lyric(&query).await?;
        Ok(response)
    }

    pub(super) async fn like_song(&mut self, song_id: &str, like: bool) -> Result<ApiResponse> {
        let query = self
            .query_with_cookie()
            .param("id", song_id)
            .param("like", if like { "true" } else { "false" });
        let response = self.client.like(&query).await?;
        Ok(response)
    }

    pub(super) async fn likelist(&mut self, uid: &str) -> Result<ApiResponse> {
        let query = self.query_with_cookie().param("uid", uid);
        let response = self.client.likelist(&query).await?;
        Ok(response)
    }

    pub(super) async fn song_like_check(&mut self, ids_json: &str) -> Result<ApiResponse> {
        let query = self.query_with_cookie().param("ids", ids_json);
        let response = self.client.song_like_check(&query).await?;
        Ok(response)
    }

    async fn song_url(&mut self, song_id: &str) -> Result<ApiResponse> {
        let query = self
            .query_with_cookie()
            .param("id", song_id)
            .param("br", "320000");
        let response = self.client.song_url(&query).await?;
        Ok(response)
    }

    async fn song_url_v1(&mut self, song_id: &str, level: &str) -> Result<ApiResponse> {
        let query = self
            .query_with_cookie()
            .param("id", song_id)
            .param("level", level);
        let response = self.client.song_url_v1(&query).await?;
        Ok(response)
    }

    /// 客户端下载链接（新版）：登录后可用，对免费歌曲能拿到比播放更高的档位。
    async fn song_download_url_v1(&mut self, song_id: &str, level: &str) -> Result<ApiResponse> {
        let query = self
            .query_with_cookie()
            .param("id", song_id)
            .param("level", level);
        let response = self.client.song_download_url_v1(&query).await?;
        Ok(response)
    }

    /// 下载取链：`download/url/v1` 优先（未登录/无版权时回包里的 url 为空），
    /// 退回 `player/url/v1`，再退回旧 `/song/url`。
    pub(super) async fn audio_download_url(
        &mut self,
        song_id: &str,
        level: &str,
    ) -> Result<AudioDownloadSource> {
        if let Ok(response) = self.song_download_url_v1(song_id, level).await
            && let Some(source) = parse_audio_source(&response, level)
        {
            return Ok(source);
        }

        let response = self.song_url_v1(song_id, level).await?;
        if let Some(source) = parse_audio_source(&response, level) {
            return Ok(source);
        }

        let fallback = self.song_url(song_id).await?;
        parse_audio_source(&fallback, level).ok_or_else(|| {
            anyhow!(
                "song download url not found for id {} at level {}",
                song_id,
                level
            )
        })
    }

    pub(super) async fn song_stream_url_with_quality(
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

    pub(super) async fn vip_info(&mut self) -> Result<ApiResponse> {
        let query = self.query_with_cookie();
        let response = self.client.vip_info(&query).await?;
        Ok(response)
    }

    pub(super) async fn vip_info_v2(&mut self) -> Result<ApiResponse> {
        let query = self.query_with_cookie();
        let response = self.client.vip_info_v2(&query).await?;
        Ok(response)
    }

    pub(super) async fn search(
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
        let response = self.client.cloudsearch(&query).await?;
        Ok(response)
    }

    /// Download path: fetch the original artwork. The bytes are embedded into
    /// the tag as-is, so skip the full decode and only prove the container is
    /// complete.
    pub(super) async fn fetch_original_cover_bytes(&self, url: &str) -> Result<Vec<u8>> {
        let url = cover_url_for_download(url);
        Ok(self
            .fetch_cover_with_timeout(
                &url,
                COVER_NETWORK_TIMEOUT,
                false,
                false,
                MAX_ORIGINAL_COVER_BYTES,
            )
            .await?
            .0)
    }

    /// Raw-bytes helper retained for cover transport tests.
    #[cfg(test)]
    pub(super) async fn fetch_cover_bytes(&self, url: &str) -> Result<Vec<u8>> {
        self.fetch_cover_bytes_with_timeout(url, COVER_NETWORK_TIMEOUT)
            .await
    }

    /// Playback cover path: request the CDN's display-sized artwork.
    pub(super) async fn fetch_display_cover_bytes(&self, url: &str) -> Result<Vec<u8>> {
        self.fetch_cover_bytes_with_timeout(&cover_url_for_display(url), COVER_NETWORK_TIMEOUT)
            .await
    }

    /// UI consumers keep the one validated decode, reduced off the reactor.
    pub(super) async fn fetch_cover_image(
        &self,
        url: &str,
    ) -> Result<std::sync::Arc<image::DynamicImage>> {
        let url = cover_url_for_display(url);
        self.fetch_cover_with_timeout(
            &url,
            COVER_NETWORK_TIMEOUT,
            true,
            true,
            MAX_COVER_IMAGE_BYTES,
        )
        .await?
        .1
        .ok_or_else(|| anyhow!("cover image URL was empty"))
    }

    async fn fetch_cover_bytes_with_timeout(
        &self,
        url: &str,
        timeout: Duration,
    ) -> Result<Vec<u8>> {
        Ok(self
            .fetch_cover_with_timeout(url, timeout, false, true, MAX_COVER_IMAGE_BYTES)
            .await?
            .0)
    }

    async fn fetch_cover_with_timeout(
        &self,
        url: &str,
        timeout: Duration,
        keep_image: bool,
        decode_image: bool,
        max_bytes: usize,
    ) -> Result<(Vec<u8>, Option<std::sync::Arc<image::DynamicImage>>)> {
        let url = url.trim();
        if url.is_empty() {
            return Ok((Vec::new(), None));
        }

        // Do not reset the budget between headers and individual body chunks.
        let bytes = compio::time::timeout(timeout, async {
            let response = self.http.get(url)?.send().await?;
            let response = error_for_status(response)?;
            let expected_len = response.content_length();
            if let Some(content_len) = expected_len
                && content_len > max_bytes as u64
            {
                return Err(anyhow!("cover image exceeds {max_bytes} byte limit"));
            }

            let mut bytes = Vec::with_capacity(expected_len.unwrap_or(64 * 1024) as usize);
            let mut stream = response.bytes_stream();
            while let Some(chunk) = stream.next().await {
                let chunk = chunk.with_context(|| format!("download cover image failed: {url}"))?;
                if bytes.len().saturating_add(chunk.len()) > max_bytes {
                    return Err(anyhow!("cover image exceeds {max_bytes} byte limit"));
                }
                bytes.extend_from_slice(&chunk);
            }

            if let Some(expected_len) = expected_len
                && bytes.len() != expected_len as usize
            {
                return Err(anyhow!(
                    "cover image length mismatch: expected {expected_len}, got {}",
                    bytes.len()
                ));
            }
            if bytes.is_empty() {
                return Err(anyhow!("cover image response was empty"));
            }
            Ok(bytes)
        })
        .await
        .map_err(|_| ncm_api::NcmError::Timeout { timeout })
        .with_context(|| format!("download cover image timed out: {url}"))??;

        if !decode_image {
            validate_cover_bytes(&bytes).with_context(|| format!("invalid cover image: {url}"))?;
            return Ok((bytes, None));
        }

        // Blocking decoders cannot be cancelled. The closure, not its caller,
        // owns admission until it finishes, even if the awaiting task is dropped.
        let permit = COVER_VALIDATION_GATE.acquire(timeout).await?;
        let validated = compio::runtime::spawn_blocking(move || -> Result<_> {
            let _permit = permit;
            validate_cover_bytes(&bytes)?;
            let decoded = load_from_memory(&bytes)?;
            let image = keep_image.then(|| std::sync::Arc::new(decoded.thumbnail(500, 500)));
            Ok((bytes, image))
        })
        .await
        .map_err(|_| anyhow!("cover image validation task panicked"))?
        .with_context(|| format!("invalid cover image: {url}"))?;
        Ok(validated)
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

/// 下载取链的结果：直链、载荷类型（mp3/flac）与实际档位（可能被账号权限降级）。
#[derive(Debug, Clone)]
pub(super) struct AudioDownloadSource {
    pub(super) url: String,
    pub(super) file_type: String,
    pub(super) level: String,
}

/// 从 `…/url` 系列回包里取第一条 `data[0]`：空 url 视为不可用。
fn parse_audio_source(
    response: &ApiResponse,
    requested_level: &str,
) -> Option<AudioDownloadSource> {
    let entry = response.body.pointer("/data/0")?;
    let url = entry.get("url").and_then(|value| value.as_str())?.trim();
    if url.is_empty() {
        return None;
    }

    let file_type = entry
        .get("type")
        .and_then(|value| value.as_str())
        .unwrap_or("mp3")
        .trim()
        .to_ascii_lowercase();
    let level = entry
        .get("level")
        .and_then(|value| value.as_str())
        .unwrap_or(requested_level)
        .trim();

    Some(AudioDownloadSource {
        url: url.to_string(),
        file_type,
        level: level.to_string(),
    })
}

pub(super) fn error_for_status(resp: Response) -> Result<Response> {
    let status = resp.status();
    let url = resp.url();
    let reason = status.canonical_reason().unwrap_or_default();
    if status.is_client_error() || status.is_server_error() {
        bail!("{url} {status} {reason}");
    } else {
        Ok(resp)
    }
}

#[cfg(test)]
mod tests;
