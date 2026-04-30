# meanderings

design notes, proposals, and comparisons. some shipped, some were
rejected, some are still open. **NOT ALL SHOULD SHIP** -- these are just
(sometimes wild) meanderings.

Proposals live in `*.prop.md` files. Each has a YAML frontmatter:

```yaml
---
status: open | implemented | rejected | shelved
date: YYYY-MM-DD
description: one-line hook
---
```

Files prefixed with `_` are no longer "active" (implemented or shelved),
so they sort to the bottom of `ls`.

## Style: no verdicts

Proposals describe the **gap**, the **options**, **trade-offs**, and
**open questions** -- not whether to ship. Avoid `Lean:`,
`Recommendation:`, `Preference:`, `should live in edit`, etc. The
ship / no-ship call happens at implementation time with current context;
baking a verdict into the proposal biases that decision and ages badly.

If something has been decided, change `status:` and capture the outcome
in a short closing section (see the `_*.prop.md` files for shape).

## Index

Run `./index.sh` to print an up-to-date markdown index grouped by status.
Other `.md` files in this directory are ignored.
