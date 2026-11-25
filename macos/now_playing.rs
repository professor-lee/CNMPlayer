//! macOS「正在播放」服务端，对应 Linux 上的 MPRIS、Windows 上的 SMTC。
//!
//! 由 `src/app/mpris_bridge.rs` 以 `#[cfg(target_os = "macos")] #[path = ...] mod imp;` 挂载，
//! 对外接口与 Linux 版 `MprisBridge` 相同：`update` 推送最新快照，`drain_control_events` 取回
//! 控制中心、媒体键、耳机按键与进度拖动。
//!
//! MPNowPlayingInfoCenter / MPRemoteCommandCenter 都在主线程操作：`update` 把快照投递到主队列，
//! 由 `macos/main_thread.rs` 转的事件循环执行；遥控命令的回调也在主线程触发，经通道交回程序主体。
//!
//! 控制中心显示的应用名称与图标由系统按进程（应用包）决定，没有公开接口可改，所以不使用
//! identity（设置里的「正在播放名称」）。

use super::MprisControlEvent;
use super::MprisSyncPayload;
use super::PlaybackRuntimeState;
use super::PlaybackTrack;
use crate::data::config::CacheConfig;
use block2::RcBlock;
use dispatch2::DispatchQueue;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{AnyThread, MainThreadMarker};
use objc2_app_kit::NSImage;
use objc2_core_foundation::CGSize;
use objc2_foundation::{NSData, NSMutableDictionary, NSNumber, NSString};
use objc2_media_player::{
    MPChangePlaybackPositionCommandEvent, MPMediaItemArtwork, MPMediaItemPropertyAlbumArtist,
    MPMediaItemPropertyAlbumTitle, MPMediaItemPropertyArtist, MPMediaItemPropertyArtwork,
    MPMediaItemPropertyPlaybackDuration, MPMediaItemPropertyTitle, MPNowPlayingInfoCenter,
    MPNowPlayingInfoMediaType, MPNowPlayingInfoPropertyElapsedPlaybackTime,
    MPNowPlayingInfoPropertyMediaType, MPNowPlayingInfoPropertyPlaybackRate,
    MPNowPlayingPlaybackState, MPRemoteCommand, MPRemoteCommandCenter, MPRemoteCommandEvent,
    MPRemoteCommandHandlerStatus,
};
use std::cell::RefCell;
use std::path::Path;
use std::ptr::NonNull;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender};

thread_local! {
    /// 只在主线程上读写。
    static SESSION: RefCell<Option<Session>> = const { RefCell::new(None) };
}

pub struct MprisBridge {
    event_rx: Receiver<MprisControlEvent>,
}

impl MprisBridge {
    /// 封面直接以内存中的图片交给系统，也不需要 Linux 那样的封面缓存目录。
    pub fn new(
        _cache_root: &Path,
        _cache_policy: &CacheConfig,
        _identity: std::sync::Arc<String>,
    ) -> Self {
        let (event_tx, event_rx) = mpsc::channel::<MprisControlEvent>();
        DispatchQueue::main().exec_async(move || {
            let Some(mtm) = MainThreadMarker::new() else {
                return;
            };
            let session = Session::new(mtm, event_tx);
            SESSION.with_borrow_mut(|slot| *slot = Some(session));
        });
        Self { event_rx }
    }

    pub fn update(&self, payload: MprisSyncPayload) {
        DispatchQueue::main().exec_async(move || {
            SESSION.with_borrow_mut(|slot| {
                if let Some(session) = slot {
                    session.apply(payload);
                }
            });
        });
    }

    pub fn drain_control_events(&self) -> Vec<MprisControlEvent> {
        self.event_rx.try_iter().collect()
    }
}

impl Drop for MprisBridge {
    /// 退出时撤下遥控命令并清空「正在播放」，控制中心不再残留本程序的卡片。
    fn drop(&mut self) {
        DispatchQueue::main().exec_async(|| {
            SESSION.with_borrow_mut(|slot| drop(slot.take()));
        });
    }
}

struct Session {
    center: Retained<MPNowPlayingInfoCenter>,
    /// 注册过的遥控命令与对应的 target，撤销时逐个移除。
    targets: Vec<(Retained<MPRemoteCommand>, Retained<AnyObject>)>,
    track: Option<TrackInfo>,
}

/// 当前曲目不随进度变化的部分；进度与播放速率每次快照都重写。
struct TrackInfo {
    title: Retained<NSString>,
    artist: Retained<NSString>,
    album: Retained<NSString>,
    duration_secs: f64,
    cover: Option<Arc<[u8]>>,
    artwork: Option<Retained<MPMediaItemArtwork>>,
}

impl Session {
    fn new(_mtm: MainThreadMarker, events: Sender<MprisControlEvent>) -> Self {
        // SAFETY: 主线程上取共享的命令中心与信息中心。
        let (commands, center) = unsafe {
            (
                MPRemoteCommandCenter::sharedCommandCenter(),
                MPNowPlayingInfoCenter::defaultCenter(),
            )
        };
        let mut targets = Vec::new();
        // SAFETY: 以下均为命令中心自带的命令对象。
        let simple = unsafe {
            [
                (commands.playCommand(), MprisControlEvent::Play),
                (commands.pauseCommand(), MprisControlEvent::Pause),
                (
                    commands.togglePlayPauseCommand(),
                    MprisControlEvent::PlayPause,
                ),
                (commands.stopCommand(), MprisControlEvent::Stop),
                (commands.nextTrackCommand(), MprisControlEvent::Next),
                (commands.previousTrackCommand(), MprisControlEvent::Previous),
            ]
        };
        for (command, event) in simple {
            let tx = events.clone();
            let handler = RcBlock::new(move |_: NonNull<MPRemoteCommandEvent>| {
                let _ = tx.send(event);
                MPRemoteCommandHandlerStatus::Success
            });
            targets.push(register(command, &handler));
        }

        // SAFETY: 同上。
        let seek = unsafe { commands.changePlaybackPositionCommand() };
        let handler = RcBlock::new(move |event: NonNull<MPRemoteCommandEvent>| {
            // SAFETY: changePlaybackPositionCommand 只派发 MPChangePlaybackPositionCommandEvent。
            let event = unsafe {
                event
                    .cast::<MPChangePlaybackPositionCommandEvent>()
                    .as_ref()
            };
            // SAFETY: 读取事件携带的目标位置（秒）。
            let seconds = unsafe { event.positionTime() };
            let _ = events.send(MprisControlEvent::SeekAbsoluteMicros(
                (seconds.max(0.0) * 1_000_000.0) as i64,
            ));
            MPRemoteCommandHandlerStatus::Success
        });
        targets.push(register(Retained::into_super(seek), &handler));

        Self {
            center,
            targets,
            track: None,
        }
    }

    fn apply(&mut self, payload: MprisSyncPayload) {
        if let Some(track) = payload.track {
            self.set_track(&track);
        }
        let Some(track) = &self.track else {
            return;
        };

        let playing = payload.playback == PlaybackRuntimeState::Playing;
        let elapsed = payload.position.as_secs_f64();
        let elapsed = if track.duration_secs > 0.0 {
            elapsed.min(track.duration_secs)
        } else {
            elapsed
        };

        let info = NSMutableDictionary::<NSString, AnyObject>::new();
        // SAFETY: 以下键都是 MediaPlayer 导出的常量字符串。
        unsafe {
            info.insert(MPMediaItemPropertyTitle, &track.title);
            info.insert(MPMediaItemPropertyArtist, &track.artist);
            info.insert(MPMediaItemPropertyAlbumArtist, &track.artist);
            info.insert(MPMediaItemPropertyAlbumTitle, &track.album);
            info.insert(
                MPNowPlayingInfoPropertyMediaType,
                &NSNumber::new_usize(MPNowPlayingInfoMediaType::Audio.0),
            );
            if track.duration_secs > 0.0 {
                info.insert(
                    MPMediaItemPropertyPlaybackDuration,
                    &NSNumber::new_f64(track.duration_secs),
                );
            }
            info.insert(
                MPNowPlayingInfoPropertyElapsedPlaybackTime,
                &NSNumber::new_f64(elapsed),
            );
            // 速率让系统在两次快照之间自己推算进度；暂停或缓冲时为 0。
            info.insert(
                MPNowPlayingInfoPropertyPlaybackRate,
                &NSNumber::new_f64(if playing { 1.0 } else { 0.0 }),
            );
            if let Some(artwork) = &track.artwork {
                info.insert(MPMediaItemPropertyArtwork, artwork);
            }
        }

        let state = match payload.playback {
            PlaybackRuntimeState::Playing => MPNowPlayingPlaybackState::Playing,
            PlaybackRuntimeState::Paused => MPNowPlayingPlaybackState::Paused,
            PlaybackRuntimeState::Stopped => MPNowPlayingPlaybackState::Stopped,
        };
        // SAFETY: 主线程上更新共享的信息中心。
        unsafe {
            self.center.setNowPlayingInfo(Some(&info));
            self.center.setPlaybackState(state);
        }
    }

    fn set_track(&mut self, track: &PlaybackTrack) {
        let cover = track.cover.clone().filter(|bytes| !bytes.is_empty());
        // 同一张封面不重复解码：曲目元数据变化（如歌词到达）时封面通常不变。
        let artwork = match &self.track {
            Some(previous) if matches!((&previous.cover, &cover), (Some(a), Some(b)) if Arc::ptr_eq(a, b)) => {
                previous.artwork.clone()
            }
            _ => cover.as_deref().and_then(artwork),
        };
        self.track = Some(TrackInfo {
            title: NSString::from_str(track.title.trim()),
            artist: NSString::from_str(track.artist.trim()),
            album: NSString::from_str(track.album.trim()),
            duration_secs: track.duration_ms.max(0) as f64 / 1000.0,
            cover,
            artwork,
        });
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        for (command, target) in &self.targets {
            // SAFETY: target 正是 addTargetWithHandler 返回的对象。
            unsafe {
                command.removeTarget(Some(target));
                command.setEnabled(false);
            }
        }
        // SAFETY: 主线程上清空共享的信息中心。
        unsafe {
            self.center.setNowPlayingInfo(None);
            self.center
                .setPlaybackState(MPNowPlayingPlaybackState::Stopped);
        }
    }
}

fn register(
    command: Retained<MPRemoteCommand>,
    handler: &block2::DynBlock<
        dyn Fn(NonNull<MPRemoteCommandEvent>) -> MPRemoteCommandHandlerStatus,
    >,
) -> (Retained<MPRemoteCommand>, Retained<AnyObject>) {
    // SAFETY: handler 只做通道发送，返回值为合法的处理状态。
    let target = unsafe {
        command.setEnabled(true);
        command.addTargetWithHandler(handler)
    };
    (command, target)
}

/// 把封面字节解码成系统需要的图片；格式不认识时不显示封面，标题等其余信息照常。
fn artwork(bytes: &[u8]) -> Option<Retained<MPMediaItemArtwork>> {
    let data = NSData::with_bytes(bytes);
    let image = NSImage::initWithData(NSImage::alloc(), &data)?;
    let size: CGSize = image.size();
    let handler = RcBlock::new(move |_: CGSize| -> NonNull<NSImage> {
        // 块持有图片，系统在块存活期间都可以取用。
        NonNull::from(&*image)
    });
    // SAFETY: 尺寸来自图片本身，处理块始终返回同一张有效图片。
    Some(unsafe {
        MPMediaItemArtwork::initWithBoundsSize_requestHandler(
            MPMediaItemArtwork::alloc(),
            size,
            &handler,
        )
    })
}
