use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct PlaylistItem {
    pub path: PathBuf,
    pub title: String,
}

#[derive(Debug, Default, Clone)]
pub struct Playlist {
    pub items: Vec<PlaylistItem>,
    pub selected: usize,
    pub current: Option<usize>,
}

impl Playlist {
    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn clamp_selected(&mut self) {
        if self.items.is_empty() {
            self.selected = 0;
        } else if self.selected >= self.items.len() {
            self.selected = self.items.len() - 1;
        }
    }

    pub fn move_up(&mut self) {
        if self.selected > 0 {
            self.selected -= 1;
        }
    }

    pub fn move_down(&mut self) {
        if !self.items.is_empty() {
            self.selected = (self.selected + 1).min(self.items.len() - 1);
        }
    }

    pub fn next_index_sequence(&self) -> Option<usize> {
        let cur = self.current?;
        if self.items.is_empty() {
            None
        } else {
            Some((cur + 1) % self.items.len())
        }
    }

    pub fn next_index_no_wrap(&self) -> Option<usize> {
        let cur = self.current?;
        if self.items.is_empty() {
            return None;
        }
        if cur + 1 >= self.items.len() {
            None
        } else {
            Some(cur + 1)
        }
    }

    pub fn prev_index_sequence(&self) -> Option<usize> {
        let cur = self.current?;
        if self.items.is_empty() {
            None
        } else {
            Some((cur + self.items.len() - 1) % self.items.len())
        }
    }

    pub fn prev_index_no_wrap(&self) -> Option<usize> {
        let cur = self.current?;
        if self.items.is_empty() {
            return None;
        }
        if cur == 0 { None } else { Some(cur - 1) }
    }
}
