//! Decodes VT sequences into the [`Input`] vocabulary.
//!
//! The lower half of the two-stage parse: [`crate::vt`] turns bytes into
//! VT tokens, and this turns those tokens into keys, mouse events, and
//! text. Bracketed paste and X10 mouse fall-back buffering live here
//! because both need to hold state across tokens.

use std::mem;

use super::*;
use crate::helpers::{CoordType, Point, Size};
use crate::vt;

/// Parses VT sequences into input events.
pub struct Parser {
    bracketed_paste: bool,
    bracketed_paste_buf: Vec<u8>,
    x10_mouse_want: bool,
    x10_mouse_buf: [char; 3],
    x10_mouse_len: usize,
}

impl Parser {
    /// Creates a new parser that turns VT sequences into input events.
    ///
    /// Keep the instance alive for the lifetime of the input stream.
    pub fn new() -> Self {
        Self {
            bracketed_paste: false,
            bracketed_paste_buf: Vec::new(),
            x10_mouse_want: false,
            x10_mouse_buf: ['\0'; 3],
            x10_mouse_len: 0,
        }
    }

    /// Takes an [`vt::Stream`] and returns a [`Stream`]
    /// that turns VT sequences into input events.
    pub fn parse<'parser, 'vt, 'input>(
        &'parser mut self,
        stream: vt::Stream<'vt, 'input>,
    ) -> Stream<'parser, 'vt, 'input> {
        Stream { parser: self, stream }
    }
}

/// An iterator that parses VT sequences into input events.
pub struct Stream<'parser, 'vt, 'input> {
    parser: &'parser mut Parser,
    stream: vt::Stream<'vt, 'input>,
}

impl<'input> Iterator for Stream<'_, '_, 'input> {
    type Item = Input<'input>;

    fn next(&mut self) -> Option<Input<'input>> {
        loop {
            if self.parser.bracketed_paste {
                return self.handle_bracketed_paste();
            }

            if self.parser.x10_mouse_want {
                return self.parse_x10_mouse_coordinates();
            }

            const KEYPAD_LUT: [u8; 8] = [
                vk::UP.value() as u8,    // A
                vk::DOWN.value() as u8,  // B
                vk::RIGHT.value() as u8, // C
                vk::LEFT.value() as u8,  // D
                0,                       // E
                vk::END.value() as u8,   // F
                0,                       // G
                vk::HOME.value() as u8,  // H
            ];

            match self.stream.next()? {
                vt::Token::Text(text) => {
                    return Some(Input::Text(text));
                }
                vt::Token::Ctrl(ch) => match ch {
                    '\0' | '\t' | '\r' => return Some(Input::Keyboard(InputKey::new(ch as u32))),
                    '\n' => return Some(Input::Keyboard(kbmod::CTRL | vk::RETURN)),
                    ..='\x1a' => {
                        // Shift control code to A-Z
                        let key = ch as u32 | 0x40;
                        return Some(Input::Keyboard(kbmod::CTRL | InputKey::new(key)));
                    }
                    '\x7f' => return Some(Input::Keyboard(vk::BACK)),
                    _ => {}
                },
                vt::Token::Esc(ch) => {
                    match ch {
                        '\0' => return Some(Input::Keyboard(vk::ESCAPE)),
                        '\n' => return Some(Input::Keyboard(kbmod::CTRL_ALT | vk::RETURN)),
                        // ESC + DEL: terminals send this for Alt+Backspace.
                        '\x7f' => return Some(Input::Keyboard(kbmod::ALT | vk::BACK)),
                        ' '..='~' => {
                            let ch = ch as u32;
                            let key = ch & !0x20; // Shift a-z to A-Z
                            let modifiers =
                                if (ch & 0x20) != 0 { kbmod::ALT } else { kbmod::ALT_SHIFT };
                            return Some(Input::Keyboard(modifiers | InputKey::new(key)));
                        }
                        _ => {}
                    }
                }
                vt::Token::SS3(ch) => match ch {
                    'A'..='H' => {
                        let vk = KEYPAD_LUT[ch as usize - 'A' as usize];
                        if vk != 0 {
                            return Some(Input::Keyboard(InputKey::new(vk as u32)));
                        }
                    }
                    'P'..='S' => {
                        let key = vk::F1.value() + ch as u32 - 'P' as u32;
                        return Some(Input::Keyboard(InputKey::new(key)));
                    }
                    _ => {}
                },
                vt::Token::Csi(csi) => {
                    match csi.final_byte {
                        'A'..='H' => {
                            let vk = KEYPAD_LUT[csi.final_byte as usize - 'A' as usize];
                            if vk != 0 {
                                return Some(Input::Keyboard(
                                    InputKey::new(vk as u32) | Self::parse_modifiers(csi),
                                ));
                            }
                        }
                        'Z' => return Some(Input::Keyboard(kbmod::SHIFT | vk::TAB)),
                        '~' => {
                            const LUT: [u8; 35] = [
                                0,
                                vk::HOME.value() as u8,   // 1
                                vk::INSERT.value() as u8, // 2
                                vk::DELETE.value() as u8, // 3
                                vk::END.value() as u8,    // 4
                                vk::PRIOR.value() as u8,  // 5
                                vk::NEXT.value() as u8,   // 6
                                0,
                                0,
                                0,
                                0,
                                0,
                                0,
                                0,
                                0,
                                vk::F5.value() as u8, // 15
                                0,
                                vk::F6.value() as u8,  // 17
                                vk::F7.value() as u8,  // 18
                                vk::F8.value() as u8,  // 19
                                vk::F9.value() as u8,  // 20
                                vk::F10.value() as u8, // 21
                                0,
                                vk::F11.value() as u8, // 23
                                vk::F12.value() as u8, // 24
                                vk::F13.value() as u8, // 25
                                vk::F14.value() as u8, // 26
                                0,
                                vk::F15.value() as u8, // 28
                                vk::F16.value() as u8, // 29
                                0,
                                vk::F17.value() as u8, // 31
                                vk::F18.value() as u8, // 32
                                vk::F19.value() as u8, // 33
                                vk::F20.value() as u8, // 34
                            ];
                            const LUT_LEN: u16 = LUT.len() as u16;

                            match csi.params[0] {
                                0..LUT_LEN => {
                                    let vk = LUT[csi.params[0] as usize];
                                    if vk != 0 {
                                        return Some(Input::Keyboard(
                                            InputKey::new(vk as u32) | Self::parse_modifiers(csi),
                                        ));
                                    }
                                }
                                200 => self.parser.bracketed_paste = true,
                                _ => {}
                            }
                        }
                        'u' => {
                            // Kitty keyboard protocol CSI-u encoding:
                            //   CSI <codepoint> ; <modifier> u
                            // Triggered under flag 1 for modified control combos
                            // (Ctrl+letter, Alt+letter, Cmd+anything, etc.).
                            let code = csi.params[0] as u32;
                            let vk = match code {
                                // Backspace codepoint → our BACK vk.
                                127 => 0x08,
                                // Lowercase ASCII letter → uppercase vk code.
                                c if (b'a' as u32..=b'z' as u32).contains(&c) => c - 0x20,
                                c => c,
                            };
                            // InputKey value occupies lower 24 bits; reject anything else
                            // (functional-key codepoints in the ≥57344 range etc.).
                            if vk != 0 && vk < 0x01000000 {
                                return Some(Input::Keyboard(
                                    InputKey::new(vk) | Self::parse_modifiers(csi),
                                ));
                            }
                        }
                        'm' | 'M' if csi.private_byte == '<' => {
                            let btn = csi.params[0];
                            let mut mouse = InputMouse {
                                state: InputMouseState::None,
                                modifiers: kbmod::NONE,
                                position: Default::default(),
                                scroll: Default::default(),
                            };

                            mouse.state = InputMouseState::None;

                            // SGR mouse btn carries Shift (0x04), Alt (0x08),
                            // and Ctrl (0x10) bits OR'd into params[0]. Strip
                            // them before matching the action so e.g. Shift+
                            // Left-click (btn=4) still resolves to Left, not
                            // a no-op state. Bit 0x20 is the motion flag --
                            // drag events arrive as btn = 32 + button, with
                            // final='M'. We treat motion-with-button as the
                            // same state as the press, so the higher level
                            // sees a continuous Left/Middle/Right stream.
                            let action_btn = csi.params[0] & !0x1c;
                            match action_btn {
                                btn @ 0..3 if csi.final_byte == 'M' => match btn {
                                    0 => mouse.state = InputMouseState::Left,
                                    1 => mouse.state = InputMouseState::Middle,
                                    2 => mouse.state = InputMouseState::Right,
                                    _ => {}
                                },
                                btn @ 32..35 if csi.final_byte == 'M' => match btn - 32 {
                                    0 => mouse.state = InputMouseState::Left,
                                    1 => mouse.state = InputMouseState::Middle,
                                    2 => mouse.state = InputMouseState::Right,
                                    _ => {}
                                },
                                btn @ 64..68 => {
                                    let delta = if (btn & 1) != 0 { 3 } else { -3 };
                                    let idx = if (btn & 2) != 0 { 0 } else { 1 };
                                    mouse.scroll.as_array()[idx] += delta;
                                    mouse.state = InputMouseState::Scroll;
                                }
                                _ => {}
                            }

                            mouse.modifiers = kbmod::NONE;
                            mouse.modifiers |=
                                if (btn & 0x04) != 0 { kbmod::SHIFT } else { kbmod::NONE };
                            mouse.modifiers |=
                                if (btn & 0x08) != 0 { kbmod::ALT } else { kbmod::NONE };
                            mouse.modifiers |=
                                if (btn & 0x10) != 0 { kbmod::CTRL } else { kbmod::NONE };

                            mouse.position.x = csi.params[1] as CoordType - 1;
                            mouse.position.y = csi.params[2] as CoordType - 1;
                            return Some(Input::Mouse(mouse));
                        }
                        'M' if csi.param_count == 0 => {
                            self.parser.x10_mouse_want = true;
                        }
                        't' if csi.params[0] == 8 => {
                            // Window Size
                            let width = (csi.params[2] as CoordType).clamp(1, 32767);
                            let height = (csi.params[1] as CoordType).clamp(1, 32767);
                            return Some(Input::Resize(Size { width, height }));
                        }
                        _ => {}
                    }
                }
                _ => {}
            }
        }
    }
}

impl<'input> Stream<'_, '_, 'input> {
    /// Once we encounter the start of a bracketed paste
    /// we seek to the end of the paste in this function.
    ///
    /// A bracketed paste is basically:
    /// ```text
    /// <ESC>[201~    lots of text    <ESC>[201~
    /// ```
    ///
    /// That in between text is then expected to be taken literally.
    /// It can be in between anything though, including other escape sequences.
    /// This is the reason why this is a separate method.
    #[cold]
    fn handle_bracketed_paste(&mut self) -> Option<Input<'input>> {
        let beg = self.stream.offset();
        let mut end = beg;

        while let Some(token) = self.stream.next() {
            if let vt::Token::Csi(csi) = token
                && csi.final_byte == '~'
                && csi.params[0] == 201
            {
                self.parser.bracketed_paste = false;
                break;
            }
            end = self.stream.offset();
        }

        if end != beg {
            self.parser
                .bracketed_paste_buf
                .extend_from_slice(&self.stream.input().as_bytes()[beg..end]);
        }

        if !self.parser.bracketed_paste {
            Some(Input::Paste(mem::take(&mut self.parser.bracketed_paste_buf)))
        } else {
            None
        }
    }

    /// Implements the X10 mouse protocol via `CSI M CbCxCy`.
    ///
    /// You want to send numeric mouse coordinates.
    /// You have CSI sequences with numeric parameters.
    /// So, of course you put the coordinates as shifted ASCII characters after
    /// the end of the sequence. Limited coordinate range and complicated parsing!
    /// This is so puzzling to me. The existence of this function makes me unhappy.
    #[cold]
    fn parse_x10_mouse_coordinates(&mut self) -> Option<Input<'input>> {
        while self.parser.x10_mouse_len < 3 && !self.stream.done() {
            self.parser.x10_mouse_buf[self.parser.x10_mouse_len] = self.stream.next_char();
            self.parser.x10_mouse_len += 1;
        }
        if self.parser.x10_mouse_len < 3 {
            return None;
        }

        let b = self.parser.x10_mouse_buf[0] as u32;
        let x = self.parser.x10_mouse_buf[1] as CoordType - 0x21;
        let y = self.parser.x10_mouse_buf[2] as CoordType - 0x21;
        let action = match b & 0b11 {
            0 => InputMouseState::Left,
            1 => InputMouseState::Middle,
            2 => InputMouseState::Right,
            _ => InputMouseState::None,
        };
        let modifiers = {
            let mut m = kbmod::NONE;
            if (b & 0b00100) != 0 {
                m |= kbmod::SHIFT;
            }
            if (b & 0b01000) != 0 {
                m |= kbmod::ALT;
            }
            if (b & 0b10000) != 0 {
                m |= kbmod::CTRL;
            }
            m
        };

        self.parser.x10_mouse_want = false;
        self.parser.x10_mouse_len = 0;

        Some(Input::Mouse(InputMouse {
            state: action,
            modifiers,
            position: Point { x, y },
            scroll: Default::default(),
        }))
    }

    fn parse_modifiers(csi: &vt::Csi) -> InputKeyMod {
        let mut modifiers = kbmod::NONE;
        let p1 = csi.params[1].saturating_sub(1);
        if (p1 & 0x01) != 0 {
            modifiers |= kbmod::SHIFT;
        }
        if (p1 & 0x02) != 0 {
            modifiers |= kbmod::ALT;
        }
        if (p1 & 0x04) != 0 {
            modifiers |= kbmod::CTRL;
        }
        // Bit 3 is Super in the kitty keyboard protocol — on macOS that's Cmd.
        if (p1 & 0x08) != 0 {
            modifiers |= kbmod::CMD;
        }
        modifiers
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vt;

    fn parse_one(bytes: &str) -> Option<Input<'_>> {
        let mut vt_parser = vt::Parser::new();
        let mut in_parser = Parser::new();
        let stream = vt_parser.parse(bytes);
        in_parser.parse(stream).next()
    }

    fn key(input: Option<Input<'_>>) -> InputKey {
        match input {
            Some(Input::Keyboard(k)) => k,
            _ => panic!("expected keyboard input"),
        }
    }

    #[test]
    fn bare_del_decodes_as_backspace() {
        assert_eq!(key(parse_one("\x7f")), vk::BACK);
    }

    #[test]
    fn esc_del_decodes_as_alt_backspace() {
        assert_eq!(key(parse_one("\x1b\x7f")), kbmod::ALT | vk::BACK);
    }

    #[test]
    fn esc_letter_decodes_as_alt_letter() {
        assert_eq!(key(parse_one("\x1bb")), kbmod::ALT | vk::B);
    }

    #[test]
    fn kitty_csi_u_backspace_with_alt() {
        // CSI 127 ; 3 u  -> Alt+Backspace under the kitty keyboard protocol.
        assert_eq!(key(parse_one("\x1b[127;3u")), kbmod::ALT | vk::BACK);
    }

    #[test]
    fn kitty_csi_u_letter_with_cmd() {
        // CSI 99 ; 9 u  -> Cmd+C (modifier 9 - 1 = 8 = Super bit).
        assert_eq!(key(parse_one("\x1b[99;9u")), kbmod::CMD | vk::C);
    }

    #[test]
    fn sgr_mouse_drag_resolves_as_held_button() {
        // SGR motion-with-button: btn = 32 + button. Cell-motion tracking
        // (mode 1002) reports drag this way. Must resolve to the held
        // button's state so drag-select works.
        let m = match parse_one("\x1b[<32;10;5M") {
            Some(Input::Mouse(m)) => m,
            _ => panic!("expected mouse input"),
        };
        assert!(matches!(m.state, InputMouseState::Left));
    }

    #[test]
    fn sgr_mouse_shift_left_click_resolves_as_left_with_shift() {
        // SGR mouse press: CSI < <btn> ; <x> ; <y> M
        // Shift bit (0x04) OR'd with Left (0) -> btn = 4. Must still
        // resolve to Left state with SHIFT modifier set.
        let m = match parse_one("\x1b[<4;10;5M") {
            Some(Input::Mouse(m)) => m,
            _ => panic!("expected mouse input"),
        };
        assert!(matches!(m.state, InputMouseState::Left));
        assert!(m.modifiers.contains(kbmod::SHIFT));
    }
}
