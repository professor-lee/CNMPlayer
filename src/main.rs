mod app;
mod data;
mod render;
mod tmplayer;
mod ui;

use crate::tmplayer::audio::cava::MiniCavaState;
use anyhow::Result;
use app::App;
use compio::fs::{create_dir_all, remove_file};
use compio::runtime::spawn;
use compio::time::sleep;
use crossterm::event::{
    DisableMouseCapture, EnableMouseCapture, Event, EventStream, MouseEventKind,
};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use data::config::Config;
use data::theme_loader::ThemeLoader;
use directories::BaseDirs;
use ftail::Ftail;
use futures::{FutureExt, Stream, StreamExt, select_biased};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::widgets::{Block, Borders, Clear};
use see::unsync::Receiver;
use std::future::pending;
use std::io::{self, Stdout};
use std::path::PathBuf;
use std::pin::pin;
use std::sync::LazyLock;
use std::time::Duration;

struct AppFullscreenBridge<'a> {
    app: &'a mut App,
}

impl tmplayer::HostPlaybackBridge for AppFullscreenBridge<'_> {
    async fn tick(&mut self) {
        self.app.fullscreen_tick_playback().await;
    }

    fn metadata_signature(&self) -> u64 {
        self.app.fullscreen_metadata_signature()
    }

    fn runtime_snapshot(&self) -> tmplayer::HostPlaybackRuntimeSnapshot {
        let runtime = self.app.fullscreen_runtime_snapshot();
        tmplayer::HostPlaybackRuntimeSnapshot {
            current_index: runtime.current_index,
            current_liked: runtime.now_playing_liked,
            state: match runtime.state {
                app::PlaybackRuntimeState::Playing => tmplayer::HostPlaybackState::Playing,
                app::PlaybackRuntimeState::Paused => tmplayer::HostPlaybackState::Paused,
                app::PlaybackRuntimeState::Stopped => tmplayer::HostPlaybackState::Stopped,
            },
            repeat_mode: match runtime.repeat_mode {
                app::PlaybackRepeatMode::Sequence => tmplayer::HostRepeatMode::Sequence,
                app::PlaybackRepeatMode::Shuffle => tmplayer::HostRepeatMode::Shuffle,
                app::PlaybackRepeatMode::LoopAll => tmplayer::HostRepeatMode::LoopAll,
                app::PlaybackRepeatMode::LoopOne => tmplayer::HostRepeatMode::LoopOne,
            },
            position: runtime.position,
            volume: runtime.volume,
        }
    }

    fn snapshot(&mut self) -> tmplayer::HostPlaybackSnapshot {
        let snapshot = self.app.fullscreen_playback_snapshot();

        if snapshot.now_playing.is_none() {
            return tmplayer::HostPlaybackSnapshot {
                playlist: Vec::new(),
                current_index: None,
                current_track: None,
                current_liked: snapshot.now_playing_liked,
                state: tmplayer::HostPlaybackState::Stopped,
                repeat_mode: match snapshot.repeat_mode {
                    app::PlaybackRepeatMode::Sequence => tmplayer::HostRepeatMode::Sequence,
                    app::PlaybackRepeatMode::Shuffle => tmplayer::HostRepeatMode::Shuffle,
                    app::PlaybackRepeatMode::LoopAll => tmplayer::HostRepeatMode::LoopAll,
                    app::PlaybackRepeatMode::LoopOne => tmplayer::HostRepeatMode::LoopOne,
                },
                position: Duration::from_secs(0),
            };
        }

        let playlist = snapshot
            .queue
            .iter()
            .map(|track| tmplayer::FullscreenPlaylistItemSeed {
                id: Some(track.song_id.clone()),
                title: track.title.clone(),
                artist: track.artist.clone(),
                album: track.album.clone(),
                duration: Duration::from_millis(track.duration_ms.max(0) as u64),
            })
            .collect::<Vec<_>>();

        let current_track =
            snapshot
                .now_playing
                .as_ref()
                .map(|track| tmplayer::FullscreenTrackSeed {
                    playlist_index: snapshot.current_index,
                    title: track.title.clone(),
                    artist: track.artist.clone(),
                    album: track.album.clone(),
                    duration: Duration::from_millis(track.duration_ms.max(0) as u64),
                    liked: snapshot.now_playing_liked,
                    cover: track.cover.clone(),
                    lyrics: track.lyrics.clone(),
                });

        tmplayer::HostPlaybackSnapshot {
            playlist,
            current_index: snapshot.current_index,
            current_track,
            current_liked: snapshot.now_playing_liked,
            state: match snapshot.state {
                app::PlaybackRuntimeState::Playing => tmplayer::HostPlaybackState::Playing,
                app::PlaybackRuntimeState::Paused => tmplayer::HostPlaybackState::Paused,
                app::PlaybackRuntimeState::Stopped => tmplayer::HostPlaybackState::Stopped,
            },
            repeat_mode: match snapshot.repeat_mode {
                app::PlaybackRepeatMode::Sequence => tmplayer::HostRepeatMode::Sequence,
                app::PlaybackRepeatMode::Shuffle => tmplayer::HostRepeatMode::Shuffle,
                app::PlaybackRepeatMode::LoopAll => tmplayer::HostRepeatMode::LoopAll,
                app::PlaybackRepeatMode::LoopOne => tmplayer::HostRepeatMode::LoopOne,
            },
            position: snapshot.position,
        }
    }

    fn config_snapshot(&self) -> tmplayer::HostConfigSync {
        self.app.fullscreen_config_snapshot()
    }

    async fn apply_config_sync(&mut self, config: tmplayer::HostConfigSync) {
        self.app.fullscreen_apply_config_sync(config).await;
    }

    async fn toggle_play_pause(&mut self) {
        self.app.fullscreen_toggle_play_pause().await;
    }

    async fn play_previous(&mut self) {
        self.app.fullscreen_play_previous().await;
    }

    async fn play_next(&mut self) {
        self.app.fullscreen_play_next().await;
    }

    async fn play_queue_index(&mut self, index: usize) {
        self.app.fullscreen_play_queue_index(index).await;
    }

    fn seek_to_ratio(&mut self, ratio: f32) {
        self.app.fullscreen_seek_to_ratio(ratio);
    }

    fn set_volume(&mut self, volume: f32) {
        self.app.fullscreen_set_volume(volume);
    }

    fn toggle_repeat_mode(&mut self) {
        self.app.fullscreen_toggle_repeat_mode();
    }

    async fn toggle_like_current(&mut self) {
        self.app.fullscreen_toggle_like().await;
    }
}

pub struct Storage {
    cache: PathBuf,
    config: PathBuf,
}

fn try_get_storage() -> Option<Storage> {
    let app = "cnmplayer";
    let base = BaseDirs::new()?;
    let cache = base.cache_dir().join(&app);
    let config = base.config_dir().join(&app);
    let storage = Storage { cache, config };
    Some(storage)
}

fn stroage_or_abort() -> Storage {
    let msg = "Failed to initialize workdir, abort!";
    try_get_storage().expect(&msg)
}

pub static STORAGE: LazyLock<Storage> = LazyLock::new(|| stroage_or_abort());

async fn init_logger() -> Result<()> {
    create_dir_all(&STORAGE.cache).await?;
    let log_file = STORAGE.cache.join("Player.log");
    let _ = remove_file(&log_file).await;
    let ftail = Ftail::new().single_file_env_level(&log_file, false);
    Ok(ftail.init()?)
}

#[compio::main]
async fn main() -> Result<()> {
    init_logger().await?;
    let config = Config::load_or_default()?;
    let theme = ThemeLoader::load(&config.theme).unwrap_or_default();
    let mut app = App::new(config, theme).await?;

    let mut terminal = init_terminal()?;
    let run_result = run_app(&mut terminal, &mut app).await;
    restore_terminal(&mut terminal)?;
    run_result
}

fn init_terminal() -> Result<Terminal<CrosstermBackend<Stdout>>> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    Ok(Terminal::new(backend)?)
}

fn restore_terminal(terminal: &mut Terminal<CrosstermBackend<Stdout>>) -> Result<()> {
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        DisableMouseCapture,
        LeaveAlternateScreen
    )?;
    terminal.show_cursor()?;
    Ok(())
}

pub fn launch<F: Future + 'static>(future: F) {
    spawn(future).detach();
}

pub fn state<T, S>(source: S) -> Receiver<T>
where
    T: Default + 'static,
    S: Stream<Item = T> + 'static,
{
    let (tx, rx) = see::unsync::channel(T::default());
    launch(async move {
        let mut source = pin!(source);
        while let Some(item) = source.next().await {
            if tx.send(item).is_err() {
                break;
            }
        }
    });

    rx
}

fn input_event() -> impl Stream<Item = impl AsyncFn(&mut App)> {
    EventStream::new().filter_map(async |x| {
        let x = x.ok()?;
        match x {
            Event::Key(_) | Event::Resize(_, _) => (),
            Event::Mouse(e)
                if matches!(
                    e.kind,
                    MouseEventKind::Down(_) | MouseEventKind::ScrollUp | MouseEventKind::ScrollDown
                ) => {}
            _ => return None,
        }
        let d = async move |app: &mut App| {
            match x {
                Event::Key(key) => app.handle_key(key).await,
                Event::Mouse(mouse) => app.handle_mouse(mouse).await,
                _ => {}
            };
            app.sync_on_change();
        };
        Some(d)
    })
}

async fn run_app(terminal: &mut Terminal<CrosstermBackend<Stdout>>, app: &mut App) -> Result<()> {
    let mut input = pin!(input_event());

    loop {
        app.tick().await;

        if app.consume_fullscreen_launch_request() {
            let bootstrap = app.build_fullscreen_bootstrap().await;
            launch_tmplayer_fullscreen(terminal, app, bootstrap).await?;
            continue;
        }

        if app.should_quit {
            app.persist_playback_memory_on_exit();
            break Ok(());
        }
        terminal.draw(|frame| {
            ui::draw(frame, app);
            ui::draw_settings(frame, app);
        })?;

        select_biased! {
            f = input.next().fuse() => if let Some(f) = f { f(app).await },
            _ = wait_cava_event(&mut app.cava).fuse() => (),
            _ = sleep(Duration::from_secs(1)).fuse() => (),
        }
    }
}

async fn wait_cava_event(cava: &mut Option<MiniCavaState>) {
    match cava {
        Some(cava) => {
            let _ = cava.event.changed().await;
            cava.event.mark_unchanged();
        }
        None => pending().await,
    }
}

async fn launch_tmplayer_fullscreen(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    app: &mut App,
    bootstrap: tmplayer::FullscreenBootstrap,
) -> Result<()> {
    play_fullscreen_transition(terminal, app, true).await?;
    app.suspend_main_cava_for_fullscreen();
    restore_terminal(terminal)?;

    let config = app.config.clone();
    let mut bridge = AppFullscreenBridge { app };
    let status_text = match tmplayer::run_fullscreen(&config, bootstrap, Some(&mut bridge)).await {
        Ok(tmplayer::FullscreenExit::BackToHost) => String::new(),
        Ok(tmplayer::FullscreenExit::BackToHostOpenSettings) => {
            bridge.app.open_settings_from_fullscreen();
            String::new()
        }
        Err(err) => format!("TMPlayer 运行失败: {}", err),
    };

    *terminal = init_terminal()?;
    app.resume_main_cava_after_fullscreen();
    play_fullscreen_transition(terminal, app, false).await?;
    if !status_text.is_empty() {
        app.set_runtime_status(status_text);
    }
    Ok(())
}

async fn play_fullscreen_transition(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    app: &mut App,
    opening: bool,
) -> Result<()> {
    let steps: u16 = 10;

    for step in 0..=steps {
        let progress = if opening { step } else { steps - step };
        terminal.draw(|frame| {
            ui::draw(frame, app);

            let full = frame.area();
            if full.height == 0 || full.width == 0 {
                return;
            }

            let bar_h = 5_u16.min(full.height);
            let span = full.height.saturating_sub(bar_h);
            let animated = bar_h + ((span as u32 * progress as u32) / steps as u32) as u16;
            let overlay = Rect {
                x: full.x,
                y: full.y + full.height.saturating_sub(animated),
                width: full.width,
                height: animated,
            };

            frame.render_widget(Clear, overlay);
            frame.render_widget(
                Block::default()
                    .borders(Borders::TOP)
                    .border_style(Style::default().fg(app.theme.color_surface()))
                    .style(Style::default().bg(app.theme.color_base())),
                overlay,
            );
        })?;

        sleep(Duration::from_millis(14)).await;
    }

    Ok(())
}
