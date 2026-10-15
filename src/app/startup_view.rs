use super::*;

impl App {
    pub(super) fn begin_startup_loading(&mut self, target: Page) {
        self.page = Page::Loading;
        self.close_overlay();
        self.startup.begin(target);
    }

    pub(super) fn finish_startup_loading(&mut self) {
        self.startup.finish();
    }

    pub(super) fn tick_startup_loading(&mut self) {
        if self.page == Page::Loading
            && let Some(target) = self.startup.tick()
        {
            self.page = target;
        }
    }

    pub(super) fn startup_loading_progress(&self) -> f32 {
        self.startup.current_progress()
    }

    pub fn startup_loading_progress_for_width(&self, _bar_width: u16) -> f32 {
        if self.page != Page::Loading {
            return 0.0;
        }

        if self.startup.started_at.is_none() {
            return 0.0;
        }

        self.startup_loading_progress()
    }
}
