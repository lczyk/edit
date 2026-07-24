use edit::framebuffer::IndexedColor;
use edit::helpers::*;
use edit::input::vk;
use edit::tui::*;
use stdext::arena_format;

use crate::state::*;

pub fn draw_statusbar(ctx: &mut Context, state: &mut State) {
    // Warning flash override: drop the normal metadata row entirely while a
    // warning is live so the message is unambiguous. Reverts on next render
    // past the deadline (see state::current_warning).
    if let Some(msg) = current_warning() {
        ctx.table_begin("statusbar-warning");
        ctx.attr_focus_well();
        ctx.attr_background_rgba(ctx.indexed(IndexedColor::BrightYellow));
        ctx.attr_foreground_rgba(ctx.indexed(IndexedColor::Black));
        ctx.attr_intrinsic_size(Size { width: COORD_TYPE_SAFE_MAX, height: 1 });
        ctx.attr_padding(Rect::two(0, 1));
        ctx.table_next_row();
        ctx.label("warning", &msg);
        ctx.attr_overflow(Overflow::TruncateTail);
        ctx.table_end();
        return;
    }

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

    let picker_clicked = ctx.button(
        "language",
        tb.language().map_or("Plain Text", |l| l.name),
        ButtonStyle::default(),
    );
    if picker_clicked {
        state.modal = Some(crate::modals::Modal::LanguagePicker);
    }
    if state.wants_statusbar_focus {
        state.wants_statusbar_focus = false;
        ctx.steal_focus();
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

    if doc.read_only {
        ctx.label("readonly", "RO");
    }

    if let Some(deadline) = state.saved_flash_until {
        if std::time::Instant::now() < deadline {
            ctx.label("saved", "Saved");
        } else {
            state.saved_flash_until = None;
        }
    }

    if tb.is_dirty() {
        ctx.label("dirty", "*");
    }

    if doc.file_changed_on_disk {
        ctx.block_begin("file-changed");
        ctx.attr_foreground_rgba(ctx.indexed(IndexedColor::Red));
        ctx.label("file-changed", edit::glyphs::file_changed());
        ctx.block_end();
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
