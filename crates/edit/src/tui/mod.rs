//! An immediate mode UI framework for terminals.
//!
//! # Why immediate mode?
//!
//! This uses an "immediate mode" design, similar to [ImGui](https://github.com/ocornut/imgui).
//! The reason for this is that I expect the UI needs for any terminal application to be
//! fairly minimal, and for that purpose an immediate mode design is much simpler to use.
//!
//! So what's "immediate mode"? The primary alternative is called "retained mode".
//! The difference is that when you create a button in this framework in one frame,
//! and you stop telling this framework in the next frame, the button will vanish.
//! When you use a regular retained mode UI framework, you create the button once,
//! set up callbacks for when it is clicked, and then stop worrying about it.
//!
//! The downside of immediate mode is that your UI code _may_ become cluttered.
//! The upside however is that you cannot leak UI elements, you don't need to
//! worry about lifetimes nor callbacks, and that simple UIs are simple to write.
//!
//! More importantly though, the primary reason for this is that the
//! lack of callbacks means we can use this design across a plain C ABI,
//! which we'll need once plugins come into play. GTK's `g_signal_connect`
//! shows that the alternative can be rather cumbersome.
//!
//! # Design overview
//!
//! While this file is fairly lengthy, the overall algorithm is simple.
//! On the first frame ever:
//! * Prepare an empty `arena_next`.
//! * Parse the incoming [`input::Input`] which should be a resize event.
//! * Create a new [`Context`] instance and give it the caller.
//! * Now the caller will draw their UI with the [`Context`] by calling the
//!   various [`Context`] UI methods, such as [`Context::block_begin()`] and
//!   [`Context::block_end()`]. These two are the basis which all other UI
//!   elements are built upon by the way. Each UI element that is created gets
//!   allocated onto `arena_next` and inserted into the UI tree.
//!   That tree works exactly like the DOM tree in HTML: Each node in the tree
//!   has a parent, children, and siblings. The tree layout at the end is then
//!   a direct mirror of the code "layout" that created it.
//! * Once the caller is done and drops the [`Context`], it'll secretly call
//!   `report_context_completion`. This causes a number of things:
//!   * The DOM tree that was built is stored in `prev_tree`.
//!   * A hashmap of all nodes is built and stored in `prev_node_map`.
//!   * `arena_next` is swapped with `arena_prev`.
//!   * Each UI node is measured and laid out.
//! * Now the caller is expected to repeat this process with a [`None`]
//!   input event until [`Tui::needs_settling()`] returns false.
//!   This is necessary, because when [`Context::button()`] returns `true`
//!   in one frame, it may change the state in the caller's code
//!   and require another frame to be drawn.
//! * Finally a call to [`Tui::render()`] will render the UI tree into the
//!   framebuffer and return VT output.
//!
//! On every subsequent frame the process is similar, but one crucial element
//! of any immediate mode UI framework is added:
//! Now when the caller draws their UI, the various [`Context`] UI elements
//! have access to `prev_node_map` and the previously built UI tree.
//! This allows the UI framework to reuse the previously computed layout for
//! hit tests, caching scroll offsets, and so on.
//!
//! In the end it looks very similar:
//! * Prepare an empty `arena_next`.
//! * Parse the incoming [`input::Input`]...
//!   * **BUT** now we can hit-test mouse clicks onto the previously built
//!     UI tree. This way we can delegate focus on left mouse clicks.
//! * Create a new [`Context`] instance and give it the caller.
//! * The caller draws their UI with the [`Context`]...
//!   * **BUT** we can preserve the UI state across frames.
//! * Continue rendering until [`Tui::needs_settling()`] returns false.
//! * And the final call to [`Tui::render()`].
//!
//! # Classnames and node IDs
//!
//! So how do we find which node from the previous tree correlates to the
//! current node? Each node needs to be constructed with a "classname".
//! The classname is hashed with the parent node ID as the seed. This derived
//! hash is then used as the new child node ID. Under the assumption that the
//! collision likelihood of the hash function is low, this serves as true IDs.
//!
//! This has the nice added property that finding a node with the same ID
//! guarantees that all of the parent nodes must have equivalent IDs as well.
//! This turns "is the focus anywhere inside this subtree" into an O(1) check.
//!
//! The reason "classnames" are used is because I was hoping to add theming
//! in the future with a syntax similar to CSS (simplified, however).
//!
//! # Example
//!
//! ```
//! use edit::helpers::Size;
//! use edit::input::Input;
//! use edit::tui::*;
//! use stdext::{arena, arena_format};
//!
//! struct State {
//!     counter: i32,
//! }
//!
//! fn main() {
//!     arena::init(128 * 1024 * 1024).unwrap();
//!
//!     // Create a `Tui` instance which holds state across frames.
//!     let mut tui = Tui::new().unwrap();
//!     let mut state = State { counter: 0 };
//!     let input = Input::Resize(Size { width: 80, height: 24 });
//!
//!     // Pass the input to the TUI.
//!     {
//!         let mut ctx = tui.create_context(Some(input));
//!         draw(&mut ctx, &mut state);
//!     }
//!
//!     // Continue until the layout has settled.
//!     while tui.needs_settling() {
//!         let mut ctx = tui.create_context(None);
//!         draw(&mut ctx, &mut state);
//!     }
//!
//!     // Render the output.
//!     let scratch = arena::scratch_arena(None);
//!     let output = tui.render(&*scratch);
//!     println!("{}", output);
//! }
//!
//! fn draw(ctx: &mut Context, state: &mut State) {
//!     ctx.table_begin("classname");
//!     {
//!         ctx.table_next_row();
//!
//!         // Thanks to the lack of callbacks, we can use a primitive
//!         // if condition here, as well as in any potential C code.
//!         if ctx.button("button", "Click me!", ButtonStyle::default()) {
//!             state.counter += 1;
//!         }
//!
//!         // Similarly, formatting and showing labels is straightforward.
//!         // It's impossible to forget updating the label this way.
//!         ctx.label("label", &arena_format!(ctx.arena(), "Counter: {}", state.counter));
//!     }
//!     ctx.table_end();
//! }
//! ```

use std::collections::HashMap;
#[cfg(debug_assertions)]
use std::collections::HashSet;
use std::sync::atomic::{AtomicU32, Ordering};
use std::{io, iter, mem, ptr, time};

use stdext::arena::{Arena, scratch_arena};
use stdext::collections::{BString, BVec};
use stdext::{arena_format, opt_ptr_eq, str_from_raw_parts};

use crate::buffer::{
    CursorMovement, MINIMAP_SOURCE_ROWS_PER_CELL, RcTextBuffer, TextBuffer, TextBufferCell,
};
use crate::clipboard::Clipboard;
use crate::document::WriteableDocument;
use crate::framebuffer::{Attributes, Framebuffer, INDEXED_COLORS_COUNT, IndexedColor};
use crate::hash::*;
use crate::helpers::*;
use crate::input::{InputKeyMod, kbmod, vk};
use crate::oklab::StraightRgba;
use crate::{input, paint, unicode};
use stdext::simd;

/// Map a scrollbar thumb drag to a content scroll offset.
///
/// `trackable` is the travel available to the thumb -- the track height
/// minus the thumb itself -- and `scrollable` is the content distance
/// that travel maps onto. Both scrollbars in this file derive those two
/// differently (a textarea counts visual lines and allows one viewport of
/// overscroll; a scrollarea measures its child) but agree from here on,
/// so the arithmetic lives in one place.
///
/// Returns `drag_start` unchanged when there is nowhere to travel.
/// `trackable` really can reach zero: `draw_scrollbar` rounds the thumb
/// to the nearest whole row and floors it at one, so a one-row track
/// showing two rows of content yields a thumb as tall as its track --
/// which divided straight through would panic.
fn scrollbar_drag_offset(
    drag_start: CoordType,
    delta_y: CoordType,
    scrollable: CoordType,
    trackable: CoordType,
) -> CoordType {
    if scrollable <= 0 || trackable <= 0 {
        return drag_start;
    }
    drag_start + (delta_y as i64 * scrollable as i64 / trackable as i64) as CoordType
}

mod node;
mod textarea;

use node::*;

const ROOT_ID: u64 = 0x14057B7EF767814F; // Knuth's MMIX constant
const SHIFT_TAB: InputKey = vk::TAB.with_modifiers(kbmod::SHIFT);
const KBMOD_FOR_WORD_NAV: InputKeyMod =
    if cfg!(any(target_os = "macos", target_os = "ios")) { kbmod::ALT } else { kbmod::CTRL };
// Primary application modifier: Cmd on macOS, Ctrl elsewhere. The default
// for the standard Cut/Copy/Paste/Undo/Redo/SelectAll chords in the textarea.
const KBMOD_PRIMARY: InputKeyMod =
    if cfg!(any(target_os = "macos", target_os = "ios")) { kbmod::CMD } else { kbmod::CTRL };

/// The chords a textarea handles itself: editing operations that belong to the
/// widget rather than to any one screen, so they work the same in the document
/// and in a modal's input field.
///
/// Defaults to `KBMOD_PRIMARY` plus the usual letter. A host with a keybinding
/// config calls [`set_textarea_chords`] to replace them -- otherwise rebinding
/// e.g. `undo` would edit the menubar's label and nothing else. `vk::NULL`
/// leaves an action unbound, which is what an empty binding means.
#[derive(Clone, Copy)]
pub struct TextareaChords {
    pub cut: InputKey,
    pub copy: InputKey,
    pub paste: InputKey,
    pub undo: InputKey,
    pub redo: InputKey,
    pub select_all: InputKey,
}

impl Default for TextareaChords {
    fn default() -> Self {
        Self {
            cut: vk::X.with_modifiers(KBMOD_PRIMARY),
            copy: vk::C.with_modifiers(KBMOD_PRIMARY),
            paste: vk::V.with_modifiers(KBMOD_PRIMARY),
            undo: vk::Z.with_modifiers(KBMOD_PRIMARY),
            redo: vk::Y.with_modifiers(KBMOD_PRIMARY),
            select_all: vk::A.with_modifiers(KBMOD_PRIMARY),
        }
    }
}

/// Six slots, in the order [`TextareaChords`] declares them. Atomics rather
/// than a lock because this is written once at startup and read per keystroke.
static TEXTAREA_CHORDS: [AtomicU32; 6] = [const { AtomicU32::new(u32::MAX) }; 6];

/// Overrides the chords a textarea handles. Call once, before the first frame.
pub fn set_textarea_chords(chords: TextareaChords) {
    let vals = [chords.cut, chords.copy, chords.paste, chords.undo, chords.redo, chords.select_all];
    for (slot, key) in TEXTAREA_CHORDS.iter().zip(vals) {
        slot.store(key.value(), Ordering::Relaxed);
    }
}

fn textarea_chords() -> TextareaChords {
    // u32::MAX marks "never set"; no real chord can collide with it, since the
    // key occupies the low 24 bits and the modifiers the top 8.
    if TEXTAREA_CHORDS[0].load(Ordering::Relaxed) == u32::MAX {
        return TextareaChords::default();
    }
    let k = |i: usize| InputKey::new(TEXTAREA_CHORDS[i].load(Ordering::Relaxed));
    TextareaChords { cut: k(0), copy: k(1), paste: k(2), undo: k(3), redo: k(4), select_all: k(5) }
}

type Input<'input> = input::Input<'input>;
type InputKey = input::InputKey;
type InputMouseState = input::InputMouseState;

/// Since [`TextBuffer`] creation and management is expensive,
/// we cache instances of them for reuse between frames.
/// This is used for [`Context::editline()`].
struct CachedTextBuffer {
    node_id: u64,
    editor: RcTextBuffer,
    seen: bool,
}

/// Since [`Context::editline()`] and [`Context::textarea()`]
/// do almost the same thing, this abstracts over the two.
enum TextBufferPayload<'a> {
    Editline(&'a mut dyn WriteableDocument),
    Textarea(RcTextBuffer),
}

/// In order for the TUI to show the correct Ctrl/Alt/Shift
/// translations, this struct lets you set them.
pub struct ModifierTranslations {
    pub ctrl: &'static str,
    pub alt: &'static str,
    pub shift: &'static str,
    pub cmd: &'static str,
}

/// Controls to which node the floater is anchored.
#[derive(Default, Clone, Copy, PartialEq, Eq)]
pub enum Anchor {
    /// The floater is attached relative to the node created last.
    #[default]
    Last,
    /// The floater is attached relative to the current node (= parent of new nodes).
    Parent,
    /// The floater is attached relative to the root node (= usually the viewport).
    Root,
}

/// Controls the position of the floater. See [`Context::attr_float`].
#[derive(Default)]
pub struct FloatSpec {
    /// Controls to which node the floater is anchored.
    pub anchor: Anchor,
    // Specifies the origin of the container relative to the container size. [0, 1]
    pub gravity_x: f32,
    pub gravity_y: f32,
    // Specifies an offset from the origin in cells.
    pub offset_x: f32,
    pub offset_y: f32,
}

/// Informs you about the change that was made to the list selection.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ListSelection {
    /// The selection wasn't changed.
    Unchanged,
    /// The selection was changed to the current list item.
    Selected,
    /// The selection was changed to the current list item
    /// *and* the item was also activated (Enter or Double-click).
    Activated,
}

/// Controls the position of a node relative to its parent.
#[derive(Default)]
pub enum Position {
    /// The child is stretched to fill the parent.
    #[default]
    Stretch,
    /// The child is positioned at the left edge of the parent.
    Left,
    /// The child is positioned at the center of the parent.
    Center,
    /// The child is positioned at the right edge of the parent.
    Right,
}

/// Controls the text overflow behaviour of a label
/// when the text doesn't fit the container.
#[derive(Default, Clone, Copy, PartialEq, Eq)]
pub enum Overflow {
    /// Text is simply cut off when it doesn't fit.
    #[default]
    Clip,
    /// An ellipsis is shown at the end of the text.
    TruncateHead,
    /// An ellipsis is shown in the middle of the text.
    TruncateMiddle,
    /// An ellipsis is shown at the beginning of the text.
    TruncateTail,
}

/// Controls the style with which a button label renders
#[derive(Clone, Copy)]
pub struct ButtonStyle {
    accelerator: Option<char>,
    checked: Option<bool>,
    bracketed: bool,
}

impl ButtonStyle {
    /// Draw an accelerator label: `[_E_xample button]` or `[Example button(X)]`
    ///
    /// Must provide an upper-case ASCII character.
    pub fn accelerator(self, char: char) -> Self {
        Self { accelerator: Some(char), ..self }
    }
    /// Draw a checkbox prefix: `[🗹 Example Button]`
    pub fn checked(self, checked: bool) -> Self {
        Self { checked: Some(checked), ..self }
    }
    /// Draw with or without brackets: `[Example Button]` or `Example Button`
    pub fn bracketed(self, bracketed: bool) -> Self {
        Self { bracketed, ..self }
    }
}

impl Default for ButtonStyle {
    fn default() -> Self {
        Self {
            accelerator: None,
            checked: None,
            bracketed: true, // Default style for most buttons. Brackets may be disabled e.g. for buttons in menus
        }
    }
}

/// There's two types of lifetimes the TUI code needs to manage:
/// * Across frames
/// * Per frame
///
/// [`Tui`] manages the first one. It's also the entrypoint for
/// everything else you may want to do.
pub struct Tui {
    /// Arena used for the previous frame.
    arena_prev: Arena,
    /// Arena used for the current frame.
    arena_next: Arena,
    /// The UI tree built in the previous frame.
    /// This refers to memory in `arena_prev`.
    prev_tree: Tree<'static>,
    /// A hashmap of all nodes built in the previous frame.
    /// This refers to memory in `arena_prev`.
    prev_node_map: NodeMap<'static>,
    /// The framebuffer used for rendering.
    framebuffer: Framebuffer,

    modifier_translations: ModifierTranslations,
    floater_default_bg: StraightRgba,
    floater_default_fg: StraightRgba,
    modal_default_bg: StraightRgba,
    modal_default_fg: StraightRgba,

    /// Last known terminal size.
    ///
    /// This lives here instead of [`Context`], because we need to
    /// track the state across frames and input events.
    /// This also applies to the remaining members in this block below.
    size: Size,
    /// Last known mouse position.
    mouse_position: Point,
    /// Between mouse down and up, the position where the mouse was pressed.
    /// Otherwise, this contains Point::MIN.
    mouse_down_position: Point,
    /// Node ID of the node that was clicked on.
    /// Used for tracking drag targets.
    left_mouse_down_target: u64,
    /// Timestamp of the last mouse up event.
    /// Used for tracking double/triple clicks.
    mouse_up_timestamp: std::time::Instant,
    /// The current mouse state.
    mouse_state: InputMouseState,
    /// Whether the mouse is currently being dragged.
    mouse_is_drag: bool,
    /// The number of clicks that have happened in a row.
    /// Gets reset when the mouse was released for a while.
    mouse_click_counter: CoordType,
    /// The path to the node that was clicked on.
    mouse_down_node_path: Vec<u64>,
    /// The position of the first click in a double/triple click series.
    first_click_position: Point,
    /// The node ID of the node that was first clicked on
    /// in a double/triple click series.
    first_click_target: u64,

    /// Persistent per-modal drag offsets in cells, keyed by modal node id.
    /// Added on top of the centered float position so a dragged modal stays put.
    modal_drag_offset: HashMap<u64, Point>,
    /// Node id of the modal currently being title-bar dragged (0 = none).
    modal_drag_target: u64,
    /// The modal's committed offset, snapshotted when the drag started.
    modal_drag_start: Point,

    /// Path to the currently focused node.
    focused_node_path: Vec<u64>,
    /// Contains the last element in [`Tui::focused_node_path`].
    /// This way we can track if the focus changed, because then we
    /// need to scroll the node into view if it's within a scrollarea.
    focused_node_for_scrolling: u64,

    /// A list of cached text buffers used for [`Context::editline()`].
    cached_text_buffers: Vec<CachedTextBuffer>,

    /// The clipboard contents.
    clipboard: Clipboard,

    settling_have: i32,
    settling_want: i32,
    read_timeout: time::Duration,

    /// Tui-level anim state (frame timing + floater open-timer table).
    /// See [`paint::anim::TuiAnimState`].
    anim: paint::anim::TuiAnimState,
}

impl Tui {
    /// Creates a new [`Tui`] instance for storing state across frames.
    pub fn new() -> io::Result<Self> {
        let arena_prev = Arena::new(128 * MEBI)?;
        let arena_next = Arena::new(128 * MEBI)?;
        // SAFETY: Since `prev_tree` refers to `arena_prev`/`arena_next`, from its POV the lifetime
        // is `'static`, requiring us to use `transmute` to circumvent the borrow checker.
        let prev_tree = Tree::new(unsafe { mem::transmute::<&Arena, &Arena>(&arena_next) });

        let mut tui = Self {
            arena_prev,
            arena_next,
            prev_tree,
            prev_node_map: Default::default(),
            framebuffer: Framebuffer::new(),

            modifier_translations: ModifierTranslations {
                ctrl: "Ctrl",
                alt: "Alt",
                shift: "Shift",
                cmd: "Cmd",
            },
            floater_default_bg: StraightRgba::zero(),
            floater_default_fg: StraightRgba::zero(),
            modal_default_bg: StraightRgba::zero(),
            modal_default_fg: StraightRgba::zero(),

            size: Size { width: 0, height: 0 },
            mouse_position: Point::MIN,
            mouse_down_position: Point::MIN,
            left_mouse_down_target: 0,
            mouse_up_timestamp: std::time::Instant::now(),
            mouse_state: InputMouseState::None,
            mouse_is_drag: false,
            mouse_click_counter: 0,
            mouse_down_node_path: Vec::with_capacity(16),
            first_click_position: Point::MIN,
            first_click_target: 0,

            modal_drag_offset: HashMap::new(),
            modal_drag_target: 0,
            modal_drag_start: Point::default(),

            focused_node_path: Vec::with_capacity(16),
            focused_node_for_scrolling: ROOT_ID,

            cached_text_buffers: Vec::with_capacity(16),

            clipboard: Default::default(),

            settling_have: 0,
            settling_want: 0,
            read_timeout: time::Duration::MAX,

            anim: paint::anim::TuiAnimState::default(),
        };
        Self::clean_node_path(&mut tui.mouse_down_node_path);
        Self::clean_node_path(&mut tui.focused_node_path);
        Ok(tui)
    }

    /// Sets up the framebuffer's color palette.
    pub fn setup_indexed_colors(&mut self, colors: [StraightRgba; INDEXED_COLORS_COUNT]) {
        self.framebuffer.set_indexed_colors(colors);
    }

    /// Enables ANSI-16 SGR emission for palette-matched colors. See
    /// [`crate::framebuffer::Framebuffer::set_emit_indexed_codes`].
    pub fn setup_emit_indexed_codes(&mut self, value: bool) {
        self.framebuffer.set_emit_indexed_codes(value);
    }

    /// Set up translations for Ctrl/Alt/Shift modifiers.
    pub fn setup_modifier_translations(&mut self, translations: ModifierTranslations) {
        self.modifier_translations = translations;
    }

    /// Set the default background color for floaters (dropdowns, etc.).
    pub fn set_floater_default_bg(&mut self, color: StraightRgba) {
        self.floater_default_bg = color;
    }

    /// Set the default foreground color for floaters (dropdowns, etc.).
    pub fn set_floater_default_fg(&mut self, color: StraightRgba) {
        self.floater_default_fg = color;
    }

    /// Set the default background color for modals.
    pub fn set_modal_default_bg(&mut self, color: StraightRgba) {
        self.modal_default_bg = color;
    }

    /// Set the default foreground color for modals.
    pub fn set_modal_default_fg(&mut self, color: StraightRgba) {
        self.modal_default_fg = color;
    }

    /// If the TUI is currently running animations, etc.,
    /// this will return a timeout smaller than [`time::Duration::MAX`].
    pub fn read_timeout(&mut self) -> time::Duration {
        mem::replace(&mut self.read_timeout, time::Duration::MAX)
    }

    /// Caps `read_timeout` to one animation frame, so the main loop
    /// wakes up in time to render the next step of an in-flight
    /// animation. Idempotent: callers may invoke this from any anim
    /// site (floater open, scroll/cursor advance, line-move trail)
    /// without worrying about ordering vs the existing cap.
    fn request_animation_frame(&mut self) {
        if self.read_timeout > paint::FRAME_INTERVAL {
            self.read_timeout = paint::FRAME_INTERVAL;
        }
    }

    /// Returns the viewport size.
    pub fn size(&self) -> Size {
        // We don't use the size stored in the framebuffer, because until
        // `render()` is called, the framebuffer will use a stale size.
        self.size
    }

    /// Returns an indexed color from the framebuffer.
    #[inline]
    pub fn indexed(&self, index: IndexedColor) -> StraightRgba {
        self.framebuffer.indexed(index)
    }

    /// Returns an indexed color from the framebuffer with the given alpha.
    /// See [`Framebuffer::indexed_alpha()`].
    #[inline]
    pub fn indexed_alpha(
        &self,
        index: IndexedColor,
        numerator: u32,
        denominator: u32,
    ) -> StraightRgba {
        self.framebuffer.indexed_alpha(index, numerator, denominator)
    }

    /// Returns a color in contrast with the given color.
    /// See [`Framebuffer::contrasted()`].
    pub fn contrasted(&self, color: StraightRgba) -> StraightRgba {
        self.framebuffer.contrasted(color)
    }

    /// Returns the clipboard.
    pub fn clipboard_ref(&self) -> &Clipboard {
        &self.clipboard
    }

    /// Returns the clipboard (mutable).
    pub fn clipboard_mut(&mut self) -> &mut Clipboard {
        &mut self.clipboard
    }

    /// Starts a new frame and returns a [`Context`] for it.
    pub fn create_context<'a, 'input>(
        &'a mut self,
        input: Option<Input<'input>>,
    ) -> Context<'a, 'input> {
        // SAFETY: Since we have a unique `&mut self`, nothing is holding onto `arena_prev`,
        // which will become `arena_next` and get reset. It's safe to reset and reuse its memory.
        mem::swap(&mut self.arena_prev, &mut self.arena_next);
        unsafe { self.arena_next.reset(0) };

        // In the input handler below we transformed a mouse up into a release event.
        // Now, a frame later, we must reset it back to none, to stop it from triggering things.
        // Same for Scroll events.
        if self.mouse_state > InputMouseState::Right {
            self.mouse_down_position = Point::MIN;
            self.mouse_down_node_path.clear();
            self.left_mouse_down_target = 0;
            self.mouse_state = InputMouseState::None;
            self.mouse_is_drag = false;
            self.modal_drag_target = 0;
        }

        let now = std::time::Instant::now();
        let mut input_text = None;
        let mut input_keyboard = None;
        let mut input_mouse_modifiers = kbmod::NONE;
        let mut input_mouse_click = 0;
        let mut input_scroll_delta = Point { x: 0, y: 0 };
        // `input_consumed` should be `true` if we're in the settling phase which is indicated by
        // `self.needs_settling() == true`. However, there's a possibility for it being true from
        // a previous frame, and we do have fresh new input. In that case want `input_consumed`
        // to be false of course which is ensured by checking for `input.is_none()`.
        let input_consumed = self.needs_settling() && input.is_none();

        if self.scroll_to_focused() {
            self.needs_more_settling();
        }

        match input {
            None => {}
            Some(Input::Resize(resize)) => {
                assert!(resize.width > 0 && resize.height > 0);
                assert!(resize.width < 32768 && resize.height < 32768);
                self.size = resize;
            }
            Some(Input::Text(text)) => {
                input_text = Some(text);
                // TODO: the .len()==1 check causes us to ignore keyboard inputs that are faster than we process them.
                // For instance, imagine the user presses "A" twice and we happen to read it in a single chunk.
                // This causes us to ignore the keyboard input here. We need a way to inform the caller over
                // how much of the input text we actually processed in a single frame. Or perhaps we could use
                // the needs_settling logic?
                if text.len() == 1 {
                    let ch = text.as_bytes()[0];
                    input_keyboard = InputKey::from_ascii(ch as char)
                }
            }
            Some(Input::Paste(paste)) => {
                let clipboard = self.clipboard_mut();
                clipboard.write(paste);
                clipboard.mark_as_synchronized();
                input_keyboard = Some(KBMOD_PRIMARY | vk::V);
            }
            Some(Input::Keyboard(keyboard)) => {
                input_keyboard = Some(keyboard);
            }
            Some(Input::Mouse(mouse)) => {
                let mut next_state = mouse.state;
                let next_position = mouse.position;
                let next_scroll = mouse.scroll;
                let mouse_down = self.mouse_state == InputMouseState::None
                    && next_state != InputMouseState::None;
                let mouse_up = self.mouse_state != InputMouseState::None
                    && next_state == InputMouseState::None;
                let is_scroll = next_scroll != Point::default();
                let is_drag = self.mouse_state == InputMouseState::Left
                    && next_state == InputMouseState::Left
                    && next_position != self.mouse_position;

                let mut hovered_node = None; // Needed for `mouse_down`
                let mut focused_node = None; // Needed for `mouse_down` and `is_click`
                let mut hovered_modal = None; // (id, outer) of the topmost hit modal
                if mouse_down || mouse_up {
                    // Roots (aka windows) are ordered in Z order, so we iterate
                    // them in reverse order, from topmost to bottommost.
                    for root in self.prev_tree.iterate_roots_rev() {
                        // Find the node that contains the cursor.
                        Tree::visit_all(root, root, true, |node| {
                            let n = node.borrow();
                            if !n.outer_clipped.contains(next_position) {
                                // Skip the entire sub-tree, because it doesn't contain the cursor.
                                return VisitControl::SkipChildren;
                            }
                            hovered_node = Some(node);
                            if n.attributes.focusable {
                                focused_node = Some(node);
                            }
                            VisitControl::Continue
                        });

                        // This root/window contains the cursor.
                        // We don't care about any lower roots.
                        if hovered_node.is_some() {
                            let r = root.borrow();
                            if matches!(r.content, NodeContent::Modal(_)) {
                                hovered_modal = Some((r.id, r.outer));
                            }
                            break;
                        }

                        // This root is modal and swallows all clicks,
                        // no matter whether the click was inside it or not.
                        if matches!(root.borrow().content, NodeContent::Modal(_)) {
                            break;
                        }
                    }
                }

                if is_scroll {
                    next_state = self.mouse_state;
                } else if is_drag {
                    self.mouse_is_drag = true;
                    // Carry a title-bar drag into the modal's persistent offset.
                    if self.modal_drag_target != 0 {
                        let off = Point {
                            x: self.modal_drag_start.x
                                + (next_position.x - self.mouse_down_position.x),
                            y: self.modal_drag_start.y
                                + (next_position.y - self.mouse_down_position.y),
                        };
                        self.modal_drag_offset.insert(self.modal_drag_target, off);
                    }
                } else if mouse_down {
                    // Transition from no mouse input to some mouse input --> Record the mouse down position.
                    Self::build_node_path(hovered_node, &mut self.mouse_down_node_path);

                    // On left-mouse-down we change focus.
                    let mut target = 0;
                    if next_state == InputMouseState::Left {
                        target = focused_node.map_or(0, |n| n.borrow().id);
                        Self::build_node_path(focused_node, &mut self.focused_node_path);
                        self.needs_more_settling(); // See `needs_more_settling()`.
                    }

                    // Double-/Triple-/Etc.-clicks are triggered on mouse-down,
                    // unlike the first initial click, which is triggered on mouse-up.
                    if self.mouse_click_counter != 0 {
                        // Allow a 1-cell tolerance on repeat-click position so
                        // tiny mouse jitter between clicks doesn't reset the
                        // counter and demote a double-click to two singles.
                        let dx = (self.first_click_position.x - next_position.x).abs();
                        let dy = (self.first_click_position.y - next_position.y).abs();
                        let position_drift = dx > 1 || dy > 1;
                        if self.first_click_target != target
                            || position_drift
                            || (now - self.mouse_up_timestamp)
                                > std::time::Duration::from_millis(250)
                        {
                            // If the cursor moved / the focus changed in between, or if the user did a slow click,
                            // we reset the click counter. On mouse-up it'll transition to a regular click.
                            self.mouse_click_counter = 0;
                            self.first_click_position = Point::MIN;
                            self.first_click_target = 0;
                        } else {
                            self.mouse_click_counter += 1;
                            input_mouse_click = self.mouse_click_counter;
                        };
                    }

                    // Gets reset at the start of this function.
                    self.left_mouse_down_target = target;
                    self.mouse_down_position = next_position;

                    // Grab the modal title bar (its top border row) to start dragging.
                    self.modal_drag_target = 0;
                    if next_state == InputMouseState::Left
                        && let Some((id, rect)) = hovered_modal
                        && next_position.y == rect.top
                        && next_position.x >= rect.left
                        && next_position.x < rect.right
                    {
                        self.modal_drag_target = id;
                        self.modal_drag_start =
                            self.modal_drag_offset.get(&id).copied().unwrap_or_default();
                    }
                } else if mouse_up {
                    // Transition from some mouse input to no mouse input --> The mouse button was released.
                    next_state = InputMouseState::Release;

                    let target = focused_node.map_or(0, |n| n.borrow().id);

                    if self.left_mouse_down_target == 0 || self.left_mouse_down_target != target {
                        // If `left_mouse_down_target == 0`, then it wasn't a left-click, in which case
                        // the target gets reset. Same, if the focus changed in between any clicks.
                        self.mouse_click_counter = 0;
                        self.first_click_position = Point::MIN;
                        self.first_click_target = 0;
                    } else if self.mouse_click_counter == 0 {
                        // No focus change, and no previous clicks? This is an initial, regular click.
                        self.mouse_click_counter = 1;
                        self.first_click_position = self.mouse_down_position;
                        self.first_click_target = target;
                        input_mouse_click = 1;
                    }

                    self.mouse_up_timestamp = now;
                }

                input_mouse_modifiers = mouse.modifiers;
                input_scroll_delta = next_scroll;
                self.mouse_position = next_position;
                self.mouse_state = next_state;
            }
        }

        if !input_consumed {
            // Every time there's input, we naturally need to re-render at least once.
            self.settling_have = 0;
            self.settling_want = 1;
        }

        // TODO: There should be a way to do this without unsafe.
        // Allocating from the arena borrows the arena, and so allocating the tree here borrows self.
        // This conflicts with us passing a mutable reference to `self` into the struct below.
        let tree = Tree::new(unsafe { mem::transmute::<&Arena, &Arena>(&self.arena_next) });

        Context {
            tui: self,

            input_text,
            input_keyboard,
            input_mouse_modifiers,
            input_mouse_click,
            input_scroll_delta,
            input_consumed,

            tree,
            last_modal: None,
            focused_node: None,
            next_block_id_mixin: 0,
            needs_settling: false,

            #[cfg(debug_assertions)]
            seen_ids: HashSet::new(),
        }
    }

    fn report_context_completion<'a>(&'a mut self, ctx: &mut Context<'a, '_>) {
        // If this hits, you forgot to block_end() somewhere. The best way to figure
        // out where is to do a binary search of commenting out code in main.rs.
        crate::sanity_assert!(
            dangling_parent,
            ctx.tree.current_node.borrow().stack_parent.is_none(),
            "Dangling parent! Did you miss a block_end?"
        );

        // End the root node.
        ctx.block_end();

        // Ensure that focus doesn't escape the active modal.
        if let Some(node) = ctx.last_modal
            && !self.is_subtree_focused(&node.borrow())
        {
            ctx.steal_focus_for(node);
        }

        // If nodes have appeared or disappeared, we need to re-render.
        // Same, if the focus has changed (= changes the highlight color, etc.).
        let mut needs_settling = ctx.needs_settling;
        needs_settling |= self.prev_tree.checksum != ctx.tree.checksum;

        // Adopt the new tree and recalculate the node hashmap.
        //
        // SAFETY: The memory used by the tree is owned by the `self.arena_next` right now.
        // Stealing the tree here thus doesn't need to copy any memory unless someone resets the arena.
        // (The arena is reset in `reset()` above.)
        unsafe {
            self.prev_tree = mem::transmute_copy(&ctx.tree);
            self.prev_node_map = NodeMap::new(mem::transmute(&self.arena_next), &self.prev_tree);
        }

        let mut focus_path_pop_min = 0;
        // If the user pressed Escape, we move the focus to a parent node.
        if !ctx.input_consumed && ctx.consume_shortcut(vk::ESCAPE) {
            focus_path_pop_min = 1;
        }

        // Remove any unknown nodes from the focus path.
        // It's important that we do this after the tree has been swapped out,
        // so that pop_focusable_node() has access to the newest version of the tree.
        needs_settling |= self.pop_focusable_node(focus_path_pop_min);

        // `needs_more_settling()` depends on the current value
        // of `settling_have` and so we increment it first.
        self.settling_have += 1;

        if needs_settling {
            self.needs_more_settling();
        }

        // Remove cached text editors that are no longer in use.
        self.cached_text_buffers.retain(|c| c.seen);

        for root in Tree::iterate_siblings(Some(self.prev_tree.root_first)) {
            let mut root = root.borrow_mut();
            root.compute_intrinsic_size(unsafe { mem::transmute(&self.arena_next) });
        }

        let viewport = self.size.as_rect();

        for root in Tree::iterate_siblings(Some(self.prev_tree.root_first)) {
            let mut root = root.borrow_mut();
            let root = &mut *root;

            if let Some(float) = &root.attributes.float {
                let mut x = 0;
                let mut y = 0;

                if let Some(node) = root.parent {
                    let node = node.borrow();
                    x = node.outer.left;
                    y = node.outer.top;
                }

                let size = root.intrinsic_to_outer();

                x += (float.offset_x - float.gravity_x * size.width as f32) as CoordType;
                y += (float.offset_y - float.gravity_y * size.height as f32) as CoordType;

                // A dragged modal keeps a persistent offset; clamp it so the
                // whole window stays on-screen (rather than getting clipped).
                if let Some(off) = self.modal_drag_offset.get(&root.id) {
                    x = (x + off.x).clamp(0, (viewport.right - size.width).max(0));
                    y = (y + off.y).clamp(0, (viewport.bottom - size.height).max(0));
                }

                root.outer.left = x;
                root.outer.top = y;
                root.outer.right = x + size.width;
                root.outer.bottom = y + size.height;
                root.outer = root.outer.intersect(viewport);
            } else {
                root.outer = viewport;
            }

            root.inner = root.outer_to_inner(root.outer);
            root.outer_clipped = root.outer;
            root.inner_clipped = root.inner;

            let outer = root.outer;
            root.layout_children(outer);
        }
    }

    fn build_node_path(node: Option<&NodeCell>, path: &mut Vec<u64>) {
        path.clear();
        if let Some(mut node) = node {
            loop {
                let n = node.borrow();
                path.push(n.id);
                node = match n.parent {
                    Some(parent) => parent,
                    None => break,
                };
            }
            path.reverse();
        } else {
            path.push(ROOT_ID);
        }
    }

    fn clean_node_path(path: &mut Vec<u64>) {
        Self::build_node_path(None, path);
    }

    /// After you finished processing all input, continue redrawing your UI until this returns false.
    pub fn needs_settling(&mut self) -> bool {
        self.settling_have <= self.settling_want
    }

    fn needs_more_settling(&mut self) {
        // If the focus has changed, the new node may need to be re-rendered.
        // Same, every time we encounter a previously unknown node via `get_prev_node`,
        // because that means it likely failed to get crucial information such as the layout size.
        crate::sanity_assert!(
            settling_have_bound,
            self.settling_have <= 15,
            "settling_have={}",
            self.settling_have
        );
        self.settling_want = (self.settling_have + 1).min(20);
    }

    /// Renders the last frame into the framebuffer and returns the VT output.
    pub fn render<'a>(&mut self, arena: &'a Arena) -> BString<'a> {
        let now = time::Instant::now();
        self.anim.dt_secs = paint::anim::frame_dt_secs(self.anim.last_frame_time, now);
        self.anim.last_frame_time = Some(now);
        self.anim.frame = self.anim.frame.wrapping_add(1);

        // Drop slide-animation entries for nodes that no longer exist in the
        // current tree (dropdown closed, modal dismissed). Lookup uses the
        // same prev_node_map the renderer walks. Same treatment for
        // per-textarea anim state -- entries persist across frames keyed
        // by node id, drop when the textarea unmounts.
        self.anim.floater_opened_at.retain(|id, _| self.prev_node_map.get(*id).is_some());
        self.anim.textareas.retain(|id, _| self.prev_node_map.get(*id).is_some());
        // Drop drag offsets for dismissed modals so a reopened one re-centers.
        self.modal_drag_offset.retain(|id, _| self.prev_node_map.get(*id).is_some());

        self.framebuffer.flip(self.size);
        for child in self.prev_tree.iterate_roots() {
            let mut child = child.borrow_mut();
            self.render_node(&mut child);
        }
        self.framebuffer.render(arena)
    }

    /// Recursively shrinks `outer_clipped.bottom` / `inner_clipped.bottom`
    /// across the subtree rooted at `node` so descendants don't peek past
    /// the parent's animated clip line.
    fn clip_subtree_bottom(node: &mut Node, bottom: CoordType) {
        node.outer_clipped.bottom = node.outer_clipped.bottom.min(bottom);
        node.inner_clipped.bottom = node.inner_clipped.bottom.min(bottom);
        for child in Tree::iterate_siblings(node.children.first) {
            Self::clip_subtree_bottom(&mut child.borrow_mut(), bottom);
        }
    }

    /// Recursively intersects each node's clipped rects with the vertical
    /// band [top, bottom]. Mirror of `clip_subtree_bottom` for both edges.
    fn clip_subtree_band(node: &mut Node, top: CoordType, bottom: CoordType) {
        node.outer_clipped.top = node.outer_clipped.top.max(top);
        node.outer_clipped.bottom = node.outer_clipped.bottom.min(bottom);
        node.inner_clipped.top = node.inner_clipped.top.max(top);
        node.inner_clipped.bottom = node.inner_clipped.bottom.min(bottom);
        for child in Tree::iterate_siblings(node.children.first) {
            Self::clip_subtree_band(&mut child.borrow_mut(), top, bottom);
        }
    }

    /// Recursively renders each node and its children.
    #[allow(clippy::only_used_in_recursion)]
    fn render_node(&mut self, node: &mut Node) {
        // Slide-down open animation: shrink the visible bottom on the first
        // ~150ms after the node first appears. We mutate `outer_clipped` /
        // `inner_clipped` of the entire subtree in place; the tree is
        // rebuilt next frame so the mutation is harmless beyond render.
        // no_animations() guard is here, not in advance_floater_open --
        // single dispatch point at the Tui level.
        if !crate::glyphs::no_animations()
            && let Some(clip) = paint::anim::advance_floater_open(
                &mut self.anim.floater_opened_at,
                node.id,
                node.outer_clipped.top,
                node.outer_clipped.bottom,
                node.attributes.slide_down,
                node.attributes.scale_in,
                time::Instant::now(),
            )
        {
            match clip {
                paint::anim::FloaterClip::Band(top, bottom) => {
                    Self::clip_subtree_band(node, top, bottom);
                }
                paint::anim::FloaterClip::Bottom(bottom) => {
                    Self::clip_subtree_bottom(node, bottom);
                }
            }
            self.request_animation_frame();
        }

        let outer_clipped = node.outer_clipped;
        if outer_clipped.is_empty() {
            return;
        }

        if node.attributes.bordered {
            // ┌────┐
            {
                let scratch = scratch_arena(None);
                let mut fill = BString::empty();
                fill.push(&*scratch, crate::glyphs::box_tl());
                fill.push_repeat(
                    &*scratch,
                    crate::glyphs::box_h(),
                    (outer_clipped.right - outer_clipped.left - 2) as usize,
                );
                fill.push(&*scratch, crate::glyphs::box_tr());
                self.framebuffer.replace_text(
                    outer_clipped.top,
                    outer_clipped.left,
                    outer_clipped.right,
                    &fill,
                );
            }

            // │    │
            {
                let scratch = scratch_arena(None);
                let mut fill = BString::empty();
                fill.push(&*scratch, crate::glyphs::box_v());
                fill.push_repeat(
                    &*scratch,
                    ' ',
                    (outer_clipped.right - outer_clipped.left - 2) as usize,
                );
                fill.push(&*scratch, crate::glyphs::box_v());

                for y in outer_clipped.top + 1..outer_clipped.bottom - 1 {
                    self.framebuffer.replace_text(
                        y,
                        outer_clipped.left,
                        outer_clipped.right,
                        &fill,
                    );
                }
            }

            // └────┘
            {
                let scratch = scratch_arena(None);
                let mut fill = BString::empty();
                fill.push(&*scratch, crate::glyphs::box_bl());
                fill.push_repeat(
                    &*scratch,
                    crate::glyphs::box_h(),
                    (outer_clipped.right - outer_clipped.left - 2) as usize,
                );
                fill.push(&*scratch, crate::glyphs::box_br());
                self.framebuffer.replace_text(
                    outer_clipped.bottom - 1,
                    outer_clipped.left,
                    outer_clipped.right,
                    &fill,
                );
            }
        }

        if node.attributes.float.is_some() {
            if !node.attributes.bordered {
                let scratch = scratch_arena(None);
                let mut fill = BString::empty();
                fill.push_repeat(
                    &*scratch,
                    ' ',
                    (outer_clipped.right - outer_clipped.left) as usize,
                );

                for y in outer_clipped.top..outer_clipped.bottom {
                    self.framebuffer.replace_text(
                        y,
                        outer_clipped.left,
                        outer_clipped.right,
                        &fill,
                    );
                }
            }

            self.framebuffer.replace_attr(outer_clipped, Attributes::All, Attributes::None);

            if matches!(node.content, NodeContent::Modal(_)) {
                let rect =
                    Rect { left: 0, top: 0, right: self.size.width, bottom: self.size.height };
                let dim = self.indexed_alpha(IndexedColor::Background, 1, 2);
                self.framebuffer.blend_bg(rect, dim);
                self.framebuffer.blend_fg(rect, dim);
            }
        }

        self.framebuffer.blend_bg(outer_clipped, node.attributes.bg);
        self.framebuffer.blend_fg(outer_clipped, node.attributes.fg);

        if node.attributes.reverse {
            self.framebuffer.reverse(outer_clipped);
        }

        let inner = node.inner;
        let inner_clipped = node.inner_clipped;
        if inner_clipped.is_empty() {
            return;
        }

        match &mut node.content {
            NodeContent::Modal(title) if !title.is_empty() => {
                self.framebuffer.replace_text(
                    node.outer.top,
                    node.outer.left + 2,
                    node.outer.right - 1,
                    title,
                );
            }
            NodeContent::Text(content) => self.render_styled_text(
                inner,
                node.intrinsic_size.width,
                &content.text,
                &content.chunks,
                content.overflow,
            ),
            NodeContent::Textarea(tc) => {
                self.render_textarea_content(tc, node.id, inner, inner_clipped);
            }
            NodeContent::Scrollarea(sc) => {
                let content = node.children.first.unwrap().borrow();
                let track = Rect {
                    left: inner.right,
                    top: inner.top,
                    right: inner.right + 1,
                    bottom: inner.bottom,
                };
                sc.thumb_height = self.framebuffer.draw_scrollbar(
                    outer_clipped,
                    track,
                    sc.scroll_offset.y,
                    content.intrinsic_size.height,
                );
            }
            _ => {}
        }

        for child in Tree::iterate_siblings(node.children.first) {
            let mut child = child.borrow_mut();
            self.render_node(&mut child);
        }

        // Under `--quirks=-color`, the usual bg/fg/reverse focus cues are invisible.
        // Overlay `<>` on the focused leaf -- but only for single-line
        // widgets (buttons, menu items). Multi-line content like the
        // textarea would otherwise get markers down the entire visible
        // body, which is wrong.
        if crate::glyphs::no_color()
            && self.is_node_focused(node.id)
            && outer_clipped.right - outer_clipped.left >= 2
            && outer_clipped.bottom - outer_clipped.top == 1
            && !matches!(node.content, NodeContent::Textarea(_) | NodeContent::Scrollarea(_))
        {
            let y = outer_clipped.top;
            self.framebuffer.replace_text(y, outer_clipped.left, outer_clipped.left + 1, "<");
            self.framebuffer.replace_text(y, outer_clipped.right - 1, outer_clipped.right, ">");
        }
    }

    fn render_styled_text(
        &mut self,
        target: Rect,
        actual_width: CoordType,
        text: &str,
        chunks: &[StyledTextChunk],
        overflow: Overflow,
    ) {
        let target_width = target.width();
        // The section of `text` that is skipped by the ellipsis.
        let mut skipped = 0..0;
        // The number of columns skipped by the ellipsis.
        let mut skipped_cols = 0;

        if overflow == Overflow::Clip || target_width >= actual_width {
            self.framebuffer.replace_text(target.top, target.left, target.right, text);
        } else {
            let bytes = text.as_bytes();
            let mut cfg = unicode::MeasurementConfig::new(&bytes);

            match overflow {
                Overflow::Clip => unreachable!(),
                Overflow::TruncateHead => {
                    let beg = cfg.goto_visual(Point { x: actual_width - target_width + 1, y: 0 });
                    skipped = 0..beg.offset;
                    skipped_cols = beg.visual_pos.x - 1;
                }
                Overflow::TruncateMiddle => {
                    let mid_beg_x = (target_width - 1) / 2;
                    let mid_end_x = actual_width - target_width / 2;
                    let beg = cfg.goto_visual(Point { x: mid_beg_x, y: 0 });
                    let end = cfg.goto_visual(Point { x: mid_end_x, y: 0 });
                    skipped = beg.offset..end.offset;
                    skipped_cols = end.visual_pos.x - beg.visual_pos.x - 1;
                }
                Overflow::TruncateTail => {
                    let end = cfg.goto_visual(Point { x: target_width - 1, y: 0 });
                    skipped_cols = actual_width - end.visual_pos.x - 1;
                    skipped = end.offset..text.len();
                }
            }

            let scratch = scratch_arena(None);

            let mut modified = BString::empty();
            modified.reserve(&*scratch, text.len() + 3);
            modified.push_str(&*scratch, &text[..skipped.start]);
            modified.push(&*scratch, crate::glyphs::ellipsis_char());
            modified.push_str(&*scratch, &text[skipped.end..]);

            self.framebuffer.replace_text(target.top, target.left, target.right, &modified);
        }

        if !chunks.is_empty() {
            let bytes = text.as_bytes();
            let mut cfg = unicode::MeasurementConfig::new(&bytes).with_cursor(unicode::Cursor {
                visual_pos: Point { x: target.left, y: 0 },
                ..Default::default()
            });

            let mut iter = chunks.iter().peekable();

            while let Some(chunk) = iter.next() {
                let beg = chunk.offset;
                let end = iter.peek().map_or(text.len(), |c| c.offset);

                if beg >= skipped.start && end <= skipped.end {
                    // Chunk is fully inside the text skipped by the ellipsis.
                    // We don't need to render it at all.
                    continue;
                }

                if beg < skipped.start {
                    let beg = cfg.goto_offset(beg).visual_pos.x;
                    let end = cfg.goto_offset(end.min(skipped.start)).visual_pos.x;
                    let rect =
                        Rect { left: beg, top: target.top, right: end, bottom: target.bottom };
                    self.framebuffer.blend_fg(rect, chunk.fg);
                    self.framebuffer.replace_attr(rect, chunk.attr, chunk.attr);
                }

                if end > skipped.end {
                    let beg = cfg.goto_offset(beg.max(skipped.end)).visual_pos.x - skipped_cols;
                    let end = cfg.goto_offset(end).visual_pos.x - skipped_cols;
                    let rect =
                        Rect { left: beg, top: target.top, right: end, bottom: target.bottom };
                    self.framebuffer.blend_fg(rect, chunk.fg);
                    self.framebuffer.replace_attr(rect, chunk.attr, chunk.attr);
                }
            }
        }
    }

    fn was_mouse_down_on_node(&self, id: u64) -> bool {
        self.mouse_down_node_path.last() == Some(&id)
    }

    fn was_mouse_down_on_subtree(&self, node: &Node) -> bool {
        self.mouse_down_node_path.get(node.depth) == Some(&node.id)
    }

    fn is_node_focused(&self, id: u64) -> bool {
        // We construct the focused_node_path always with at least 1 element (the root id).
        unsafe { *self.focused_node_path.last().unwrap_unchecked() == id }
    }

    fn is_subtree_focused(&self, node: &Node) -> bool {
        self.focused_node_path.get(node.depth) == Some(&node.id)
    }

    fn is_subtree_focused_alt(&self, id: u64, depth: usize) -> bool {
        self.focused_node_path.get(depth) == Some(&id)
    }

    fn pop_focusable_node(&mut self, pop_minimum: usize) -> bool {
        let last_before = self.focused_node_path.last().cloned().unwrap_or(0);

        // Remove `pop_minimum`-many nodes from the end of the focus path.
        let path = &self.focused_node_path[..];
        let path = &path[..path.len().saturating_sub(pop_minimum)];
        let mut len = 0;

        for (i, &id) in path.iter().enumerate() {
            // Truncate the path so that it only contains nodes that still exist.
            let Some(node) = self.prev_node_map.get(id) else {
                break;
            };

            let n = node.borrow();
            // If the caller requested upward movement, pop out of the current focus void, if any.
            // This is kind of janky, to be fair.
            if pop_minimum != 0 && n.attributes.focus_void {
                break;
            }

            // Skip over those that aren't focusable.
            if n.attributes.focusable {
                // At this point `n.depth == i` should be true,
                // but I kind of don't want to rely on that.
                len = i + 1;
            }
        }

        self.focused_node_path.truncate(len);

        // If it's empty now, push `ROOT_ID` because there must always be >=1 element.
        if self.focused_node_path.is_empty() {
            self.focused_node_path.push(ROOT_ID);
        }

        // Return true if the focus path changed.
        let last_after = self.focused_node_path.last().cloned().unwrap_or(0);
        last_before != last_after
    }

    // Scroll the focused node(s) into view inside scrollviews
    fn scroll_to_focused(&mut self) -> bool {
        let focused_id = self.focused_node_path.last().cloned().unwrap_or(0);
        if self.focused_node_for_scrolling == focused_id {
            return false;
        }

        let Some(node) = self.prev_node_map.get(focused_id) else {
            // Node not found because we're using the old layout tree.
            // Retry in the next rendering loop.
            return true;
        };

        let mut node = node.borrow_mut();
        let mut scroll_to = node.outer;

        while node.parent.is_some() && node.attributes.float.is_none() {
            let n = &mut *node;
            if let NodeContent::Scrollarea(sc) = &mut n.content {
                let off_y = sc.scroll_offset.y.max(0);
                let mut y = off_y;
                y = y.min(scroll_to.top - n.inner.top + off_y);
                y = y.max(scroll_to.bottom - n.inner.bottom + off_y);
                sc.scroll_offset.y = y;
                scroll_to = n.outer;
            }
            node = node.parent.unwrap().borrow_mut();
        }

        self.focused_node_for_scrolling = focused_id;
        true
    }
}

/// Context is a temporary object that is created for each frame.
/// Its primary purpose is to build a UI tree.
pub struct Context<'a, 'input> {
    tui: &'a mut Tui,

    /// Current text input, if any.
    input_text: Option<&'input str>,
    /// Current keyboard input, if any.
    input_keyboard: Option<InputKey>,
    input_mouse_modifiers: InputKeyMod,
    input_mouse_click: CoordType,
    /// By how much the mouse wheel was scrolled since the last frame.
    input_scroll_delta: Point,
    input_consumed: bool,

    tree: Tree<'a>,
    last_modal: Option<&'a NodeCell<'a>>,
    focused_node: Option<&'a NodeCell<'a>>,
    next_block_id_mixin: u64,
    needs_settling: bool,

    #[cfg(debug_assertions)]
    seen_ids: HashSet<u64>,
}

impl<'a> Drop for Context<'a, '_> {
    fn drop(&mut self) {
        let tui: &'a mut Tui = unsafe { mem::transmute(&mut *self.tui) };
        tui.report_context_completion(self);
    }
}

impl<'a> Context<'a, '_> {
    /// Get an arena for temporary allocations such as for [`arena_format`].
    pub fn arena(&self) -> &'a Arena {
        // TODO:
        // `Context` borrows `Tui` for lifetime 'a, so `self.tui` should be `&'a Tui`, right?
        // And if I do `&self.tui.arena` then that should be 'a too, right?
        // Searching for and failing to find a workaround for this was _very_ annoying.
        //
        // SAFETY: Both the returned reference and its allocations outlive &self.
        unsafe { mem::transmute::<&'_ Arena, &'a Arena>(&self.tui.arena_next) }
    }

    /// Returns the viewport size.
    pub fn size(&self) -> Size {
        self.tui.size()
    }

    /// Returns an indexed color from the framebuffer.
    #[inline]
    pub fn indexed(&self, index: IndexedColor) -> StraightRgba {
        self.tui.framebuffer.indexed(index)
    }

    /// Returns an indexed color from the framebuffer with the given alpha.
    /// See [`Framebuffer::indexed_alpha()`].
    #[inline]
    pub fn indexed_alpha(
        &self,
        index: IndexedColor,
        numerator: u32,
        denominator: u32,
    ) -> StraightRgba {
        self.tui.framebuffer.indexed_alpha(index, numerator, denominator)
    }

    /// Returns a color in contrast with the given color.
    /// See [`Framebuffer::contrasted()`].
    pub fn contrasted(&self, color: StraightRgba) -> StraightRgba {
        self.tui.framebuffer.contrasted(color)
    }

    /// Returns the clipboard.
    pub fn clipboard_ref(&self) -> &Clipboard {
        &self.tui.clipboard
    }

    /// Returns the clipboard (mutable).
    pub fn clipboard_mut(&mut self) -> &mut Clipboard {
        &mut self.tui.clipboard
    }

    /// Tell the UI framework that your state changed and you need another layout pass.
    pub fn needs_rerender(&mut self) {
        // If this hits, the call stack is responsible is trying to deadlock you.
        crate::sanity_assert!(
            rerender_settling_bound,
            self.tui.settling_have < 15,
            "settling_have={}",
            self.tui.settling_have
        );
        self.needs_settling = true;
    }

    /// Begins a generic UI block (container) with a unique ID derived from the given `classname`.
    pub fn block_begin(&mut self, classname: &'static str) {
        let parent = self.tree.current_node;

        let mut id = hash_str(parent.borrow().id, classname);
        if self.next_block_id_mixin != 0 {
            id = hash(id, &self.next_block_id_mixin.to_ne_bytes());
            self.next_block_id_mixin = 0;
        }

        // If this hits, you have tried to create a block with the same ID as a previous one
        // somewhere up this call stack. Change the classname, or use next_block_id_mixin().
        // TODO: HashMap
        #[cfg(debug_assertions)]
        if !self.seen_ids.insert(id) {
            panic!("Duplicate node ID: {id:x}");
        }

        let node = Tree::alloc_node(self.arena());
        {
            let mut n = node.borrow_mut();
            n.id = id;
            n.classname = classname;
        }

        self.tree.push_child(node);
    }

    /// Ends the current UI block, returning to its parent container.
    pub fn block_end(&mut self) {
        self.tree.pop_stack();
        self.block_end_move_focus();
    }

    fn block_end_move_focus(&mut self) {
        // At this point, it's more like "focus_well?" instead of "focus_well!".
        let focus_well = self.tree.last_node;

        // Remember the focused node, if any, because once the code below runs,
        // we need it for the `Tree::visit_all` call.
        if self.is_focused() {
            self.focused_node = Some(focus_well);
        }

        // The mere fact that there's a `focused_node` indicates that we're the
        // first `block_end()` call that's a focus well and also contains the focus.
        let Some(focused) = self.focused_node else {
            return;
        };

        // Filter down to nodes that are focus wells and contain the focus. They're
        // basically the "tab container". We test for the node depth to ensure that
        // we don't accidentally pick a focus well next to or inside the focused node.
        {
            let n = focus_well.borrow();
            if !n.attributes.focus_well || n.depth > focused.borrow().depth {
                return;
            }
        }

        // Filter down to Tab/Shift+Tab inputs.
        if self.input_consumed {
            return;
        }
        let Some(input) = self.input_keyboard else {
            return;
        };
        if !matches!(input, SHIFT_TAB | vk::TAB) {
            return;
        }

        let forward = input == vk::TAB;
        let mut focused_start = focused;
        let mut focused_next = focused;

        // We may be in a focus void right now (= doesn't want to be tabbed into),
        // so first we must go up the tree until we're outside of it.
        loop {
            if ptr::eq(focused_start, focus_well) {
                // If we hit the root / focus well, we weren't in a focus void,
                // and can reset `focused_before` to the current focused node.
                focused_start = focused;
                break;
            }

            focused_start = focused_start.borrow().parent.unwrap();
            if focused_start.borrow().attributes.focus_void {
                break;
            }
        }

        Tree::visit_all(focus_well, focused_start, forward, |node| {
            let n = node.borrow();
            if n.attributes.focusable && !ptr::eq(node, focused_start) {
                focused_next = node;
                VisitControl::Stop
            } else if n.attributes.focus_void {
                VisitControl::SkipChildren
            } else {
                VisitControl::Continue
            }
        });

        if ptr::eq(focused_next, focused_start) {
            return;
        }

        Tui::build_node_path(Some(focused_next), &mut self.tui.focused_node_path);
        self.set_input_consumed();
        self.needs_rerender();
    }

    /// Mixes in an extra value to the next UI block's ID for uniqueness.
    /// Use this when you build a list of items with the same classname.
    pub fn next_block_id_mixin(&mut self, id: u64) {
        self.next_block_id_mixin = id;
    }

    fn attr_focusable(&mut self) {
        let mut last_node = self.tree.last_node.borrow_mut();
        last_node.attributes.focusable = true;
    }

    /// If this is the first time the current node is being drawn,
    /// it'll steal the active focus.
    pub fn focus_on_first_present(&mut self) {
        let steal = {
            let mut last_node = self.tree.last_node.borrow_mut();
            last_node.attributes.focusable = true;
            self.tui.prev_node_map.get(last_node.id).is_none()
        };
        if steal {
            self.steal_focus();
        }
    }

    /// Steals the focus unconditionally.
    pub fn steal_focus(&mut self) {
        self.steal_focus_for(self.tree.last_node);
    }

    fn steal_focus_for(&mut self, node: &NodeCell<'a>) {
        if !self.tui.is_node_focused(node.borrow().id) {
            Tui::build_node_path(Some(node), &mut self.tui.focused_node_path);
            self.needs_rerender();
        }
    }

    /// If the current node owns the focus, it'll be given to the parent.
    pub fn toss_focus_up(&mut self) {
        if self.tui.pop_focusable_node(1) {
            self.needs_rerender();
        }
    }

    /// If the parent node owns the focus, it'll be given to the current node.
    pub fn inherit_focus(&mut self) {
        let mut last_node = self.tree.last_node.borrow_mut();
        let Some(parent) = last_node.parent else {
            return;
        };

        last_node.attributes.focusable = true;

        // Mark the parent as focusable, so that if the user presses Escape,
        // and `block_end` bubbles the focus up the tree, it'll stop on our parent,
        // which will then focus us on the next iteration.
        let mut parent = parent.borrow_mut();
        parent.attributes.focusable = true;

        if self.tui.is_node_focused(parent.id) {
            self.needs_rerender();
            self.tui.focused_node_path.push(last_node.id);
        }
    }

    /// Causes keyboard focus to be unable to escape this node and its children.
    /// It's a "well" because if the focus is inside it, it can't escape.
    pub fn attr_focus_well(&mut self) {
        let mut last_node = self.tree.last_node.borrow_mut();
        last_node.attributes.focus_well = true;
    }

    /// Explicitly sets the intrinsic size of the current node.
    /// The intrinsic size is the size the node ideally wants to be.
    pub fn attr_intrinsic_size(&mut self, size: Size) {
        let mut last_node = self.tree.last_node.borrow_mut();
        last_node.intrinsic_size = size;
        last_node.intrinsic_size_set = true;
    }

    /// Turns the current node into a floating node,
    /// like a popup, modal or a tooltip.
    pub fn attr_float(&mut self, spec: FloatSpec) {
        let last_node = self.tree.last_node;
        let anchor = {
            let ln = last_node.borrow();
            match spec.anchor {
                Anchor::Last if ln.siblings.prev.is_some() => ln.siblings.prev,
                Anchor::Last | Anchor::Parent => ln.parent,
                // By not giving such floats a parent, they get the same origin as the original root node,
                // but they also gain their own "root id" in the tree. That way, their focus path is totally unique,
                // which means that we can easily check if a modal is open by calling `is_focused()` on the original root.
                Anchor::Root => None,
            }
        };

        self.tree.move_node_to_root(last_node, anchor);

        let mut ln = last_node.borrow_mut();
        ln.attributes.focus_well = true;
        ln.attributes.float = Some(FloatAttributes {
            gravity_x: spec.gravity_x.clamp(0.0, 1.0),
            gravity_y: spec.gravity_y.clamp(0.0, 1.0),
            offset_x: spec.offset_x,
            offset_y: spec.offset_y,
        });
        ln.attributes.bg = self.tui.floater_default_bg;
        ln.attributes.fg = self.tui.floater_default_fg;
    }

    /// Gives the current node a border.
    pub fn attr_border(&mut self) {
        let mut last_node = self.tree.last_node.borrow_mut();
        last_node.attributes.bordered = true;
    }

    /// Marks the current node as a slide-down floater. The first frame it
    /// appears, the visible height clips to 0 and grows to the full layout
    /// height over a short window. Cheap fade-in for menubar dropdowns.
    pub fn attr_slide_down(&mut self) {
        let mut last_node = self.tree.last_node.borrow_mut();
        last_node.attributes.slide_down = true;
    }

    /// Marks the current node as a scale-in floater. The visible band starts
    /// as a one-row strip at the node's vertical centre and expands to the
    /// full layout height. Suits centred modals.
    pub fn attr_scale_in(&mut self) {
        let mut last_node = self.tree.last_node.borrow_mut();
        last_node.attributes.scale_in = true;
    }

    /// Sets the current node's position inside the parent.
    pub fn attr_position(&mut self, align: Position) {
        let mut last_node = self.tree.last_node.borrow_mut();
        last_node.attributes.position = align;
    }

    /// Assigns padding to the current node.
    pub fn attr_padding(&mut self, padding: Rect) {
        let mut last_node = self.tree.last_node.borrow_mut();
        last_node.attributes.padding = Self::normalize_rect(padding);
    }

    fn normalize_rect(rect: Rect) -> Rect {
        Rect {
            left: rect.left.max(0),
            top: rect.top.max(0),
            right: rect.right.max(0),
            bottom: rect.bottom.max(0),
        }
    }

    /// Assigns a sRGB background color to the current node.
    pub fn attr_background_rgba(&mut self, bg: StraightRgba) {
        let mut last_node = self.tree.last_node.borrow_mut();
        last_node.attributes.bg = bg;
    }

    /// Assigns a sRGB foreground color to the current node.
    pub fn attr_foreground_rgba(&mut self, fg: StraightRgba) {
        let mut last_node = self.tree.last_node.borrow_mut();
        last_node.attributes.fg = fg;
    }

    /// Applies reverse-video to the current node:
    /// Background and foreground colors are swapped.
    pub fn attr_reverse(&mut self) {
        let mut last_node = self.tree.last_node.borrow_mut();
        last_node.attributes.reverse = true;
    }

    /// Checks if the current keyboard input matches the given shortcut,
    /// consumes it if it is and returns true in that case.
    pub fn consume_shortcut(&mut self, shortcut: InputKey) -> bool {
        if !self.input_consumed && self.input_keyboard == Some(shortcut) {
            self.set_input_consumed();
            true
        } else {
            false
        }
    }

    /// Returns current keyboard input, if any.
    /// Returns None if the input was already consumed.
    pub fn keyboard_input(&self) -> Option<InputKey> {
        if self.input_consumed { None } else { self.input_keyboard }
    }

    /// Mouse-wheel delta since the last frame. Reported in cells; `y > 0`
    /// is wheel-down (content scrolls up), `y < 0` is wheel-up. Always
    /// observable -- the field is read by the textarea path
    /// unconditionally but it isn't "consumed" in the same sense as
    /// keyboard input, so external callers can react to it
    /// (follow/pause toggles, etc.) regardless of consumption order.
    pub fn scroll_delta(&self) -> Point {
        self.input_scroll_delta
    }

    #[inline]
    pub fn set_input_consumed(&mut self) {
        crate::sanity_assert!(
            input_not_already_consumed,
            !self.input_consumed,
            "set_input_consumed called twice"
        );
        self.set_input_consumed_unchecked();
    }

    #[inline]
    fn set_input_consumed_unchecked(&mut self) {
        self.input_consumed = true;
    }

    /// Returns whether the mouse was pressed down on the current node.
    pub fn was_mouse_down(&mut self) -> bool {
        let last_node = self.tree.last_node.borrow();
        self.tui.was_mouse_down_on_node(last_node.id)
    }

    /// Returns whether the mouse was pressed down on the current node's subtree.
    pub fn contains_mouse_down(&mut self) -> bool {
        let last_node = self.tree.last_node.borrow();
        self.tui.was_mouse_down_on_subtree(&last_node)
    }

    /// Returns whether the current node is focused.
    pub fn is_focused(&mut self) -> bool {
        let last_node = self.tree.last_node.borrow();
        self.tui.is_node_focused(last_node.id)
    }

    /// Returns whether the current node's subtree is focused.
    pub fn contains_focus(&mut self) -> bool {
        let last_node = self.tree.last_node.borrow();
        self.tui.is_subtree_focused(&last_node)
    }

    /// Begins a modal window. Call [`Context::modal_end()`].
    pub fn modal_begin(&mut self, classname: &'static str, title: &str) {
        self.block_begin(classname);
        self.attr_float(FloatSpec {
            anchor: Anchor::Root,
            gravity_x: 0.5,
            gravity_y: 0.5,
            offset_x: self.tui.size.width as f32 * 0.5,
            offset_y: self.tui.size.height as f32 * 0.5,
        });
        self.attr_border();
        self.attr_background_rgba(self.tui.modal_default_bg);
        self.attr_foreground_rgba(self.tui.modal_default_fg);
        self.attr_focus_well();
        self.attr_scale_in();
        self.focus_on_first_present();

        let mut last_node = self.tree.last_node.borrow_mut();
        let title = if title.is_empty() {
            BString::empty()
        } else {
            arena_format!(self.arena(), " {} ", title)
        };
        last_node.content = NodeContent::Modal(title);
        self.last_modal = Some(self.tree.last_node);
    }

    /// Ends the current modal window block.
    /// Returns true if the user pressed Escape (a request to close).
    pub fn modal_end(&mut self) -> bool {
        self.block_end();

        // Consume the input unconditionally, so that the root (the "main window")
        // doesn't accidentally receive any input via `consume_shortcut()`.
        if self.contains_focus() {
            let exit = !self.input_consumed && self.input_keyboard == Some(vk::ESCAPE);
            self.set_input_consumed_unchecked();
            exit
        } else {
            false
        }
    }

    /// Begins a table block. Call [`Context::table_end()`].
    /// Tables are the primary way to create a grid layout,
    /// and to layout controls on a single row (= a table with 1 row).
    pub fn table_begin(&mut self, classname: &'static str) {
        self.block_begin(classname);

        let mut last_node = self.tree.last_node.borrow_mut();
        last_node.content = NodeContent::Table(TableContent {
            columns: BVec::empty(),
            cell_gap: Default::default(),
        });
    }

    /// Assigns widths to the columns of the current table.
    /// By default, the table will left-align all columns.
    pub fn table_set_columns(&mut self, columns: &[CoordType]) {
        let mut last_node = self.tree.last_node.borrow_mut();
        if let NodeContent::Table(spec) = &mut last_node.content {
            spec.columns.clear();
            spec.columns.extend_from_slice(self.arena(), columns);
        } else {
            crate::sanity_assert!(table_set_columns_outside_table, false);
        }
    }

    /// Assigns the gap between cells in the current table.
    pub fn table_set_cell_gap(&mut self, cell_gap: Size) {
        let mut last_node = self.tree.last_node.borrow_mut();
        if let NodeContent::Table(spec) = &mut last_node.content {
            spec.cell_gap = cell_gap;
        } else {
            crate::sanity_assert!(table_set_cell_gap_outside_table, false);
        }
    }

    /// Starts the next row in the current table.
    pub fn table_next_row(&mut self) {
        {
            let current_node = self.tree.current_node.borrow();

            // If this is the first call to table_next_row() inside a new table, the
            // current_node will refer to the table. Otherwise, it'll refer to the current row.
            if !matches!(current_node.content, NodeContent::Table(_)) {
                let Some(parent) = current_node.parent else {
                    return;
                };

                let parent = parent.borrow();
                // Neither the current nor its parent nodes are a table?
                // You definitely called this outside of a table block.
                crate::sanity_assert!(
                    table_next_row_outside_table,
                    matches!(parent.content, NodeContent::Table(_)),
                    "table_next_row called outside a table block"
                );

                self.block_end();
                self.table_end_row();

                self.next_block_id_mixin(parent.child_count as u64);
            }
        }

        self.block_begin("row");
    }

    fn table_end_row(&mut self) {
        self.table_move_focus(vk::LEFT, vk::RIGHT);
    }

    /// Ends the current table block.
    pub fn table_end(&mut self) {
        let current_node = self.tree.current_node.borrow();

        // If this is the first call to table_next_row() inside a new table, the
        // current_node will refer to the table. Otherwise, it'll refer to the current row.
        if !matches!(current_node.content, NodeContent::Table(_)) {
            self.block_end();
            self.table_end_row();
        }

        self.block_end(); // table
        self.table_move_focus(vk::UP, vk::DOWN);
    }

    fn table_move_focus(&mut self, prev_key: InputKey, next_key: InputKey) {
        // Filter down to table rows that are focused.
        if !self.contains_focus() {
            return;
        }

        // Filter down to our prev/next inputs.
        if self.input_consumed {
            return;
        }
        let Some(input) = self.input_keyboard else {
            return;
        };
        if input != prev_key && input != next_key {
            return;
        }

        let container = self.tree.last_node;
        let Some(&focused_id) = self.tui.focused_node_path.get(container.borrow().depth + 1) else {
            return;
        };

        let mut prev_next = NodeSiblings { prev: None, next: None };
        let mut focused = None;

        // Iterate through the cells in the row / the rows in the table, looking for focused_id.
        // Take note of the previous and next focusable cells / rows around the focused one.
        for cell in Tree::iterate_siblings(container.borrow().children.first) {
            let n = cell.borrow();
            if n.id == focused_id {
                focused = Some(cell);
            } else if n.attributes.focusable {
                if focused.is_none() {
                    prev_next.prev = Some(cell);
                } else {
                    prev_next.next = Some(cell);
                    break;
                }
            }
        }

        if focused.is_none() {
            return;
        }

        let forward = input == next_key;
        let children_idx = if forward { NodeChildren::FIRST } else { NodeChildren::LAST };
        let siblings_idx = if forward { NodeSiblings::NEXT } else { NodeSiblings::PREV };
        let Some(focused_next) =
            prev_next.get(siblings_idx).or_else(|| container.borrow().children.get(children_idx))
        else {
            return;
        };

        Tui::build_node_path(Some(focused_next), &mut self.tui.focused_node_path);
        self.set_input_consumed();
        self.needs_rerender();
    }

    /// Creates a simple text label.
    pub fn label(&mut self, classname: &'static str, text: &str) {
        self.styled_label_begin(classname);
        self.styled_label_add_text(text);
        self.styled_label_end();
    }

    /// Creates a styled text label.
    ///
    /// # Example
    /// ```
    /// use edit::framebuffer::IndexedColor;
    /// use edit::tui::Context;
    ///
    /// fn draw(ctx: &mut Context) {
    ///     ctx.styled_label_begin("label");
    ///     // Shows "Hello" in the inherited foreground color.
    ///     ctx.styled_label_add_text("Hello");
    ///     // Shows ", World!" next to "Hello" in red.
    ///     ctx.styled_label_set_foreground(ctx.indexed(IndexedColor::Red));
    ///     ctx.styled_label_add_text(", World!");
    /// }
    /// ```
    pub fn styled_label_begin(&mut self, classname: &'static str) {
        self.block_begin(classname);
        self.tree.last_node.borrow_mut().content = NodeContent::Text(TextContent {
            text: BString::empty(),
            chunks: BVec::empty(),
            overflow: Overflow::Clip,
        });
    }

    /// Changes the active pencil color of the current label.
    pub fn styled_label_set_foreground(&mut self, fg: StraightRgba) {
        let mut node = self.tree.last_node.borrow_mut();
        let NodeContent::Text(content) = &mut node.content else {
            unreachable!();
        };

        let last = content.chunks.last().unwrap_or(&INVALID_STYLED_TEXT_CHUNK);
        if last.offset != content.text.len() && last.fg != fg {
            content.chunks.push(
                self.arena(),
                StyledTextChunk { offset: content.text.len(), fg, attr: last.attr },
            );
        }
    }

    /// Changes the active pencil attributes of the current label.
    pub fn styled_label_set_attributes(&mut self, attr: Attributes) {
        let mut node = self.tree.last_node.borrow_mut();
        let NodeContent::Text(content) = &mut node.content else {
            unreachable!();
        };

        let last = content.chunks.last().unwrap_or(&INVALID_STYLED_TEXT_CHUNK);
        if last.offset != content.text.len() && last.attr != attr {
            content.chunks.push(
                self.arena(),
                StyledTextChunk { offset: content.text.len(), fg: last.fg, attr },
            );
        }
    }

    /// Adds text to the current label.
    pub fn styled_label_add_text(&mut self, text: &str) {
        let mut node = self.tree.last_node.borrow_mut();
        let NodeContent::Text(content) = &mut node.content else {
            unreachable!();
        };

        content.text.push_str(self.arena(), text);
    }

    /// Ends the current label block.
    pub fn styled_label_end(&mut self) {
        {
            let mut last_node = self.tree.last_node.borrow_mut();
            let NodeContent::Text(content) = &last_node.content else {
                return;
            };

            let cursor = unicode::MeasurementConfig::new(&content.text.as_bytes())
                .goto_visual(Point { x: CoordType::MAX, y: 0 });
            last_node.intrinsic_size.width = cursor.visual_pos.x;
            last_node.intrinsic_size.height = 1;
            last_node.intrinsic_size_set = true;
        }

        self.block_end();
    }

    /// Sets the overflow behaviour of the current label.
    pub fn attr_overflow(&mut self, overflow: Overflow) {
        let mut last_node = self.tree.last_node.borrow_mut();
        let NodeContent::Text(content) = &mut last_node.content else {
            return;
        };

        content.overflow = overflow;
    }

    /// Creates a button with the given text.
    /// Returns true if the button was activated.
    pub fn button(&mut self, classname: &'static str, text: &str, style: ButtonStyle) -> bool {
        self.button_label(classname, text, style);
        self.attr_focusable();
        if self.is_focused() {
            self.attr_reverse();
        }
        self.button_activated()
    }

    /// Creates a checkbox with the given text.
    /// Returns true if the checkbox was activated.
    pub fn checkbox(&mut self, classname: &'static str, text: &str, checked: &mut bool) -> bool {
        self.styled_label_begin(classname);
        self.attr_focusable();
        if self.is_focused() {
            self.attr_reverse();
        }
        self.styled_label_add_text(if *checked { "[x " } else { "[ " });
        self.styled_label_add_text(text);
        self.styled_label_add_text("]");
        self.styled_label_end();

        let activated = self.button_activated();
        if activated {
            *checked = !*checked;
        }
        activated
    }

    fn button_activated(&mut self) -> bool {
        if !self.input_consumed
            && ((self.input_mouse_click != 0 && self.contains_mouse_down())
                || self.input_keyboard == Some(vk::RETURN)
                || self.input_keyboard == Some(vk::SPACE))
            && self.is_focused()
        {
            self.set_input_consumed();
            true
        } else {
            false
        }
    }

    /// Creates a text input field.
    /// Returns true if the text contents changed.
    pub fn editline(&mut self, classname: &'static str, text: &mut dyn WriteableDocument) -> bool {
        self.textarea_internal(classname, TextBufferPayload::Editline(text))
    }

    /// Creates a scrollable area.
    pub fn scrollarea_begin(&mut self, classname: &'static str, intrinsic_size: Size) {
        self.block_begin(classname);

        let container_node = self.tree.last_node;
        {
            let mut container = self.tree.last_node.borrow_mut();
            container.content = NodeContent::Scrollarea(ScrollareaContent {
                scroll_offset: Point::MIN,
                scroll_offset_y_drag_start: CoordType::MIN,
                thumb_height: 0,
            });

            if intrinsic_size.width > 0 || intrinsic_size.height > 0 {
                container.intrinsic_size.width = intrinsic_size.width.max(0);
                container.intrinsic_size.height = intrinsic_size.height.max(0);
                container.intrinsic_size_set = true;
            }
        }

        self.block_begin("content");
        self.inherit_focus();

        // Ensure that attribute modifications apply to the outer container.
        self.tree.last_node = container_node;
    }

    /// Scrolls the current scrollable area to the given position.
    pub fn scrollarea_scroll_to(&mut self, pos: Point) {
        let mut container = self.tree.last_node.borrow_mut();
        if let NodeContent::Scrollarea(sc) = &mut container.content {
            sc.scroll_offset = pos;
        } else {
            crate::sanity_assert!(scrollarea_scroll_to_outside_scrollarea, false);
        }
    }

    /// Ends the current scrollarea block.
    pub fn scrollarea_end(&mut self) {
        self.block_end(); // content block
        self.block_end(); // outer container

        let mut container = self.tree.last_node.borrow_mut();
        let container_id = container.id;
        let container_depth = container.depth;
        let Some(prev_container) = self.tui.prev_node_map.get(container_id) else {
            return;
        };

        let prev_container = prev_container.borrow();
        let NodeContent::Scrollarea(sc) = &mut container.content else {
            unreachable!();
        };

        if sc.scroll_offset == Point::MIN
            && let NodeContent::Scrollarea(sc_prev) = &prev_container.content
        {
            *sc = sc_prev.clone();
        }

        if !self.input_consumed {
            let container_rect = prev_container.inner;

            if self.input_scroll_delta != Point::default()
                && container_rect.contains(self.tui.mouse_position)
            {
                sc.scroll_offset.x += self.input_scroll_delta.x;
                sc.scroll_offset.y += self.input_scroll_delta.y;
                self.set_input_consumed();
            } else if self.tui.mouse_state != InputMouseState::None {
                match self.tui.mouse_state {
                    InputMouseState::Left if self.tui.mouse_is_drag => {
                        // We don't need to look up the previous track node,
                        // since it has a fixed size based on the container size.
                        let track_rect = Rect {
                            left: container_rect.right,
                            top: container_rect.top,
                            right: container_rect.right + 1,
                            bottom: container_rect.bottom,
                        };
                        if track_rect.contains(self.tui.mouse_down_position) {
                            if sc.scroll_offset_y_drag_start == CoordType::MIN {
                                sc.scroll_offset_y_drag_start = sc.scroll_offset.y;
                            }

                            let content = prev_container.children.first.unwrap().borrow();
                            let content_rect = content.inner;
                            let content_height = content_rect.height();
                            let track_height = track_rect.height();
                            let scrollable_height = content_height - track_height;
                            sc.scroll_offset.y = scrollbar_drag_offset(
                                sc.scroll_offset_y_drag_start,
                                self.tui.mouse_position.y - self.tui.mouse_down_position.y,
                                scrollable_height,
                                track_height - sc.thumb_height,
                            );

                            self.set_input_consumed();
                        }
                    }
                    InputMouseState::Release => {
                        sc.scroll_offset_y_drag_start = CoordType::MIN;
                    }
                    _ => {}
                }
            } else if self.tui.is_subtree_focused_alt(container_id, container_depth)
                && let Some(key) = self.input_keyboard
            {
                match key {
                    vk::PRIOR => sc.scroll_offset.y -= prev_container.inner_clipped.height(),
                    vk::NEXT => sc.scroll_offset.y += prev_container.inner_clipped.height(),
                    vk::END => sc.scroll_offset.y = CoordType::MAX,
                    vk::HOME => sc.scroll_offset.y = 0,
                    _ => return,
                }
                self.set_input_consumed();
            }
        }
    }

    /// Creates a list where exactly one item is selected.
    pub fn list_begin(&mut self, classname: &'static str) {
        self.block_begin(classname);
        self.attr_focusable();

        let mut last_node = self.tree.last_node.borrow_mut();
        let content = self
            .tui
            .prev_node_map
            .get(last_node.id)
            .and_then(|node| match &node.borrow().content {
                NodeContent::List(content) => {
                    Some(ListContent { selected: content.selected, selected_node: None })
                }
                _ => None,
            })
            .unwrap_or(ListContent { selected: 0, selected_node: None });

        last_node.attributes.focus_void = true;
        last_node.content = NodeContent::List(content);
    }

    /// Creates a list item with the given text.
    pub fn list_item(&mut self, select: bool, text: &str) -> ListSelection {
        self.styled_list_item_begin();
        self.styled_label_add_text(text);
        self.styled_list_item_end(select)
    }

    /// Creates a list item consisting of a styled label.
    /// See [`Context::styled_label_begin`].
    pub fn styled_list_item_begin(&mut self) {
        let list = self.tree.current_node;
        let idx = list.borrow().child_count;

        self.next_block_id_mixin(idx as u64);
        self.styled_label_begin("item");
        self.styled_label_add_text("  ");
        self.attr_focusable();
    }

    /// Ends the current styled list item.
    pub fn styled_list_item_end(&mut self, select: bool) -> ListSelection {
        self.styled_label_end();

        let list = self.tree.current_node;

        let selected_before;
        let selected_now;
        let focused;
        {
            let mut list = list.borrow_mut();
            let content = match &mut list.content {
                NodeContent::List(content) => content,
                _ => unreachable!(),
            };

            let item = self.tree.last_node.borrow();
            let item_id = item.id;
            selected_before = content.selected == item_id;
            focused = self.is_focused();

            // Inherit the default selection & Click changes selection
            selected_now = selected_before || (select && content.selected == 0) || focused;

            // Note down the selected node for keyboard navigation.
            if selected_now {
                content.selected_node = Some(self.tree.last_node);
                if !selected_before {
                    content.selected = item_id;
                    self.needs_rerender();
                }
            }
        }

        // Clicking an item activates it
        let clicked =
            !self.input_consumed && (self.input_mouse_click == 2 && self.was_mouse_down());
        // Pressing Enter on a selected item activates it as well
        let entered = focused
            && selected_before
            && !self.input_consumed
            && matches!(self.input_keyboard, Some(vk::RETURN));
        let activated = clicked || entered;
        if activated {
            self.set_input_consumed();
        }

        if selected_before && activated {
            ListSelection::Activated
        } else if selected_now && !selected_before {
            ListSelection::Selected
        } else {
            ListSelection::Unchanged
        }
    }

    /// [`Context::steal_focus`], but for a list view.
    ///
    /// This exists, because didn't want to figure out how to get
    /// [`Context::styled_list_item_end`] to recognize a regular,
    /// programmatic focus steal.
    pub fn list_item_steal_focus(&mut self) {
        self.steal_focus();

        match &mut self.tree.current_node.borrow_mut().content {
            NodeContent::List(content) => {
                content.selected = self.tree.last_node.borrow().id;
                content.selected_node = Some(self.tree.last_node);
            }
            _ => unreachable!(),
        }
    }

    /// Ends the current list block.
    pub fn list_end(&mut self) {
        self.block_end();

        let contains_focus;
        let selected_now;
        let mut selected_next;
        {
            let list = self.tree.last_node.borrow();

            contains_focus = self.tui.is_subtree_focused(&list);
            selected_now = match &list.content {
                NodeContent::List(content) => content.selected_node,
                _ => unreachable!(),
            };
            selected_next = match selected_now.or(list.children.first) {
                Some(node) => node,
                None => return,
            };
        }

        if contains_focus
            && !self.input_consumed
            && let Some(key) = self.input_keyboard
            && let Some(selected_now) = selected_now
        {
            let list = self.tree.last_node.borrow();

            if let Some(prev_container) = self.tui.prev_node_map.get(list.id) {
                let mut consumed = true;

                match key {
                    vk::PRIOR => {
                        selected_next = selected_now;
                        for _ in 0..prev_container.borrow().inner_clipped.height() - 1 {
                            let node = selected_next.borrow();
                            selected_next = match node.siblings.prev {
                                Some(node) => node,
                                None => break,
                            };
                        }
                    }
                    vk::NEXT => {
                        selected_next = selected_now;
                        for _ in 0..prev_container.borrow().inner_clipped.height() - 1 {
                            let node = selected_next.borrow();
                            selected_next = match node.siblings.next {
                                Some(node) => node,
                                None => break,
                            };
                        }
                    }
                    vk::END => {
                        selected_next = list.children.last.unwrap_or(selected_next);
                    }
                    vk::HOME => {
                        selected_next = list.children.first.unwrap_or(selected_next);
                    }
                    vk::UP => {
                        selected_next = selected_now
                            .borrow()
                            .siblings
                            .prev
                            .or(list.children.last)
                            .unwrap_or(selected_next);
                    }
                    vk::DOWN => {
                        selected_next = selected_now
                            .borrow()
                            .siblings
                            .next
                            .or(list.children.first)
                            .unwrap_or(selected_next);
                    }
                    _ => consumed = false,
                }

                if consumed {
                    self.set_input_consumed();
                }
            }
        }

        // Now that we know which item is selected we can mark it as such.
        if !opt_ptr_eq(selected_now, Some(selected_next))
            && let NodeContent::List(content) = &mut self.tree.last_node.borrow_mut().content
        {
            content.selected_node = Some(selected_next);
        }

        // Now that we know which item is selected we can mark it as such.
        if let NodeContent::Text(content) = &mut selected_next.borrow_mut().content {
            unsafe {
                content.text.as_bytes_mut()[0] = b'>';
            }
        }

        // If the list has focus, we also delegate focus to the selected item and colorize it.
        if contains_focus {
            {
                let mut node = selected_next.borrow_mut();
                node.attributes.bg = self.indexed(IndexedColor::Green);
                node.attributes.fg = self.contrasted(self.indexed(IndexedColor::Green));
            }
            self.steal_focus_for(selected_next);
        }
    }

    /// Creates a menubar, to be shown at the top of the screen.
    pub fn menubar_begin(&mut self) {
        self.table_begin("menubar");
        self.attr_focus_well();
        self.table_next_row();
    }

    /// Appends a menu to the current menubar.
    ///
    /// Returns true if the menu is open. Continue appending items to it in that case.
    pub fn menubar_menu_begin(&mut self, text: &str, accelerator: char) -> bool {
        let accelerator =
            if cfg!(any(target_os = "macos", target_os = "ios")) { '\0' } else { accelerator };
        let mixin = self.tree.current_node.borrow().child_count as u64;
        self.next_block_id_mixin(mixin);

        self.button_label(
            "menu_button",
            text,
            ButtonStyle::default().accelerator(accelerator).bracketed(false),
        );
        self.attr_focusable();
        self.attr_padding(Rect::two(0, 1));

        let contains_focus = self.contains_focus();
        let keyboard_focus = accelerator != '\0'
            && !contains_focus
            && self.consume_shortcut(kbmod::ALT | InputKey::new(accelerator as u32));

        if contains_focus || keyboard_focus {
            self.attr_background_rgba(self.tui.floater_default_bg);
            self.attr_foreground_rgba(self.tui.floater_default_fg);

            if self.is_focused() {
                self.attr_background_rgba(self.indexed(IndexedColor::Green));
                self.attr_foreground_rgba(self.contrasted(self.indexed(IndexedColor::Green)));
            }

            self.next_block_id_mixin(mixin);
            self.table_begin("flyout");
            self.attr_float(FloatSpec {
                anchor: Anchor::Last,
                gravity_x: 0.0,
                gravity_y: 0.0,
                offset_x: 0.0,
                offset_y: 1.0,
            });
            self.attr_border();
            self.attr_focus_well();
            self.attr_slide_down();

            if keyboard_focus {
                self.steal_focus();
            }

            true
        } else {
            false
        }
    }

    /// Appends a button to the current menu.
    pub fn menubar_menu_button(
        &mut self,
        text: &str,
        accelerator: char,
        shortcut: InputKey,
    ) -> bool {
        self.menubar_menu_checkbox(text, accelerator, shortcut, false)
    }

    /// Appends a checkbox to the current menu.
    /// Returns true if the checkbox was activated.
    pub fn menubar_menu_checkbox(
        &mut self,
        text: &str,
        accelerator: char,
        shortcut: InputKey,
        checked: bool,
    ) -> bool {
        self.table_next_row();
        self.attr_focusable();

        // First menu item? Steal focus.
        if self.tree.current_node.borrow_mut().siblings.prev.is_none() {
            self.inherit_focus();
        }

        if self.is_focused() {
            self.attr_background_rgba(self.indexed(IndexedColor::Green));
            self.attr_foreground_rgba(self.contrasted(self.indexed(IndexedColor::Green)));
        }

        let clicked =
            self.button_activated() || self.consume_shortcut(InputKey::new(accelerator as u32));

        self.button_label(
            "menu_checkbox",
            text,
            ButtonStyle::default().bracketed(false).checked(checked).accelerator(accelerator),
        );
        self.menubar_shortcut(shortcut);

        if clicked {
            // TODO: This should reassign the previous focused path.
            self.needs_rerender();
            Tui::clean_node_path(&mut self.tui.focused_node_path);
        }

        clicked
    }

    /// Ends the current menu.
    pub fn menubar_menu_end(&mut self) {
        self.table_end();

        if !self.input_consumed
            && let Some(key) = self.input_keyboard
            && matches!(key, vk::ESCAPE | vk::UP | vk::DOWN)
        {
            if matches!(key, vk::UP | vk::DOWN) {
                // If the focus is on the menubar, and the user presses up/down,
                // focus the first/last item of the flyout respectively.
                let ln = self.tree.last_node.borrow();
                if self.tui.is_node_focused(ln.parent.map_or(0, |n| n.borrow().id)) {
                    let selected_next =
                        if key == vk::UP { ln.children.last } else { ln.children.first };
                    if let Some(selected_next) = selected_next {
                        self.steal_focus_for(selected_next);
                        self.set_input_consumed();
                    }
                }
            } else if self.contains_focus() {
                // Otherwise, if the menu is the focused one and the
                // user presses Escape, pass focus back to the menubar.
                self.tui.pop_focusable_node(1);
            }
        }
    }

    /// Ends the current menubar.
    pub fn menubar_end(&mut self) {
        self.table_end();
    }

    /// Renders a button label with an optional accelerator character
    /// May also renders a checkbox or square brackets for inline buttons
    fn button_label(&mut self, classname: &'static str, text: &str, style: ButtonStyle) {
        // Label prefix
        self.styled_label_begin(classname);
        if style.bracketed {
            self.styled_label_add_text("[");
        }
        if let Some(checked) = style.checked {
            self.styled_label_add_text(if checked { "x " } else { "  " });
        }
        // Label text
        match style.accelerator {
            Some(accelerator) if accelerator.is_ascii_uppercase() => {
                // Complex case:
                // Locate the offset of the accelerator character in the label text
                let mut off = text.len();
                for (i, c) in text.bytes().enumerate() {
                    // Perfect match (uppercase character) --> stop
                    if c as char == accelerator {
                        off = i;
                        break;
                    }
                    // Inexact match (lowercase character) --> use first hit
                    if (c & !0x20) as char == accelerator && off == text.len() {
                        off = i;
                    }
                }

                if off < text.len() {
                    // Add an underline to the accelerator.
                    self.styled_label_add_text(&text[..off]);
                    self.styled_label_set_attributes(Attributes::Underlined);
                    self.styled_label_add_text(&text[off..off + 1]);
                    self.styled_label_set_attributes(Attributes::None);
                    self.styled_label_add_text(&text[off + 1..]);
                } else {
                    // Add the accelerator in parentheses and underline it.
                    let ch = accelerator as u8;
                    self.styled_label_add_text(text);
                    self.styled_label_add_text("(");
                    self.styled_label_set_attributes(Attributes::Underlined);
                    self.styled_label_add_text(unsafe { str_from_raw_parts(&ch, 1) });
                    self.styled_label_set_attributes(Attributes::None);
                    self.styled_label_add_text(")");
                }
            }
            _ => {
                // Simple case:
                // no accelerator character
                self.styled_label_add_text(text);
            }
        }
        // Label postfix
        if style.bracketed {
            self.styled_label_add_text("]");
        }
        self.styled_label_end();
    }

    fn menubar_shortcut(&mut self, shortcut: InputKey) {
        let shortcut_letter = shortcut.value() as u8 as char;
        if shortcut_letter.is_ascii_uppercase() {
            let mut shortcut_text = BString::empty();
            if shortcut.modifiers_contains(kbmod::CTRL) {
                shortcut_text.push_str(self.arena(), self.tui.modifier_translations.ctrl);
                shortcut_text.push(self.arena(), '+');
            }
            if shortcut.modifiers_contains(kbmod::ALT) {
                shortcut_text.push_str(self.arena(), self.tui.modifier_translations.alt);
                shortcut_text.push(self.arena(), '+');
            }
            if shortcut.modifiers_contains(kbmod::SHIFT) {
                shortcut_text.push_str(self.arena(), self.tui.modifier_translations.shift);
                shortcut_text.push(self.arena(), '+');
            }
            if shortcut.modifiers_contains(kbmod::CMD) {
                shortcut_text.push_str(self.arena(), self.tui.modifier_translations.cmd);
                shortcut_text.push(self.arena(), '+');
            }
            shortcut_text.push(self.arena(), shortcut_letter);

            self.label("shortcut", &shortcut_text);
        } else {
            self.block_begin("shortcut");
            self.block_end();
        }
        self.attr_padding(Rect { left: 2, top: 0, right: 2, bottom: 0 });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drag_maps_travel_onto_content_proportionally() {
        // Half the available travel scrolls half the content.
        assert_eq!(scrollbar_drag_offset(0, 5, 100, 10), 50);
        assert_eq!(scrollbar_drag_offset(0, 10, 100, 10), 100);
    }

    #[test]
    fn drag_is_relative_to_where_it_started() {
        assert_eq!(scrollbar_drag_offset(20, 1, 100, 10), 30);
    }

    #[test]
    fn dragging_up_scrolls_back() {
        assert_eq!(scrollbar_drag_offset(50, -5, 100, 10), 0);
        assert_eq!(scrollbar_drag_offset(50, -10, 100, 10), -50);
    }

    #[test]
    fn a_thumb_filling_its_track_does_not_divide_by_zero() {
        // draw_scrollbar rounds the thumb to whole rows and floors it at
        // one, so a one-row track with two rows of content leaves no
        // travel at all. Dividing through would panic.
        assert_eq!(scrollbar_drag_offset(7, 3, 1, 0), 7);
    }

    #[test]
    fn nothing_to_scroll_holds_position() {
        assert_eq!(scrollbar_drag_offset(7, 3, 0, 10), 7);
        assert_eq!(scrollbar_drag_offset(7, 3, -1, 10), 7);
    }
}
