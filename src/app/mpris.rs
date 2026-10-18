use super::App;
use super::mpris_bridge::MprisControlEvent;
use super::mpris_bridge::MprisSyncPayload;
use super::playback_controller::PlaybackRuntimeState;
use super::playback_controller::PlaybackTrack;
use super::playback_controller::map_audio_state;
use std::collections::hash_map::DefaultHasher;
use std::hash::Hash;
use std::hash::Hasher;
use std::time::Duration;
use std::time::Instant;

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
        self.playback.page_advance = None;
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

        if !metadata_changed && !playback_changed && !periodic_tick {
            return;
        }

        let payload = MprisSyncPayload {
            playback: self.playback.playback_state,
            position: self.playback.audio_player.display_position(),
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
