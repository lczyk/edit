// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::num::ParseIntError;

use edit::framebuffer::IndexedColor;
use edit::helpers::*;
use edit::icu;
use edit::input::{kbmod, vk};
use edit::tui::*;
use stdext::string_from_utf8_lossy_owned;

use crate::state::*;

pub fn draw_editor(ctx: &mut Context, state: &mut State) {
    if !matches!(state.wants_search.kind, StateSearchKind::Hidden | StateSearchKind::Disabled) {
        draw_search(ctx, state);
    }

    let size = ctx.size();
    // TODO: The layout code should be able to just figure out the height on its own.
    let height_reduction = match state.wants_search.kind {
        StateSearchKind::Search => 4,
        StateSearchKind::Replace => 5,
        _ => 2,
    };

    ctx.textarea("textarea", state.document.buffer.clone());
    ctx.inherit_focus();

    ctx.attr_intrinsic_size(Size { width: 0, height: size.height - height_reduction });
}

fn draw_search(ctx: &mut Context, state: &mut State) {
    if let Err(err) = icu::init() {
        error_log_add(ctx, state, err.into());
        state.wants_search.kind = StateSearchKind::Disabled;
        return;
    }

    let doc = &state.document;

    let mut action = None;
    let mut focus = StateSearchKind::Hidden;

    if state.wants_search.focus {
        state.wants_search.focus = false;
        focus = StateSearchKind::Search;

        // If the selection is empty, focus the search input field.
        // Otherwise, focus the replace input field, if it exists.
        // Only prefill from a single-line selection; multi-line selections are
        // almost never useful as search needles.
        let mut buf = doc.buffer.borrow_mut();
        let single_line = buf
            .selection_range()
            .is_some_and(|(b, e)| b.logical_pos.y == e.logical_pos.y);
        if single_line
            && let Some(selection) = buf.extract_user_selection(false)
        {
            state.search_needle = string_from_utf8_lossy_owned(selection);
            focus = state.wants_search.kind;
        }
    }

    ctx.block_begin("search");
    ctx.attr_focus_well();
    ctx.attr_background_rgba(ctx.indexed(IndexedColor::White));
    ctx.attr_foreground_rgba(ctx.indexed(IndexedColor::Black));
    {
        if ctx.contains_focus() && ctx.consume_shortcut(vk::ESCAPE) {
            state.wants_search.kind = StateSearchKind::Hidden;
        }

        ctx.table_begin("needle");
        ctx.table_set_cell_gap(Size { width: 1, height: 0 });
        {
            {
                ctx.table_next_row();
                ctx.label("label", "Find:");

                if ctx.editline("needle", &mut state.search_needle) {
                    action = Some(SearchAction::Search);
                }
                if !state.search_success {
                    ctx.attr_background_rgba(ctx.indexed(IndexedColor::Red));
                    ctx.attr_foreground_rgba(ctx.indexed(IndexedColor::BrightWhite));
                }
                ctx.attr_intrinsic_size(Size { width: COORD_TYPE_SAFE_MAX, height: 1 });
                if focus == StateSearchKind::Search {
                    ctx.steal_focus();
                }
                if ctx.is_focused() && ctx.consume_shortcut(vk::RETURN) {
                    action = Some(SearchAction::Search);
                }
            }

            if state.wants_search.kind == StateSearchKind::Replace {
                ctx.table_next_row();
                ctx.label("label", "Replace:");

                ctx.editline("replacement", &mut state.search_replacement);
                ctx.attr_intrinsic_size(Size { width: COORD_TYPE_SAFE_MAX, height: 1 });
                if focus == StateSearchKind::Replace {
                    ctx.steal_focus();
                }
                if ctx.is_focused() {
                    if ctx.consume_shortcut(vk::RETURN) {
                        action = Some(SearchAction::Replace);
                    } else if ctx.consume_shortcut(kbmod::CTRL_ALT | vk::RETURN) {
                        action = Some(SearchAction::ReplaceAll);
                    }
                }
            }
        }
        ctx.table_end();

        ctx.table_begin("options");
        ctx.table_set_cell_gap(Size { width: 2, height: 0 });
        {
            let mut change = false;
            let mut change_action = Some(SearchAction::Search);

            ctx.table_next_row();

            change |=
                ctx.checkbox("match-case", "Match Case", &mut state.search_options.match_case);
            change |=
                ctx.checkbox("whole-word", "Whole Word", &mut state.search_options.whole_word);
            change |= ctx.checkbox("use-regex", "Use Regex", &mut state.search_options.use_regex);
            if state.wants_search.kind == StateSearchKind::Replace
                && ctx.button("replace-all", "Replace All", ButtonStyle::default())
            {
                change = true;
                change_action = Some(SearchAction::ReplaceAll);
            }
            if ctx.button("close", "Close", ButtonStyle::default()) {
                state.wants_search.kind = StateSearchKind::Hidden;
            }

            if change {
                action = change_action;
                state.wants_search.focus = true;
                ctx.needs_rerender();
            }
        }
        ctx.table_end();
    }
    ctx.block_end();

    if let Some(action) = action {
        search_execute(ctx, state, action);
    }
}

pub enum SearchAction {
    Search,
    Replace,
    ReplaceAll,
}

pub fn search_execute(ctx: &mut Context, state: &mut State, action: SearchAction) {
    let doc = &mut state.document;

    state.search_success = match action {
        SearchAction::Search => {
            doc.buffer.borrow_mut().find_and_select(&state.search_needle, state.search_options)
        }
        SearchAction::Replace => doc.buffer.borrow_mut().find_and_replace(
            &state.search_needle,
            state.search_options,
            state.search_replacement.as_bytes(),
        ),
        SearchAction::ReplaceAll => doc.buffer.borrow_mut().find_and_replace_all(
            &state.search_needle,
            state.search_options,
            state.search_replacement.as_bytes(),
        ),
    }
    .is_ok();

    ctx.needs_rerender();
}

pub fn draw_handle_wants_exit(ctx: &mut Context, state: &mut State) {
    if !state.document.buffer.borrow().is_dirty() {
        state.exit = true;
        state.wants_exit = false;
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
                state.wants_exit = false;
            }
            Err(err) => {
                error_log_add(ctx, state, err);
                state.wants_exit = false;
            }
        },
        Action::Discard => {
            state.exit = true;
            state.wants_exit = false;
        }
        Action::Cancel => {
            state.wants_exit = false;
        }
    }

    ctx.needs_rerender();
}

pub fn draw_goto_menu(ctx: &mut Context, state: &mut State) {
    let mut done = false;

    let doc = &mut state.document;
    ctx.modal_begin("goto", "Go to Line:Column…");
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
        state.wants_goto = false;
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
