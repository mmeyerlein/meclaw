# Contributing to meclaw

meclaw is one Linux binary that runs a tree of agents. Every folder in the tree is one actor
with one `config.json` and one SQLite file, and the edges between folders are the routes a
message may take. Issues, discussions and pull requests are open.

This is a 0.x proof of concept for the DSL and the self-modifying substrate; the version
that shipped last is the top entry in [`CHANGELOG.md`](CHANGELOG.md). The mutation substrate
is real and tested, and so is the authoring path on top of it: `templates/builder` drafts a
manifest, `templates/submit` hands it in, and the colony is what applies it. macOS support,
federation, more than one builder per scope and a native Anthropic provider do not exist. A
change that claims one of them does not match the tree and will not land. Keep me honest.

## Build it

Linux and rustup. `rust-toolchain.toml` pins an exact Rust version, so there is nothing to
choose: rustup fetches that toolchain on the first `cargo` command, and your build is the one
CI runs. Edition 2024 sets the floor at 1.85, and the pin sits well above it.

The pin came out of GH #406. On an unpinned channel the gate depended on the calendar.
Byte-identical code passed clippy one evening and failed it the next, because a new stable had
promoted a lint. "Green locally" then said nothing, since the workstation and CI were never on
the same compiler. Raising the pin is therefore its own commit, and that commit handles
whatever lints the new version denies.

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

- Run the suite in debug (`cargo test`). That is the canonical, deterministic run. A couple of
  tests exercise a validation gate that is on by default only in debug builds, so they report
  as failures under `cargo test --release`. Debug is green.
- A couple of tests can flake in release builds under heavy parallelism, on wall-clock timing:
  `paket_4` (backpressure `term_timeout`) and `phase_8` (MockOpenAI). Both are timing artifacts
  of the test harness. Debug is the canonical run, so if you hit one in release, re-run it
  before you chase it.
- New behavior comes with a test. The hot routing paths are byte-pinned against fixtures, so
  they cannot drift unnoticed. A failing fixture gate is doing its job, so do not edit the
  fixture to make it pass before you understand why it moved.

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
contract, its params, and the one message in front of it. The edges do the thinking: routing,
filtering, fan-out, loopback. A tool loop is an edge that routes back into the `llm` cell. Read
`examples/hello` (two cells, one edge), then `examples/swarm` (the loop as an edge).

## Good first contributions

These are genuinely useful and scoped to land without a week of context:

- Example colonies. New trees under `examples/`: a summarizer, a router, a retry-with-backoff
  shape, a multi-tool agent. Keep them small and runnable under the daemon, with a short README
  in the folder that matches the voice of the others. `examples/hello` and `examples/swarm` are
  the ones to copy from.
- Template cells. Reusable subtrees under a `templates/` directory: a well-built `code` tool, a
  store-backed memory, a clean dispatcher or collector. The `code` cell is the Swiss army knife
  here.
- Docs. Clarify a section of `docs/`, add a worked example, fix a place where the spec and the
  code have drifted apart.

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
  `.github/gates/adr-anchors.tsv`, and the `gate` job resolves every one of them: deleting the
  code that pins an accepted decision is a red run until the ADR is superseded.
- Match the surrounding voice in any prose. Confident and credible, without hype. Spaced
  em-dashes read as machine-written, so leave them out.

## Named conventions

Code comments across the tree cite house rules by name. The short registry, so the citations
resolve without the private process docs:

- Rule 12, timeouts in two layers. Every I/O operation in cell code carries its own
  `tokio::time::timeout`, driven by params and cut tight; the substrate's per-message timeout
  is a generous backstop and never the primary shield. A timeout that covers only part of the
  operation is no timeout.
- Rule 14, body blob pointers. The substrate resolves `text_id` and `messages_id` pointers
  inside `messages[]` at the delivery boundary, recursively, bounded by
  `blob_max_recursion_depth` and a per-path visited set (issue #19). The former emission ban is
  gone: a cell may emit them, and no cell ever sees one. `{text_id}` leaves in the `system`
  tree are resolved at the same boundary under the same guards (issue #86); only the
  substitution differs, since a leaf becomes `{"text": ...}` where a pointer in `messages[]`
  becomes a turn. Both slots resolve against one working copy per delivery, so a failure in
  either dead-letters the body unchanged. `attachments[]` refs are a different class and the
  substrate leaves them alone: the consuming cell reads them on demand, through a read-only
  store handle it receives only when its contract declares `consumes.body.attachments`
  (issue #87).
- R9, CLI shape. Flags only, nginx style. No subcommands.
- A1', panic-free hot path. The colony routing and dispatch path never panics on pathological
  input; it answers with errors, skips, or dead letters. A panic there takes down every cell in
  the colony at once.
- 30s failure markers. Generous timeouts (the 30s convention) for failure markers in tests, so
  they survive parallel cargo load; tight timing discriminators only where the test explains
  why.
- Topology tests. `#[tokio::test(flavor = "multi_thread", worker_threads = 4)]` for anything
  that boots a real topology.
- Demo discipline. A test that claims to prove X proves it through a positive receipt signal,
  never through negative side effects.

## Coding standards

No `unwrap()` or `panic!()` outside tests. Libraries return `Result` with `thiserror` errors,
and the binary may use `anyhow::Result`. Every public item carries a doc comment. No blocking
sync I/O in async code.

## License

By contributing, you agree your work is dual-licensed under MIT or Apache-2.0, the same terms
as the project. See [LICENSE-MIT](LICENSE-MIT) and [LICENSE-APACHE](LICENSE-APACHE).
