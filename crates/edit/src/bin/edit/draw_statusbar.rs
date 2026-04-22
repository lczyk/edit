// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use edit::framebuffer::IndexedColor;
use edit::fuzzy::score_fuzzy;
use edit::helpers::*;
use edit::icu;
use edit::input::vk;
use edit::lsh::LANGUAGES;
use edit::tui::*;
use stdext::arena::scratch_arena;
use stdext::arena_format;
use stdext::collections::BVec;

use crate::state::*;

pub fn draw_statusbar(ctx: &mut Context, state: &mut State) {
    ctx.table_begin("statusbar");
    ctx.attr_focus_well();
    ctx.attr_background_rgba(state.menubar_color_bg);
    ctx.attr_foreground_rgba(state.menubar_color_fg);
    ctx.table_set_cell_gap(Size { width: 2, height: 0 });
    ctx.attr_intrinsic_size(Size { width: COORD_TYPE_SAFE_MAX, height: 1 });
    ctx.attr_padding(Rect::two(0, 1));

    let doc = &state.document;
    let mut tb = doc.buffer.borrow_mut();

    ctx.table_next_row();

        state.wants_language_picker |= ctx.button(
            "language",
            tb.language().map_or("Plain Text", |l| l.name),
            ButtonStyle::default(),
        );
        if state.wants_statusbar_focus {
            state.wants_statusbar_focus = false;
            ctx.steal_focus();
        }

        if ctx.button("newline", if tb.is_crlf() { "CRLF" } else { "LF" }, ButtonStyle::default()) {
            let is_crlf = tb.is_crlf();
            tb.normalize_newlines(!is_crlf);
        }

        state.wants_encoding_picker |=
            ctx.button("encoding", tb.encoding(), ButtonStyle::default());
        if state.wants_encoding_picker {
            if doc.path.is_some() {
                ctx.block_begin("frame");
                ctx.attr_float(FloatSpec {
                    anchor: Anchor::Last,
                    gravity_x: 0.0,
                    gravity_y: 1.0,
                    offset_x: 0.0,
                    offset_y: 0.0,
                });
                ctx.attr_padding(Rect::two(0, 1));
                ctx.attr_border();
                {
                    if ctx.button("reopen", "Reopen with encoding…", ButtonStyle::default()) {
                        state.wants_encoding_change = StateEncodingChange::Reopen;
                    }
                    ctx.focus_on_first_present();
                    if ctx.button("convert", "Convert to encoding…", ButtonStyle::default()) {
                        state.wants_encoding_change = StateEncodingChange::Convert;
                    }
                }
                ctx.block_end();
            } else {
                // Can't reopen a file that doesn't exist.
                state.wants_encoding_change = StateEncodingChange::Convert;
            }

            if !ctx.contains_focus() {
                state.wants_encoding_picker = false;
                ctx.needs_rerender();
            }
        }

        state.wants_indentation_picker |= ctx.button(
            "indentation",
            &arena_format!(
                ctx.arena(),
                "{}:{}",
                if tb.indent_with_tabs() { "Tabs" } else { "Spaces" },
                tb.tab_size(),
            ),
            ButtonStyle::default(),
        );
        if state.wants_indentation_picker {
            ctx.table_begin("indentation-picker");
            ctx.attr_float(FloatSpec {
                anchor: Anchor::Last,
                gravity_x: 0.0,
                gravity_y: 1.0,
                offset_x: 0.0,
                offset_y: 0.0,
            });
            ctx.attr_border();
            ctx.attr_padding(Rect::two(0, 1));
            ctx.table_set_cell_gap(Size { width: 1, height: 0 });
            {
                if ctx.contains_focus() && ctx.consume_shortcut(vk::RETURN) {
                    ctx.toss_focus_up();
                }

                ctx.table_next_row();

                ctx.list_begin("type");
                ctx.focus_on_first_present();
                ctx.attr_padding(Rect::two(0, 1));
                {
                    if ctx.list_item(tb.indent_with_tabs(), "Tabs") != ListSelection::Unchanged {
                        tb.set_indent_with_tabs(true);
                        ctx.needs_rerender();
                    }
                    if ctx.list_item(!tb.indent_with_tabs(), "Spaces") != ListSelection::Unchanged {
                        tb.set_indent_with_tabs(false);
                        ctx.needs_rerender();
                    }
                }
                ctx.list_end();

                ctx.list_begin("width");
                ctx.attr_padding(Rect::two(0, 2));
                {
                    for width in 1u8..=8 {
                        let ch = [b'0' + width];
                        let label = unsafe { std::str::from_utf8_unchecked(&ch) };

                        if ctx.list_item(tb.tab_size() == width as CoordType, label)
                            != ListSelection::Unchanged
                        {
                            tb.set_tab_size(width as CoordType);
                            ctx.needs_rerender();
                        }
                    }
                }
                ctx.list_end();
            }
            ctx.table_end();

            if !ctx.contains_focus() {
                state.wants_indentation_picker = false;
                ctx.needs_rerender();
            }
        }

        ctx.label(
            "location",
            &arena_format!(
                ctx.arena(),
                "{}:{}",
                tb.cursor_logical_pos().y + 1,
                tb.cursor_logical_pos().x + 1
            ),
        );

        if tb.is_overtype() && ctx.button("overtype", "OVR", ButtonStyle::default()) {
            tb.set_overtype(false);
            ctx.needs_rerender();
        }

        if tb.is_dirty() {
            ctx.label("dirty", "*");
        }

    ctx.block_begin("filename-container");
    ctx.attr_intrinsic_size(Size { width: COORD_TYPE_SAFE_MAX, height: 1 });
    {
        ctx.label("filename", &doc.filename);
        ctx.attr_overflow(Overflow::TruncateMiddle);
        ctx.attr_position(Position::Right);
    }
    ctx.block_end();

    ctx.table_end();
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
        state.wants_language_picker = false;
        ctx.needs_rerender();
    }
}

pub fn draw_dialog_encoding_change(ctx: &mut Context, state: &mut State) {
    let encoding = state.document.buffer.borrow().encoding();
    let reopen = state.wants_encoding_change == StateEncodingChange::Reopen;
    let width = (ctx.size().width - 20).max(10);
    let height = (ctx.size().height - 10).max(10);
    let mut change = None;
    let mut done = encoding.is_empty();

    ctx.modal_begin(
        "encode",
        if reopen { "Reopen with encoding…" } else { "Convert to encoding…" },
    );
    {
        ctx.table_begin("encoding-search");
        ctx.table_set_columns(&[0, COORD_TYPE_SAFE_MAX]);
        ctx.table_set_cell_gap(Size { width: 1, height: 0 });
        ctx.inherit_focus();
        {
            ctx.table_next_row();
            ctx.inherit_focus();

            ctx.label("needle-label", "Find:");

            if ctx.editline("needle", &mut state.encoding_picker_needle) {
                encoding_picker_update_list(state);
            }
            ctx.inherit_focus();
        }
        ctx.table_end();

        ctx.scrollarea_begin("scrollarea", Size { width, height });
        ctx.attr_background_rgba(ctx.indexed_alpha(IndexedColor::Black, 1, 4));
        {
            ctx.list_begin("encodings");
            ctx.inherit_focus();

            for enc in state
                .encoding_picker_results
                .as_deref()
                .unwrap_or_else(|| icu::get_available_encodings().preferred)
            {
                if ctx.list_item(enc.canonical == encoding, enc.label) == ListSelection::Activated {
                    change = Some(enc.canonical);
                    break;
                }
                ctx.attr_overflow(Overflow::TruncateTail);
            }
            ctx.list_end();
        }
        ctx.scrollarea_end();
    }
    done |= ctx.modal_end();
    done |= change.is_some();

    if let Some(encoding) = change {
        let doc = &mut state.document;
        if reopen && doc.path.is_some() {
            let mut res = Ok(());
            if doc.buffer.borrow().is_dirty() {
                res = doc.save(None);
            }
            if res.is_ok() {
                res = doc.reread(Some(encoding));
            }
            if let Err(err) = res {
                error_log_add(ctx, state, err);
            }
        } else {
            doc.buffer.borrow_mut().set_encoding(encoding);
        }
    }

    if done {
        state.wants_encoding_change = StateEncodingChange::None;
        state.encoding_picker_needle.clear();
        state.encoding_picker_results = None;
        ctx.needs_rerender();
    }
}

fn encoding_picker_update_list(state: &mut State) {
    state.encoding_picker_results = None;

    let needle = state.encoding_picker_needle.trim_ascii();
    if needle.is_empty() {
        return;
    }

    let encodings = icu::get_available_encodings();
    let scratch = scratch_arena(None);
    let mut matches = BVec::empty();

    for enc in encodings.all {
        let local_scratch = scratch_arena(Some(&scratch));
        let (score, _) = score_fuzzy(&local_scratch, enc.label, needle, true);

        if score > 0 {
            matches.push(&*scratch, (score, *enc));
        }
    }

    matches.sort_unstable_by_key(|b| std::cmp::Reverse(b.0));
    state.encoding_picker_results = Some(Vec::from_iter(matches.iter().map(|(_, enc)| *enc)));
}

