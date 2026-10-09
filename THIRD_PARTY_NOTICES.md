# Third-party notices

CNMPlayer includes vendored source, adaptations of third-party algorithms and Cargo dependencies.
The notices below preserve attribution and licensing for the listed components in source and binary
distributions. This file accompanies the executable and CNMPlayer's `LICENSE` in release archives;
the component licenses below are not replaced by CNMPlayer's AGPL-3.0-only license.

## ncm-api-rs (crate name: ncm-api)

- Project: ncm-api-rs
- Upstream: https://github.com/imsyy/ncm-api-rs
- Local path in this repository: `ncm-api-rs/`
- Declared upstream license: WTFPL (Version 2)
- License text in this repository: `ncm-api-rs/LICENSE`
- Usage in this project: NetEase Cloud Music API client used by CNMPlayer networking features
- Local modification status: vendored source includes CNMPlayer maintenance changes; the local license file preserves the upstream WTFPL text

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
  offset and the `sin¹² · 0.55` brightness pulse.
- Local modification status: adapted, not copied verbatim — the density gate (`hash % 5`), the
  per-cell glyph selection, the 15 s idle timeout and the 0.04 extinguish threshold are dropped
  (brightness dims continuously and the dot is never dropped); the braille raster of this
  project is kept and the pulse is multiplied onto the existing per-particle fade state.

## Cava (internal Rust spectrum port and ScopeGain derivation)

- Upstream: https://github.com/karlstav/cava
- Reference commit: [`6d43df3b2c7882122585c02c064b20009842a6f8`](https://github.com/karlstav/cava/tree/6d43df3b2c7882122585c02c064b20009842a6f8)
- Upstream license: MIT; copyright (c) 2015 Karl Stavestrand in `LICENSE`, and copyright (c) 2022
  Karl Stavestrand in `cavacore.h`. Both original notices and their full terms are preserved below.
- Usage: `src/tmplayer/audio/spectrum.rs` and its core implementation faithfully port the spectrum
  algorithm from `cavacore.c` / `cavacore.h`, with the relevant output processing from `cava.c`.
  This serves fullscreen bars and the collapsed mini spectrum using CNMPlayer's playback PCM.
- Port/adaptation scope: FFT window sizes and Hann windows, bass and normal FFT band mapping,
  magnitude accumulation and scaling, autosensitivity, frame-skip/framerate accounting, falloff
  and integral smoothing, and the applicable channel/output processing. The PCM transport,
  sample-rate/Nyquist adaptation, elapsed-time pause silence, lifecycle and terminal rendering
  are integrated into CNMPlayer rather than implementing Cava's external input/output protocol.
- The oscilloscope's `ScopeGain` in `src/tmplayer/app/scope.rs` retains an integral-inspired
  attack derived from Cava's `process [smoothing]`, adapted to elapsed time and a normalized
  global gain. Its release uses CNMPlayer's existing EaseInOut curve for pause, stop and stale
  PCM; it is not the spectrum core or a verbatim copy of the upstream implementation.
- The FFT backend is RealFFT/RustFFT, not FFTW. No FFTW source or library is included by this port.
  Faithfulness refers to Cava's algorithm semantics, not bitwise identity with FFTW floating-point
  results. Cava's system audio capture, configuration parser and external process are not imported.

### Cava upstream `LICENSE`

Source: https://github.com/karlstav/cava/blob/6d43df3b2c7882122585c02c064b20009842a6f8/LICENSE

```text
Copyright (c) 2015 Karl Stavestrand <karl@stavestrand.no>

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

### Cava core upstream `cavacore.h` notice

Source: https://github.com/karlstav/cava/blob/6d43df3b2c7882122585c02c064b20009842a6f8/cavacore.h

```text
Copyright (c) 2022 Karl Stavestrand <karl@stavestrand.no>

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

## RealFFT (`realfft` 3.5)

- Upstream: https://github.com/HEnquist/realfft
- Package metadata: https://docs.rs/crate/realfft/3.5.0/source/Cargo.toml.orig
- Declared license: MIT
- Author attribution from upstream package metadata: HEnquist <henrik.enquist@gmail.com>
- Usage: real-to-complex FFT backend for the internal Cava spectrum port, built on RustFFT.
- The upstream 3.5.0 package declares MIT but supplies no standalone license file or copyright
  notice. No unprovided copyright year is asserted here. The complete standard MIT permission
  and warranty terms are reproduced below, with the upstream author attribution above retained.
  Standard text reference: https://spdx.org/licenses/MIT.html

### MIT terms

```text
Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

## RustFFT (`rustfft` 6.4)

- Upstream: https://github.com/ejmahler/RustFFT
- Upstream offers `MIT OR Apache-2.0`; CNMPlayer distributes this dependency under the MIT option.
- Usage: pure Rust FFT implementation used through RealFFT for the internal Cava spectrum port.
- License text source: https://docs.rs/crate/rustfft/6.4.1/source/LICENSE-MIT

### Upstream MIT license text

```text
Copyright (c) 2015 The RustFFT Developers

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

## Notes

- Non-vendored Rust dependencies are consumed through Cargo and are listed in `Cargo.toml` and `Cargo.lock`.
- This file preserves the listed notices; it is not an exhaustive license inventory for every Cargo dependency and is not legal advice.