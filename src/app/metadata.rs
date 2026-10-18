use super::App;
use super::api::ApiState;
use super::cache_maintenance::CacheMaintenance;
use super::latest_fetch;
use super::playback_controller::PlaybackTrack;
use crate::data::atomic_file::write_atomic;
use crate::data::persistence::PersistenceHandle;
use crate::lyrics::LyricLine;
use crate::lyrics::parse_lyric_response;
use crate::render::wake::WakeSignal;
use cyper::Client;
use futures::channel::mpsc as async_mpsc;
use see::unsync as watch;
use std::collections::hash_map::DefaultHasher;
use std::hash::Hash;
use std::hash::Hasher;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;

pub(super) const COVER_CACHE_SUBDIR: &str = "cover";

const COVER_FETCH_RETRY_MS: u64 = 1500;

const LYRICS_FETCH_RETRY_MS: u64 = 1500;

#[derive(Debug, Clone)]
pub(super) struct CoverFetchRequest {
    pub(super) song_id: String,
    pub(super) url: String,
    pub(super) generation: u64,
}

#[derive(Debug, Clone)]
pub(super) struct CoverFetchResult {
    pub(super) song_id: String,
    pub(super) url: String,
    pub(super) generation: u64,
    pub(super) bytes: Option<Arc<[u8]>>,
}

fn cover_cache_path_for_dir(cache_dir: &Path, url: &str) -> Option<PathBuf> {
    // The cache key carries the requested target size, so a display cover and
    // an eventual original cover cannot alias each other.
    let key = super::api::cover_url_for_display(url);
    if key.is_empty() {
        return None;
    }

    let mut hasher = DefaultHasher::new();
    key.hash(&mut hasher);
    Some(cache_dir.join(format!("{:016x}.img", hasher.finish())))
}

pub(super) fn validate_cached_cover(bytes: &[u8]) -> bool {
    if bytes.is_empty() {
        return false;
    }
    super::api::validate_cover_bytes(bytes).is_ok() && image::load_from_memory(bytes).is_ok()
}

/// Only validated API bytes enter this path. Keep the buffer shared with the
/// FIFO job until completion, then recover ownership without copying its data.
async fn persist_fetched_cover(
    persistence: &PersistenceHandle,
    maintenance: &CacheMaintenance,
    path: PathBuf,
    bytes: Arc<[u8]>,
) -> Arc<[u8]> {
    let job_bytes = bytes.clone();
    let maintenance = maintenance.clone();
    let (done_tx, done_rx) = futures::channel::oneshot::channel();
    let queued = persistence.enqueue(move || {
        let _write_lease = maintenance.protect(path.clone());
        let result = write_atomic(&path, &job_bytes);
        if result.is_ok()
            && let Some(directory) = path.parent()
        {
            maintenance.request(directory.to_path_buf());
        }
        drop(job_bytes);
        let _ = done_tx.send(());
        result
    });
    if queued.is_ok() {
        let _ = done_rx.await;
    } else if let Err(error) = queued {
        log::warn!("cover persistence queue unavailable: {error:#}");
    }
    bytes
}

#[derive(Debug, Clone)]
pub(super) struct LyricFetchRequest {
    pub(super) song_id: String,
    pub(super) cookie: Option<String>,
    pub(super) generation: u64,
}

#[derive(Debug, Clone)]
pub(super) struct LyricFetchResult {
    pub(super) song_id: String,
    pub(super) generation: u64,
    /// `None` is retryable; `Some(empty)` records confirmed absence in both
    /// the current track and queue, suppressing subsequent lyric requests.
    pub(super) lyrics: Option<Vec<LyricLine>>,
}

pub(super) async fn loop_cover_fetch(
    rx: watch::Receiver<Option<CoverFetchRequest>>,
    tx: async_mpsc::Sender<CoverFetchResult>,
    client: Client,
    cache_dir: PathBuf,
    persistence: PersistenceHandle,
    maintenance: CacheMaintenance,
    wake: WakeSignal,
) {
    let Ok(api) = ApiState::new(None, client) else {
        return;
    };
    let process_fn = async move |req: &CoverFetchRequest| {
        if req.url.is_empty() {
            return None;
        }
        if let Some(path) = cover_cache_path_for_dir(&cache_dir, &req.url) {
            let cached = compio::runtime::spawn_blocking(move || {
                let bytes = std::fs::read(path).ok()?;
                validate_cached_cover(&bytes).then(|| Arc::<[u8]>::from(bytes))
            })
            .await
            .ok()
            .flatten();
            if cached.is_some() {
                return cached;
            }
        }
        let bytes = api.fetch_display_cover_bytes(&req.url).await.ok()?;
        let path = cover_cache_path_for_dir(&cache_dir, &req.url)?;
        Some(persist_fetched_cover(&persistence, &maintenance, path, Arc::from(bytes)).await)
    };
    latest_fetch::run_latest(
        rx,
        tx,
        async move |req: CoverFetchRequest| {
            let bytes = process_fn(&req).await;
            CoverFetchResult {
                song_id: req.song_id,
                url: req.url,
                generation: req.generation,
                bytes,
            }
        },
        wake,
    )
    .await;
}

enum LyricResponse {
    Present(Vec<LyricLine>),
    Absent,
    Retryable,
}

fn classify_lyric_response(response: anyhow::Result<ncm_api::ApiResponse>) -> LyricResponse {
    let Ok(response) = response else {
        return LyricResponse::Retryable;
    };
    let code = response.body.get("code").and_then(|value| {
        value
            .as_i64()
            .or_else(|| value.as_str().and_then(|code| code.parse().ok()))
    });
    // The API normalizes some error statuses to 200, so inspect the body code
    // before either accepting lyrics or recording a negative cache entry.
    if response.status != 200 || code != Some(200) {
        return LyricResponse::Retryable;
    }
    if let Some(lines) = parse_lyric_response(&response.body) {
        return LyricResponse::Present(lines);
    }

    // Empty successful documents (including nolyric/uncollected responses)
    // confirm absence. Nonempty unparseable or wrongly typed lyric fields do
    // not: a later request must still be allowed to recover their contents.
    let empty = ["yrc", "lrc", "ytlrc", "tlyric", "romalrc"]
        .iter()
        .all(|key| {
            let field = &response.body[*key];
            let text = match field {
                serde_json::Value::Null => return true,
                serde_json::Value::String(text) => return text.trim().is_empty(),
                serde_json::Value::Object(fields) if fields.is_empty() => return true,
                serde_json::Value::Object(fields) => fields.get("lyric"),
                _ => return false,
            };
            match text {
                Some(serde_json::Value::Null) => true,
                Some(serde_json::Value::String(text)) => text.trim().is_empty(),
                _ => false,
            }
        });
    if empty {
        LyricResponse::Absent
    } else {
        LyricResponse::Retryable
    }
}

async fn fetch_lyrics_from(
    mut fetch: impl AsyncFnMut(bool) -> anyhow::Result<ncm_api::ApiResponse>,
) -> Option<Vec<LyricLine>> {
    let first = classify_lyric_response(fetch(true).await);
    if let LyricResponse::Present(lines) = first {
        return Some(lines);
    }
    match classify_lyric_response(fetch(false).await) {
        LyricResponse::Present(lines) => Some(lines),
        LyricResponse::Absent if matches!(first, LyricResponse::Absent) => Some(Vec::new()),
        _ => None,
    }
}

/// Fetch the complete lyric document, including YRC word timings and
/// translations, regardless of `word_lyrics` / `lyric_translation`. Those
/// settings are display-only and must never gate this path. `Some(empty)` is
/// confirmed absence from both endpoints; `None` remains eligible for retry.
pub(super) async fn fetch_lyrics(api: &mut ApiState, song_id: &str) -> Option<Vec<LyricLine>> {
    fetch_lyrics_from(async |new_endpoint| {
        if new_endpoint {
            api.lyric_new(song_id).await
        } else {
            api.lyric(song_id).await
        }
    })
    .await
}

fn apply_lyric_result(
    result: LyricFetchResult,
    generation: u64,
    inflight_song_id: &mut Option<String>,
    now_playing: &mut Option<PlaybackTrack>,
    queue: &mut [PlaybackTrack],
    index: Option<usize>,
) {
    if result.generation != generation {
        return;
    }
    if inflight_song_id.as_deref() == Some(result.song_id.as_str()) {
        *inflight_song_id = None;
    }
    let Some(now) = now_playing.as_mut() else {
        return;
    };
    if now.song_id != result.song_id || now.lyrics.is_some() {
        return;
    }
    let Some(lyrics) = result.lyrics else {
        return;
    };
    now.lyrics = Some(lyrics.clone());
    if let Some(slot) = index.and_then(|index| queue.get_mut(index))
        && slot.song_id == result.song_id
    {
        slot.lyrics = Some(lyrics);
    }
}

fn lyric_fetch_due(
    now_playing: Option<&PlaybackTrack>,
    inflight_song_id: Option<&str>,
    last_attempt_at: Option<Instant>,
    now: Instant,
) -> bool {
    now_playing.is_some_and(|track| {
        track.lyrics.is_none()
            && inflight_song_id != Some(track.song_id.as_str())
            && last_attempt_at.is_none_or(|last| {
                now.duration_since(last) >= Duration::from_millis(LYRICS_FETCH_RETRY_MS)
            })
    })
}

pub(super) async fn loop_lyric_fetch(
    rx: watch::Receiver<Option<LyricFetchRequest>>,
    tx: async_mpsc::Sender<LyricFetchResult>,
    mut api: ApiState,
) {
    let wake = api.wake_signal();
    let mut process_fn = async move |req: &LyricFetchRequest| {
        if let Some(cookie) = &req.cookie {
            api.set_cookie(cookie.to_string());
        }

        fetch_lyrics(&mut api, &req.song_id).await
    };
    latest_fetch::run_latest(
        rx,
        tx,
        async move |req: LyricFetchRequest| {
            let lyrics = process_fn(&req).await;
            LyricFetchResult {
                song_id: req.song_id,
                generation: req.generation,
                lyrics,
            }
        },
        wake,
    )
    .await;
}

impl App {
    /// Playback position quantized to the global frame grid / lyric divisor.
    pub(crate) fn lyric_position_ms(&self) -> u64 {
        crate::lyrics::quantized_position_ms(
            self.playback_position().as_millis() as u64,
            self.config.ui_fps,
            self.config.lyric_frame_divisor,
        )
    }

    pub(crate) fn lyric_options(&self) -> crate::ui::lyric_view::LyricOptions {
        crate::ui::lyric_view::LyricOptions {
            word_lyrics: self.config.word_lyrics,
            translation: self.config.lyric_translation,
        }
    }

    fn apply_cover_fetch_result(&mut self, result: CoverFetchResult) {
        if result.generation != self.cover_fetch_generation
            || self.cover_fetch_inflight_url.as_deref() != Some(result.url.as_str())
        {
            return;
        }
        self.cover_fetch_inflight_url = None;
        let Some(bytes) = result.bytes else {
            return;
        };
        if result.song_id.is_empty() {
            if self.playback.playback_queue_cover_url.as_deref() == Some(result.url.as_str()) {
                self.playback.playback_queue_cover = Some(bytes);
                self.playback.playback_queue_cover_url = Some(result.url);
            }
            return;
        }

        let Some(now) = self.playback.now_playing.as_ref() else {
            return;
        };
        if now.song_id != result.song_id
            || now.cover_url.as_deref().map(str::trim) != Some(result.url.as_str())
            || now.cover.is_some()
        {
            return;
        }
        if let Some(now_mut) = self.playback.now_playing.as_mut() {
            now_mut.cover = Some(bytes.clone());
        }
        if let Some(index) = self.playback.playback_index
            && let Some(slot) = self.playback.playback_queue.get_mut(index)
        {
            slot.cover = Some(bytes);
        }
    }

    pub(super) fn maybe_schedule_now_playing_cover_fetch(&mut self) {
        let (song_id, url) = match self.playback.now_playing.as_ref() {
            Some(now) if now.cover.is_none() => {
                let Some(url) = now.cover_url.clone() else {
                    return;
                };
                (now.song_id.clone(), url)
            }
            Some(_) => {
                self.cover_fetch_inflight_url = None;
                return;
            }
            None => {
                return;
            }
        };

        let url = url.trim().to_string();
        if url.is_empty() || self.cover_fetch_inflight_url.as_deref() == Some(url.as_str()) {
            return;
        }

        let now_at = Instant::now();
        if let Some(last) = self.cover_fetch_last_attempt_at
            && now_at.duration_since(last) < Duration::from_millis(COVER_FETCH_RETRY_MS)
        {
            return;
        }

        let generation = self.cover_fetch_generation.wrapping_add(1);
        let req = CoverFetchRequest {
            song_id,
            url: url.clone(),
            generation,
        };
        if self.cover_fetch_tx.send(Some(req)).is_ok() {
            self.cover_fetch_generation = generation;
            self.cover_fetch_inflight_url = Some(url);
            self.cover_fetch_last_attempt_at = Some(now_at);
        }
    }

    fn maybe_schedule_queue_cover_fetch(&mut self) {
        let Some(url) = self.playback.playback_queue_cover_url.clone() else {
            self.playback.playback_queue_cover = None;
            return;
        };
        let url = url.trim().to_string();
        if url.is_empty() {
            self.playback.playback_queue_cover = None;
            return;
        }
        if self.playback.playback_queue_cover.is_some() || self.cover_fetch_inflight_url.is_some() {
            return;
        }
        let now = Instant::now();
        if self.queue_cover_fetch_last_attempt_at.is_some_and(|last| {
            now.duration_since(last) < Duration::from_millis(COVER_FETCH_RETRY_MS)
        }) {
            return;
        }
        let generation = self.cover_fetch_generation.wrapping_add(1);
        if self
            .cover_fetch_tx
            .send(Some(CoverFetchRequest {
                song_id: String::new(),
                url: url.clone(),
                generation,
            }))
            .is_ok()
        {
            self.cover_fetch_generation = generation;
            self.cover_fetch_inflight_url = Some(url);
            self.queue_cover_fetch_last_attempt_at = Some(now);
        }
    }

    pub(super) fn tick_cover_fetch(&mut self) {
        let needs_now = self
            .playback
            .now_playing
            .as_ref()
            .map(|now| now.cover.is_none() && now.cover_url.is_some())
            .unwrap_or(false);
        while let Ok(result) = self.cover_fetch_rx.try_recv() {
            self.apply_cover_fetch_result(result);
        }
        if needs_now {
            self.maybe_schedule_now_playing_cover_fetch();
        }
        if self.cover_fetch_inflight_url.is_none() {
            self.maybe_schedule_queue_cover_fetch();
        }
    }

    fn apply_lyric_fetch_result(&mut self, result: LyricFetchResult) {
        apply_lyric_result(
            result,
            self.lyric_fetch_generation,
            &mut self.lyric_fetch_inflight_song_id,
            &mut self.playback.now_playing,
            &mut self.playback.playback_queue,
            self.playback.playback_index,
        );
    }

    pub(super) fn maybe_schedule_now_playing_lyric_fetch(&mut self) {
        let song_id = match self.playback.now_playing.as_ref() {
            Some(now) if now.lyrics.is_none() => now.song_id.clone(),
            Some(_) => {
                self.lyric_fetch_inflight_song_id = None;
                return;
            }
            None => {
                return;
            }
        };

        let now_at = Instant::now();
        if !lyric_fetch_due(
            self.playback.now_playing.as_ref(),
            self.lyric_fetch_inflight_song_id.as_deref(),
            self.lyric_fetch_last_attempt_at,
            now_at,
        ) {
            return;
        }

        let generation = self.lyric_fetch_generation.wrapping_add(1);
        let req = LyricFetchRequest {
            song_id: song_id.clone(),
            generation,
            cookie: self
                .api
                .session_cookie()
                .map(|value| value.to_string())
                .or_else(|| self.session_cookie.clone()),
        };
        if self.lyric_fetch_tx.send(Some(req)).is_ok() {
            self.lyric_fetch_generation = generation;
            self.lyric_fetch_inflight_song_id = Some(song_id);
            self.lyric_fetch_last_attempt_at = Some(now_at);
        }
    }

    pub(super) fn tick_lyric_fetch(&mut self) {
        let needs_schedule = self
            .playback
            .now_playing
            .as_ref()
            .map(|now| now.lyrics.is_none())
            .unwrap_or(false);
        if self.lyric_fetch_inflight_song_id.is_none() && !needs_schedule {
            return;
        }

        while let Ok(result) = self.lyric_fetch_rx.try_recv() {
            self.apply_lyric_fetch_result(result);
        }

        self.maybe_schedule_now_playing_lyric_fetch();
    }

    pub(super) fn trim_non_current_cover_memory(&mut self, current_index: usize) {
        let cover_cache_dir = &self.cover_cache_dir;
        let persistence = &self.persistence;
        let maintenance = &self.cache_maintenance;
        for (idx, track) in self.playback.playback_queue.iter_mut().enumerate() {
            if idx == current_index {
                continue;
            }

            if let (Some(bytes), Some(url)) = (track.cover.take(), track.cover_url.as_deref())
                && let Some(path) = cover_cache_path_for_dir(cover_cache_dir, url)
            {
                let maintenance = maintenance.clone();
                if let Err(error) = persistence.enqueue(move || {
                    if validate_cached_cover(&bytes) {
                        let _write_lease = maintenance.protect(path.clone());
                        write_atomic(&path, &bytes)?;
                        if let Some(directory) = path.parent() {
                            maintenance.request(directory.to_path_buf());
                        }
                    }
                    Ok(())
                }) {
                    log::warn!("cover cache write not admitted: {error:#}");
                }
            }
        }
    }
}

#[cfg(test)]
mod tests;
