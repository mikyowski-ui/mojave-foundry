# mojave-foundry: agent instructions

mojave-foundry converts a user's own Fallout: New Vegas install into a
Fallout 2 format mod layer that `vault-engine` can load. It is an offline
tool. It ships no game data.

## Asset hygiene

- No game asset ever enters git: nothing from a New Vegas install, nothing
  from a Fallout 2 install, nothing extracted from either, and no converted
  output. `.gitignore` blocks the file types and output directories; do not
  work around it with `git add -f`.
- Every unit and integration test runs on synthetic fixtures built in memory
  by code in this repository (or small hand-authored text under
  `tests/fixtures/`). A test must not need a game install.
- Do not paste game data into source, tests, docs, commit messages or issue
  text. That includes record contents, dialogue, script text, texture pixels
  and palettes.
- Real-data runs write only to the `--out` directory, which must sit outside
  git or under an ignored path.

## Reference policy

mojave-foundry is **not** a clean-room project (owner decision,
2026-11-19). Its own code may be written with any public reference: format
documentation, wikis, tool documentation, and inspection of the owner's own
game files. The clean-room wall belongs to `vault-engine` and applies only
to what changes that engine's code; see "vault-engine boundary" below.

Three rules still hold here, for licensing rather than clean-room reasons:

- **`fnv_decomp`** is a read-only reference for non-data runtime behaviour.
  Never copy or port its code line by line. Never vendor or mirror it. Never
  build it. Never require a decrypted executable.
- **`nif.xml`** is format documentation only. Every NIF parser here is
  written by hand. No code generation from `nif.xml`, and no copying of its
  text into this repository.
- No third-party source code is pasted into this repository, whatever its
  origin.

## vault-engine boundary

- `vault-formats` is a one-way dependency pinned by revision in the root
  `Cargo.toml`. Never edit `vault-engine` from this repository, and never
  add a dependency in the other direction.
- When this pipeline needs something `vault-engine` lacks, write an owner
  brief under `docs/vault-engine-briefs/`. A brief describes Fallout 1 and 2
  engine behaviour and file layouts in generic terms. It never mentions New
  Vegas, its file formats, this pipeline's internals, or `fnv_decomp`.
- **Briefs are clean-room.** They change the engine's code, so the
  `vault-engine` wall applies to them in full: a brief is derived only from
  `vault-engine`'s own source and approved specs at the pinned revision.
  Whoever writes a brief must not read, search for or reference
  decompilations or re-implementations of the Fallout 1 or 2 executables
  (`fallout1-re`, `fallout2-ce`, `sfall` and the like) while doing so.
- Until a brief lands upstream, the local adapter lives in `crates/emit`
  behind a trait, so that swapping to the upstream writer is a one-file
  change.

## Engineering rules

- Edition 2024, `rust-version` 1.88, `unsafe_code = "forbid"`, clippy `all`
  at `-D warnings`. These match `vault-engine`.
- Determinism: identical inputs produce byte-identical outputs. Iterate in
  sorted order (`BTreeMap`, sorted `Vec`), never in hash order. No
  timestamps, host names or absolute paths in any output. The number of
  `--jobs` must not change any output byte.
- Parsers return `Result`. Malformed input is an error with an offset, not
  a panic, an unbounded allocation or an endless loop. Bound every count and
  length read from a file against the bytes remaining.
- `docs/contracts.md` holds the interfaces shared between crates. Change it
  first, then the code.

## Gates

A milestone is done when all three pass:

```
cargo check --workspace --all-targets
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

## Working with several agents

- At most four agents build at once, each with `-j 4`: the development
  machine has 15 GB of RAM.
- Each agent owns whole crates. Two agents never edit the same crate, and
  only the lead edits the root `Cargo.toml`, `Cargo.lock`, `docs/contracts.md`
  and `crates/mojave-convert`.
- Agents build only their own crates (`cargo test -p <crate>`). The lead
  runs the workspace gates once per wave.
- In a sandbox where `~/.cargo/registry` is read-only, set
  `CARGO_HOME=$PWD/target/cargo-home`.

## Milestones

- M0 to M3 are done: extraction to MIR, the sprite foundry, native
  Fallout 2 emission, and the Goodsprings exterior map. `docs/handoff-pass-1.md`
  says what was verified, what is deferred, and what comes next.
- M4's first wave is done: actor documents, skeletons, animations and the
  `critters` stage (`docs/contracts.md` section 10, handoff section 7).
- `crates/ragdoll` (M4, death art) and `crates/transpile` (M5) are still
  placeholders. Do not add code to them before that work starts.
