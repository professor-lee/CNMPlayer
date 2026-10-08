use super::{DownloadPathEdit, HitRect, Overlay};
use crate::render::motion::{Curve, MODAL_ANIM_DURATION, Toggle};
use std::time::Instant;

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
    pub(crate) motion: Toggle,
    pub close_pending: bool,
}

impl Default for SettingsController {
    fn default() -> Self {
        Self {
            selected: 0,
            playback_selected: 0,
            lyrics_selected: 0,
            keybind_selected: 0,
            keybind_rebinding: None,
            keybind_scroll: 0,
            download_selected: 0,
            download_path_edit: None,
            download_reset_armed: false,
            item_hits: Vec::new(),
            last_click: None,
            motion: Toggle::new(false),
            close_pending: false,
        }
    }
}

impl SettingsController {
    pub fn open_motion(&mut self, now: Instant) {
        self.close_pending = false;
        self.motion
            .set(true, now, MODAL_ANIM_DURATION, Curve::EaseOut);
    }

    pub fn begin_close(&mut self, now: Instant) {
        self.close_pending = true;
        self.motion
            .set(false, now, MODAL_ANIM_DURATION, Curve::EaseInOut);
    }

    pub fn tick_motion(&mut self, visible: bool, now: Instant) -> bool {
        if !visible {
            self.reset_motion();
            return false;
        }
        if !self.close_pending {
            self.open_motion(now);
        }
        self.motion.tick(now)
    }

    pub fn reset_motion(&mut self) {
        self.motion = Toggle::new(false);
        self.close_pending = false;
    }

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

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn settings_motion_opens_and_closes_with_dirty_endpoints() {
        let mut settings = SettingsController::default();
        let start = Instant::now();
        settings.open_motion(start);
        assert!(settings.tick_motion(true, start + Duration::from_millis(90)));
        assert!(settings.motion.value() > 0.0 && settings.motion.value() < 1.0);
        assert!(settings.motion.is_running());
        assert!(settings.tick_motion(true, start + MODAL_ANIM_DURATION));
        assert_eq!(settings.motion.value(), 1.0);
        assert!(!settings.motion.is_running());

        let close = start + MODAL_ANIM_DURATION;
        settings.begin_close(close);
        assert!(settings.tick_motion(true, close + Duration::from_millis(90)));
        assert!(settings.motion.value() > 0.0 && settings.motion.value() < 1.0);
        settings.begin_close(close + Duration::from_millis(90));
        assert!(settings.tick_motion(true, close + MODAL_ANIM_DURATION));
        assert_eq!(settings.motion.value(), 0.0);
        assert!(settings.close_pending);
        assert!(!settings.motion.is_running());
        assert!(!settings.tick_motion(
            true,
            close + MODAL_ANIM_DURATION + Duration::from_millis(50)
        ));
    }

    #[test]
    fn dismissing_settings_cancels_motion_before_a_fresh_open() {
        let mut settings = SettingsController::default();
        let start = Instant::now();
        settings.open_motion(start);
        settings.tick_motion(true, start + Duration::from_millis(90));
        settings.begin_close(start + Duration::from_millis(90));
        settings.tick_motion(false, start + Duration::from_millis(100));
        assert_eq!(settings.motion.value(), 0.0);
        assert!(!settings.motion.is_running());
        assert!(!settings.close_pending);
        settings.open_motion(start + Duration::from_secs(1));
        assert_eq!(settings.motion.value(), 0.0);
        settings.tick_motion(true, start + Duration::from_secs(1) + MODAL_ANIM_DURATION);
        assert_eq!(settings.motion.value(), 1.0);
    }
}
