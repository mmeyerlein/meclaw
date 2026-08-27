# Contributing to meclaw

meclaw is a framework for building agentic harnesses, and swarms of them, as a directory tree.
Issues, discussions, and PRs are all open. This file tells you how to build it, how to test it,
where the truth lives, and what makes a good first contribution.

First rule: read the honest status before you start. meclaw is a **0.x** proof of concept
for the DSL and the self-modifying substrate; the version that shipped last is the top entry
in [`CHANGELOG.md`](CHANGELOG.md). The mutation substrate is real and tested, and so
is the authoring path on top of it: `templates/builder` drafts a manifest, `templates/submit`
hands it in, and the colony is what applies it. If a change claims macOS support, or claims
federation / multi-builder / a native Anthropic provider, it does not match reality and will not
land. Keep us honest.

## Build it

Linux and rustup. `rust-toolchain.toml` pins an **exact** Rust version, so there is nothing to
choose: rustup fetches that toolchain on the first `cargo` command and your build is the same
one CI runs. The floor is set by edition 2024, which needs 1.85 or newer; the pin is currently
well above it.

The pin is deliberate (GH #406). An unpinned channel made the gate depend on the calendar —
byte-identical code passed clippy one evening and failed it the next, because a new stable had
promoted a lint — and it meant "green locally" said nothing, since the workstation and CI were
never on the same compiler. Raising the pin is therefore its own commit, which handles whatever
lints the new version denies in the same move. A build break from a newer stable is a piece of
scheduled work, not something that happens to a release.

```bash
git clone https://github.com/mmeyerlein/meclaw
cd meclaw
cargo build --release
# binary: ./target/release/meclaw
```

Run a colony as a daemon and watch it in the UI:

```bash
./target/release/meclaw --root ./examples/swarm --daemon --api 127.0.0.1:7777
# open http://127.0.0.1:7777/ui/
```

The `llm` cells talk to any OpenAI-compatible endpoint (OpenRouter by default). Drop a key in
`examples/swarm/.env` and start the daemon with `--env ./examples/swarm/.env`. See
`examples/swarm/README.md` and `examples/hello/README.md` for the full walkthrough.

## Test it

```bash
cargo test                 # the full suite, debug
cargo clippy -- -D warnings
cargo fmt --check
```

Notes that will save you time:

- Run the suite in **debug** (`cargo test`). This is the canonical, deterministic run. A couple
  of tests exercise a validation gate that is on by default only in debug builds, so they report
  as failures under `cargo test --release`. Debug is green.
- A couple of tests can flake in **release** builds under heavy parallelism, on wall-clock timing
  rather than logic, notably `paket_4` (backpressure `term_timeout`) and `phase_8` (MockOpenAI).
  These are test-harness timing artifacts, not product bugs. Debug is the canonical run; if you
  hit one in release, re-run rather than chase it.
- New behavior comes with a test. The hot routing paths are byte-pinned against fixtures on
  purpose, so they cannot quietly drift. If a fixture gate fails, that is the point. Do not
  edit the fixture to make it pass without understanding why it moved.

## Where the truth lives

`docs/` is the spec, and the spec is the source of truth. Before anything non-trivial, read
[`docs/meclaw-overview.md`](docs/meclaw-overview.md). It is the full system spec: the cell
model, the actor substrate, routing, mutations, the lot. `docs/cell-types.md` covers each
built-in cell, `docs/config.md` covers the `config.json` format.

When code and a comment disagree, the code wins. When the spec and the code disagree, that is a
bug worth an issue.

## The model in one breath

A colony is a folder. Folders marked `type: "hive"` are scopes that hold the graph. Every other
node is a Cell, an actor with one mailbox and one job. Cells are dumb: a cell knows its
contract, its params, and the one message in front of it, nothing else. The edges do the
thinking: routing, filtering, fan-out, loopback. The tool-loop is not a `while` loop, it is an
edge that routes back. Read `examples/hello` (two cells, one edge), then `examples/swarm` (the
loop as an edge), and it clicks.

## Good first contributions

These are genuinely useful and scoped to land without a week of context:

- **Example colonies.** New showcase trees under `examples/`. A summarizer, a router, a
  retry-with-backoff shape, a multi-tool agent. Small, real, runnable under the daemon, with a
  short voice-matched README in the folder. Use `examples/hello` and `examples/swarm` as the
  template.
- **Template cells.** Reusable subtrees under a `templates/` directory: a well-built `code`
  tool, a store-backed memory, a clean dispatcher or collector. The `code` cell is the Swiss
  army knife here.
- **Docs.** Clarify a section of `docs/`, add a worked example, fix a place where the spec and
  the code have drifted apart. Precision is the whole game.

Browse the issues labelled `good first issue` for specifics. If you want to attempt something
bigger off the roadmap (more than one builder per scope, federation, capability checks with
teeth, durability hardening), open an issue first so we can talk shape before you write code.

## Pull requests

- One logical change per PR. Keep commits clean.
- `cargo test`, `cargo clippy -- -D warnings`, and `cargo fmt --check` all green.
- New behavior has a test. New cells and examples run under the daemon.
- A PR that settles an architectural question says so, and the decision gets an ADR in the
  maintainers' `plans/adr/` (kept out of this clone) carrying a `Pinned-by:` line naming the
  test or symbol that embodies it. The anchors themselves do travel, as
  `.github/gates/adr-anchors.tsv`, and the `gates` job resolves every one of them: deleting the
  code that pins an accepted decision is a red run until the ADR is superseded.
- Match the surrounding voice in any prose. Confident, credible, no hype. And no spaced
  em-dashes, they read as machine-written.

## Named conventions

Code comments across the tree cite house rules by name. The short registry, so the
citations resolve without the private process docs:

- **Rule 12 (timeouts, two layers):** every I/O operation in cell code carries its own
  `tokio::time::timeout` (params-driven, precise); the substrate's per-message timeout is
  a generous backstop, never the primary shield. A timeout that covers only part of the
  operation is not a timeout.
- **Rule 14 (body blob pointers):** the substrate resolves `text_id`/`messages_id`
  pointers inside `messages[]` at the delivery boundary, recursively, bounded by
  `blob_max_recursion_depth` and a per-path visited set (issue #19). The former emission
  ban is gone: a cell may emit them, and no cell ever sees one. `{text_id}` leaves in the
  `system` tree are resolved at the same boundary under the same guards (issue #86); only
  the substitution differs, a leaf becomes `{"text": …}` rather than a turn. Both slots
  resolve against one working copy per delivery, so a failure in either dead-letters the
  body unchanged. `attachments[]` refs are a different class and stay unresolved by the
  substrate on purpose: the consuming cell reads them on demand, through a read-only store
  handle it only receives when its contract declares `consumes.body.attachments` (issue #87).
- **R9 (CLI shape):** flags only, nginx style. No subcommands.
- **A1' (panic-free hot path):** the colony routing/dispatch path never panics on
  pathological input; it answers with errors, skips, or dead letters. A panic there takes
  the whole colony, not one cell.
- **30s failure markers:** generous timeouts (30s convention) for failure markers in
  tests, robust under parallel cargo load; tight timing discriminators only where the
  test explains why.
- **Topology tests:** `#[tokio::test(flavor = "multi_thread", worker_threads = 4)]` for
  anything that boots a real topology.
- **Coding standards:** no `unwrap()`/`panic!()` outside tests, `thiserror` errors in
  libraries, doc comments on public items, no blocking sync I/O in async.
- **Demo discipline:** a test that claims to prove X proves it through a positive
  receipt signal, never through negative side effects.

## License

By contributing, you agree your work is dual-licensed under MIT or Apache-2.0, the same terms
as the project. See [LICENSE-MIT](LICENSE-MIT) and [LICENSE-APACHE](LICENSE-APACHE).
