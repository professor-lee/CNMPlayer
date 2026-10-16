use crate::data::config::Language;
use unicode_width::UnicodeWidthChar;

pub(super) const MAX_INPUT_LEN: usize = 64;

pub(super) fn truncate_text(text: &str, max_chars: usize) -> String {
    let mut out = String::new();
    for (index, ch) in text.chars().enumerate() {
        if index >= max_chars {
            out.push_str("...");
            return out;
        }
        out.push(ch);
    }
    out
}

pub(super) fn char_count(text: &str) -> usize {
    text.chars().count()
}

pub(super) fn byte_index_for_char(text: &str, char_index: usize) -> usize {
    if char_index == 0 {
        return 0;
    }

    text.char_indices()
        .nth(char_index)
        .map(|(idx, _)| idx)
        .unwrap_or(text.len())
}

pub(super) fn insert_char_at(text: &mut String, char_index: usize, ch: char) {
    let byte_index = byte_index_for_char(text, char_index);
    text.insert(byte_index, ch);
}

pub(super) fn remove_char_before(text: &mut String, char_index: usize) -> usize {
    if char_index == 0 {
        return 0;
    }

    let start = byte_index_for_char(text, char_index - 1);
    let end = byte_index_for_char(text, char_index);
    if start < end && end <= text.len() {
        text.drain(start..end);
    }
    char_index.saturating_sub(1)
}

pub(super) fn remove_char_at(text: &mut String, char_index: usize) {
    let start = byte_index_for_char(text, char_index);
    let end = byte_index_for_char(text, char_index + 1);
    if start < end && end <= text.len() {
        text.drain(start..end);
    }
}

pub(super) fn char_index_for_display_column(text: &str, column: u16) -> usize {
    let mut width = 0usize;
    let target = column as usize;
    let mut index = 0usize;

    for ch in text.chars() {
        let ch_width = ch.width().unwrap_or(1).max(1);
        if width + ch_width > target {
            break;
        }
        width += ch_width;
        index += 1;
    }

    index
}

/// 与 `App::lang_text` 同一套文案选择，供拿不到 `App` 的请求函数使用。
pub(super) fn lang_text<'a>(lang: Language, zh: &'a str, en: &'a str) -> &'a str {
    match lang {
        Language::Zh => zh,
        Language::En => en,
    }
}
