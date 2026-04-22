//! Dev-only JSONL log of inputs and resulting buffer state.
//!
//! Enabled with `--logfile=PATH` in debug builds. Intended for the
//! feedback loop of "I pressed X and expected Y" — not for crash debugging.

use std::cell::RefCell;
use std::fs::File;
use std::io::{BufWriter, Write as _};
use std::path::Path;
use std::time::Instant;

use edit::buffer::TextBuffer;
use edit::input::{Input, InputMouseState};

thread_local! {
    static LOG: RefCell<Option<State>> = const { RefCell::new(None) };
}

struct State {
    file: BufWriter<File>,
    start: Instant,
}

pub fn open(path: &Path) -> std::io::Result<()> {
    let file = BufWriter::new(File::create(path)?);
    LOG.with(|l| {
        *l.borrow_mut() = Some(State { file, start: Instant::now() });
    });
    Ok(())
}

pub fn is_enabled() -> bool {
    LOG.with(|l| l.borrow().is_some())
}

/// Describe an input without logging. Call before `draw()` — the returned
/// string is then handed to [`log`] after `draw()` alongside the resulting
/// buffer snapshot.
pub fn describe(input: &Input<'_>) -> String {
    match input {
        Input::Resize(sz) => {
            format!(r#"{{"kind":"resize","w":{},"h":{}}}"#, sz.width, sz.height)
        }
        Input::Text(s) => {
            format!(r#"{{"kind":"text","text":{}}}"#, json_str(s))
        }
        Input::Paste(b) => {
            let preview = String::from_utf8_lossy(b);
            let preview: String = preview.chars().take(40).collect();
            format!(r#"{{"kind":"paste","bytes":{},"preview":{}}}"#, b.len(), json_str(&preview))
        }
        Input::Keyboard(k) => {
            format!(r#"{{"kind":"key","key":"{k}"}}"#)
        }
        Input::Mouse(m) => {
            let state = match m.state {
                InputMouseState::None => "none",
                InputMouseState::Left => "left",
                InputMouseState::Middle => "middle",
                InputMouseState::Right => "right",
                InputMouseState::Release => "release",
                InputMouseState::Scroll => "scroll",
            };
            format!(
                r#"{{"kind":"mouse","state":"{state}","mods":"{}","pos":[{},{}],"scroll":[{},{}]}}"#,
                m.modifiers, m.position.x, m.position.y, m.scroll.x, m.scroll.y,
            )
        }
    }
}

pub fn log(input_desc: &str, buffer: Option<&TextBuffer>) {
    LOG.with(|l| {
        let mut slot = l.borrow_mut();
        let Some(state) = slot.as_mut() else { return };
        let ts = state.start.elapsed().as_millis();
        let buf_json = buffer.map(render_buffer).unwrap_or_else(|| "null".into());
        let _ =
            writeln!(state.file, r#"{{"ts_ms":{ts},"input":{input_desc},"buffer":{buf_json}}}"#);
        let _ = state.file.flush();
    });
}

fn render_buffer(tb: &TextBuffer) -> String {
    let logical = tb.cursor_logical_pos();
    let visual = tb.cursor_visual_pos();
    let offset = tb.cursor_offset();
    let preferred = tb.preferred_column();
    let dirty = tb.is_dirty();
    let lines = tb.logical_line_count();
    let sel = match tb.selection_range() {
        Some((a, b)) => format!(
            r#"[[{},{}],[{},{}]]"#,
            a.logical_pos.x, a.logical_pos.y, b.logical_pos.x, b.logical_pos.y
        ),
        None => "null".into(),
    };
    format!(
        r#"{{"cursor":[{},{}],"visual":[{},{}],"offset":{offset},"preferred_col":{preferred},"selection":{sel},"dirty":{dirty},"lines":{lines}}}"#,
        logical.x, logical.y, visual.x, visual.y,
    )
}

fn json_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for ch in s.chars() {
        match ch {
            '"' => out.push_str(r#"\""#),
            '\\' => out.push_str(r"\\"),
            '\n' => out.push_str(r"\n"),
            '\r' => out.push_str(r"\r"),
            '\t' => out.push_str(r"\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!(r"\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}
