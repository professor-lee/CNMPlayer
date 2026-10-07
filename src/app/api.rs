use anyhow::{Context, Result, anyhow, bail};
use cyper::{Client, Response};
use futures::StreamExt;
use image::load_from_memory;
use ncm_api::{ApiClient, ApiResponse, Query};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

const MAX_COVER_IMAGE_BYTES: usize = 5 * 1024 * 1024;
const COVER_NETWORK_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_COVER_VALIDATIONS: usize = 2;
static COVER_VALIDATION_GATE: CoverValidationGate = CoverValidationGate::new();

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
pub struct ApiState {
    client: ApiClient,
    cookie: Option<String>,
    http: Client,
    wake: crate::render::wake::WakeSignal,
}

impl ApiState {
    pub fn new(cookie: Option<String>, http: Client) -> Result<Self> {
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

    /// 私人漫游（私人 FM 模式选择）
    pub async fn personal_fm_mode(&mut self, mode: &str, limit: usize) -> Result<ApiResponse> {
        let query = self
            .query_with_cookie()
            .param("mode", mode)
            .param("limit", &limit.max(1).to_string());
        let response = self.client.personal_fm_mode(&query).await?;
        Ok(response)
    }

    pub async fn playlist_detail(&mut self, id: &str) -> Result<ApiResponse> {
        let query = self.query_with_cookie().param("id", id);
        let response = self.client.playlist_detail(&query).await?;
        Ok(response)
    }
    pub async fn playlist_track_all(
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

    /// 客户端下载链接（新版）：登录后可用，对免费歌曲能拿到比播放更高的档位。
    pub async fn song_download_url_v1(
        &mut self,
        song_id: &str,
        level: &str,
    ) -> Result<ApiResponse> {
        let query = self
            .query_with_cookie()
            .param("id", song_id)
            .param("level", level);
        let response = self.client.song_download_url_v1(&query).await?;
        Ok(response)
    }

    /// 下载取链：`download/url/v1` 优先（未登录/无版权时回包里的 url 为空），
    /// 退回 `player/url/v1`，再退回旧 `/song/url`。
    pub async fn audio_download_url(
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
        let response = self.client.cloudsearch(&query).await?;
        Ok(response)
    }

    /// Fetch a complete cover within one 30s network budget, then await bounded validation.
    pub async fn fetch_cover_bytes(&self, url: &str) -> Result<Vec<u8>> {
        self.fetch_cover_bytes_with_timeout(url, COVER_NETWORK_TIMEOUT)
            .await
    }

    /// UI consumers keep the one validated decode, reduced off the reactor.
    pub async fn fetch_cover_image(
        &self,
        url: &str,
    ) -> Result<std::sync::Arc<image::DynamicImage>> {
        self.fetch_cover_with_timeout(url, COVER_NETWORK_TIMEOUT, true)
            .await?
            .1
            .ok_or_else(|| anyhow!("cover image URL was empty"))
    }

    async fn fetch_cover_bytes_with_timeout(
        &self,
        url: &str,
        timeout: Duration,
    ) -> Result<Vec<u8>> {
        Ok(self.fetch_cover_with_timeout(url, timeout, false).await?.0)
    }

    async fn fetch_cover_with_timeout(
        &self,
        url: &str,
        timeout: Duration,
        keep_image: bool,
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
                && content_len > MAX_COVER_IMAGE_BYTES as u64
            {
                return Err(anyhow!(
                    "cover image exceeds {} byte limit",
                    MAX_COVER_IMAGE_BYTES
                ));
            }

            let mut bytes = Vec::with_capacity(expected_len.unwrap_or(64 * 1024) as usize);
            let mut stream = response.bytes_stream();
            while let Some(chunk) = stream.next().await {
                let chunk = chunk.with_context(|| format!("download cover image failed: {url}"))?;
                if bytes.len().saturating_add(chunk.len()) > MAX_COVER_IMAGE_BYTES {
                    return Err(anyhow!(
                        "cover image exceeds {} byte limit",
                        MAX_COVER_IMAGE_BYTES
                    ));
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

        // Blocking decoders cannot be cancelled. The closure, not its caller,
        // owns admission until it finishes, even if the awaiting task is dropped.
        let permit = COVER_VALIDATION_GATE.acquire(timeout).await?;
        let validated = compio::runtime::spawn_blocking(move || -> Result<_> {
            let _permit = permit;
            // Decoders can recover incomplete images. A cache entry must carry
            // the format's terminal marker as well as successfully decode.
            match image::guess_format(&bytes)? {
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
pub struct AudioDownloadSource {
    pub url: String,
    pub file_type: String,
    pub level: String,
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

#[cfg(test)]
mod tests {
    use super::ApiState;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::mpsc;
    use std::time::Duration;

    #[compio::test]
    async fn busy_cover_decoder_waits_for_capacity_instead_of_losing_image() {
        static GATE: super::CoverValidationGate = super::CoverValidationGate::new();
        let first = GATE.try_acquire().unwrap();
        let second = GATE.try_acquire().unwrap();
        let release = compio::runtime::spawn(async move {
            compio::time::sleep(Duration::from_millis(30)).await;
            drop(first);
        });
        let third = GATE.acquire(Duration::from_secs(1)).await.unwrap();
        assert!(
            GATE.try_acquire().is_err(),
            "waiting did not increase decoder concurrency"
        );
        drop(third);
        drop(second);
        release.await.unwrap();
    }

    #[compio::test]
    async fn cover_network_deadline_is_not_reset_after_headers() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let png = tiny_png();
        let server = compio::runtime::spawn_blocking(move || {
            let (mut socket, _) = listener.accept().unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let mut request = [0; 1024];
            assert_ne!(socket.read(&mut request).unwrap(), 0);
            std::thread::sleep(Duration::from_millis(200));
            write!(
                socket,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                png.len()
            )
            .unwrap();
            socket.write_all(&png[..8]).unwrap();
            std::thread::sleep(Duration::from_millis(200));
            // A single deadline has expired; a fresh body deadline would accept this PNG.
            let _ = socket.write_all(&png[8..]);
        });
        let http = cyper::Client::builder().no_proxy().build().unwrap();
        let api = ApiState::new(None, http).unwrap();
        let url = format!("http://{address}/slow-cover");
        let result = compio::time::timeout(
            Duration::from_secs(2),
            api.fetch_cover_bytes_with_timeout(&url, Duration::from_millis(300)),
        )
        .await;
        server.await.unwrap();
        let error = result
            .expect("the cover deadline must beat the watchdog")
            .unwrap_err();
        assert!(matches!(
            error.downcast_ref::<ncm_api::NcmError>(),
            Some(ncm_api::NcmError::Timeout { timeout })
                if *timeout == Duration::from_millis(300)
        ));
        let message = format!("{error:#}");
        assert!(message.contains("complete response"));
        assert!(message.contains(&url));
    }

    #[compio::test]
    async fn cover_validation_admission_lives_until_blocking_work_finishes() {
        static GATE: super::CoverValidationGate = super::CoverValidationGate::new();
        let first = GATE.try_acquire().unwrap();
        let second = GATE.try_acquire().unwrap();
        assert!(GATE.try_acquire().unwrap_err().to_string().contains("busy"));
        let (release, wait_for_release) = mpsc::channel();
        let worker = compio::runtime::spawn_blocking(move || {
            let _permit = first;
            let _ = wait_for_release.recv_timeout(Duration::from_secs(2));
        });
        drop(worker);
        // Dropping the caller does not free the running decoder's slot.
        assert!(GATE.try_acquire().is_err());
        let _ = release.send(());
        compio::time::timeout(Duration::from_secs(1), async {
            loop {
                if let Ok(permit) = GATE.try_acquire() {
                    drop(permit);
                    break;
                }
                compio::time::sleep(Duration::from_millis(1)).await;
            }
        })
        .await
        .expect("finishing blocking work must release its admission slot");
        drop(second);
        let _first = GATE.try_acquire().unwrap();
        let _second = GATE.try_acquire().unwrap();
    }

    fn tiny_png() -> Vec<u8> {
        use image::ImageEncoder;
        let mut bytes = Vec::new();
        image::codecs::png::PngEncoder::new(&mut bytes)
            .write_image(&[0, 0, 0, 255], 1, 1, image::ExtendedColorType::Rgba8)
            .unwrap();
        bytes
    }

    #[compio::test]
    async fn cover_download_accepts_only_complete_valid_images() {
        let png = tiny_png();
        let length_header = format!("Content-Length: {}\r\n", png.len());
        let cases: &[(&[u8], &str, bool)] = &[
            (&png, &length_header, true),
            (&png, "", true),
            (b"not-an-image", "Content-Length: 12\r\n", false),
            (&png[..20], &length_header, false),
            (&png[..png.len() - 12], "", false),
            (b"a\r\nabc", "Transfer-Encoding: chunked\r\n", false),
        ];
        for (index, (body, headers, expected_ok)) in cases.iter().enumerate() {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let address = listener.local_addr().unwrap();
            let body = body.to_vec();
            let headers = headers.to_string();
            let server = compio::runtime::spawn_blocking(move || {
                let (mut socket, _) = listener.accept().unwrap();
                let mut request = [0; 1024];
                let _ = socket.read(&mut request);
                write!(
                    socket,
                    "HTTP/1.1 200 OK\r\n{headers}Connection: close\r\n\r\n"
                )
                .unwrap();
                socket.write_all(&body).unwrap();
            });
            let http = cyper::Client::builder().no_proxy().build().unwrap();
            let api = ApiState::new(None, http).unwrap();
            let result = api
                .fetch_cover_bytes(&format!("http://{address}/cover-{index}"))
                .await;
            server.await.unwrap();
            assert_eq!(result.is_ok(), *expected_ok, "case {index}: {result:?}");
            if *expected_ok {
                assert_eq!(result.unwrap(), png);
            }
        }
    }

    #[compio::test]
    async fn cover_download_rejects_declared_oversize_before_body() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = compio::runtime::spawn_blocking(move || {
            let (mut socket, _) = listener.accept().unwrap();
            let mut request = [0; 1024];
            let _ = socket.read(&mut request);
            write!(
                socket,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                super::MAX_COVER_IMAGE_BYTES + 1
            )
            .unwrap();
        });
        let http = cyper::Client::builder().no_proxy().build().unwrap();
        let api = ApiState::new(None, http).unwrap();
        assert!(
            api.fetch_cover_bytes(&format!("http://{address}/oversize"))
                .await
                .is_err()
        );
        server.await.unwrap();
    }

    #[compio::test]
    async fn cover_download_caps_body_without_content_length() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = compio::runtime::spawn_blocking(move || {
            let (mut socket, _) = listener.accept().unwrap();
            let mut request = [0; 1024];
            let _ = socket.read(&mut request);
            socket
                .write_all(b"HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n")
                .unwrap();
            let block = [0; 8192];
            for _ in 0..=(super::MAX_COVER_IMAGE_BYTES / block.len()) {
                if socket.write_all(&block).is_err() {
                    break;
                }
            }
        });
        let http = cyper::Client::builder().no_proxy().build().unwrap();
        let api = ApiState::new(None, http).unwrap();
        let error = api
            .fetch_cover_bytes(&format!("http://{address}/unbounded"))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("byte limit"));
        server.await.unwrap();
    }
}
