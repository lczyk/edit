//! Compile-only test pinning the `edit::mount` public surface. Doesn't
//! exercise the loop (that needs a real tty); just makes sure the names
//! and signatures stay reachable from outside the crate.

use std::ops::ControlFlow;

use edit::mount::{MountOpts, mount};
use edit::tui::Context;

#[test]
fn mount_opts_default_constructs() {
    let _opts = MountOpts::default();
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
