# Edit

A simple editor for simple needs.

This editor pays homage to the classic [MS-DOS Editor](https://en.wikipedia.org/wiki/MS-DOS_Editor), but with a modern interface and input controls similar to VS Code. The goal is to provide an accessible editor that even users largely unfamiliar with terminals can easily use.

## Build Instructions

* [Install Rust](https://www.rust-lang.org/tools/install)
* Clone the repository
* If you're using nightly Rust:
  ```sh
  cargo build --release --config .cargo/release.toml
  ```
* If you're using stable Rust:
  * Ideally: Set the environment variable `RUSTC_BOOTSTRAP=1` and use the **nightly** build instructions above.
    This is recommended, because it drastically reduces the binary size and slightly improves performance.
  * Otherwise, simply run:
    ```sh
    cargo build --release
    ```

### ICU library configuration

This project optionally depends on the ICU library for its Search and Replace functionality.

By default, the project will look for the following library names:

 Variable | macOS | Linux / Other
----------|-------|---------------
`EDIT_CFG_ICUUC_SONAME` | `libicucore.dylib` | `libicuuc.so`
`EDIT_CFG_ICUI18N_SONAME` | `libicucore.dylib` | `libicui18n.so`

The unversioned `libicuuc.so` is a symlink that ships in the development
package, not the runtime one. On a machine with only `libicuuc.so.76`,
either install that package (`sudo apt install libicu-dev`, or your
distribution's equivalent) or point at the versioned library directly:

```sh
EDIT_CFG_ICUUC_SONAME=libicuuc.so.76 EDIT_CFG_ICUI18N_SONAME=libicui18n.so.76 make build
```

Setting the SONAME does not disable renaming auto-detection, so on Linux
that is normally the only thing you need to set.

This project assumes that ICU exports symbols without `_` prefix and without version suffix, such as `u_errorName`. If your installation uses versioned exports, set:
* `EDIT_CFG_ICU_CPP_EXPORTS=true` — look for C++ symbols such as `_u_errorName`. Enabled by default on macOS.
* `EDIT_CFG_ICU_RENAMING_VERSION=76` — look for symbols such as `u_errorName_76`.
* `EDIT_CFG_ICU_RENAMING_AUTO_DETECT=true` -- detect the version at runtime. Enabled by default on Linux unless `EDIT_CFG_ICU_RENAMING_VERSION` is set.

Search and Replace degrade gracefully when ICU can't be loaded. To check
whether it is wired up, `make test-icu` finds an installed ICU, builds
against it, and fails rather than skipping if the search tests can't run.
