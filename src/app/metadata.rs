use super::*;

impl App {
    pub(super) fn cover_cache_path_for_url(&self, url: &str) -> Option<PathBuf> {
        cover_cache_path_for_dir(&self.cover_cache_dir, url)
    }

    pub(super) async fn load_cover_from_disk_cache(&self, url: &str) -> Option<Vec<u8>> {
        let path = self.cover_cache_path_for_url(url)?;
        compio::runtime::spawn_blocking(move || {
            std::fs::read(path).ok().and_then(validate_cached_cover)
        })
        .await
        .ok()
        .flatten()
    }

    pub(super) async fn persist_cover_to_disk_cache(&self, url: &str, bytes: Vec<u8>) -> Vec<u8> {
        let Some(path) = self.cover_cache_path_for_url(url) else {
            return bytes;
        };
        persist_fetched_cover(&self.persistence.handle(), path, bytes).await
    }

    pub(super) async fn fetch_cover_with_disk_cache(&self, url: &str) -> Option<Vec<u8>> {
        if let Some(bytes) = self.load_cover_from_disk_cache(url).await {
            return Some(bytes);
        }

        let bytes = self.api.fetch_cover_bytes(url).await.ok()?;
        Some(self.persist_cover_to_disk_cache(url, bytes).await)
    }

    pub(super) fn apply_cover_fetch_result(&mut self, result: CoverFetchResult) {
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

    pub(super) fn maybe_schedule_queue_cover_fetch(&mut self) {
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
            self.cover_fetch_last_attempt_at = Some(Instant::now());
        }
    }

    pub(super) fn tick_cover_fetch(&mut self) {
        let needs_now = self
            .playback
            .now_playing
            .as_ref()
            .map(|now| now.cover.is_none() && now.cover_url.is_some())
            .unwrap_or(false);
        let needs_queue = self.playback.playback_queue_cover_url.is_some()
            && self.playback.playback_queue_cover.is_none();
        let _ = needs_queue;
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

    pub(super) fn apply_lyric_fetch_result(&mut self, result: LyricFetchResult) {
        if result.generation != self.lyric_fetch_generation {
            return;
        }
        if self.lyric_fetch_inflight_song_id.as_deref() == Some(result.song_id.as_str()) {
            self.lyric_fetch_inflight_song_id = None;
        }

        let Some(now) = self.playback.now_playing.as_ref() else {
            return;
        };
        if now.song_id != result.song_id {
            return;
        }
        if now.lyrics.is_some() {
            return;
        }

        let Some(lyrics) = result.lyrics else {
            return;
        };

        if let Some(now_mut) = self.playback.now_playing.as_mut() {
            now_mut.lyrics = Some(lyrics.clone());
        }
        if let Some(index) = self.playback.playback_index
            && let Some(slot) = self.playback.playback_queue.get_mut(index)
        {
            slot.lyrics = Some(lyrics);
        }
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

        if self.lyric_fetch_inflight_song_id.as_deref() == Some(song_id.as_str()) {
            return;
        }

        let now_at = Instant::now();
        if let Some(last) = self.lyric_fetch_last_attempt_at
            && now_at.duration_since(last) < Duration::from_millis(LYRICS_FETCH_RETRY_MS)
        {
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
        for (idx, track) in self.playback.playback_queue.iter_mut().enumerate() {
            if idx == current_index {
                continue;
            }

            if let (Some(bytes), Some(url)) = (track.cover.take(), track.cover_url.as_deref())
                && let Some(path) = cover_cache_path_for_dir(cover_cache_dir, url)
                && let Err(error) = persistence.enqueue(move || {
                    if let Some(bytes) = validate_cached_cover(bytes) {
                        write_atomic(&path, &bytes)?;
                    }
                    Ok(())
                })
            {
                log::warn!("cover cache write not admitted: {error:#}");
            }
        }
    }

    pub(super) async fn enrich_track_metadata(&mut self, track: &mut PlaybackTrack, allow_network: bool) {
        // Queue switches hydrate through the cover worker after the new track is
        // installed. Neither disk IO nor image decoding delays the transition.
        if allow_network
            && track.cover.is_none()
            && let Some(url) = track.cover_url.as_deref()
        {
            track.cover = self.fetch_cover_with_disk_cache(url).await;
        }

        if allow_network
            && track.cover.is_none()
            && let Ok(detail) = self.api.song_detail(&track.song_id).await
            && let Some(song) = detail
                .body
                .get("songs")
                .and_then(|value| value.as_array())
                .and_then(|items| items.first())
            && let Some(cover_url) = song.pointer("/al/picUrl").and_then(|value| value.as_str())
            && let Some(bytes) = self.fetch_cover_with_disk_cache(cover_url).await
        {
            track.cover = Some(bytes);
        }

        if allow_network
            && track.lyrics.is_none()
            && let Ok(lyric) = self.api.lyric(&track.song_id).await
            && let Some(raw_lrc) = lyric
                .body
                .pointer("/lrc/lyric")
                .and_then(|value| value.as_str())
        {
            track.lyrics = crate::tmplayer::playback::metadata::parse_lrc(raw_lrc)
                .or_else(|| crate::tmplayer::playback::metadata::parse_plain_lyrics(raw_lrc));
        }
    }

    pub fn current_page_lyric_lines(&self) -> (String, String) {
        let Some(track) = self.playback.now_playing.as_ref() else {
            return (String::new(), String::new());
        };
        let Some(lines) = track.lyrics.as_ref() else {
            return (String::new(), String::new());
        };
        if lines.is_empty() {
            return (String::new(), String::new());
        }

        let pos_ms = self.playback_position().as_millis() as u64;
        let mut idx = 0usize;
        for (line_idx, line) in lines.iter().enumerate() {
            if line.start_ms <= pos_ms {
                idx = line_idx;
            } else {
                break;
            }
        }

        let current = lines
            .get(idx)
            .map(|line| line.text.clone())
            .unwrap_or_default();
        let next = lines
            .get(idx + 1)
            .map(|line| line.text.clone())
            .unwrap_or_default();
        (current, next)
    }
}
