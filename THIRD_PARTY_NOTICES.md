# Third-party notices

This repository includes third-party code copied into the source tree.
The notice below documents attribution and licensing for that code.

## ncm-api-rs (crate name: ncm-api)

- Project: ncm-api-rs
- Upstream: https://github.com/imsyy/ncm-api-rs
- Local path in this repository: `ncm-api-rs/`
- Declared upstream license: WTFPL (Version 2)
- Upstream license file: `ncm-api-rs/LICENSE`
- Usage in this project: NetEase Cloud Music API client used by CNMPlayer networking features
- Local modification status: copied from upstream source as-is for repository self-containment

### Upstream license text (WTFPL v2)

```text
            DO WHAT THE FUCK YOU WANT TO PUBLIC LICENSE
                    Version 2, December 2004

 Copyright (C) 2004 Sam Hocevar <sam@hocevar.net>

 Everyone is permitted to copy and distribute verbatim or modified
 copies of this license document, and changing it is allowed as long
 as the name is changed.

            DO WHAT THE FUCK YOU WANT TO PUBLIC LICENSE
   TERMS AND CONDITIONS FOR COPYING, DISTRIBUTION AND MODIFICATION

  0. You just DO WHAT THE FUCK YOU WANT TO.
```

## codex (OpenAI codex-cli, Rust TUI)

- Project: codex
- Upstream: https://github.com/openai/codex
- Declared upstream license: Apache License 2.0
- Usage in this project: the vector-mode "settled dust" twinkle in `src/tmplayer/render/vector_renderer.rs`
  ports the deterministic starfield formula from the upstream `tui` crate's `sparkle_field.rs`
  (`render_stars`): the two-round `0x45d9f3b` coordinate hash, the 4–7 s per-star period, the phase
  offset and the `sin¹² · 0.55` brightness pulse with the 0.04 extinguish threshold.
- Local modification status: adapted, not copied verbatim — the density gate (`hash % 5`), the
  per-cell glyph selection and the 15 s idle timeout are dropped; the braille raster of this
  project is kept and the pulse is multiplied onto the existing per-particle fade state.

## Notes

- Non-vendored Rust dependencies are consumed through Cargo and are listed in `Cargo.toml` and `Cargo.lock`.
- This file is an attribution summary for repository distribution and is not legal advice.