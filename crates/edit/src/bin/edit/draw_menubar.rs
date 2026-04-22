// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use edit::helpers::*;
use edit::tui::*;
use stdext::arena_format;

use crate::keybindings::{self, Action};
use crate::settings::Settings;
use crate::state::*;

pub fn draw_menubar(ctx: &mut Context, state: &mut State) {
    ctx.menubar_begin();
    ctx.attr_background_rgba(state.menubar_color_bg);
    ctx.attr_foreground_rgba(state.menubar_color_fg);
    {
        let contains_focus = ctx.contains_focus();

        if ctx.menubar_menu_begin("File", 'F') {
            draw_menu_file(ctx, state);
        }
        if !contains_focus && ctx.consume_shortcut(keybindings::chord(Action::FocusMenubar)) {
            ctx.steal_focus();
        }
        if state.documents.active().is_some() {
            if ctx.menubar_menu_begin("Edit", 'E') {
                draw_menu_edit(ctx, state);
            }
            if ctx.menubar_menu_begin("View", 'V') {
                draw_menu_view(ctx, state);
            }
        }
        if ctx.menubar_menu_begin("Help", 'H') {
            draw_menu_help(ctx, state);
        }
    }
    ctx.menubar_end();
}

fn draw_menu_file(ctx: &mut Context, state: &mut State) {
    if ctx.menubar_menu_button("New File", 'N', keybindings::chord(Action::NewFile)) {
        draw_add_untitled_document(ctx, state);
    }
    if ctx.menubar_menu_button("Open File…", 'O', keybindings::chord(Action::OpenFile)) {
        state.wants_file_picker = StateFilePicker::Open;
    }
    if state.documents.active().is_some() {
        if ctx.menubar_menu_button("Save", 'S', keybindings::chord(Action::Save)) {
            state.wants_save = true;
        }
        if ctx.menubar_menu_button("Save As…", 'A', keybindings::chord(Action::SaveAs)) {
            state.wants_file_picker = StateFilePicker::SaveAs;
        }
    }
    #[allow(irrefutable_let_patterns)]
    if let path = Settings::borrow().path.as_path()
        && !path.as_os_str().is_empty()
        && ctx.menubar_menu_button("Preferences", 'P', keybindings::chord(Action::OpenPreferences))
    {
        match state.documents.add_file_path(path) {
            Ok(doc) => {
                if let mut tb = doc.buffer.borrow_mut()
                    && tb.text_length() == 0
                {
                    Settings::bootstrap(&mut tb);
                }
            }
            Err(err) => error_log_add(ctx, state, err),
        }
    }
    if state.documents.active().is_some()
        && ctx.menubar_menu_button("Close File", 'C', keybindings::chord(Action::CloseFile))
    {
        state.wants_close = true;
    }
    if ctx.menubar_menu_button("Exit", 'X', keybindings::chord(Action::Exit)) {
        state.wants_exit = true;
    }
    ctx.menubar_menu_end();
}

fn draw_menu_edit(ctx: &mut Context, state: &mut State) {
    let doc = state.documents.active().unwrap();
    let mut tb = doc.buffer.borrow_mut();

    if ctx.menubar_menu_button("Undo", 'U', keybindings::chord(Action::Undo)) {
        tb.undo();
        ctx.needs_rerender();
    }
    if ctx.menubar_menu_button("Redo", 'R', keybindings::chord(Action::Redo)) {
        tb.redo();
        ctx.needs_rerender();
    }
    if ctx.menubar_menu_button("Cut", 'T', keybindings::chord(Action::Cut)) {
        tb.cut(ctx.clipboard_mut());
        ctx.needs_rerender();
    }
    if ctx.menubar_menu_button("Copy", 'C', keybindings::chord(Action::Copy)) {
        tb.copy(ctx.clipboard_mut());
        ctx.needs_rerender();
    }
    if ctx.menubar_menu_button("Paste", 'P', keybindings::chord(Action::Paste)) {
        tb.paste(ctx.clipboard_ref());
        ctx.needs_rerender();
    }
    if state.wants_search.kind != StateSearchKind::Disabled {
        if ctx.menubar_menu_button("Find", 'F', keybindings::chord(Action::Find)) {
            state.wants_search.kind = StateSearchKind::Search;
            state.wants_search.focus = true;
        }
        if ctx.menubar_menu_button("Replace", 'L', keybindings::chord(Action::Replace)) {
            state.wants_search.kind = StateSearchKind::Replace;
            state.wants_search.focus = true;
        }
    }
    if ctx.menubar_menu_button("Select All", 'A', keybindings::chord(Action::SelectAll)) {
        tb.select_all();
        ctx.needs_rerender();
    }
    ctx.menubar_menu_end();
}

fn draw_menu_view(ctx: &mut Context, state: &mut State) {
    if let Some(doc) = state.documents.active() {
        let mut tb = doc.buffer.borrow_mut();
        let word_wrap = tb.is_word_wrap_enabled();

        // All values on the statusbar are currently document specific.
        if ctx.menubar_menu_button(
            "Focus Statusbar",
            'S',
            keybindings::chord(Action::FocusStatusbar),
        ) {
            state.wants_statusbar_focus = true;
        }
        if ctx.menubar_menu_button("Go to File…", 'F', keybindings::chord(Action::GoToFile)) {
            state.wants_go_to_file = true;
        }
        if ctx.menubar_menu_button("Go to Line:Column…", 'G', keybindings::chord(Action::GoToLine))
        {
            state.wants_goto = true;
        }
        if ctx.menubar_menu_checkbox(
            "Word Wrap",
            'W',
            keybindings::chord(Action::ToggleWordWrap),
            word_wrap,
        ) {
            tb.set_word_wrap(!word_wrap);
            ctx.needs_rerender();
        }
    }

    ctx.menubar_menu_end();
}

fn draw_menu_help(ctx: &mut Context, state: &mut State) {
    if ctx.menubar_menu_button("About", 'A', keybindings::chord(Action::OpenAbout)) {
        state.wants_about = true;
    }
    ctx.menubar_menu_end();
}

pub fn draw_dialog_about(ctx: &mut Context, state: &mut State) {
    ctx.modal_begin("about", "About");
    {
        ctx.block_begin("content");
        ctx.inherit_focus();
        ctx.attr_padding(Rect::three(1, 2, 1));
        {
            ctx.label("description", "Microsoft Edit");
            ctx.attr_overflow(Overflow::TruncateTail);
            ctx.attr_position(Position::Center);

            ctx.label(
                "version",
                &arena_format!(ctx.arena(), "{}{}", "Version: ", env!("CARGO_PKG_VERSION")),
            );
            ctx.attr_overflow(Overflow::TruncateHead);
            ctx.attr_position(Position::Center);

            ctx.label("copyright", "Copyright (c) Microsoft Corporation");
            ctx.attr_overflow(Overflow::TruncateTail);
            ctx.attr_position(Position::Center);

            ctx.block_begin("choices");
            ctx.inherit_focus();
            ctx.attr_padding(Rect::three(1, 2, 0));
            ctx.attr_position(Position::Center);
            {
                if ctx.button("ok", "Ok", ButtonStyle::default()) {
                    state.wants_about = false;
                }
                ctx.inherit_focus();
            }
            ctx.block_end();
        }
        ctx.block_end();
    }
    if ctx.modal_end() {
        state.wants_about = false;
    }
}
