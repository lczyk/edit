---
status: open
date: 2026-04-23
description: vim-style swap files for crash/sigkill/perms-lost recovery
---

# Recovery cache

Future plan to make `edit` survive crashes, SIGKILL, lost write permissions, and
other failure modes that today result in lost edits. Captured here so the design
isn't re-derived later. Bring this into `AGENTS.md` once implemented.

## Problem

The current model has no safety net:

- File opened with write perms, then perms revoked mid-session → save fails →
  user sees an error and loses the buffer when they exit.
- `edit` killed with `SIGKILL` / power loss / terminal crash → unsaved edits gone.
- File deleted on disk while editing → save may recreate, but the editor has no
  notion of "I already had a copy."

Vim solves this with `.swp` files. We want the same idea, scoped to a
single-buffer editor.

## Behaviour

1. On every dirty edit, periodically flush a swap snapshot to a per-file entry
   in the user's cache dir.
2. On graceful close (saved or explicitly discarded), evict the swap entry.
3. On reopen, if a swap entry exists for that file, prompt:
   `Restore unsaved changes from previous session? [Restore / Discard / Cancel]`.
4. If a save fails (perms revoked, disk full, etc.), keep the swap entry. The
   error message points the user at it.

The combination means: the only way to lose unsaved edits is to explicitly
discard them.

## Storage

- Cache dir: `$XDG_CACHE_HOME/edit/swap/` (fallback `~/.cache/edit/swap/`).
  Auto-created on first write.
- Entry filename: `<dev>-<ino>.swap` when a `FileId` is known (existing files);
  `<sha256(canonical_path)>.swap` otherwise (new files that haven't been saved
  yet).
- Atomic write: write to `<key>.swap.tmp`, fsync, rename. Avoids partial files
  if killed mid-flush.
- Format: one-line JSON header + `\n` + editable bytes.
  Header records: `path`, `file_id` (dev/ino), `mtime_ns`, `size`,
  `encoding`, `cursor: [x, y]`. Body is the buffer's *editable* content as
  UTF-8 — see the diff-mode note below. We always re-encode on save, so
  storing UTF-8 is fine even if the original file was another encoding;
  the header preserves the original encoding.

## Lifecycle

| Event                         | Cache action                                      |
| ----------------------------- | ------------------------------------------------- |
| Dirty edit + ≥1s since flush  | Write `<key>.swap`                                |
| `Document::save` Ok           | Delete `<key>.swap`                               |
| Exit prompt → Discard         | Delete `<key>.swap`                               |
| Exit prompt → Save fails      | Keep `<key>.swap`; show error referencing path    |
| SIGKILL / panic / power loss  | Last-flushed `<key>.swap` survives                |
| Open file with matching swap  | Show restore prompt                               |
| Open file, swap mtime mismatch | Show conflict variant of restore prompt          |
| Startup                       | Prune `<key>.swap` older than 30 days             |

## API surface

- New module `crates/edit/src/bin/edit/swap.rs`:
  - `pub fn cache_dir() -> Option<PathBuf>`
  - `pub struct SwapKey` (file_id or path-hash)
  - `pub struct SwapEntry { path, file_id, mtime_ns, size, encoding, cursor, contents }`
  - `pub fn read(key: &SwapKey) -> io::Result<Option<SwapEntry>>`
  - `pub fn write(key: &SwapKey, entry: &SwapEntry) -> io::Result<()>`
  - `pub fn delete(key: &SwapKey) -> io::Result<()>`
  - `pub fn prune_stale(max_age: Duration)`
- `TextBuffer::save_to_bytes(&mut self) -> Vec<u8>` — dump buffer for the swap
  body (or reuse `write_file` against a `Cursor<Vec<u8>>`). In diff mode this
  must return the editable content — see below.
- `Document` gains `swap_key: Option<SwapKey>`.
- `State` gains `wants_swap_restore: Option<SwapEntry>`,
  `last_swap_write_generation: u32`, `last_swap_write_time: Instant`.
- New restore-prompt modal in `draw_editor.rs`, drawn before normal editor
  while `wants_swap_restore.is_some()`.

## Periodic flush

In the main loop after each frame:

```text
if doc.buffer.is_dirty()
    && tb.generation() != state.last_swap_write_generation
    && state.last_swap_write_time.elapsed() >= Duration::from_secs(1)
{
    swap::write(...);   // log on err, don't block
    state.last_swap_write_generation = tb.generation();
    state.last_swap_write_time = Instant::now();
}
```

Also flush immediately when `wants_exit` becomes true, before the dirty prompt
is shown — so a SIGINT inside the modal still leaves a fresh swap.

## Edge cases

| Case                                  | Behaviour                                          |
| ------------------------------------- | ------------------------------------------------- |
| Perms revoked mid-session             | Save fails → swap retained → restore on next launch |
| SIGKILL / power loss                  | Swap survives → restore prompt next launch        |
| File deleted on disk while editing    | Save recreates if dir writable; swap stays until save Ok |
| File mtime changed since swap captured | Restore prompt shows conflict warning             |
| Two `edit` instances on same file     | Both flush to same key; last write wins (v1 limit) |
| RO file edited                        | Buffer gates block edits → swap stays empty       |
| New file (no file_id at open)         | Key by canonical path hash until first save       |

## Test plan

- `chmod 644 foo`, edit, `kill -9`, relaunch → restore prompt, accept → unsaved
  edits present.
- Edit, save normally, relaunch → no prompt (swap evicted).
- Edit, exit → Don't Save → no prompt next launch.
- `chmod 444 foo` mid-edit, exit → Save fails, swap survives, `chmod 644 foo`,
  relaunch → restore.
- Mtime conflict: edit, `kill -9`, `touch foo`, relaunch → conflict variant.
- 30-day-old swap files in cache dir → pruned on launch.
- `make verify` green.

## Out of scope (v1)

- Concurrent-edit lock (vim-style `.swp` lock file refusing a second open).
- Per-edit incremental swap (full-buffer rewrite is fine for this editor's scale).
- Cross-machine swap portability.

## Interaction with diff mode

See [diff-viewer.md](diff-viewer.md). Diff mode replaces the buffer with a view
that interleaves the editable file with baseline-deleted stripes, and installs
those stripes as `TextBuffer` locked ranges. A naive "dump the buffer" flush
would persist the stripes — wrong.

Rules:

- **Flush body is editable content, not raw buffer.** At flush time, if
  `buf.locked_ranges().is_empty()` dump the buffer as usual; otherwise dump
  `diff_mode::extract_real(&buf)`, which concatenates everything outside the
  locked ranges. The same rule applies to `TextBuffer::save_to_bytes`.
- **Cursor header is real-content coords.** In diff mode, translate the
  cursor's view offset through `diff_mode::view_to_real_off` before writing
  the `cursor` field. On restore the file reloads as plain editable text;
  the user re-enters diff mode manually if they want it.
- **Swap does not persist diff-mode state.** No baseline, no locked ranges,
  no decorations. Diff mode is a view over whatever real bytes are live; on
  restore, the user can toggle it back on and it re-reads the baseline from
  git.
- **Redundant flushes from rediff.** A rediff rebuilds the view buffer via
  `refresh_view_content`, which bumps `buffer.generation()` even though the
  editable bytes are unchanged. The standard flush check (`generation !=
  last_swap_write_generation`) will fire one extra write per rediff. Cost is
  one `extract_real` walk — cheap at the 5 MB size cap. If this matters,
  track a separate "user-edit generation" that rediff leaves alone.
- **Save path is already correct.** `Document::save` writes
  `diff_mode::extract_real` when diff mode is active, so swap eviction on
  successful save needs no diff-mode awareness.
