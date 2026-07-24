use std::sync::Mutex;
use std::time::{Duration, Instant};

use edit::oklab::StraightRgba;
use edit::tui::*;
use edit::{buffer, icu};

use crate::apperr;
use crate::document::Document;

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

    pub wants_statusbar_focus: bool,
    pub wants_indentation_picker: bool,
    /// The open dialog, if any. One slot, so two can never paint at once.
    pub modal: Option<crate::modals::Modal>,
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

            wants_statusbar_focus: false,
            wants_indentation_picker: false,
            modal: None,
            goto_target: Default::default(),
            goto_invalid: false,

            osc_title_file_status: Default::default(),
            osc_clipboard_sync: false,
            exit: false,

            saved_flash_until: None,
        })
    }

    /// Clear the queued errors and close the log. Both dismissal paths
    /// (the Ok button and the modal's own close) go through here so the
    /// count and the open dialog cannot disagree -- leaving `modal` set
    /// with nothing to show would redraw an empty log every frame.
    pub fn dismiss_errors(&mut self) {
        self.error_log_count = 0;
        self.modal = None;
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
