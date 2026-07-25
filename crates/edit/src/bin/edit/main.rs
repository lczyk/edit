mod apperr;
mod cli;
#[cfg(debug_assertions)]
mod devlog;
mod document;
mod draw_editor;
mod draw_menubar;
mod draw_statusbar;
mod keybindings;
mod minimap;
mod modals;
mod settings;
mod state;

use std::borrow::Cow;
use std::time::Duration;
use std::{env, process};

use draw_editor::*;
use draw_menubar::*;
use draw_statusbar::*;
use edit::framebuffer::IndexedColor;
use edit::helpers::*;
use edit::input::{self, vk};
use edit::tui::*;
use edit::vt;
use edit::{base64, sys};
use state::*;
use stdext::arena::{self, Arena, scratch_arena};
use stdext::collections::BString;

use crate::settings::Settings;

#[cfg(target_pointer_width = "32")]
const SCRATCH_ARENA_CAPACITY: usize = 128 * MEBI;
#[cfg(target_pointer_width = "64")]
const SCRATCH_ARENA_CAPACITY: usize = 512 * MEBI;

// NOTE: Before our main() gets called, Rust initializes its stdlib. This pulls in the entire
// std::io::{stdin, stdout, stderr} machinery, and probably some more, which amounts to about 20KB.
// It can technically be avoided nowadays with `#![no_main]`. Maybe a fun project for later? :)
fn main() -> process::ExitCode {
    let argv0 = env::args_os().next();
    let name = argv0
        .as_deref()
        .and_then(|p| std::path::Path::new(p).file_stem())
        .and_then(|s| s.to_str())
        .unwrap_or("edit");

    // --eat wins over --help so `edit --eat --help` prints eat's help.
    // Symlink (name == "eat") also takes priority.
    if name == "eat" {
        return edit::eat::main();
    }
    if env::args_os().any(|a| a == "--eat") {
        // SAFETY: single-threaded at this point in main.
        unsafe { std::env::set_var("EDIT_EAT_VIA_FLAG", "1") };
        return edit::eat::main();
    }

    // --help/-h anywhere in remaining args prints edit's help.
    if env::args_os().any(|a| a == "-h" || a == "--help") {
        cli::print_help();
        return process::ExitCode::SUCCESS;
    }

    if cfg!(debug_assertions) {
        let hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            drop(edit::term::RestoreModes);
            drop(sys::Deinit);
            hook(info);
        }));
    }

    match run() {
        Ok(()) => process::ExitCode::SUCCESS,
        Err(err) => {
            sys::write_stdout(&format!("{}\n", FormatApperr::from(err)));
            process::ExitCode::FAILURE
        }
    }
}

fn run() -> apperr::Result<()> {
    let _sys_deinit = sys::init();
    arena::init(SCRATCH_ARENA_CAPACITY)?;

    let Some(path) = cli::parse_args()? else {
        return Ok(());
    };
    let document = document::Document::open(&path)?;
    let mut state = State::new(document)?;

    if let Err(err) = Settings::reload() {
        state.add_error(err);
    }
    if let Err(err) = keybindings::load_or_create() {
        state.add_error(err);
    }
    // The textarea handles these six itself, so that they also work in a
    // modal's input field. Hand it the configured chords, or rebinding them
    // would relabel the menubar and change nothing else.
    {
        use keybindings::{Action, chord};
        edit::tui::set_textarea_chords(edit::tui::TextareaChords {
            cut: chord(Action::Cut),
            copy: chord(Action::Copy),
            paste: chord(Action::Paste),
            undo: chord(Action::Undo),
            redo: chord(Action::Redo),
            select_all: chord(Action::SelectAll),
        });
    }
    // This will reopen stdin if it's redirected (which may fail) and switch
    // the terminal to raw mode which prevents the user from pressing Ctrl+C.
    // `handle_args` may want to print a help message (must not fail),
    // and reads files (may hang; should be cancellable with Ctrl+C).
    // As such, we call this after `handle_args`.
    sys::switch_modes()?;

    let mut vt_parser = vt::Parser::new();
    let mut input_parser = input::Parser::new();
    let mut tui = Tui::new()?;

    let _restore = {
        // Palette: use the terminal's reported OSC 4 / 10 / 11 colours
        // (so themes the user picked at the terminal level apply
        // naturally), falling back to the baked-in DEFAULT_THEME for
        // any slot the terminal doesn't report. opt into indexed
        // emission so terminals which drop OSC 4 (e.g. tmux) still
        // render via their own palette.
        let (probe, restore) = edit::term::setup(&mut vt_parser, edit::framebuffer::DEFAULT_THEME);
        if probe.ambiguous_width == 2 {
            edit::unicode::setup_ambiguous_width(2);
            state.document.buffer.borrow_mut().reflow();
        }
        tui.setup_indexed_colors(probe.indexed_colors);
        tui.setup_emit_indexed_codes(true);
        restore
    };

    edit::notify::set_handler(state::push_warning);

    state.menubar_color_bg = tui.indexed(IndexedColor::Background).oklab_blend(tui.indexed_alpha(
        IndexedColor::BrightBlue,
        1,
        2,
    ));
    state.menubar_color_fg = tui.contrasted(state.menubar_color_bg);
    let floater_bg = tui
        .indexed_alpha(IndexedColor::Background, 2, 3)
        .oklab_blend(tui.indexed_alpha(IndexedColor::Foreground, 1, 3));
    let floater_fg = tui.contrasted(floater_bg);
    tui.setup_modifier_translations(ModifierTranslations {
        ctrl: "Ctrl",
        alt: "Alt",
        shift: "Shift",
        cmd: "Cmd",
    });
    tui.set_floater_default_bg(floater_bg);
    tui.set_floater_default_fg(floater_fg);
    tui.set_modal_default_bg(floater_bg);
    tui.set_modal_default_fg(floater_fg);

    sys::inject_window_size_into_stdin();

    const GUTTER_REDIFF_DEBOUNCE: Duration = Duration::from_millis(300);
    const MINIMAP_REBUILD_DEBOUNCE: Duration = Duration::from_millis(300);
    const LANGUAGE_REDETECT_DEBOUNCE: Duration = Duration::from_millis(500);
    const DISK_CHECK_INTERVAL: Duration = Duration::from_secs(2);

    loop {
        // Process a batch of input.
        {
            let scratch = scratch_arena(None);
            let read_timeout = vt_parser.read_timeout().min(tui.read_timeout());
            let Some(input) = sys::read_stdin(&scratch, read_timeout) else {
                break;
            };

            let vt_iter = vt_parser.parse(&input);
            let mut input_iter = input_parser.parse(vt_iter);

            while {
                let input = input_iter.next();
                let more = input.is_some();

                #[cfg(debug_assertions)]
                let logged_input =
                    if devlog::is_enabled() { input.as_ref().map(devlog::describe) } else { None };

                let mut ctx = tui.create_context(input);

                draw(&mut ctx, &mut state);

                #[cfg(debug_assertions)]
                if let Some(desc) = logged_input {
                    let snapshot = state.document.buffer.borrow();
                    devlog::log(&desc, Some(&snapshot));
                }

                more
            } {}
        }

        // Continue rendering until the layout has settled.
        // This can take >1 frame, if the input focus is tossed between different controls.
        while tui.needs_settling() {
            let mut ctx = tui.create_context(None);

            draw(&mut ctx, &mut state);
        }

        if state.exit {
            break;
        }

        // Refresh the git-baseline gutter marks if the buffer changed and
        // the debounce window has elapsed. Synchronous; the diff is fast
        // and the git subprocess only runs on first refresh.
        state.document.gutter_check_dirty();
        if state.document.gutter_should_rebuild(GUTTER_REDIFF_DEBOUNCE) {
            state.document.gutter_refresh();
        }

        // Skip width updates until a real size is known -- the initial
        // {0,0} would compute width=0 and hide the rail until next resize.
        if tui.size().width > 0 {
            state.document.set_minimap_target_width(desired_minimap_width(tui.size().width));
        }
        state.document.minimap_check_dirty();
        if state.document.minimap_should_rebuild(MINIMAP_REBUILD_DEBOUNCE) {
            state.document.minimap_refresh();
        }

        state.document.check_disk_fingerprint(DISK_CHECK_INTERVAL);

        // Re-run language auto-detect on plain docs as their content grows.
        // No-op once a language is found or the user picks one explicitly.
        state.document.language_check_dirty();
        if state.document.language_should_redetect(LANGUAGE_REDETECT_DEBOUNCE) {
            state.document.language_redetect();
        }

        // Render the UI and write it to the terminal.
        {
            let scratch = scratch_arena(None);
            let mut output = tui.render(&scratch);

            write_terminal_title(&scratch, &mut output, &mut state);

            if state.osc_clipboard_sync {
                write_osc_clipboard(&scratch, &mut output, &mut tui, &mut state);
            }

            sys::write_stdout(&output);
        }
    }

    Ok(())
}

fn draw(ctx: &mut Context, state: &mut State) {
    handle_global_shortcuts(ctx, state);

    draw_menubar(ctx, state);
    draw_editor(ctx, state);
    draw_statusbar(ctx, state);

    if ctx.clipboard_ref().wants_host_sync() {
        ctx.clipboard_mut().mark_as_synchronized();
        state.osc_clipboard_sync = true;
    }
    modals::raise_errors_if_any(state);
    modals::draw_active(ctx, state);

    if let Some(key) = ctx.keyboard_input()
        && key == vk::F3
    {
        search_execute(ctx, state, SearchAction::Search);
        ctx.needs_rerender();
        ctx.set_input_consumed();
    }
}

/// Lines moved per "small jump" action.
const SMALL_JUMP_LINES: CoordType = 3;

fn handle_global_shortcuts(ctx: &mut Context, state: &mut State) {
    use keybindings::{Action, chord};

    let search_enabled = state.wants_search.kind != StateSearchKind::Disabled;

    if ctx.consume_shortcut(chord(Action::Exit)) {
        state.modal = Some(modals::Modal::ConfirmExit);
    } else if ctx.consume_shortcut(chord(Action::Save)) {
        save_document(ctx, state);
    } else if ctx.consume_shortcut(chord(Action::GoToLine)) {
        state.modal = Some(modals::Modal::GoToLine);
    } else if ctx.consume_shortcut(chord(Action::ToggleColumnGuides)) {
        let mut tb = state.document.buffer.borrow_mut();
        let on = tb.is_column_guides_enabled();
        tb.set_column_guides_enabled(!on);
        ctx.needs_rerender();
    } else if ctx.consume_shortcut(chord(Action::ToggleWordWrap)) {
        // The menubar displayed this chord and the config file accepted it, but
        // nothing ever dispatched it -- word wrap only toggled via the
        // textarea's own hardcoded Alt+Z, which no rebinding could move. Same
        // for the two below. The Alt+Z arm stays as an always-on alias; this
        // runs first, so a configured chord wins.
        let mut tb = state.document.buffer.borrow_mut();
        let on = tb.is_word_wrap_enabled();
        tb.set_word_wrap(!on);
        ctx.needs_rerender();
    } else if ctx.consume_shortcut(chord(Action::FocusStatusbar)) {
        state.wants_statusbar_focus = true;
    } else if ctx.consume_shortcut(chord(Action::OpenAbout)) {
        state.modal = Some(modals::Modal::About);
    } else if search_enabled && ctx.consume_shortcut(chord(Action::Find)) {
        state.wants_search.kind = StateSearchKind::Search;
        state.wants_search.focus = true;
    } else if search_enabled && ctx.consume_shortcut(chord(Action::Replace)) {
        state.wants_search.kind = StateSearchKind::Replace;
        state.wants_search.focus = true;
    } else {
        // Everything above acts on the window; everything in there acts on the
        // document, so it must not fire while a text field owns the keyboard.
        // Consuming unconditionally meant Cmd+Shift+K deleted a document line
        // while the user was typing in the Find box, and the macOS cursor-motion
        // chords (Cmd+arrows, Cmd+Backspace) moved and edited the document from
        // inside every input field in the program.
        if ctx.focus_is_in_text_field() || !handle_document_shortcuts(ctx, state) {
            return;
        }
    }

    ctx.needs_rerender();
}

/// Returns whether a chord was consumed.
fn handle_document_shortcuts(ctx: &mut Context, state: &mut State) -> bool {
    use edit::buffer::MoveLineDirection;
    use keybindings::{Action, chord};

    if ctx.consume_shortcut(chord(Action::MoveLineUp)) {
        state.document.buffer.borrow_mut().move_selected_lines(MoveLineDirection::Up);
    } else if ctx.consume_shortcut(chord(Action::MoveLineDown)) {
        state.document.buffer.borrow_mut().move_selected_lines(MoveLineDirection::Down);
    } else if ctx.consume_shortcut(chord(Action::DeleteLine)) {
        state.document.buffer.borrow_mut().delete_lines();
    } else if ctx.consume_shortcut(chord(Action::ToggleLineComment)) {
        // TODO: when we have a user-facing warning/toast system, surface a
        // "no comment syntax for this file" hint instead of silent noop.
        let lang = state.document.buffer.borrow().language();
        let line_tok = lang.and_then(|l| l.line_comment);
        let block_tok = lang.and_then(|l| l.block_comment);
        if let Some(tok) =
            line_tok.or_else(|| document::fallback_line_comment(&state.document.path))
        {
            state.document.buffer.borrow_mut().toggle_line_comment(tok);
        } else if let Some((open, close)) = block_tok {
            state.document.buffer.borrow_mut().toggle_per_line_block_comment(open, close);
        }
    } else if ctx.consume_shortcut(chord(Action::SmallJumpUpSelect)) {
        edit::buffer::small_jump_select(&mut state.document.buffer.borrow_mut(), -SMALL_JUMP_LINES);
    } else if ctx.consume_shortcut(chord(Action::SmallJumpDownSelect)) {
        edit::buffer::small_jump_select(&mut state.document.buffer.borrow_mut(), SMALL_JUMP_LINES);
    } else if ctx.consume_shortcut(chord(Action::SmallJumpUp)) {
        edit::buffer::small_jump(&mut state.document.buffer.borrow_mut(), -SMALL_JUMP_LINES);
    } else if ctx.consume_shortcut(chord(Action::SmallJumpDown)) {
        edit::buffer::small_jump(&mut state.document.buffer.borrow_mut(), SMALL_JUMP_LINES);
    } else if ctx.consume_shortcut(chord(Action::LineStart)) {
        edit::buffer::smart_line_start(&mut state.document.buffer.borrow_mut(), false);
    } else if ctx.consume_shortcut(chord(Action::LineEnd)) {
        edit::buffer::line_end(&mut state.document.buffer.borrow_mut(), false);
    } else if ctx.consume_shortcut(chord(Action::LineStartSelect)) {
        edit::buffer::smart_line_start(&mut state.document.buffer.borrow_mut(), true);
    } else if ctx.consume_shortcut(chord(Action::LineEndSelect)) {
        edit::buffer::line_end(&mut state.document.buffer.borrow_mut(), true);
    } else if ctx.consume_shortcut(chord(Action::DeleteToLineStart)) {
        state.document.buffer.borrow_mut().delete_to_line_edge(false);
    } else if ctx.consume_shortcut(chord(Action::DeleteToLineEnd)) {
        state.document.buffer.borrow_mut().delete_to_line_edge(true);
    } else if ctx.consume_shortcut(chord(Action::JumpDocumentStart)) {
        let mut tb = state.document.buffer.borrow_mut();
        tb.cursor_move_to_logical(Point { x: 0, y: 0 });
        tb.set_preferred_column(0);
        tb.make_cursor_visible();
    } else if ctx.consume_shortcut(chord(Action::JumpDocumentEnd)) {
        let mut tb = state.document.buffer.borrow_mut();
        tb.cursor_move_to_logical(Point::MAX);
        let x = tb.cursor_visual_pos().x;
        tb.set_preferred_column(x);
        tb.make_cursor_visible();
    } else {
        return false;
    }

    true
}

/// Pick minimap cell width for a terminal `terminal_width` cells wide. 0
/// disables the rail (terminal too narrow); same thresholds apply in both
/// unicode and ascii-quirk modes.
fn desired_minimap_width(terminal_width: CoordType) -> u8 {
    const NARROW_FLOOR: CoordType = 30;
    const TWO_CELL_THRESHOLD: CoordType = 60;
    if terminal_width < NARROW_FLOOR {
        return 0;
    }
    if terminal_width < TWO_CELL_THRESHOLD {
        return 1;
    }
    2
}

fn write_terminal_title<'a>(arena: &'a Arena, output: &mut BString<'a>, state: &mut State) {
    let filename = state.document.filename.as_str();
    let dirty = state.document.buffer.borrow().is_dirty();

    if filename == state.osc_title_file_status.filename
        && dirty == state.osc_title_file_status.dirty
    {
        return;
    }

    output.push_str(arena, "\x1b]0;");
    if !filename.is_empty() {
        if dirty {
            output.push_str(arena, edit::glyphs::modified_dot());
        }
        output.push_str(arena, &sanitize_control_chars(filename));
        output.push_str(arena, " - ");
    }
    output.push_str(arena, "edit\x1b\\");

    state.osc_title_file_status.filename = filename.to_string();
    state.osc_title_file_status.dirty = dirty;
}

#[cold]
fn write_osc_clipboard<'a>(
    arena: &'a Arena,
    output: &mut BString<'a>,
    tui: &mut Tui,
    state: &mut State,
) {
    let clipboard = tui.clipboard_mut();
    let data = clipboard.read();

    if !data.is_empty() {
        // Rust doubles the size of a string when it needs to grow it.
        // If `data` is *really* large, this may then double
        // the size of the `output` from e.g. 100MB to 200MB. Not good.
        // We can avoid that by reserving the needed size in advance.
        output.reserve_exact(arena, base64::encode_len(data.len()) + 16);
        output.push_str(arena, "\x1b]52;c;");
        base64::encode(arena, output, data);
        output.push_str(arena, "\x1b\\");
    }

    state.osc_clipboard_sync = false;
}

/// Strips all C0 control characters from the string and replaces them with "_".
///
/// Jury is still out on whether this should also strip C1 control characters.
/// That requires parsing UTF8 codepoints, which is annoying.
fn sanitize_control_chars(text: &str) -> Cow<'_, str> {
    if let Some(off) = text.bytes().position(|b| (..0x20).contains(&b)) {
        let mut sanitized = text.to_string();
        // SAFETY: We only search for ASCII and replace it with ASCII.
        let vec = unsafe { sanitized.as_bytes_mut() };

        for i in &mut vec[off..] {
            *i = if (..0x20).contains(i) { b'_' } else { *i }
        }

        Cow::Owned(sanitized)
    } else {
        Cow::Borrowed(text)
    }
}
