//! Terminal setup + probing. Switch alt-screen + mouse + bracketed paste
//! modes, push kitty kbd proto flag 1, query OSC 4/10/11 palette, probe
//! ambiguous-width via DSR. Returns the captured palette + width as
//! [`TerminalProbe`]; restore happens via the [`RestoreModes`] drop guard.
//!
//! Hoisted out of `bin/edit/main.rs::setup_terminal` so external mount
//! callers (eat's alt-screen modes, any future embedder of edit's tui) get
//! the same orchestration without duplicating the escape-sequence wiring.
//!
//! The caller decides what to do with the probe result (apply the palette,
//! reflow buffers for ambiguous-width change, etc.) -- this module only
//! emits + parses.

use std::time::Duration;

use stdext::arena::scratch_arena;

use crate::framebuffer::{INDEXED_COLORS_COUNT, IndexedColor};
use crate::helpers::CoordType;
use crate::oklab::StraightRgba;
use crate::{sys, vt};

/// Result of [`setup`]: terminal-reported palette + ambiguous-width measurement.
pub struct TerminalProbe {
    pub indexed_colors: [StraightRgba; INDEXED_COLORS_COUNT],
    pub ambiguous_width: CoordType,
}

/// RAII guard. Dropping it emits the inverse of the mode-switch sequences
/// (alt-screen leave, mouse off, bracketed paste off, kitty kbd pop, cursor
/// show, default cursor style, clear title).
pub struct RestoreModes;

impl Drop for RestoreModes {
    fn drop(&mut self) {
        // Reverse order of the mode-set in `setup`. We deliberately do NOT
        // reset 1036 (meta-sends-escape) because most apps expect it on.
        // `CSI < u` pops the kitty kbd proto flags we pushed.
        sys::write_stdout("\x1b[<u\x1b[0 q\x1b[?25h\x1b]0;\x07\x1b[?1002;1006;2004l\x1b[?1049l");
    }
}

/// Emit setup + probe sequences, then read terminal responses until CSI c
/// (terminal capabilities reply) closes the round-trip. `fallback_palette`
/// seeds the returned palette; OSC 4/10/11 responses overwrite individual
/// slots. Slots the terminal doesn't report stay at the fallback value.
pub fn setup(
    vt_parser: &mut vt::Parser,
    fallback_palette: [StraightRgba; INDEXED_COLORS_COUNT],
) -> (TerminalProbe, RestoreModes) {
    sys::write_stdout(concat!(
        // 1049: Alternative Screen Buffer
        // 1002: Cell Motion Mouse Tracking
        // 1006: SGR Mouse Mode
        // 2004: Bracketed Paste Mode
        // 1036: meta-sends-escape (Alt -> ESC+char)
        "\x1b[?1049h\x1b[?1002;1006;2004h\x1b[?1036h",
        // Kitty keyboard protocol: push flag 1 for disambiguated escape codes.
        // Unsupporting terminals silently ignore.
        "\x1b[>1u",
        // OSC 4 palette queries for indices 0..16.
        "\x1b]4;0;?;1;?;2;?;3;?;4;?;5;?;6;?;7;?\x07",
        "\x1b]4;8;?;9;?;10;?;11;?;12;?;13;?;14;?;15;?\x07",
        // OSC 10/11 for default fg/bg.
        "\x1b]10;?\x07\x1b]11;?\x07",
        // Ambiguous-width probe: write "…" and ask for cursor position.
        // Windows conhost (old) measures by display width, not wcwidth.
        "\r…\x1b[6n",
        // CSI c: device attributes. All terminals reply; we use it to
        // bound the read loop since not every terminal supports the OSC
        // queries above.
        "\x1b[c",
    ));

    let mut indexed_colors = fallback_palette;
    let mut ambiguous_width = 1;
    let mut done = false;
    let mut osc_buffer = String::new();

    while !done {
        let scratch = scratch_arena(None);

        // High read timeout: a lone ESC at this point almost certainly
        // came from a VT response, not a keypress.
        let Some(input) = sys::read_stdin(&scratch, Duration::from_secs(3)) else {
            break;
        };

        let mut vt_stream = vt_parser.parse(&input);
        while let Some(token) = vt_stream.next() {
            match token {
                vt::Token::Csi(csi) => match csi.final_byte {
                    'c' => done = true,
                    // CPR (Cursor Position Report) response.
                    'R' => ambiguous_width = csi.params[1] as CoordType - 1,
                    _ => {}
                },
                vt::Token::Osc { mut data, partial } => {
                    if partial {
                        osc_buffer.push_str(data);
                        continue;
                    }
                    if !osc_buffer.is_empty() {
                        osc_buffer.push_str(data);
                        data = &osc_buffer;
                    }

                    let mut splits = data.split_terminator(';');

                    let color = match splits.next().unwrap_or("") {
                        // `4;<color>;rgb:<r>/<g>/<b>`
                        "4" => match splits.next().unwrap_or("").parse::<usize>() {
                            Ok(val) if val < 16 => &mut indexed_colors[val],
                            _ => continue,
                        },
                        // `10;rgb:<r>/<g>/<b>`
                        "10" => &mut indexed_colors[IndexedColor::Foreground as usize],
                        // `11;rgb:<r>/<g>/<b>`
                        "11" => &mut indexed_colors[IndexedColor::Background as usize],
                        _ => continue,
                    };

                    let color_param = splits.next().unwrap_or("");
                    if !color_param.starts_with("rgb:") {
                        continue;
                    }

                    let mut iter = color_param[4..].split_terminator('/');
                    let rgb_parts = [(); 3].map(|_| iter.next().unwrap_or("0"));
                    let mut rgb = 0;

                    for part in rgb_parts {
                        if part.len() == 2 || part.len() == 4 {
                            let Ok(mut val) = usize::from_str_radix(part, 16) else {
                                continue;
                            };
                            if part.len() == 4 {
                                // Round from 16 bits to 8 bits.
                                val = (val * 0xff + 0x7fff) / 0xffff;
                            }
                            rgb = (rgb >> 8) | ((val as u32) << 16);
                        }
                    }

                    *color = StraightRgba::from_le(rgb | 0xff000000);
                    osc_buffer.clear();
                }
                _ => {}
            }
        }
    }

    (TerminalProbe { indexed_colors, ambiguous_width }, RestoreModes)
}
