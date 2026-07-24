//! Compile-only test pinning the `edit::mount` public surface. Doesn't
//! exercise the loop (that needs a real tty); just makes sure the names
//! and signatures stay reachable from outside the crate.

use std::ops::ControlFlow;

use edit::mount::{MountOpts, mount};
use edit::tui::Context;

#[test]
fn mount_opts_default_constructs() {
    let opts = MountOpts::default();
    assert!(opts.on_probe.is_none(), "callers opt in to the probe hook");
}

#[test]
fn on_probe_receives_the_probed_width() {
    // The hook is what lets a caller reflow a buffer it built before
    // mounting, so the width it observes has to be the probed one.
    let seen = std::rc::Rc::new(std::cell::Cell::new(0));
    let sink = std::rc::Rc::clone(&seen);

    let opts = MountOpts {
        on_probe: Some(Box::new(move |probe| sink.set(probe.ambiguous_width))),
        ..Default::default()
    };

    // mount() itself needs a tty; invoke the callback directly with a
    // probe standing in for what term::setup would have produced.
    let probe = edit::term::TerminalProbe {
        indexed_colors: edit::framebuffer::DEFAULT_THEME,
        ambiguous_width: 2,
    };
    (opts.on_probe.unwrap())(&probe);

    assert_eq!(seen.get(), 2);
}

#[allow(dead_code)]
fn mount_signature_smoke() {
    // Not called -- mount() needs a tty + arena+sys init. This exists so
    // the signature (`FnMut(&mut Context) -> ControlFlow<()>`) is locked
    // in by the type checker.
    fn _check(opts: MountOpts) -> std::io::Result<()> {
        mount(opts, |_ctx: &mut Context| ControlFlow::Break(()))
    }
}
