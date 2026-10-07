use crate::render::motion::Toggle;

/// Search-box input and animation state.
pub(crate) struct InputController {
    pub search_box_input: String,
    pub search_box_cursor: usize,
    pub(crate) search_motion: Toggle,
}

impl Default for InputController {
    fn default() -> Self {
        Self { search_box_input: String::new(), search_box_cursor: 0, search_motion: Toggle::new(false) }
    }
}

impl InputController {
    pub fn set_text(&mut self, text: String) {
        self.search_box_input = text;
        self.search_box_cursor = self.search_box_input.chars().count();
    }

    pub fn insert(&mut self, ch: char) -> bool {
        if self.search_box_input.chars().count() >= super::MAX_INPUT_LEN {
            return false;
        }
        super::insert_char_at(&mut self.search_box_input, self.search_box_cursor, ch);
        self.search_box_cursor += 1;
        true
    }

    pub fn backspace(&mut self) {
        if self.search_box_cursor > 0 {
            self.search_box_cursor =
                super::remove_char_before(&mut self.search_box_input, self.search_box_cursor);
        }
    }

    pub fn delete(&mut self) {
        super::remove_char_at(&mut self.search_box_input, self.search_box_cursor);
    }

    pub fn move_cursor(&mut self, delta: isize) {
        let len = self.search_box_input.chars().count() as isize;
        self.search_box_cursor = (self.search_box_cursor as isize + delta).clamp(0, len) as usize;
    }
}
