use std::time::Duration;

/// Fixed Nerd Font icon set used by the application.
///
/// Terminal protocols do not expose a reliable glyph-availability query. A
/// terminal can accept a private-use codepoint yet render a tofu box, so the
/// application does not guess from `TERM` and does not silently change layout.
/// Users who need a fallback must provide a Nerd Font or use a terminal/font
/// configuration that supports these glyphs.
#[derive(Debug, Clone, Copy, Default)]
pub struct UiIcons;

impl UiIcons {
    pub const fn new() -> Self {
        Self
    }

    pub const fn previous(self) -> &'static str {
        "[\u{f048}]"
    }

    pub const fn next(self) -> &'static str {
        "[\u{f051}]"
    }

    pub const fn play_pause(self, playing: bool) -> &'static str {
        if playing { "[\u{f04c}]" } else { "[\u{f04b}]" }
    }

    pub const fn heart(self, liked: bool) -> &'static str {
        if liked { "\u{f004}" } else { "\u{f08a}" }
    }

    pub const fn sequence(self) -> &'static str {
        "\u{f08f}"
    }

    pub const fn shuffle(self) -> &'static str {
        "\u{f074}"
    }

    pub const fn loop_all(self) -> &'static str {
        "\u{f0b6}"
    }

    pub const fn loop_one(self) -> &'static str {
        "\u{f01e}"
    }

    pub const fn download(self) -> char {
        '\u{ec74}'
    }

    pub const fn downloaded(self) -> char {
        '\u{f00c}'
    }

    pub fn downloading(self, phase: Duration) -> char {
        const FRAMES: [char; 10] = [
            '\u{280b}', '\u{2819}', '\u{2839}', '\u{2838}', '\u{283c}', '\u{2834}', '\u{2826}',
            '\u{2827}', '\u{2807}', '\u{280f}',
        ];
        let tick = phase.as_millis() / 100;
        FRAMES[(tick % FRAMES.len() as u128) as usize]
    }
}

#[cfg(test)]
mod tests;
