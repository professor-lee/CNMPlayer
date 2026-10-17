use crate::tmplayer::app::state::AppState;

pub(super) fn lang_text<'a>(app: &AppState, zh: &'a str, en: &'a str) -> &'a str {
    match app.language {
        crate::data::config::Language::Zh => zh,
        crate::data::config::Language::En => en,
    }
}

pub(super) fn lang_on_off(app: &AppState, enabled: bool) -> &'static str {
    match app.language {
        crate::data::config::Language::Zh => {
            if enabled {
                "开"
            } else {
                "关"
            }
        }
        crate::data::config::Language::En => {
            if enabled {
                "On"
            } else {
                "Off"
            }
        }
    }
}
