//! Browse-page drawers share the fullscreen motion, snapshot and cell compositor.
use super::App;
use super::Page;
use super::author::AuthorState;
use super::playlist::PlaylistState;
use crate::render::drawer;
use crate::render::motion::{Curve, Transition};
use crate::render::snapshot;
use ratatui::{Frame, Terminal, backend::Backend, buffer::Buffer};
use std::time::{Duration, Instant};

const PAGE_SLIDE_DURATION: Duration = Duration::from_millis(220);

#[derive(Clone, Copy, PartialEq, Eq)]
enum Direction {
    Open,
    Close,
}

// Ordinary pages remain in BrowseController while navigating. Only same-page
// replacement needs to retain a model until its one-off snapshot is prepared.
enum Source {
    Page(Page),
    PlaylistSection,
    Playlist(PlaylistState),
    Author(AuthorState),
}

struct Request {
    source: Source,
    direction: Direction,
}

struct Drawer {
    source: Buffer,
    direction: Direction,
    offset: Transition,
    started: bool,
}

#[derive(Default)]
pub(super) struct PageTransitionController {
    pending: Option<Request>,
    preserved: Option<Source>,
    drawer: Option<Drawer>,
}

impl App {
    pub(super) fn preserve_browse_page_for_transition(&mut self, page: Page) {
        if self.page != page {
            return;
        }
        self.page_transition.preserved = match page {
            Page::Playlist => Some(Source::Playlist(std::mem::take(&mut self.browse.playlist))),
            Page::Author => Some(Source::Author(std::mem::take(&mut self.browse.author))),
            _ => None,
        };
    }

    pub(super) fn open_browse_page(&mut self, page: Page) {
        let source = self.page_transition.preserved.take().unwrap_or_else(|| {
            if self.page == Page::Playlist
                && page == Page::Playlist
                && self.playlist_section_return_snapshot.is_some()
            {
                Source::PlaylistSection
            } else {
                Source::Page(self.page)
            }
        });
        self.queue_page_transition(page, source, Direction::Open);
    }

    pub(super) fn close_browse_page(&mut self, page: Page) {
        self.queue_page_transition(page, Source::Page(self.page), Direction::Close);
    }

    pub(super) fn close_playlist_section(&mut self, parent: PlaylistState) {
        let outgoing = std::mem::replace(&mut self.browse.playlist, parent);
        self.queue_page_transition(Page::Playlist, Source::Playlist(outgoing), Direction::Close);
    }

    fn queue_page_transition(&mut self, page: Page, source: Source, direction: Direction) {
        self.page = page;
        self.page_transition.pending = Some(Request { source, direction });
        self.input.player_bar_progress_drag = None;
        self.page_lyrics_release();
        self.clear_content_hits();
        self.clear_player_bar_hits();
    }

    /// Prepare once, after navigation/data work and before starting visible time.
    /// Capture renders a retained source without submitting it to the terminal.
    pub(crate) fn prepare_page_transition<B: Backend>(
        &mut self,
        terminal: &mut Terminal<B>,
    ) -> Result<(), B::Error> {
        let Some(request) = self.page_transition.pending.take() else {
            return Ok(());
        };
        terminal.autoresize()?;
        let area = terminal.get_frame().area();
        self.set_terminal_size(area.width, area.height);
        if self.is_small_window_context() || matches!(self.page, Page::Login | Page::Loading) {
            self.cancel_page_transition();
            return Ok(());
        }
        let previous = self.page_transition.drawer.take();
        let reversed_offset = previous
            .as_ref()
            .filter(|drawer| {
                drawer.direction == Direction::Open
                    && request.direction == Direction::Close
                    && drawer.source.area == area
            })
            .map(|drawer| drawer.offset.value());
        let destination = self.page;
        let mut source_playlist = None;
        let mut source_author = None;
        let mut section = false;
        match request.source {
            Source::Page(page) => self.page = page,
            Source::PlaylistSection => {
                self.page = Page::Playlist;
                source_playlist = self.playlist_section_return_snapshot.take();
                section = true;
            }
            Source::Playlist(playlist) => {
                self.page = Page::Playlist;
                source_playlist = Some(playlist);
            }
            Source::Author(author) => {
                self.page = Page::Author;
                source_author = Some(author);
            }
        }
        if let Some(playlist) = source_playlist.as_mut() {
            std::mem::swap(playlist, &mut self.browse.playlist);
        }
        if let Some(author) = source_author.as_mut() {
            std::mem::swap(author, &mut self.browse.author);
        }
        let captured = snapshot::capture(terminal, |frame| crate::ui::draw_page(frame, self));
        if let Some(playlist) = source_playlist.as_mut() {
            std::mem::swap(playlist, &mut self.browse.playlist);
        }
        if let Some(author) = source_author.as_mut() {
            std::mem::swap(author, &mut self.browse.author);
        }
        if section {
            self.playlist_section_return_snapshot = source_playlist;
        }
        self.page = destination;
        let source = captured?;
        let initial = reversed_offset.unwrap_or(if request.direction == Direction::Open {
            1.0
        } else {
            0.0
        });
        self.page_transition.drawer = Some(Drawer {
            source,
            direction: request.direction,
            offset: Transition::new(initial),
            started: false,
        });
        Ok(())
    }

    pub(crate) fn page_transition_active(&self) -> bool {
        self.page_transition.pending.is_some() || self.page_transition.drawer.is_some()
    }

    pub(super) fn page_transition_can_go_back(&self) -> bool {
        self.page_transition
            .pending
            .as_ref()
            .map(|request| request.direction)
            .or_else(|| {
                self.page_transition
                    .drawer
                    .as_ref()
                    .map(|drawer| drawer.direction)
            })
            == Some(Direction::Open)
    }

    pub(super) fn tick_page_transition(&mut self, now: Instant) {
        if let Some(drawer) = self.page_transition.drawer.as_mut() {
            drawer.offset.tick(now);
        }
    }

    pub(crate) fn render_page_transition(&mut self, frame: &mut Frame) {
        let Some(drawer) = self.page_transition.drawer.as_ref() else {
            return;
        };
        if drawer.source.area != frame.area() {
            self.cancel_page_transition();
            return;
        }
        let offset = (f32::from(frame.area().width) * drawer.offset.value()).round() as u16;
        match drawer.direction {
            Direction::Open => {
                drawer::reveal_from_right(frame.buffer_mut(), &drawer.source, offset);
            }
            Direction::Close => {
                let area = frame.area();
                drawer::paint(frame.buffer_mut(), &drawer.source, i32::from(offset), area);
            }
        }
    }

    /// The initial endpoint must be submitted before the clock starts. Keep the
    /// drawer through its final submission, then free the transient snapshot.
    pub(crate) fn page_transition_presented(&mut self, now: Instant) {
        let Some(drawer) = self.page_transition.drawer.as_mut() else {
            return;
        };
        if !drawer.started {
            let target = if drawer.direction == Direction::Open {
                0.0
            } else {
                1.0
            };
            drawer
                .offset
                .retarget(target, now, PAGE_SLIDE_DURATION, Curve::EaseOut);
            drawer.started = true;
        } else if !drawer.offset.is_running() {
            self.page_transition.drawer = None;
        }
    }

    pub(crate) fn cancel_page_transition(&mut self) {
        self.page_transition = PageTransitionController::default();
    }
}
