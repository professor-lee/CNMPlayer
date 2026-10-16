pub mod input;
pub mod layout;
pub mod modals;
mod page;
mod text;

use self::layout::UiLayout;
use self::page::{PlaylistDrawer, draw_page, draw_page_reveal_with_snapshot};
use crate::render::cover_pipeline::CoverPipeline;
use crate::render::wake::WakeSignal;
use crate::tmplayer::app::state::AppState;
use anyhow::Result;
use crossterm::terminal;
use ratatui::Terminal;
use ratatui::backend::Backend;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

/// Fullscreen rendering resources only. The Host owns terminal modes and the
/// sole double-buffered output surface; snapshots live in transition calls.
pub struct Tui<'a, B: Backend> {
    terminal: &'a mut Terminal<B>,
    pub should_quit: bool,
    covers: CoverPipeline,
    sidebar: PlaylistDrawer,
}

impl<'a, B: Backend> Tui<'a, B>
where
    B::Error: std::error::Error + Send + Sync + 'static,
{
    pub fn new(terminal: &'a mut Terminal<B>, wake: WakeSignal) -> Self {
        Self {
            terminal,
            should_quit: false,
            covers: CoverPipeline::new(wake),
            sidebar: PlaylistDrawer::default(),
        }
    }

    pub fn terminal_mut(&mut self) -> &mut Terminal<B> {
        self.terminal
    }
    pub fn request_quit(&mut self) {
        self.should_quit = true;
    }

    pub fn area(&mut self) -> std::result::Result<Rect, B::Error> {
        self.terminal.autoresize()?;
        Ok(self.terminal.get_frame().area())
    }
    pub fn poll_cover_frames(&mut self) -> bool {
        self.covers.poll()
    }

    pub fn draw(&mut self, app: &mut AppState) -> Result<UiLayout> {
        // 点击作者名/专辑名这类"退出后交给宿主"的请求：请求一旦写下就退出。
        // 退出判定只在 draw 里做一次，避免每条事件分支各自记一遍。
        if app.exit_request.is_some() {
            self.should_quit = true;
        }

        let mut layout_out = UiLayout::default();

        // 小窗口显示开启时，全屏页过小不再显示提示，而是直接请求退出回主程序。
        // 在 draw 之前检测，避免过小提示闪现一帧。
        if app.config.small_window_display
            && let Ok((width, height)) = terminal::size()
            && (width < 50 || height < 12)
        {
            self.should_quit = true;
            return Ok(layout_out);
        }

        layout_out = draw_page(self.terminal, app, &mut self.covers, &mut self.sidebar)?;

        Ok(layout_out)
    }
    pub fn draw_reveal(
        &mut self,
        app: &mut AppState,
        host_snapshot: &Buffer,
        progress: f32,
    ) -> Result<UiLayout> {
        draw_page_reveal_with_snapshot(
            self.terminal,
            app,
            &mut self.covers,
            &mut self.sidebar,
            Some(host_snapshot),
            progress,
        )
        .map_err(Into::into)
    }
}

#[cfg(test)]
mod tests;
