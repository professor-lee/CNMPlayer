use super::{PlaybackRuntimeState, PlaybackTrack};
use crate::data::config::CacheConfig;
use std::path::Path;
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct MprisSyncPayload {
    pub playback: PlaybackRuntimeState,
    pub position: Duration,
    pub track: Option<PlaybackTrack>,
}

#[derive(Debug, Clone, Copy)]
pub enum MprisControlEvent {
    Play,
    Pause,
    PlayPause,
    Stop,
    Next,
    Previous,
    SeekRelativeMicros(i64),
    SeekAbsoluteMicros(i64),
}

#[cfg(target_os = "linux")]
mod imp {
    use super::{CacheConfig, MprisControlEvent, MprisSyncPayload, Path};
    use crate::app::player::cleanup_cache_dir;
    use crate::launch;
    use mpris_server::{Metadata, PlaybackStatus, Player, Time, zbus};
    use std::collections::hash_map::DefaultHasher;
    use std::fs;
    use std::hash::{Hash, Hasher};
    use std::sync::mpsc::{self as std_mpsc, Receiver, Sender};

    pub struct MprisBridge {
        tx: see::unsync::Sender<Option<MprisSyncPayload>>,
        event_rx: Receiver<MprisControlEvent>,
    }

    impl MprisBridge {
        pub fn new(cache_root: &Path, cache_policy: &CacheConfig) -> Self {
            let art_dir = cache_root.join("mpris_art");
            let _ = fs::create_dir_all(&art_dir);
            let _ = cleanup_cache_dir(&art_dir, cache_policy);

            let (tx, mut rx) = see::unsync::channel(None);
            let (event_tx, event_rx) = std_mpsc::channel::<MprisControlEvent>();
            let cache_policy = cache_policy.clone();

            let task = async move {
                let player = match Player::builder("cnmplayer")
                    .can_play(true)
                    .can_pause(true)
                    .can_seek(true)
                    .can_go_next(true)
                    .can_go_previous(true)
                    .build()
                    .await
                {
                    Ok(player) => player,
                    Err(err) => {
                        log::warn!("mpris player init failed: {err}");
                        return;
                    }
                };

                bind_control_callbacks(&player, &event_tx);

                launch(player.run());

                while rx.changed().await.is_ok() {
                    let Some(payload) = rx.borrow_and_update().clone() else {
                        continue;
                    };

                    if let Err(err) =
                        apply_snapshot(&player, &art_dir, &cache_policy, payload).await
                    {
                        log::debug!("mpris sync failed: {err}");
                    }
                }
            };
            launch(task);

            Self { tx, event_rx }
        }

        pub fn update(&self, payload: MprisSyncPayload) {
            let _ = self.tx.send_replace(Some(payload));
        }

        pub fn drain_control_events(&self) -> Vec<MprisControlEvent> {
            let mut out = Vec::new();
            while let Ok(ev) = self.event_rx.try_recv() {
                out.push(ev);
            }
            out
        }
    }

    fn bind_control_callbacks(player: &Player, event_tx: &Sender<MprisControlEvent>) {
        let tx = event_tx.clone();
        player.connect_play(move |_| {
            let _ = tx.send(MprisControlEvent::Play);
        });

        let tx = event_tx.clone();
        player.connect_pause(move |_| {
            let _ = tx.send(MprisControlEvent::Pause);
        });

        let tx = event_tx.clone();
        player.connect_play_pause(move |_| {
            let _ = tx.send(MprisControlEvent::PlayPause);
        });

        let tx = event_tx.clone();
        player.connect_stop(move |_| {
            let _ = tx.send(MprisControlEvent::Stop);
        });

        let tx = event_tx.clone();
        player.connect_next(move |_| {
            let _ = tx.send(MprisControlEvent::Next);
        });

        let tx = event_tx.clone();
        player.connect_previous(move |_| {
            let _ = tx.send(MprisControlEvent::Previous);
        });

        let tx = event_tx.clone();
        player.connect_seek(move |_, offset| {
            let _ = tx.send(MprisControlEvent::SeekRelativeMicros(offset.as_micros()));
        });

        let tx = event_tx.clone();
        player.connect_set_position(move |_, _, position| {
            let _ = tx.send(MprisControlEvent::SeekAbsoluteMicros(position.as_micros()));
        });
    }

    async fn apply_snapshot(
        player: &Player,
        art_dir: &Path,
        cache_policy: &CacheConfig,
        payload: MprisSyncPayload,
    ) -> zbus::Result<()> {
        player
            .set_playback_status(match payload.playback {
                super::PlaybackRuntimeState::Playing => PlaybackStatus::Playing,
                super::PlaybackRuntimeState::Paused => PlaybackStatus::Paused,
                super::PlaybackRuntimeState::Stopped => PlaybackStatus::Stopped,
            })
            .await?;

        player.set_position(time_from_duration(payload.position));

        if let Some(track) = payload.track {
            player
                .set_metadata(build_metadata(art_dir, cache_policy, &track))
                .await?;
        }

        Ok(())
    }

    fn build_metadata(
        art_dir: &Path,
        cache_policy: &CacheConfig,
        track: &super::PlaybackTrack,
    ) -> Metadata {
        let mut metadata = Metadata::new();

        if !track.title.trim().is_empty() {
            metadata.set_title(Some(track.title.clone()));
        }
        if !track.artist.trim().is_empty() {
            metadata.set_artist(Some([track.artist.clone()]));
            metadata.set_album_artist(Some([track.artist.clone()]));
        }
        if !track.album.trim().is_empty() {
            metadata.set_album(Some(track.album.clone()));
        }
        if track.duration_ms > 0 {
            metadata.set_length(Some(Time::from_micros(
                track
                    .duration_ms
                    .saturating_mul(1000)
                    .clamp(i64::MIN / 2, i64::MAX / 2),
            )));
        }

        if !track.song_id.trim().is_empty() {
            metadata.set_url(Some(format!(
                "https://music.163.com/#/song?id={}",
                track.song_id
            )));
            metadata.set_comment(Some([format!("song_id={}", track.song_id)]));
        }

        if let Some(lyrics) = &track.lyrics {
            if !lyrics.is_empty() {
                let text = lyrics
                    .iter()
                    .map(|line| line.text.trim())
                    .filter(|line| !line.is_empty())
                    .collect::<Vec<_>>()
                    .join("\n");
                if !text.is_empty() {
                    metadata.set_lyrics(Some(text));
                }
            }
        }

        if let Some(bytes) = track.cover.as_deref() {
            if !bytes.is_empty() {
                if let Some(art_url) =
                    persist_cover_as_file_url(art_dir, cache_policy, &track.song_id, bytes)
                {
                    metadata.set_art_url(Some(art_url));
                }
            }
        }

        metadata
    }

    fn persist_cover_as_file_url(
        art_dir: &Path,
        cache_policy: &CacheConfig,
        song_id: &str,
        bytes: &[u8],
    ) -> Option<String> {
        let mut hasher = DefaultHasher::new();
        bytes.hash(&mut hasher);
        let hash = hasher.finish();

        let safe_id = sanitize_for_filename(song_id);
        let filename = format!("{}_{}.img", safe_id, hash);
        let path = art_dir.join(filename);

        if !path.is_file() {
            fs::write(&path, bytes).ok()?;
            let _ = cleanup_cache_dir(art_dir, cache_policy);
            if !path.is_file() {
                fs::write(&path, bytes).ok()?;
            }
        }

        Some(format!("file://{}", path.to_string_lossy()))
    }

    fn sanitize_for_filename(input: &str) -> String {
        let mut out = String::with_capacity(input.len());
        for ch in input.chars() {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                out.push(ch);
            }
        }
        if out.is_empty() {
            "track".to_string()
        } else {
            out
        }
    }

    fn time_from_duration(dur: std::time::Duration) -> Time {
        let micros_u128 = dur.as_micros();
        let micros = if micros_u128 > i64::MAX as u128 {
            i64::MAX
        } else {
            micros_u128 as i64
        };
        Time::from_micros(micros)
    }
}

#[cfg(not(target_os = "linux"))]
mod imp {
    use super::{CacheConfig, MprisControlEvent, MprisSyncPayload, Path};

    pub struct MprisBridge;

    impl MprisBridge {
        pub fn new(_cache_root: &Path, _cache_policy: &CacheConfig) -> Self {
            Self
        }

        pub fn update(&self, _payload: MprisSyncPayload) {}

        pub fn drain_control_events(&self) -> Vec<MprisControlEvent> {
            Vec::new()
        }
    }
}

pub use imp::MprisBridge;
