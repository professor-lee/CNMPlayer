use crate::app::{App, Overlay, Page};
use ratatui::Frame;

pub mod author;
pub mod home;
pub mod loading;
pub mod login;
pub mod page_lyrics;
pub mod player_bar;
pub mod playlist;
pub mod search;
pub mod search_box;
pub mod settings;
pub mod small_window;
pub mod theme;

pub fn draw_settings(frame: &mut Frame, app: &mut App) {
    if app.is_small_window_context() {
        return;
    }
    if matches!(
        app.overlay,
        Some(Overlay::Settings)
            | Some(Overlay::SettingsPlayback)
            | Some(Overlay::SettingsKeybinds)
            | Some(Overlay::SettingsAbout)
    ) {
        settings::draw_settings_modal(frame, app);
    }
    if matches!(app.overlay, Some(Overlay::SearchBox)) {
        search_box::draw_search_box_overlay(frame, app);
    }
}

pub fn draw(frame: &mut Frame, app: &mut App) {
    let size = frame.area();
    app.set_terminal_size(size.width, size.height);

    if app.config.small_window_display
        && !matches!(app.page, Page::Login | Page::Loading)
        && (size.width < crate::app::SMALL_WINDOW_MIN_WIDTH
            || size.height < crate::app::SMALL_WINDOW_MIN_HEIGHT)
    {
        match app.small_window_mode {
            Some(crate::app::SmallWindowMode::Flat) => small_window::draw_flat(frame, app),
            Some(crate::app::SmallWindowMode::Narrow) => small_window::draw_narrow(frame, app),
            None => draw_small_too_small(frame, app, size),
        }
        return;
    }

    match app.page {
        Page::Login => login::draw_login(frame, app),
        Page::Loading => loading::draw_loading(frame, app),
        Page::Home => home::draw_home(frame, app),
        Page::Playlist => playlist::draw_playlist(frame, app),
        Page::Author => author::draw_author(frame, app),
        Page::Search => search::draw_search(frame, app),
    }
}

fn draw_small_too_small(frame: &mut Frame, app: &App, area: ratatui::layout::Rect) {
    use ratatui::layout::Alignment;
    use ratatui::widgets::{Block, Paragraph};

    frame.render_widget(ratatui::widgets::Clear, area);
    frame.render_widget(
        Block::default().style(if app.config.transparent_background {
            ratatui::style::Style::default()
        } else {
            ratatui::style::Style::default().bg(app.theme.color_base())
        }),
        area,
    );
    frame.render_widget(
        Paragraph::new(match app.config.language {
            crate::data::config::Language::Zh => "终端窗口过小",
            crate::data::config::Language::En => "Terminal too small",
        })
        .style(ratatui::style::Style::default().fg(app.theme.color_subtext()))
        .alignment(Alignment::Center),
        area,
    );
}
