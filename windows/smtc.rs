//! Windows 系统媒体控件（SMTC）服务端，对应 Linux 上的 MPRIS。
//!
//! 由 `src/app/mpris_bridge.rs` 以 `#[cfg(windows)] #[path = ...] mod imp;` 挂载，对外接口与
//! Linux 版 `MprisBridge` 相同：`update` 推送最新快照，`drain_control_events` 取回媒体键、
//! 锁屏/音量浮层上的按钮与进度拖动。
//!
//! 控制台程序没有自己的窗口，拿不到 `ISystemMediaTransportControlsInterop::GetForWindow`
//! 需要的 HWND；这里借一个只用来承载 SMTC 的 `MediaPlayer`（关闭其命令管理器，按钮事件全部
//! 转给宿主），Windows 10 1607 起可用。SMTC 对象放在专用线程里，绝不阻塞 UI。

use super::MprisControlEvent;
use super::MprisSyncPayload;
use super::PlaybackRuntimeState;
use super::PlaybackTrack;
use crate::data::config::CacheConfig;
use std::path::Path;
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::Duration;
use windows::Foundation::{TimeSpan, TypedEventHandler};
use windows::Media::Playback::MediaPlayer;
use windows::Media::{
    MediaPlaybackStatus, MediaPlaybackType, PlaybackPositionChangeRequestedEventArgs,
    SystemMediaTransportControls, SystemMediaTransportControlsButton,
    SystemMediaTransportControlsButtonPressedEventArgs,
    SystemMediaTransportControlsTimelineProperties,
};
use windows::Storage::Streams::{
    DataWriter, InMemoryRandomAccessStream, RandomAccessStreamReference,
};
use windows::core::{HSTRING, Ref, Result};

pub struct MprisBridge {
    tx: Sender<MprisSyncPayload>,
    event_rx: Receiver<MprisControlEvent>,
}

impl MprisBridge {
    /// SMTC 的显示名取自可执行文件本身，未打包应用无法在运行期改，所以不使用 identity；
    /// 封面直接以内存流交给 SMTC，也不需要 Linux 那样的封面缓存目录。
    pub fn new(
        _cache_root: &Path,
        _cache_policy: &CacheConfig,
        _identity: std::sync::Arc<String>,
    ) -> Self {
        let (tx, rx) = mpsc::channel::<MprisSyncPayload>();
        let (event_tx, event_rx) = mpsc::channel::<MprisControlEvent>();
        let spawned = std::thread::Builder::new()
            .name("cnmplayer-smtc".into())
            .spawn(move || run(rx, event_tx));
        if let Err(err) = spawned {
            log::warn!("smtc thread spawn failed: {err}");
        }
        Self { tx, event_rx }
    }

    pub fn update(&self, payload: MprisSyncPayload) {
        let _ = self.tx.send(payload);
    }

    pub fn drain_control_events(&self) -> Vec<MprisControlEvent> {
        self.event_rx.try_iter().collect()
    }
}

fn run(rx: Receiver<MprisSyncPayload>, events: Sender<MprisControlEvent>) {
    let mut session = match Session::new(events) {
        Ok(session) => session,
        Err(err) => {
            log::warn!("smtc init failed: {err}");
            return;
        }
    };

    while let Ok(mut payload) = rx.recv() {
        // 积压的快照只取最新状态，但中间那次携带的曲目元数据不能丢。
        let mut track = payload.track.take();
        while let Ok(mut next) = rx.try_recv() {
            if let Some(next_track) = next.track.take() {
                track = Some(next_track);
            }
            payload = next;
        }
        payload.track = track;

        if let Err(err) = session.apply(payload) {
            log::debug!("smtc sync failed: {err}");
        }
    }
}

struct Session {
    // SMTC 属于这个 MediaPlayer，必须与之同生命周期。
    _player: MediaPlayer,
    smtc: SystemMediaTransportControls,
    status: Option<MediaPlaybackStatus>,
    duration: TimeSpan,
}

impl Session {
    fn new(events: Sender<MprisControlEvent>) -> Result<Self> {
        let player = MediaPlayer::new()?;
        player.CommandManager()?.SetIsEnabled(false)?;
        let smtc = player.SystemMediaTransportControls()?;
        smtc.SetIsEnabled(true)?;
        smtc.SetIsPlayEnabled(true)?;
        smtc.SetIsPauseEnabled(true)?;
        smtc.SetIsStopEnabled(true)?;
        smtc.SetIsNextEnabled(true)?;
        smtc.SetIsPreviousEnabled(true)?;
        smtc.SetPlaybackStatus(MediaPlaybackStatus::Closed)?;

        let tx = events.clone();
        let buttons = TypedEventHandler::<
            SystemMediaTransportControls,
            SystemMediaTransportControlsButtonPressedEventArgs,
        >::new(move |_, args| {
            if let Some(event) = map_button(args.ok()?.Button()?) {
                let _ = tx.send(event);
            }
            Ok(())
        });
        smtc.ButtonPressed(&buttons)?;

        let seek = TypedEventHandler::<
            SystemMediaTransportControls,
            PlaybackPositionChangeRequestedEventArgs,
        >::new(
            move |_, args: Ref<PlaybackPositionChangeRequestedEventArgs>| {
                let position = args.ok()?.RequestedPlaybackPosition()?;
                let _ = events.send(MprisControlEvent::SeekAbsoluteMicros(
                    position.Duration.max(0) / 10,
                ));
                Ok(())
            },
        );
        smtc.PlaybackPositionChangeRequested(&seek)?;

        Ok(Self {
            _player: player,
            smtc,
            status: None,
            duration: TimeSpan::default(),
        })
    }

    fn apply(&mut self, payload: MprisSyncPayload) -> Result<()> {
        let status = match payload.playback {
            PlaybackRuntimeState::Playing => MediaPlaybackStatus::Playing,
            PlaybackRuntimeState::Paused => MediaPlaybackStatus::Paused,
            PlaybackRuntimeState::Stopped => MediaPlaybackStatus::Stopped,
        };
        if self.status != Some(status) {
            self.smtc.SetPlaybackStatus(status)?;
            self.status = Some(status);
        }

        if let Some(track) = payload.track {
            self.duration = TimeSpan {
                Duration: track.duration_ms.max(0).saturating_mul(10_000),
            };
            self.set_metadata(&track)?;
        }

        self.set_timeline(payload.position)
    }

    fn set_metadata(&self, track: &PlaybackTrack) -> Result<()> {
        let updater = self.smtc.DisplayUpdater()?;
        updater.ClearAll()?;
        updater.SetType(MediaPlaybackType::Music)?;

        let music = updater.MusicProperties()?;
        music.SetTitle(&HSTRING::from(track.title.trim()))?;
        music.SetArtist(&HSTRING::from(track.artist.trim()))?;
        music.SetAlbumArtist(&HSTRING::from(track.artist.trim()))?;
        music.SetAlbumTitle(&HSTRING::from(track.album.trim()))?;

        if let Some(bytes) = track.cover.as_deref().filter(|bytes| !bytes.is_empty()) {
            // 封面失败不影响标题等其余信息。
            match thumbnail(bytes) {
                Ok(reference) => updater.SetThumbnail(&reference)?,
                Err(err) => log::debug!("smtc thumbnail failed: {err}"),
            }
        }

        updater.Update()
    }

    fn set_timeline(&self, position: Duration) -> Result<()> {
        let position = i64::try_from(position.as_micros().saturating_mul(10)).unwrap_or(i64::MAX);
        let position = if self.duration.Duration > 0 {
            position.min(self.duration.Duration)
        } else {
            position
        };

        let timeline = SystemMediaTransportControlsTimelineProperties::new()?;
        timeline.SetStartTime(TimeSpan::default())?;
        timeline.SetMinSeekTime(TimeSpan::default())?;
        timeline.SetEndTime(self.duration)?;
        timeline.SetMaxSeekTime(self.duration)?;
        timeline.SetPosition(TimeSpan { Duration: position })?;
        self.smtc.UpdateTimelineProperties(&timeline)
    }
}

fn map_button(button: SystemMediaTransportControlsButton) -> Option<MprisControlEvent> {
    match button {
        SystemMediaTransportControlsButton::Play => Some(MprisControlEvent::Play),
        SystemMediaTransportControlsButton::Pause => Some(MprisControlEvent::Pause),
        SystemMediaTransportControlsButton::Stop => Some(MprisControlEvent::Stop),
        SystemMediaTransportControlsButton::Next => Some(MprisControlEvent::Next),
        SystemMediaTransportControlsButton::Previous => Some(MprisControlEvent::Previous),
        _ => None,
    }
}

/// 把封面字节写进内存流交给 SMTC；运行在 SMTC 专用线程上，可以同步等待。
fn thumbnail(bytes: &[u8]) -> Result<RandomAccessStreamReference> {
    let stream = InMemoryRandomAccessStream::new()?;
    let writer = DataWriter::CreateDataWriter(&stream)?;
    writer.WriteBytes(bytes)?;
    writer.StoreAsync()?.join()?;
    writer.FlushAsync()?.join()?;
    writer.DetachStream()?;
    stream.Seek(0)?;
    RandomAccessStreamReference::CreateFromStream(&stream)
}
