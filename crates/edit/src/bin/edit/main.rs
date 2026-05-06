mod apperr;
mod colormap;
#[cfg(debug_assertions)]
mod devlog;
mod documents;
mod draw_editor;
mod draw_menubar;
mod draw_statusbar;
mod git;
mod gutter_diff;
mod keybindings;
mod linediff;
mod minimap;
mod settings;
mod state;

use std::borrow::Cow;
use std::collections::HashSet;
#[cfg(debug_assertions)]
use std::path::Path;
use std::time::Duration;
use std::{env, process};

/// Opt-in toggles for non-default behaviour. Parsed from `--quirks=a,b,c`.
///
/// Each entry's canonical spelling is what `quirks.contains(...)` checks
/// throughout the codebase; any alias (kebab / underscore / shorthand) on
/// the cli or in `EDIT_QUIRKS` resolves to the same canonical entry.
const KNOWN_QUIRKS: &[polyflag::KnownToken] = &[
    polyflag::token!("weird-filenames"),
    polyflag::token!("ascii"),
    polyflag::token!("nocolor"; "no-color"),
    polyflag::token!("noanimations"; "no-animations"),
    polyflag::token!("allow-create"; "allowcreate"),
];

use draw_editor::*;
use draw_menubar::*;
use draw_statusbar::*;
use edit::framebuffer::IndexedColor;
use edit::helpers::*;
use edit::input::{self, vk};
use edit::oklab::StraightRgba;
use edit::tui::*;
use edit::vt::{self, Token};
use edit::{base64, sys, unicode};
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

    if name == "eat" {
        return eat::main();
    }

    if cfg!(debug_assertions) {
        let hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            drop(RestoreModes);
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

    let Some(path) = parse_args()? else {
        return Ok(());
    };
    let document = documents::Document::open(&path)?;
    let mut state = State::new(document)?;

    if let Err(err) = Settings::reload() {
        state.add_error(err);
    }
    if let Err(err) = keybindings::load_or_create() {
        state.add_error(err);
    }
    if let Err(err) = colormap::load_or_create() {
        state.add_error(err);
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

    let _restore = setup_terminal(&mut tui, &mut state, &mut vt_parser);

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
    });
    tui.set_floater_default_bg(floater_bg);
    tui.set_floater_default_fg(floater_fg);
    tui.set_modal_default_bg(floater_bg);
    tui.set_modal_default_fg(floater_fg);

    sys::inject_window_size_into_stdin();

    const GUTTER_REDIFF_DEBOUNCE: Duration = Duration::from_millis(300);
    const MINIMAP_REBUILD_DEBOUNCE: Duration = Duration::from_millis(300);

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

/// Returns `Some(path)` if the application should continue starting,
/// `None` if it should exit early (help/version/usage).
fn parse_args() -> apperr::Result<Option<std::path::PathBuf>> {
    let mut path: Option<std::path::PathBuf> = None;
    let mut accept_flags = true;
    let mut quirks: HashSet<&'static str> = HashSet::new();
    let mut seen_quirks_flag = false;

    // EDIT_QUIRKS layers in before any cli flag, so a `--quirks=-name`
    // override can negate an env-provided default. The env var name is
    // derived by polyflag from the prefix and flag, so cli surface and
    // env surface stay in lock-step.
    debug_assert!(check_quirks_table(), "KNOWN_QUIRKS has duplicate or empty spelling");
    let warn_deprecated = |spelling: &str, canonical: &'static str| {
        sys::write_stdout(&format!(
            "edit: warning: --quirks={spelling} is deprecated, use {canonical}\n"
        ));
    };
    if let Err(e) = polyflag::apply_env_for_flag_with_callback(
        "edit",
        "quirks",
        KNOWN_QUIRKS,
        &mut quirks,
        warn_deprecated,
    ) {
        sys::write_stdout(&format!(
            "edit: {} contains unknown quirk {:?}\nknown quirks: {}\n",
            polyflag::env_var_name("edit", "quirks"),
            e.0,
            known_quirks_for_help(),
        ));
        return Ok(None);
    }

    for arg in env::args_os().skip(1) {
        if accept_flags {
            if arg == "--" {
                accept_flags = false;
                continue;
            }
            if arg == "-h" || arg == "--help" {
                print_help();
                return Ok(None);
            }
            if arg == "-v" || arg == "--version" {
                print_version();
                return Ok(None);
            }
            if let Some(list) = arg.to_str().and_then(|s| s.strip_prefix("--quirks=")) {
                if seen_quirks_flag {
                    sys::write_stdout("edit: --quirks may only be passed once\n");
                    return Ok(None);
                }
                seen_quirks_flag = true;
                if let Err(e) =
                    polyflag::apply_with_callback(list, KNOWN_QUIRKS, &mut quirks, warn_deprecated)
                {
                    sys::write_stdout(&format!(
                        "edit: unknown quirk {:?}\nknown quirks: {}\n",
                        e.0,
                        known_quirks_for_help(),
                    ));
                    return Ok(None);
                }
                continue;
            }
            #[cfg(debug_assertions)]
            if let Some(p) = arg.to_str().and_then(|s| s.strip_prefix("--logfile=")) {
                if let Err(e) = devlog::open(Path::new(p)) {
                    sys::write_stdout(&format!("failed to open logfile: {e}\n"));
                }
                continue;
            }
            #[cfg(debug_assertions)]
            if arg == "--force-reset-config" {
                if let Err(e) = keybindings::force_reset() {
                    sys::write_stdout(&format!("failed to reset config: {e:?}\n"));
                    return Ok(None);
                }
                if let Err(e) = colormap::force_reset() {
                    sys::write_stdout(&format!("failed to reset config: {e:?}\n"));
                    return Ok(None);
                }
                sys::write_stdout("config reset\n");
                continue;
            }

            // Unknown flag: anything starting with `-` that survived the
            // checks above. Refuse rather than silently treating it as a
            // path. Use `--` to open files whose names start with `-`.
            if arg.to_str().is_some_and(|s| s.starts_with('-') && s != "-") {
                let arg = arg.to_string_lossy();
                sys::write_stdout(&format!(
                    "edit: unknown option {arg:?}\n\
                     try 'edit --help', or 'edit -- {arg}' to open a file with that name\n"
                ));
                return Ok(None);
            }
        }

        if path.is_some() {
            sys::write_stdout("edit: only one file argument is supported\n");
            return Ok(None);
        }
        path = Some(std::path::PathBuf::from(&arg));
    }

    // Apply quirks that affect global rendering state.
    edit::glyphs::set_ascii_only(quirks.contains("ascii"));
    edit::glyphs::set_no_color(quirks.contains("nocolor"));
    edit::glyphs::set_no_animations(quirks.contains("noanimations"));
    documents::set_allow_create(quirks.contains("allow-create"));

    match path {
        Some(p) => {
            // Refuse weird filenames (see [`is_safe_filename`]). Catches
            // creation of new files with surprising names and rare-but-real
            // existing files with weird names (e.g. left behind by a buggy
            // tool). Override with `--quirks=weird-filenames`.
            let (file_path, _) = documents::parse_filename_goto(&p);
            let name = file_path.file_name().and_then(|s| s.to_str()).unwrap_or("");
            if !quirks.contains("weird-filenames") && !is_safe_filename(name) {
                sys::write_stdout(&format!(
                    "edit: refusing filename {name:?}\n\
                     pass `--quirks=weird-filenames` to allow\n"
                ));
                return Ok(None);
            }
            // Refuse to create new files unless `--quirks=allow-create`.
            // edit never creates directories regardless of this quirk.
            if !quirks.contains("allow-create") && !file_path.exists() {
                let display = file_path.display();
                sys::write_stdout(&format!(
                    "edit: refusing to create new file: {display}\n\
                     pass `--quirks=allow-create` to allow\n"
                ));
                return Ok(None);
            }
            Ok(Some(p))
        }
        None => {
            print_help();
            Ok(None)
        }
    }
}

/// Safe-filename gate. A filename must:
/// - be non-empty.
/// - not be `.` or `..` (those name directories, not files).
/// - have at most one leading dot (`.gitignore` ok, `..tilde` not).
/// - not start with `-` (would be confused with a cli flag downstream).
/// - contain at least one ASCII letter (`123` is weird).
/// - be ASCII only (no unicode -- emoji, accented chars, CJK are weird).
/// - have a stem from `[A-Za-z0-9_+-]` and only alphanumeric extension
///   parts. Splitting on `.` after stripping any single leading dot:
///   the first segment is the stem; the rest are extensions and must be
///   plain alphanumerics (so `foo.tar.gz` is fine, `foo.b-c~d` is not).
///
/// Override with `--quirks=weird-filenames`.
fn is_safe_filename(name: &str) -> bool {
    if name.is_empty() || name == "." || name == ".." || name.starts_with('-') {
        return false;
    }
    if name.starts_with("..") {
        return false;
    }
    if !name.bytes().any(|b| b.is_ascii_alphabetic()) {
        return false;
    }

    let body = name.strip_prefix('.').unwrap_or(name);
    let mut parts = body.split('.');
    let Some(stem) = parts.next() else {
        return false;
    };
    if stem.is_empty()
        || !stem.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'+'))
    {
        return false;
    }
    parts.all(|ext| !ext.is_empty() && ext.bytes().all(|b| b.is_ascii_alphanumeric()))
}

/// Render a comma-separated list of accepted quirk spellings for use in
/// error messages. Canonicals are listed with any non-`Hidden` aliases
/// shown parenthetically -- `--quirks=` accepts both forms.
fn known_quirks_for_help() -> String {
    use polyflag::AliasStatus;
    let mut out = String::new();
    for kt in KNOWN_QUIRKS {
        if !out.is_empty() {
            out.push_str(", ");
        }
        out.push_str(kt.canonical);
        let mut first_alt = true;
        for alias in kt.aliases {
            if matches!(alias.status, AliasStatus::Alternative | AliasStatus::Deprecated) {
                out.push_str(if first_alt { " (" } else { ", " });
                out.push_str(alias.spelling);
                first_alt = false;
            }
        }
        if !first_alt {
            out.push(')');
        }
    }
    out
}

/// Wrap [`polyflag::check_known`] so the call site is one line. Returns
/// `true` if the table is well-formed; in debug builds a malformed table
/// panics inside `check_known` before we ever return.
fn check_quirks_table() -> bool {
    polyflag::check_known(KNOWN_QUIRKS);
    true
}

fn print_help() {
    sys::write_stdout(concat!(
        "Usage: edit [OPTIONS] [--] FILE[:LINE[:COLUMN]]\n",
        "Options:\n",
        "    -h, --help       Print this help message\n",
        "    -v, --version    Print the version number\n",
        "    --               End of options. Subsequent arguments are treated as\n",
        "                     file names even if they start with `-`.\n",
        "                     Example: `edit -- --version` opens a file called `--version`.\n",
        "    --quirks=LIST    Comma-separated opt-in toggles for non-default behaviour.\n",
        "                     Known quirks:\n",
        "                       weird-filenames -- allow weird filenames. without this\n",
        "                                          quirk, names are ASCII-only, must\n",
        "                                          contain a letter, must not start with\n",
        "                                          `-`, must have at most one leading\n",
        "                                          dot, must not be `.` or `..`, and\n",
        "                                          dot-separated parts are constrained:\n",
        "                                          stem is [A-Za-z0-9_+-], extension(s)\n",
        "                                          are alphanumeric only.\n",
        "                       ascii           -- render UI with ASCII glyphs only\n",
        "                                          (no box-drawing or other unicode).\n",
        "                       nocolor         -- suppress all SGR colour output\n",
        "                                          (text attributes still emitted).\n",
        "                                          alias: no-color\n",
        "                       noanimations    -- disable cursor / scroll / floater\n",
        "                                          motion. logic stays instant; only the\n",
        "                                          visible interpolation is suppressed.\n",
        "                                          alias: no-animations\n",
        "                       allow-create    -- allow creating new files. without this\n",
        "                                          quirk, edit refuses to open a path that\n",
        "                                          does not exist and refuses to save into\n",
        "                                          a missing file. edit never creates\n",
        "                                          directories regardless of this quirk.\n",
        "                                          alias: allowcreate\n",
        "\n",
        "Arguments:\n",
        "    FILE[:LINE[:COLUMN]]    The file to open, optionally with line and column (e.g., foo.txt:123:45)\n",
        "\n",
        "Environment:\n",
        "    EDIT_QUIRKS    Comma-separated quirks applied before any --quirks flag.\n",
        "                   Use --quirks=-NAME to negate an entry from EDIT_QUIRKS.\n",
    ));
    #[cfg(debug_assertions)]
    sys::write_stdout(concat!(
        "\nDebug-build options:\n",
        "    --logfile=PATH          Log inputs + buffer state as JSONL\n",
        "    --force-reset-config    Wipe config dir and rewrite defaults, then continue\n",
    ));
}

fn print_version() {
    sys::write_stdout(&format!("edit {}\n", version::version!()));
}

fn draw(ctx: &mut Context, state: &mut State) {
    handle_global_shortcuts(ctx, state);

    draw_menubar(ctx, state);
    draw_editor(ctx, state);
    draw_statusbar(ctx, state);

    if state.wants_exit {
        draw_handle_wants_exit(ctx, state);
    }
    if state.wants_goto {
        draw_goto_menu(ctx, state);
    }
    if state.wants_language_picker {
        draw_dialog_language_change(ctx, state);
    }
    if state.wants_about {
        draw_dialog_about(ctx, state);
    }
    if ctx.clipboard_ref().wants_host_sync() {
        ctx.clipboard_mut().mark_as_synchronized();
        state.osc_clipboard_sync = true;
    }
    if state.error_log_count != 0 {
        draw_error_log(ctx, state);
    }

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
    use edit::buffer::MoveLineDirection;
    use keybindings::{Action, chord};

    let search_enabled = state.wants_search.kind != StateSearchKind::Disabled;

    if ctx.consume_shortcut(chord(Action::Exit)) {
        state.wants_exit = true;
    } else if ctx.consume_shortcut(chord(Action::GoToLine)) {
        state.wants_goto = true;
    } else if search_enabled && ctx.consume_shortcut(chord(Action::Find)) {
        state.wants_search.kind = StateSearchKind::Search;
        state.wants_search.focus = true;
    } else if search_enabled && ctx.consume_shortcut(chord(Action::Replace)) {
        state.wants_search.kind = StateSearchKind::Replace;
        state.wants_search.focus = true;
    } else if ctx.consume_shortcut(chord(Action::MoveLineUp)) {
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
            line_tok.or_else(|| documents::fallback_line_comment(&state.document.path))
        {
            state.document.buffer.borrow_mut().toggle_line_comment(tok);
        } else if let Some((open, close)) = block_tok {
            state.document.buffer.borrow_mut().toggle_per_line_block_comment(open, close);
        }
    } else if ctx.consume_shortcut(chord(Action::SmallJumpUpSelect)) {
        small_jump_select(&mut state.document.buffer.borrow_mut(), -SMALL_JUMP_LINES);
    } else if ctx.consume_shortcut(chord(Action::SmallJumpDownSelect)) {
        small_jump_select(&mut state.document.buffer.borrow_mut(), SMALL_JUMP_LINES);
    } else if ctx.consume_shortcut(chord(Action::SmallJumpUp)) {
        small_jump(&mut state.document.buffer.borrow_mut(), -SMALL_JUMP_LINES);
    } else if ctx.consume_shortcut(chord(Action::SmallJumpDown)) {
        small_jump(&mut state.document.buffer.borrow_mut(), SMALL_JUMP_LINES);
    } else if ctx.consume_shortcut(chord(Action::LineStart)) {
        smart_line_start(&mut state.document.buffer.borrow_mut(), false);
    } else if ctx.consume_shortcut(chord(Action::LineEnd)) {
        line_end(&mut state.document.buffer.borrow_mut(), false);
    } else if ctx.consume_shortcut(chord(Action::LineStartSelect)) {
        smart_line_start(&mut state.document.buffer.borrow_mut(), true);
    } else if ctx.consume_shortcut(chord(Action::LineEndSelect)) {
        line_end(&mut state.document.buffer.borrow_mut(), true);
    } else if ctx.consume_shortcut(chord(Action::DeleteToLineStart)) {
        state.document.buffer.borrow_mut().delete_to_line_edge(false);
    } else if ctx.consume_shortcut(chord(Action::DeleteToLineEnd)) {
        state.document.buffer.borrow_mut().delete_to_line_edge(true);
    } else {
        return;
    }

    ctx.needs_rerender();
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

fn small_jump(tb: &mut edit::buffer::TextBuffer, delta: CoordType) {
    let x = tb.preferred_column();
    let max_y = (tb.visual_line_count() - 1).max(0);
    let y = (tb.cursor_visual_pos().y + delta).clamp(0, max_y);
    tb.cursor_move_to_visual(Point { x, y });
    tb.request_scroll_delta_y(delta);
}

fn small_jump_select(tb: &mut edit::buffer::TextBuffer, delta: CoordType) {
    let x = tb.preferred_column();
    let max_y = (tb.visual_line_count() - 1).max(0);
    let y = (tb.cursor_visual_pos().y + delta).clamp(0, max_y);
    tb.selection_update_visual(Point { x, y });
    tb.request_scroll_delta_y(delta);
}

fn smart_line_start(tb: &mut edit::buffer::TextBuffer, select: bool) {
    let cur = tb.cursor_logical_pos();
    let indent_end = tb.indent_end_logical_pos();
    let target = if cur.x > indent_end.x { indent_end } else { Point { x: 0, y: cur.y } };
    if select {
        tb.selection_update_logical(target);
    } else {
        tb.cursor_move_to_logical(target);
    }
    tb.set_preferred_column(tb.cursor_visual_pos().x);
    tb.make_cursor_visible();
}

fn line_end(tb: &mut edit::buffer::TextBuffer, select: bool) {
    let y = tb.cursor_logical_pos().y;
    let target = Point { x: CoordType::MAX, y };
    if select {
        tb.selection_update_logical(target);
    } else {
        tb.cursor_move_to_logical(target);
    }
    tb.set_preferred_column(tb.cursor_visual_pos().x);
    tb.make_cursor_visible();
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

struct RestoreModes;

impl Drop for RestoreModes {
    fn drop(&mut self) {
        // Same as in the beginning but in the reverse order.
        // It also includes DECSCUSR 0 to reset the cursor style and DECTCEM to show the cursor.
        // We specifically don't reset mode 1036, because most applications expect it to be set nowadays.
        // `CSI < u` pops the kitty keyboard protocol flags we pushed in setup_terminal.
        sys::write_stdout("\x1b[<u\x1b[0 q\x1b[?25h\x1b]0;\x07\x1b[?1002;1006;2004l\x1b[?1049l");
    }
}

fn setup_terminal(tui: &mut Tui, state: &mut State, vt_parser: &mut vt::Parser) -> RestoreModes {
    sys::write_stdout(concat!(
        // 1049: Alternative Screen Buffer
        //   I put the ASB switch in the beginning, just in case the terminal performs
        //   some additional state tracking beyond the modes we enable/disable.
        // 1002: Cell Motion Mouse Tracking
        // 1006: SGR Mouse Mode
        // 2004: Bracketed Paste Mode
        // 1036: Xterm: "meta sends escape" (Alt keypresses should be encoded with ESC + char)
        "\x1b[?1049h\x1b[?1002;1006;2004h\x1b[?1036h",
        // Kitty keyboard protocol: push flag 1 (disambiguate escape codes). This gets
        // us distinct Super/Cmd modifiers on keys. Terminals that don't support it
        // silently ignore the sequence and fall back to legacy encoding.
        "\x1b[>1u",
        // OSC 4 color table requests for indices 0 through 15 (base colors).
        "\x1b]4;0;?;1;?;2;?;3;?;4;?;5;?;6;?;7;?\x07",
        "\x1b]4;8;?;9;?;10;?;11;?;12;?;13;?;14;?;15;?\x07",
        // OSC 10 and 11 queries for the current foreground and background colors.
        "\x1b]10;?\x07\x1b]11;?\x07",
        // Test whether ambiguous width characters are two columns wide.
        // We use "…", because it's the most common ambiguous width character we use,
        // and the old Windows conhost doesn't actually use wcwidth, it measures the
        // actual display width of the character and assigns it columns accordingly.
        // We detect it by writing the character and asking for the cursor position.
        "\r…\x1b[6n",
        // CSI c reports the terminal capabilities.
        // It also helps us to detect the end of the responses, because not all
        // terminals support the OSC queries, but all of them support CSI c.
        "\x1b[c",
    ));

    let mut done = false;
    let mut osc_buffer = String::new();
    let (mut indexed_colors, force_colormap) = {
        let cm = colormap::borrow();
        (cm.palette, cm.use_colormap)
    };
    let mut color_responses = 0;
    let mut ambiguous_width = 1;

    while !done {
        let scratch = scratch_arena(None);

        // We explicitly set a high read timeout, because we're not
        // waiting for user keyboard input. If we encounter a lone ESC,
        // it's unlikely to be from a ESC keypress, but rather from a VT sequence.
        let Some(input) = sys::read_stdin(&scratch, Duration::from_secs(3)) else {
            break;
        };

        let mut vt_stream = vt_parser.parse(&input);
        while let Some(token) = vt_stream.next() {
            match token {
                Token::Csi(csi) => match csi.final_byte {
                    'c' => done = true,
                    // CPR (Cursor Position Report) response.
                    'R' => ambiguous_width = csi.params[1] as CoordType - 1,
                    _ => {}
                },
                Token::Osc { mut data, partial } => {
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
                        // The response is `4;<color>;rgb:<r>/<g>/<b>`.
                        "4" => match splits.next().unwrap_or("").parse::<usize>() {
                            Ok(val) if val < 16 => &mut indexed_colors[val],
                            _ => continue,
                        },
                        // The response is `10;rgb:<r>/<g>/<b>`.
                        "10" => &mut indexed_colors[IndexedColor::Foreground as usize],
                        // The response is `11;rgb:<r>/<g>/<b>`.
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
                    color_responses += 1;
                    osc_buffer.clear();
                }
                _ => {}
            }
        }
    }

    if ambiguous_width == 2 {
        unicode::setup_ambiguous_width(2);
        state.document.buffer.borrow_mut().reflow();
    }

    if force_colormap {
        // colormap.toml wins — ignore terminal responses.
        tui.setup_indexed_colors(colormap::borrow().palette);
    } else if color_responses == indexed_colors.len() {
        tui.setup_indexed_colors(indexed_colors);
    }

    RestoreModes
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

#[cfg(test)]
mod tests {
    use super::is_safe_filename;

    #[test]
    fn safe_filenames_accepted() {
        for name in [
            "foo",
            "foo.txt",
            "Foo_Bar.tar.gz",
            "_underscore",
            "v2",
            "1.txt",
            "foo+bar.tar.gz",
            "my-file_v2.tar.gz",
            ".gitignore",
            ".env.local",
        ] {
            assert!(is_safe_filename(name), "expected {name:?} to be safe");
        }
    }

    #[test]
    fn unsafe_filenames_rejected() {
        for name in [
            "",
            ".",
            "..",
            "...foo",
            "..tilde",
            "tilde~mid",
            "trailing~",
            "foo.b-c_d",
            "foo..txt",
            "-leading-dash.txt",
            "--double-dash",
            "with space.txt",
            "a/b",
            "a\\b",
            "foo:bar",
            "foo\"bar",
            "foo|bar",
            "foo$bar",
            "foo#bar",
            "foo,bar",
            "foo(bar)",
            "key=value.conf",
            "user@host.txt",
            "0123",
            "42",
            "héllo.txt",
            "résumé.pdf",
            "файл.txt",
            "你好.md",
            "rocket🚀.txt",
            "💀",
            "newline\n",
        ] {
            assert!(!is_safe_filename(name), "expected {name:?} to be unsafe");
        }
    }
}
