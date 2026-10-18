use super::App;
use super::mpris_bridge::MprisControlEvent;
use super::mpris_bridge::MprisSyncPayload;
use super::playback_controller::PlaybackRuntimeState;
use super::playback_controller::PlaybackTrack;
use super::playback_controller::map_audio_state;
use std::borrow::Cow;
use std::collections::hash_map::DefaultHasher;
use std::hash::Hash;
use std::hash::Hasher;
use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;

const MPRIS_DEFAULT_NAME: &str = "CNMPlayer";
const MPRIS_PLAY_TOKEN: &str = "@play";

fn resolve_mpris_name<'a>(template: &'a str, title: Option<&str>) -> Cow<'a, str> {
    if template.trim().is_empty() {
        return Cow::Borrowed(MPRIS_DEFAULT_NAME);
    }
    if !template.contains(MPRIS_PLAY_TOKEN) {
        return Cow::Borrowed(template);
    }
    let title = title
        .filter(|title| !title.trim().is_empty())
        .unwrap_or(MPRIS_DEFAULT_NAME);
    Cow::Owned(template.replace(MPRIS_PLAY_TOKEN, title))
}

/// Resolve only when the template or a referenced title changes, never per frame.
pub(super) struct MprisName {
    template: String,
    title: String,
    references_track: bool,
    pub(super) identity: Arc<String>,
}

impl MprisName {
    pub(super) fn new(template: &str, title: Option<&str>) -> Self {
        Self {
            template: template.to_owned(),
            title: if template.contains(MPRIS_PLAY_TOKEN) {
                title.unwrap_or_default().to_owned()
            } else {
                String::new()
            },
            references_track: template.contains(MPRIS_PLAY_TOKEN),
            identity: Arc::new(resolve_mpris_name(template, title).into_owned()),
        }
    }

    fn update(&mut self, template: &str, title: Option<&str>) -> bool {
        let template_changed = self.template != template;
        let references_track = if template_changed {
            template.contains(MPRIS_PLAY_TOKEN)
        } else {
            self.references_track
        };
        let referenced_title = if references_track {
            title.unwrap_or_default()
        } else {
            ""
        };
        if !template_changed && self.title == referenced_title {
            return false;
        }
        let identity = resolve_mpris_name(template, title);
        if template_changed {
            self.template.clear();
            self.template.push_str(template);
            self.references_track = references_track;
        }
        if self.title != referenced_title {
            self.title.clear();
            self.title.push_str(referenced_title);
        }
        if self.identity.as_str() == identity.as_ref() {
            return false;
        }
        self.identity = Arc::new(identity.into_owned());
        true
    }
}

fn mpris_metadata_signature(track: &PlaybackTrack) -> u64 {
    let mut hasher = DefaultHasher::new();
    track.song_id.hash(&mut hasher);
    track.duration_ms.hash(&mut hasher);
    track
        .cover
        .as_ref()
        .map(|bytes| bytes.len())
        .unwrap_or(0)
        .hash(&mut hasher);
    track
        .cover_url
        .as_deref()
        .unwrap_or_default()
        .hash(&mut hasher);
    track
        .lyrics
        .as_ref()
        .map(|lines| lines.len())
        .unwrap_or(0)
        .hash(&mut hasher);
    track
        .lyrics
        .as_ref()
        .and_then(|lines| lines.last().map(|line| line.start_ms))
        .unwrap_or(0)
        .hash(&mut hasher);
    hasher.finish()
}

impl App {
    pub(super) async fn apply_mpris_control_events(&mut self) {
        let events = self.mpris_bridge.drain_control_events();
        if !events.is_empty() {
            for event in events {
                match event {
                    MprisControlEvent::Play => self.mpris_play().await,
                    MprisControlEvent::Pause => self.mpris_pause(),
                    MprisControlEvent::PlayPause => self.toggle_play_pause_hotkey().await,
                    MprisControlEvent::Stop => {
                        self.playback.page_advance = None;
                        self.playback.cancel_pending_start();
                        self.playback.audio_player.stop();
                        self.playback.playback_state = PlaybackRuntimeState::Stopped;
                    }
                    MprisControlEvent::Next => self.play_next_hotkey().await,
                    MprisControlEvent::Previous => self.play_previous_hotkey().await,
                    MprisControlEvent::SeekRelativeMicros(delta) => self.mpris_seek_relative(delta),
                    MprisControlEvent::SeekAbsoluteMicros(pos) => self.mpris_seek_absolute(pos),
                }
            }
            self.wake.notify();
        }
    }

    async fn mpris_play(&mut self) {
        if self.playback.has_pending_start() {
            self.playback.set_pending_paused(false);
            return;
        }
        if self.playback.now_playing.is_none() {
            return;
        }
        if self.playback.playback_state == PlaybackRuntimeState::Stopped {
            if let Some(index) = self.playback.playback_index {
                self.play_queue_index(index, false).await;
            }
            return;
        }
        if self.playback.playback_state == PlaybackRuntimeState::Paused {
            self.playback.audio_player.toggle_play_pause();
            self.playback.playback_state = map_audio_state(self.playback.audio_player.state());
        }
    }

    fn mpris_pause(&mut self) {
        self.playback.cancel_deferred_advance();
        if self.playback.has_pending_start() {
            self.playback.set_pending_paused(true);
            return;
        }
        if self.playback.playback_state == PlaybackRuntimeState::Playing {
            self.playback.audio_player.toggle_play_pause();
            self.playback.playback_state = map_audio_state(self.playback.audio_player.state());
        }
    }

    fn mpris_seek_relative(&mut self, delta_micros: i64) {
        let total = self.playback_duration();
        let total_micros = total.as_micros();
        if total_micros == 0 {
            return;
        }

        let current_micros = self.playback.audio_player.position().as_micros() as i128;
        let target = (current_micros + delta_micros as i128).clamp(0, total_micros as i128);
        let ratio = (target as f64 / total_micros as f64) as f32;
        self.seek_to_ratio(ratio);
    }

    fn mpris_seek_absolute(&mut self, position_micros: i64) {
        let total = self.playback_duration();
        let total_micros = total.as_micros();
        if total_micros == 0 {
            return;
        }

        let target = (position_micros as i128).clamp(0, total_micros as i128);
        let ratio = (target as f64 / total_micros as f64) as f32;
        self.seek_to_ratio(ratio);
    }

    pub(super) fn sync_mpris_exposure(&mut self) {
        let identity_changed = self.mpris_name.update(
            &self.config.mpris_name,
            self.playback.now_playing.as_ref().map(|track| track.title.as_str()),
        );
        let now = Instant::now();
        let signature = self
            .playback
            .now_playing
            .as_ref()
            .map(mpris_metadata_signature)
            .unwrap_or(0);

        let metadata_changed = self.mpris_last_signature != Some(signature);
        let playback_changed = self.mpris_last_playback != self.playback.playback_state;
        let periodic_tick =
            now.duration_since(self.mpris_last_sync_at) >= Duration::from_millis(900);

        if !metadata_changed && !playback_changed && !identity_changed && !periodic_tick {
            return;
        }

        let payload = MprisSyncPayload {
            identity: self.mpris_name.identity.clone(),
            playback: self.playback.playback_state,
            position: self.playback_position(),
            track: if metadata_changed {
                self.playback.now_playing.clone()
            } else {
                None
            },
        };

        self.mpris_bridge.update(payload);
        self.mpris_last_sync_at = now;
        self.mpris_last_playback = self.playback.playback_state;
        if metadata_changed {
            self.mpris_last_signature = Some(signature);
        }
    }
}

#[cfg(test)]
mod tests;
