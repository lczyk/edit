//! The global modal dialogs: unsaved-changes confirm, go-to-line,
//! about, the language picker, and the error log.
//!
//! Grouped by what they are rather than by what opens them. They used to
//! be filed next to their trigger -- exit and goto under the editor pane,
//! about under the menubar, language under the statusbar, the error log
//! in the state module -- which made "where is this dialog drawn" a
//! question with four possible answers.
//!
//! At most one is open at a time; see [`Modal`].

use std::num::ParseIntError;

use edit::framebuffer::IndexedColor;
use edit::helpers::*;
use edit::input::vk;
use edit::lsh::LANGUAGES;
use edit::tui::*;
use stdext::arena_format;

use crate::state::*;

/// Which global dialog is open, if any.
///
/// One slot rather than a flag each: the draw pass checked four
/// independent bools in sequence, so nothing stopped two being set in the
/// same frame and both dialogs painting on top of each other.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Modal {
    /// Unsaved changes, raised on exit.
    ConfirmExit,
    GoToLine,
    LanguagePicker,
    About,
    ErrorLog,
}

/// Raise the error log if anything is queued. Errors outrank whatever
/// else is open -- they are the one dialog the user did not ask for and
/// needs to see.
pub fn raise_errors_if_any(state: &mut State) {
    if state.error_log_count != 0 {
        state.modal = Some(Modal::ErrorLog);
    }
}

/// Draw whichever dialog is open. No-op when none is.
pub fn draw_active(ctx: &mut Context, state: &mut State) {
    match state.modal {
        None => {}
        Some(Modal::ConfirmExit) => draw_handle_wants_exit(ctx, state),
        Some(Modal::GoToLine) => draw_goto_menu(ctx, state),
        Some(Modal::LanguagePicker) => draw_dialog_language_change(ctx, state),
        Some(Modal::About) => draw_dialog_about(ctx, state),
        Some(Modal::ErrorLog) => draw_error_log(ctx, state),
    }
}

pub fn draw_handle_wants_exit(ctx: &mut Context, state: &mut State) {
    if !state.document.buffer.borrow().is_dirty() {
        state.exit = true;
        state.modal = None;
        return;
    }

    enum Action {
        None,
        Save,
        Discard,
        Cancel,
    }
    let mut action = Action::None;
    let read_only = state.document.read_only;

    ctx.modal_begin("unsaved-changes", "Unsaved Changes");
    ctx.attr_background_rgba(ctx.indexed(IndexedColor::Red));
    ctx.attr_foreground_rgba(ctx.indexed(IndexedColor::BrightWhite));
    {
        let contains_focus = ctx.contains_focus();

        ctx.label(
            "description",
            if read_only {
                "File is read-only. Discard unsaved changes?"
            } else {
                "Do you want to save the changes you made?"
            },
        );
        ctx.attr_padding(Rect::three(1, 2, 1));

        ctx.table_begin("choices");
        ctx.inherit_focus();
        ctx.attr_padding(Rect::three(0, 2, 1));
        ctx.attr_position(Position::Center);
        ctx.table_set_cell_gap(Size { width: 2, height: 0 });
        {
            ctx.table_next_row();
            ctx.inherit_focus();

            if !read_only {
                if ctx.button("yes", "Save", ButtonStyle::default().accelerator('S')) {
                    action = Action::Save;
                }
                ctx.inherit_focus();
            }
            if ctx.button("no", "Don't Save", ButtonStyle::default().accelerator('N')) {
                action = Action::Discard;
            }
            if read_only {
                ctx.inherit_focus();
            }
            if ctx.button("cancel", "Cancel", ButtonStyle::default()) {
                action = Action::Cancel;
            }

            if contains_focus {
                if !read_only && ctx.consume_shortcut(vk::S) {
                    action = Action::Save;
                } else if ctx.consume_shortcut(vk::N) {
                    action = Action::Discard;
                }
            }
        }
        ctx.table_end();
    }
    if ctx.modal_end() {
        action = Action::Cancel;
    }

    match action {
        Action::None => return,
        Action::Save => match state.document.save() {
            Ok(()) => {
                state.exit = true;
                state.modal = None;
            }
            Err(err) => {
                error_log_add(ctx, state, err);
                state.modal = None;
            }
        },
        Action::Discard => {
            state.exit = true;
            state.modal = None;
        }
        Action::Cancel => {
            state.modal = None;
        }
    }

    ctx.needs_rerender();
}

pub fn draw_goto_menu(ctx: &mut Context, state: &mut State) {
    let mut done = false;

    let doc = &mut state.document;
    ctx.modal_begin("goto", "Go to Line:Column\u{2026}"); // horizontal ellipsis
    {
        if ctx.editline("goto-line", &mut state.goto_target) {
            state.goto_invalid = false;
        }
        if state.goto_invalid {
            ctx.attr_background_rgba(ctx.indexed(IndexedColor::Red));
            ctx.attr_foreground_rgba(ctx.indexed(IndexedColor::BrightWhite));
        }

        ctx.attr_intrinsic_size(Size { width: 24, height: 1 });
        ctx.steal_focus();

        if ctx.consume_shortcut(vk::RETURN) {
            match validate_goto_point(&state.goto_target) {
                Ok(point) => {
                    let mut buf = doc.buffer.borrow_mut();
                    buf.cursor_move_to_logical(point);
                    buf.make_cursor_visible();
                    done = true;
                }
                Err(_) => state.goto_invalid = true,
            }
            ctx.needs_rerender();
        }
    }
    done |= ctx.modal_end();

    if done {
        state.modal = None;
        state.goto_target.clear();
        state.goto_invalid = false;
        ctx.needs_rerender();
    }
}

fn validate_goto_point(line: &str) -> Result<Point, ParseIntError> {
    let mut coords = [0; 2];
    let (y, x) = line.split_once(':').unwrap_or((line, "0"));
    // Using a loop here avoids 2 copies of the str->int code.
    // This makes the binary more compact.
    for (i, s) in [x, y].iter().enumerate() {
        coords[i] = s.parse::<CoordType>()?.saturating_sub(1);
    }
    Ok(Point { x: coords[0], y: coords[1] })
}

pub fn draw_dialog_about(ctx: &mut Context, state: &mut State) {
    ctx.modal_begin("about", "about");
    {
        ctx.block_begin("content");
        ctx.inherit_focus();
        ctx.attr_padding(Rect::three(1, 2, 1));
        {
            ctx.label("description", "edit");
            ctx.attr_overflow(Overflow::TruncateTail);
            ctx.attr_position(Position::Center);

            ctx.label(
                "version",
                &arena_format!(ctx.arena(), "{}{}", "version: ", env!("CARGO_PKG_VERSION")),
            );
            ctx.attr_overflow(Overflow::TruncateHead);
            ctx.attr_position(Position::Center);

            ctx.block_begin("choices");
            ctx.inherit_focus();
            ctx.attr_padding(Rect::three(1, 2, 0));
            ctx.attr_position(Position::Center);
            {
                if ctx.button("ok", "ok", ButtonStyle::default()) {
                    state.modal = None;
                }
                ctx.inherit_focus();
            }
            ctx.block_end();
        }
        ctx.block_end();
    }
    if ctx.modal_end() {
        state.modal = None;
    }
}

pub fn draw_dialog_language_change(ctx: &mut Context, state: &mut State) {
    let doc = &mut state.document;
    let mut done = false;

    ctx.modal_begin("language", "Select Language Mode");
    {
        let width = (ctx.size().width - 20).max(10);
        let height = (ctx.size().height - 10).max(10);

        ctx.scrollarea_begin("scrollarea", Size { width, height });
        ctx.attr_background_rgba(ctx.indexed_alpha(IndexedColor::Black, 1, 4));
        ctx.inherit_focus();
        {
            ctx.list_begin("languages");
            ctx.inherit_focus();

            let auto_detect = doc.language_override.is_none();
            let selected = if auto_detect { None } else { doc.buffer.borrow().language() };

            if ctx.list_item(auto_detect, "Auto Detect") == ListSelection::Activated {
                doc.auto_detect_language();
                done = true;
            }

            if ctx.list_item(selected.is_none(), "Plain Text") == ListSelection::Activated {
                doc.override_language(None);
                done = true;
            }

            for lang in LANGUAGES {
                if ctx.list_item(Some(lang) == selected, lang.name) == ListSelection::Activated {
                    doc.override_language(Some(lang));
                    done = true;
                }
            }
            ctx.list_end();
        }
        ctx.scrollarea_end();
    }
    done |= ctx.modal_end();

    if done {
        state.modal = None;
        ctx.needs_rerender();
    }
}

pub fn draw_error_log(ctx: &mut Context, state: &mut State) {
    ctx.modal_begin("error", "Error");
    ctx.attr_background_rgba(ctx.indexed(IndexedColor::Red));
    ctx.attr_foreground_rgba(ctx.indexed(IndexedColor::BrightWhite));
    {
        ctx.block_begin("content");
        ctx.attr_padding(Rect::three(0, 2, 1));
        {
            let off = state.error_log_index + state.error_log.len() - state.error_log_count;

            for i in 0..state.error_log_count {
                let idx = (off + i) % state.error_log.len();
                let msg = &state.error_log[idx][..];

                if !msg.is_empty() {
                    ctx.next_block_id_mixin(i as u64);
                    ctx.label("error", msg);
                    ctx.attr_overflow(Overflow::TruncateTail);
                }
            }
        }
        ctx.block_end();

        if ctx.button("ok", "Ok", ButtonStyle::default()) {
            state.dismiss_errors();
        }
        ctx.attr_position(Position::Center);
        ctx.inherit_focus();
    }
    if ctx.modal_end() {
        state.dismiss_errors();
    }
}
