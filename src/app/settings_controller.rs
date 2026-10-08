use super::{DownloadPathEdit, HitRect, Overlay};
use std::time::Instant;

#[derive(Default)]
pub(crate) struct SettingsController {
    pub selected: usize,
    pub playback_selected: usize,
    pub lyrics_selected: usize,
    pub keybind_selected: usize,
    pub keybind_rebinding: Option<usize>,
    pub keybind_scroll: usize,
    pub download_selected: usize,
    pub download_path_edit: Option<DownloadPathEdit>,
    pub download_reset_armed: bool,
    pub item_hits: Vec<(HitRect, usize)>,
    pub last_click: Option<(Instant, Overlay, usize)>,
}

impl SettingsController {
    pub fn reset_navigation(&mut self) {
        self.selected = 0;
        self.playback_selected = 0;
        self.lyrics_selected = 0;
        self.keybind_selected = 0;
        self.keybind_rebinding = None;
        self.keybind_scroll = 0;
        self.download_selected = 0;
        self.download_path_edit = None;
        self.download_reset_armed = false;
        self.last_click = None;
    }

    pub fn select(&mut self, index: usize, count: usize) {
        self.selected = index.min(count.saturating_sub(1));
    }

    pub fn begin_download_path_edit(&mut self, value: String) {
        let cursor = value.chars().count();
        self.download_path_edit = Some(DownloadPathEdit {
            buffer: value,
            cursor,
            window_col: 0,
        });
    }

    pub fn cancel_download_path_edit(&mut self) {
        self.download_path_edit = None;
        self.download_reset_armed = false;
    }
}
