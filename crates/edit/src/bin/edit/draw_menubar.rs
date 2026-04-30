use edit::helpers::*;
use edit::tui::*;
use stdext::arena_format;

use crate::documents;
use crate::keybindings::{self, Action};
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
        if ctx.menubar_menu_begin("Edit", 'E') {
            draw_menu_edit(ctx, state);
        }
        if ctx.menubar_menu_begin("View", 'V') {
            draw_menu_view(ctx, state);
        }
        if ctx.menubar_menu_begin("Help", 'H') {
            draw_menu_help(ctx, state);
        }
    }
    ctx.menubar_end();
}

fn draw_menu_file(ctx: &mut Context, state: &mut State) {
    if ctx.menubar_menu_button("Exit", 'X', keybindings::chord(Action::Exit)) {
        state.wants_exit = true;
    }
    ctx.menubar_menu_end();
}

fn draw_menu_edit(ctx: &mut Context, state: &mut State) {
    let mut tb = state.document.buffer.borrow_mut();

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
    if ctx.menubar_menu_button(
        "Toggle Line Comment",
        'M',
        keybindings::chord(Action::ToggleLineComment),
    ) {
        let lang = tb.language();
        let line_tok = lang.and_then(|l| l.line_comment);
        let block_tok = lang.and_then(|l| l.block_comment);
        if let Some(tok) =
            line_tok.or_else(|| documents::fallback_line_comment(&state.document.path))
        {
            tb.toggle_line_comment(tok);
        } else if let Some((open, close)) = block_tok {
            tb.toggle_per_line_block_comment(open, close);
        }
        ctx.needs_rerender();
    }
    if ctx.menubar_menu_button("Toggle Block Comment", 'B', edit::input::vk::NULL) {
        if let Some((open, close)) = tb.language().and_then(|l| l.block_comment) {
            tb.toggle_block_comment(open, close);
            ctx.needs_rerender();
        } else if let Some(tok) = tb
            .language()
            .and_then(|l| l.line_comment)
            .or_else(|| documents::fallback_line_comment(&state.document.path))
        {
            // Fall back to line comments for languages with no block syntax,
            // matching vscode's blockComment behaviour.
            tb.toggle_line_comment(tok);
            ctx.needs_rerender();
        }
    }
    ctx.menubar_menu_end();
}

fn draw_menu_view(ctx: &mut Context, state: &mut State) {
    let mut tb = state.document.buffer.borrow_mut();
    let word_wrap = tb.is_word_wrap_enabled();

    if ctx.menubar_menu_button("Focus Statusbar", 'S', keybindings::chord(Action::FocusStatusbar)) {
        state.wants_statusbar_focus = true;
    }
    if ctx.menubar_menu_button("Go to Line:Column…", 'G', keybindings::chord(Action::GoToLine)) {
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
            ctx.label("description", "edit");
            ctx.attr_overflow(Overflow::TruncateTail);
            ctx.attr_position(Position::Center);

            ctx.label(
                "version",
                &arena_format!(ctx.arena(), "{}{}", "Version: ", env!("CARGO_PKG_VERSION")),
            );
            ctx.attr_overflow(Overflow::TruncateHead);
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
