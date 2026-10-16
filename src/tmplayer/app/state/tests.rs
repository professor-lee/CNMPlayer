use super::*;

#[test]
fn settings_close_releases_overlay_immediately() {
    let mut app = AppState::new(
        Config::default(),
        crate::ui::theme::Theme::default(),
        Language::En,
    );
    for overlay in [
        Overlay::SettingsModal,
        Overlay::BarSettingsModal,
        Overlay::LyricsSettingsModal,
        Overlay::DownloadSettingsModal,
        Overlay::DownloadPathEditModal,
        Overlay::HelpModal,
        Overlay::AboutModal,
        Overlay::EqModal,
    ] {
        app.overlay = overlay;
        app.close_overlay();
        assert_eq!(
            app.overlay,
            Overlay::None,
            "{overlay:?} must close without a tick"
        );
    }
}

#[test]
fn help_toggle_closes_the_whole_modal_instead_of_opening_settings() {
    let mut app = AppState::new(
        Config::default(),
        crate::ui::theme::Theme::default(),
        Language::En,
    );
    for parent in [Overlay::None, Overlay::SettingsModal] {
        app.overlay = parent;
        app.toggle_help_modal();
        assert_eq!(app.overlay, Overlay::HelpModal);

        app.toggle_help_modal();
        assert_eq!(app.overlay, Overlay::None);
    }
}
