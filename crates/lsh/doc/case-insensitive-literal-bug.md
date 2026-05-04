# `(?i:...)` literal bug

## symptom

at the top level of a definition (outside any `until` / `loop`), patterns like

```rs
if /(?i:FROM|RUN|CMD)\>/ { yield keyword.control; }
```

silently failed to match. no compile error, no runtime panic -- the `if` branch just never fired. lowercase-only inputs in the same shape (e.g. powershell's `(?i:function|param|class|...)`) matched fine, which is what hid the bug.

## root cause

`crates/lsh/src/compiler/regex.rs::emit_literal` interned the literal verbatim and tagged it with `Condition::PrefixInsensitive`. the runtime's `inlined_memicmp` (`crates/lsh/src/runtime.rs`) is documented as expecting the needle to be ascii-lowercase: it lowercases each haystack byte and compares it raw against the needle. when the literal contained any uppercase letter, every byte mismatched and the prefix check failed.

so `(?i:from)` worked (needle already lowercase), `(?i:FROM)` didn't. powershell got lucky -- all its keywords were lowercase. dockerfile didn't -- every instruction is uppercase by convention.

the charset path through `emit_charset` already lowercased before calling `emit_literal` (see the `idx.to_ascii_lowercase()` at the `[aA]` -> single-prefix optimisation), which is why single-char `(?i:...)` patterns weren't broken in the same way.

## fix

lowercase the needle in `emit_literal` when `case_insensitive` is set and any byte is uppercase. allocates only when a fix is actually needed (lowercase-only inputs hit the `&owned` borrow path without a copy). keeps the runtime invariant local to the compile-time emitter.

```rs
let owned;
let needle: &str = if case_insensitive && s.bytes().any(|b| b.is_ascii_uppercase()) {
    owned = s.to_ascii_lowercase();
    &owned
} else {
    s
};
let s = self.compiler.intern_string(needle);
```

## why it took so long to surface

- powershell.lsh, the only existing user of `(?i:keywords)`, used lowercase keywords.
- markdown.lsh's `(?i:sh|bash|json|...)` info-string matchers are all lowercase too.
- dockerfile.lsh had `(?i:FROM|RUN|...)` from day one but no one noticed -- the missing keyword.control on `FROM` looked like "the grammar just doesn't bother", not a bug.
- the `if` chain restores the input offset on failure, so a silently-failing `if` is indistinguishable from a non-matching `else if` later in the chain. nothing logged, nothing panicked.

## test coverage

dockerfile.lsh now exercises `(?i:FROM|RUN|...)`, `(?i:ENV|LABEL|ARG)`, and `(?i:AS|NONE|CMD|SHELL|CMD-SHELL)` against the fixture in `crates/lsh/tests/fixtures/dockerfile/Dockerfile`. the golden snapshot would regress if the lowercasing path were undone.
