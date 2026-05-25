//! Thin external-mount surface for edit's [`Tui`]. Bundles the boilerplate
//! that every embedder of edit's tui repeats: [`Tui::new`], the
//! [`term::setup`] probe, the input/render loop, the alt-screen restore.
//!
//! The editor's own `bin/edit/main.rs` does **not** use this -- it has a
//! richer loop with gutter refresh / minimap / disk-fingerprint / language
//! redetect / OSC clipboard / terminal title. [`mount`] is purely additive
//! for external callers (eat's snapshot+follow modes, future embedders).
//!
//! ## Caller responsibilities
//!
//! Before calling [`mount`]:
//!
//! - `stdext::arena::init(...)` -- scratch arenas are single-init; the
//!   caller picks the capacity.
//! - `edit::sys::init()` -- raw mode + signal wiring. Hold the returned
//!   `Deinit` for the lifetime of the mount.
//! - `edit::sys::switch_modes()` -- enable raw-mode keypress reads.
//! - Optional: install a panic hook that drops [`term::RestoreModes`] and
//!   `sys::Deinit` so an unwinding panic restores the terminal before the
//!   stack trace prints. Required only in debug builds; release uses
//!   `panic=abort` which skips drops anyway. See `bin/edit/main.rs` for
//!   the canonical pattern.
//!
//! [`mount`] returns when the draw callback returns
//! [`ControlFlow::Break`] or stdin closes.

use std::io;
use std::ops::ControlFlow;
use std::time::Duration;

use stdext::arena::scratch_arena;

use crate::framebuffer::{DEFAULT_THEME, INDEXED_COLORS_COUNT};
use crate::oklab::StraightRgba;
use crate::tui::{Context, Tui};
use crate::{input, sys, term, vt};

/// Knobs for [`mount`]. Defaults match the editor's own setup.
pub struct MountOpts {
    /// Seeds the probe's palette. OSC 4/10/11 responses overwrite slots
    /// the terminal reports; unreported slots stay at the fallback.
    pub fallback_palette: [StraightRgba; INDEXED_COLORS_COUNT],
    /// Forwarded to [`Tui::setup_emit_indexed_codes`] after the probe.
    /// On (default) lets terminals which drop OSC 4 (e.g. tmux) still
    /// render via their own palette; off forces exact RGB everywhere.
    pub emit_indexed_codes: bool,
    /// If set, caps the input read timeout so the draw callback fires at
    /// least every `tick_interval` even with no user input. Use for periodic
    /// refreshes (clock, disk-change poll) that can't be input-driven.
    /// `None` (default) means: block on input indefinitely (or until the
    /// vt parser / tui animation requests a shorter timeout).
    pub tick_interval: Option<Duration>,
}

impl Default for MountOpts {
    fn default() -> Self {
        Self { fallback_palette: DEFAULT_THEME, emit_indexed_codes: true, tick_interval: None }
    }
}

/// Mount edit's [`Tui`] and run the input/render loop until `draw` returns
/// [`ControlFlow::Break`] or stdin closes.
///
/// `draw` is invoked once per input event and once per settle pass. It
/// composes the node tree (typically a single [`Context::textarea`] +
/// chrome) and signals exit via the [`ControlFlow`] return.
pub fn mount<F>(opts: MountOpts, mut draw: F) -> io::Result<()>
where
    F: FnMut(&mut Context) -> ControlFlow<()>,
{
    let mut tui = Tui::new()?;
    let mut vt_parser = vt::Parser::new();
    let mut input_parser = input::Parser::new();

    let (probe, _restore) = term::setup(&mut vt_parser, opts.fallback_palette);
    tui.setup_indexed_colors(probe.indexed_colors);
    if opts.emit_indexed_codes {
        tui.setup_emit_indexed_codes(true);
    }

    sys::inject_window_size_into_stdin();

    let mut exit = false;
    while !exit {
        {
            let scratch = scratch_arena(None);
            let mut timeout = vt_parser.read_timeout().min(tui.read_timeout());
            if let Some(tick) = opts.tick_interval {
                timeout = timeout.min(tick);
            }
            let Some(inp) = sys::read_stdin(&scratch, timeout) else {
                break;
            };
            let vt_iter = vt_parser.parse(&inp);
            let mut iter = input_parser.parse(vt_iter);
            while {
                let event = iter.next();
                let more = event.is_some();
                let mut ctx = tui.create_context(event);
                if draw(&mut ctx).is_break() {
                    exit = true;
                }
                more
            } {}
        }

        while tui.needs_settling() {
            let mut ctx = tui.create_context(None);
            let _ = draw(&mut ctx);
        }

        let scratch = scratch_arena(None);
        let out = tui.render(&scratch);
        sys::write_stdout(&out);
    }

    Ok(())
}
