//! Merge-conflict markers, recognised ahead of the bytecode vm.
//!
//! The marker lines git writes (`<<<<<<<`, `|||||||`, `=======`, `>>>>>>>`)
//! are not language syntax, so no definition has to know them: the runtime
//! classifies each raw line first and hands the vm only the rest. Ours,
//! base and theirs are three alternative continuations of the vm state at
//! the opener, so each side is lexed from a fork of that state and a
//! construct opened on one side cannot leak into another.

use crate::runtime::VmState;

/// Marker runs shorter than this are ordinary text. git's default, and the
/// conflict-marker-size attribute only ever raises it in practice.
pub const MIN_RUN: usize = 7;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Marker {
    /// `<<<<<<< ours-label`
    Start,
    /// `||||||| base-label` (diff3 and zdiff3 styles only)
    Base,
    /// `=======`
    Separator,
    /// `>>>>>>> theirs-label`
    End,
}

/// Where a parsed line sits relative to a merge conflict.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ConflictTag {
    #[default]
    None,
    Marker,
    Ours,
    Base,
    Theirs,
}

/// Recognise a marker line (without its newline). The label after `<` and
/// `>` runs follows one space; git never labels `=` and its own reader
/// accepts any whitespace after `|` and `=`. A bare run is accepted for all
/// four: git writes one for an empty label and in rerere preimages.
pub fn classify(line: &[u8]) -> Option<(Marker, usize)> {
    let line = line.strip_suffix(b"\r").unwrap_or(line);
    let &first = line.first()?;
    let marker = match first {
        b'<' => Marker::Start,
        b'|' => Marker::Base,
        b'=' => Marker::Separator,
        b'>' => Marker::End,
        _ => return None,
    };
    let run = line.iter().take_while(|&&b| b == first).count();
    if run < MIN_RUN {
        return None;
    }
    let labelled = match (marker, line.get(run)) {
        (_, None) => true,
        (Marker::Start | Marker::End, Some(&b)) => b == b' ',
        (Marker::Base | Marker::Separator, Some(&b)) => b.is_ascii_whitespace(),
    };
    labelled.then_some((marker, run))
}

/// The block being lexed, with the vm states needed to fork and resume.
/// `len` is the run length that opened the block; the other markers only
/// count at that length, so the longer markers git nests inside a
/// criss-cross merge base stay ordinary text.
#[derive(Clone, Default, PartialEq, Eq)]
pub enum ConflictState {
    #[default]
    Outside,
    Ours {
        len: usize,
        fork: Box<VmState>,
    },
    Base {
        len: usize,
        fork: Box<VmState>,
        ours_end: Box<VmState>,
    },
    Theirs {
        len: usize,
        fork: Box<VmState>,
        ours_end: Box<VmState>,
    },
}

impl ConflictState {
    pub fn region(&self) -> ConflictTag {
        match self {
            ConflictState::Outside => ConflictTag::None,
            ConflictState::Ours { .. } => ConflictTag::Ours,
            ConflictState::Base { .. } => ConflictTag::Base,
            ConflictState::Theirs { .. } => ConflictTag::Theirs,
        }
    }

    /// Feed one line. Returns true when it is a marker line: the block has
    /// moved on and `vm` now holds the state the next line lexes from, so
    /// the caller must not run the vm on this line.
    pub fn step(&mut self, line: &[u8], vm: &mut VmState) -> bool {
        use ConflictState::*;

        let Some((marker, run)) = classify(line) else {
            return false;
        };
        let fork_of = |vm: &VmState| Box::new(vm.clone());

        *self = match (std::mem::take(self), marker) {
            (Outside, Marker::Start) => Ours { len: run, fork: fork_of(vm) },
            // A repeated opener inside a block: start the block over from
            // the same fork, so nothing lexed since can leak.
            (
                Ours { len, fork } | Base { len, fork, .. } | Theirs { len, fork, .. },
                Marker::Start,
            ) if run == len => {
                *vm = (*fork).clone();
                Ours { len, fork }
            }
            (Ours { len, fork }, Marker::Base) if run == len => {
                let ours_end = Box::new(std::mem::replace(vm, (*fork).clone()));
                Base { len, fork, ours_end }
            }
            (Ours { len, fork }, Marker::Separator) if run == len => {
                let ours_end = Box::new(std::mem::replace(vm, (*fork).clone()));
                Theirs { len, fork, ours_end }
            }
            (Base { len, fork, ours_end }, Marker::Separator) if run == len => {
                *vm = (*fork).clone();
                Theirs { len, fork, ours_end }
            }
            (Theirs { len, fork, ours_end }, Marker::End) if run == len => {
                *vm = resume(&fork, *ours_end, vm);
                Outside
            }
            (state, _) => {
                *self = state;
                return false;
            }
        };
        true
    }
}

/// Which side the text after the closing marker continues from. A
/// heuristic: the two sides usually end in the same state, and when they
/// differ the side that closed everything it opened (its end state equals
/// the fork) is the safer guess, ours breaking the tie because the rest of
/// the checkout is ours. A construct left open on the other side spills
/// past the block either way; only resolving the conflict fixes that.
fn resume(fork: &VmState, ours_end: VmState, theirs_end: &VmState) -> VmState {
    if !ours_end.same_construct(fork) && theirs_end.same_construct(fork) {
        theirs_end.clone()
    } else {
        ours_end
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_four_markers_are_recognised_with_and_without_labels() {
        assert_eq!(classify(b"<<<<<<< HEAD"), Some((Marker::Start, 7)));
        assert_eq!(classify(b"<<<<<<<"), Some((Marker::Start, 7)));
        assert_eq!(classify(b"||||||| ef303a3"), Some((Marker::Base, 7)));
        assert_eq!(classify(b"|||||||"), Some((Marker::Base, 7)));
        assert_eq!(classify(b"======="), Some((Marker::Separator, 7)));
        assert_eq!(classify(b"=======\t"), Some((Marker::Separator, 7)));
        assert_eq!(classify(b">>>>>>> lczyk/main"), Some((Marker::End, 7)));
        assert_eq!(classify(b">>>>>>>"), Some((Marker::End, 7)));
    }

    #[test]
    fn longer_runs_and_crlf_are_fine() {
        assert_eq!(classify(b"<<<<<<<<< nested"), Some((Marker::Start, 9)));
        assert_eq!(classify(b"=========\r"), Some((Marker::Separator, 9)));
        assert_eq!(classify(b">>>>>>> theirs\r"), Some((Marker::End, 7)));
    }

    #[test]
    fn near_misses_are_text() {
        assert_eq!(classify(b"<<<<<< six"), None);
        assert_eq!(classify(b"<<<<<<<HEAD"), None);
        assert_eq!(classify(b" <<<<<<< HEAD"), None);
        assert_eq!(classify(b">>> doctest prompt"), None);
        assert_eq!(classify(b"<<<<<<<\tHEAD"), None);
        assert_eq!(classify(b"===== ==="), None);
        assert_eq!(classify(b""), None);
    }
}
