# Contributing to keyd-viz

## Testing

keyd-viz leans on automated tests so a solo maintainer needs minimal manual QA.
The suite has several layers — design rationale in
[`docs/testing-harness-design.md`](docs/testing-harness-design.md).

### The one rule: every bug gets a regression test

When you find a bug, **write a test that fails before the fix and passes after**,
named for the symptom (e.g. `set_label_on_orphan_preserves_missing_final_newline`).
For a bug a property/mutation test surfaced, commit the **shrunk minimal case** as
a concrete `#[test]`, and keep the `.proptest-regressions` file (it replays the
failing seed deterministically). A bug without a regression test is a bug that
will come back.

### Layers, and how to run them

```sh
# Everything (fast — pure logic + headless unit tests):
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

- **Unit + round-trip + corpus** — the bulk of the suite, plus the byte-faithful
  CST round-trip gate (`serialize(parse(x)) == x`).
- **Property tests** (`crates/core/tests/properties.rs`, proptest) — a grammar-aware
  config generator asserting round-trip + parse/derive/build totality, with
  automatic shrinking to a minimal repro. Tune depth locally:
  ```sh
  PROPTEST_CASES=5000 cargo test -p keydviz-core --test properties
  ```
- **Stateful `EditSession` tests** (`crates/app/src/editing.rs`, proptest) — random
  mutation sequences asserting the editor never emits non-round-trippable text, a
  rejected edit leaves no partial state, and set-then-read-back is faithful.
- **Snapshot tests** (`crates/core/tests/snapshot.rs`, insta) — a semantic
  projection of the rendered board (no geometry). Review intended changes with
  `cargo insta review` (install: `cargo install cargo-insta`).
- **keyd differential oracle** (`crates/app/src/editing.rs`, `mod keyd_oracle`) —
  asserts the editor never false-alarms on a config the real `keyd` accepts.
  Needs keyd installed and is opt-in:
  ```sh
  KEYDVIZ_KEYD_ORACLE=1 cargo test -p keydviz --bin keydviz keyd_oracle -- --nocapture
  ```
  It also reports any disagreement against your `/etc/keyd` configs.

### Occasional / heavy (run locally, not in per-PR CI)

These saturate all cores — **on a laptop, cap `--jobs` and watch temps**:

```sh
# Mutation testing: do the tests actually CATCH injected bugs? Surfaces test gaps.
cargo install cargo-mutants
cargo mutants -p keydviz-core --jobs 4 --timeout 120

# Coverage map (also available as the manual-dispatch `coverage` CI job):
cargo install cargo-llvm-cov
cargo llvm-cov --workspace --html
```

CI runs the full per-crate test + clippy matrix (with keyd built from source so
the oracle gates), bounded to `PROPTEST_CASES=64` for determinism. Coverage is a
manual `workflow_dispatch` job only.
