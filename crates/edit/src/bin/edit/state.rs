use std::sync::Mutex;
use std::time::{Duration, Instant};

use edit::framebuffer::IndexedColor;
use edit::helpers::*;
use edit::oklab::StraightRgba;
use edit::tui::*;
use edit::{buffer, icu};

use crate::apperr;
use crate::documents::Document;

/// How long the "Saved" flash sits in the statusbar after a save. The flash
/// auto-clears on the next redraw past this deadline; since redraws are
/// driven by input/animation, an idle user will see it linger until they
/// next interact -- intentional, reads as confirmation rather than noise.
const SAVED_FLASH_DURATION: Duration = Duration::from_millis(1500);

/// How long the warning flash overrides the statusbar. Idle-linger semantics
/// match [`SAVED_FLASH_DURATION`].
pub const WARNING_FLASH_DURATION: Duration = Duration::from_secs(3);

/// Process-wide latest-warning slot. Written by [`crate::notify_handler`]
/// (which is the function pointer installed into `edit::notify` at startup);
/// read + cleared opportunistically by `draw_statusbar`. Latest-wins on
/// concurrent writes -- the rapid-fire case is acceptable per design.
static WARNING_SLOT: Mutex<Option<(String, Instant)>> = Mutex::new(None);

/// Hook installed into `edit::notify::set_handler`. Library code calls
/// `edit::notify::warn(msg)`; this stores the message in [`WARNING_SLOT`]
/// with the current instant.
pub fn push_warning(msg: &str) {
    if let Ok(mut slot) = WARNING_SLOT.lock() {
        *slot = Some((msg.to_string(), Instant::now()));
    }
}

/// Returns the current warning if it has not yet expired. Expired entries
/// are cleared eagerly so the next call cheaply returns `None`.
pub fn current_warning() -> Option<String> {
    let mut slot = WARNING_SLOT.lock().ok()?;
    let (msg, ts) = slot.as_ref()?;
    if ts.elapsed() < WARNING_FLASH_DURATION {
        Some(msg.clone())
    } else {
        *slot = None;
        None
    }
}

#[repr(transparent)]
pub struct FormatApperr(apperr::Error);

impl From<apperr::Error> for FormatApperr {
    fn from(err: apperr::Error) -> Self {
        Self(err)
    }
}

impl std::fmt::Display for FormatApperr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.0 {
            apperr::Error::SettingsInvalid(what) => {
                write!(f, "Invalid Settings: {}", what)
            }
            apperr::Error::Icu(icu::ICU_MISSING_ERROR) => {
                f.write_str("This operation requires the ICU library")
            }
            apperr::Error::Icu(ref err) => err.fmt(f),
            apperr::Error::Io(ref err) => err.fmt(f),
        }
    }
}

pub struct StateSearch {
    pub kind: StateSearchKind,
    pub focus: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum StateSearchKind {
    Hidden,
    Disabled,
    Search,
    Replace,
}

#[derive(Default)]
pub struct OscTitleFileStatus {
    pub filename: String,
    pub dirty: bool,
}

pub struct State {
    pub menubar_color_bg: StraightRgba,
    pub menubar_color_fg: StraightRgba,

    pub document: Document,

    // A ring buffer of the last 10 errors.
    pub error_log: [String; 10],
    pub error_log_index: usize,
    pub error_log_count: usize,

    pub wants_search: StateSearch,
    pub search_needle: String,
    pub search_replacement: String,
    pub search_options: buffer::SearchOptions,
    pub search_success: bool,

    pub wants_language_picker: bool,

    pub wants_statusbar_focus: bool,
    pub wants_indentation_picker: bool,
    pub wants_about: bool,
    pub wants_exit: bool,
    pub wants_goto: bool,
    pub goto_target: String,
    pub goto_invalid: bool,

    pub osc_title_file_status: OscTitleFileStatus,
    pub osc_clipboard_sync: bool,
    pub exit: bool,

    pub saved_flash_until: Option<Instant>,
}

impl State {
    pub fn new(document: Document) -> apperr::Result<Self> {
        Ok(Self {
            menubar_color_bg: StraightRgba::zero(),
            menubar_color_fg: StraightRgba::zero(),

            document,

            error_log: [const { String::new() }; 10],
            error_log_index: 0,
            error_log_count: 0,

            wants_search: StateSearch { kind: StateSearchKind::Hidden, focus: false },
            search_needle: Default::default(),
            search_replacement: Default::default(),
            search_options: Default::default(),
            search_success: true,

            wants_language_picker: false,

            wants_statusbar_focus: false,
            wants_indentation_picker: false,
            wants_about: false,
            wants_exit: false,
            wants_goto: false,
            goto_target: Default::default(),
            goto_invalid: false,

            osc_title_file_status: Default::default(),
            osc_clipboard_sync: false,
            exit: false,

            saved_flash_until: None,
        })
    }

    pub fn add_error(&mut self, err: apperr::Error) -> bool {
        let msg = format!("{}", FormatApperr::from(err));
        if msg.is_empty() {
            return false;
        }

        self.error_log[self.error_log_index] = msg;
        self.error_log_index = (self.error_log_index + 1) % self.error_log.len();
        self.error_log_count = self.error_log.len().min(self.error_log_count + 1);
        true
    }
}

pub fn save_document(ctx: &mut Context, state: &mut State) {
    match state.document.save() {
        Ok(()) => {
            state.saved_flash_until = Some(Instant::now() + SAVED_FLASH_DURATION);
            ctx.needs_rerender();
        }
        Err(err) => error_log_add(ctx, state, err),
    }
}

pub fn error_log_add(ctx: &mut Context, state: &mut State, err: apperr::Error) {
    if state.add_error(err) {
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
            state.error_log_count = 0;
        }
        ctx.attr_position(Position::Center);
        ctx.inherit_focus();
    }
    if ctx.modal_end() {
        state.error_log_count = 0;
    }
}
