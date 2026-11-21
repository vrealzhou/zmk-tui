# Mutation testing report

- Tool: `cargo-mutants 27.1.0`
- Config: [`.cargo/mutants.toml`](../.cargo/mutants.toml)
- Focused runner: [`tools/mutate.sh`](../tools/mutate.sh)

Mutation testing is intentionally scoped to pure keymap/protocol logic. The
terminal renderer, ratatui widgets, CoreBluetooth/ObjC FFI, serial threads and
hardware entry points are excluded: their mutants need a real terminal or
keyboard, and a surviving mutant there does not imply a useful test gap.

## Frame codec (complete run)

Command:

```bash
cargo mutants -f src/protocol/frame.rs --jobs 6 --timeout 60 -- --lib
```

| Result | Count |
| --- | ---: |
| Mutants tested | 20 |
| Caught | 19 |
| Missed | 1 |
| Timeout | 0 |
| Unviable | 0 |
| Mutation score | **95%** |

Evidence: [`docs/mutation/frame-caught.txt`](mutation/frame-caught.txt),
[`docs/mutation/frame-missed.txt`](mutation/frame-missed.txt).

The single survivor is:

```text
src/protocol/frame.rs:22:39: replace * with + in Vec::with_capacity(payload.len() + 8)
```

It changes only the pre-allocation size in `encode`; the resulting `Vec` grows
as needed and the encoded bytes are identical. This is an equivalent mutant for
observable behaviour, not a missing assertion. The codec's delimiter handling,
frame restart, oversized-frame rejection and empty-frame paths are otherwise
killed.

## Keycode metadata (partial + targeted rerun)

An exploratory run over seven modules found 15 survivors, all in
`src/keycodes/mod.rs`:

- `Category::label` replacements (`""`, `"xyzzy"`);
- `Keycode::describe` and `Keycode::modifier_symbols` replacements;
- `Keycode` equality mutations;
- the free `describe` function;
- a `search` accumulator mutation.

New tests now assert the exact category labels, implicit-modifier symbols,
`describe` output, equality/inequality, zero/known/unknown descriptions and the
modifier symbol table.

A targeted rerun of 30 related mutant patterns was stopped after **7 caught /
0 missed**; the remaining 23 were not run within the tooling budget. The
earlier full keycode run was also interrupted after **5 caught / 0 missed**.
Together these are a partial but clean signal: every keycode mutant actually
executed after the new tests was killed. A complete run remains nightly work.
The suite is reproducible with:

```bash
cargo mutants -f src/keycodes/mod.rs --jobs 6 --timeout 60 -- --lib
```

## Labels, mouse, modals, layout, model

These files have unit tests, but a complete mutation run was not finished. They
are included in `tools/mutate.sh` for a longer CI/nightly run. The exploratory
run had 55 caught and 15 missed before it was stopped; the misses were all in
the keycode module and are addressed above.

## Interpreting survivors

Not every survivor is a defect. Mutants that alter allocation capacity, debug
formatting, or a branch equivalent for the corpus of keycodes are semantically
equivalent. Review `docs/mutation/frame-missed.txt` and the focused run before
adding a test: a test that only exists to kill an equivalent mutant is noise.

## Commands

```bash
# Focused frame proof (fast)
cargo mutants -f src/protocol/frame.rs --jobs 6 --timeout 60 -- --lib

# Broader nightly run
JOBS=8 cargo mutants -f src/protocol/frame.rs -f src/labels.rs \
  -f src/tui/mouse.rs -f src/tui/modals.rs -f src/keycodes/mod.rs \
  -f src/layout.rs -f src/model.rs -- --lib
```
