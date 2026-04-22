// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::path::PathBuf;

use edit::framebuffer::IndexedColor;
use edit::helpers::*;
use edit::input::vk;
use edit::path;
use edit::tui::*;

use crate::state::*;

pub fn draw_save_as(ctx: &mut Context, state: &mut State) {
    if state.save_as_path.as_os_str().is_empty() {
        state.save_as_path =
            state.document.path.clone().unwrap_or_else(|| PathBuf::from(&state.document.filename));
    }

    let mut submit = false;
    let mut done = false;

    ctx.modal_begin("save-as", "Save As…");
    {
        ctx.block_begin("content");
        ctx.inherit_focus();
        ctx.attr_padding(Rect::three(1, 2, 1));
        {
            ctx.label("prompt", "Path:");

            let mut editable = state
                .save_as_path
                .as_os_str()
                .to_string_lossy()
                .into_owned();
            if ctx.editline("path", &mut editable) {
                state.save_as_path = PathBuf::from(editable);
            }
            ctx.focus_on_first_present();
            ctx.attr_intrinsic_size(Size { width: 60, height: 1 });

            if ctx.is_focused() && ctx.consume_shortcut(vk::RETURN) {
                submit = true;
            }

            ctx.table_begin("choices");
            ctx.attr_padding(Rect::three(1, 0, 0));
            ctx.attr_position(Position::Center);
            ctx.table_set_cell_gap(Size { width: 2, height: 0 });
            {
                ctx.table_next_row();
                submit |= ctx.button("ok", "Ok", ButtonStyle::default());
                if ctx.button("cancel", "Cancel", ButtonStyle::default()) {
                    done = true;
                }
            }
            ctx.table_end();
        }
        ctx.block_end();
    }
    if ctx.modal_end() {
        done = true;
    }

    if submit && !state.save_as_path.as_os_str().is_empty() {
        let candidate = path::normalize(&state.save_as_path);
        if candidate.exists() {
            state.save_as_overwrite_warning = Some(candidate);
        } else if let Err(err) = state.document.save(Some(candidate)) {
            error_log_add(ctx, state, err);
        } else {
            done = true;
            ctx.needs_rerender();
        }
    }

    if state.save_as_overwrite_warning.is_some() {
        draw_overwrite_warning(ctx, state, &mut done);
    }

    if done {
        state.wants_save_as = false;
        state.save_as_path = Default::default();
        state.save_as_overwrite_warning = None;
    }
}

fn draw_overwrite_warning(ctx: &mut Context, state: &mut State, done: &mut bool) {
    let mut save;

    ctx.modal_begin("overwrite", "Confirm Save As");
    ctx.attr_background_rgba(ctx.indexed(IndexedColor::Red));
    ctx.attr_foreground_rgba(ctx.indexed(IndexedColor::BrightWhite));
    {
        let contains_focus = ctx.contains_focus();

        ctx.label("description", "File already exists. Do you want to overwrite it?");
        ctx.attr_overflow(Overflow::TruncateTail);
        ctx.attr_padding(Rect::three(1, 2, 1));

        ctx.table_begin("choices");
        ctx.inherit_focus();
        ctx.attr_padding(Rect::three(0, 2, 1));
        ctx.attr_position(Position::Center);
        ctx.table_set_cell_gap(Size { width: 2, height: 0 });
        {
            ctx.table_next_row();
            ctx.inherit_focus();

            save = ctx.button("yes", "Yes", ButtonStyle::default());
            ctx.inherit_focus();

            if ctx.button("no", "No", ButtonStyle::default()) {
                state.save_as_overwrite_warning = None;
            }
        }
        ctx.table_end();

        if contains_focus {
            save |= ctx.consume_shortcut(vk::Y);
            if ctx.consume_shortcut(vk::N) {
                state.save_as_overwrite_warning = None;
            }
        }
    }
    if ctx.modal_end() {
        state.save_as_overwrite_warning = None;
    }

    if save
        && let Some(path) = state.save_as_overwrite_warning.take()
    {
        match state.document.save(Some(path)) {
            Ok(..) => {
                ctx.needs_rerender();
                *done = true;
            }
            Err(err) => error_log_add(ctx, state, err),
        }
    }

}
