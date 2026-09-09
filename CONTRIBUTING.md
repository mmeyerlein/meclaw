# Contributing

meclaw is one Linux binary that runs a tree of agents. Every folder in the tree is one actor
with its own `config.json` and its own SQLite file, and the edges between folders are the
routes a message may take. This is a 0.x proof of concept with one maintainer; issues,
discussions and pull requests are open. macOS support, federation and a native Anthropic
provider do not exist, and a contribution that claims one of them does not match the tree and
will not land.

## The path of a contribution

Open an issue first. GitHub is the tracker of record, and a change that arrives without one has
nowhere to be discussed before it is written.

A good issue names what you saw and what you expected, the colony layout that produced it (the
directory tree and the message you sent), and `meclaw --version`. Strip keys and private
hostnames out of any trace you paste. The [issue templates](.github/ISSUE_TEMPLATE) ask for
exactly this.

The label `ruling` means the issue waits on a decision by the maintainer before anything is
built. If your issue carries it, wait for the decision instead of opening a pull request
against it.

Then the pull request, on an issue that already exists. One logical change per PR. New
behavior comes with a test that fails without it. `cargo test`, `cargo clippy -- -D warnings`
and `cargo fmt --check` are green before you push.

## Build it

Linux and rustup. `rust-toolchain.toml` pins an exact Rust version, so rustup fetches that
toolchain on your first `cargo` command and your build is the one CI runs.

```bash
git clone https://github.com/mmeyerlein/meclaw
cd meclaw
cargo build --release
# binary: ./target/release/meclaw
```

## Test it

```bash
cargo test                 # the canonical run, debug
cargo clippy -- -D warnings
cargo fmt --check
```

Run the suite in debug. A few tests exercise a validation gate that is on by default only in
debug builds, so they report as failures under `cargo test --release`.

## Where the truth lives

`docs/` is the spec, and the spec is the source of truth. [`docs/README.md`](docs/README.md) is
the way in. [`docs/meclaw-overview.md`](docs/meclaw-overview.md) is the full system spec and
wins against every other file in `docs/`.

When code and a comment disagree, the code wins. When the spec and the code disagree, that is a
bug worth an issue.

## Docs are a contract surface

Five surfaces are public: the HTTP API, the template DSL, the template ports, the origin a
`web` cell owns, and the documented `error_code` strings
([`docs/stability.md`](docs/stability.md)). A change to one of them arrives with its
documentation in the same pull request, never as a follow-up. Match the surrounding voice in
any prose you write, and leave out spaced em-dashes, which read as machine-written.

One page answers one question, in one shape: what it is, why it exists, how it is used, one
example, where to read on. Leave out the sentence that only repeats what the command, the
heading or the paragraph above it already says, and leave out every comparison with another
project. The full set is § Documentation principles of `docs/development-rules.md`.

## The byte-pinned paths

The colony's routing and death-handling bodies are frozen byte for byte against fixtures in
`.github/fixtures/`, and a gate diffs them on every CI run. A red fixture gate means one of
those bodies moved, so find out why it moved before you touch the fixture.

## Good first contributions

- Example colonies under `examples/`: a summarizer, a router, a retry-with-backoff shape. `examples/hello` and `examples/swarm` are the ones to copy from.
- Template cells under `templates/`: a well-built `code` tool, a store-backed memory, a clean dispatcher.
- Docs drift: a place where `docs/` and the code have grown apart.

Issues labelled `good first issue` name specifics.

## License

By contributing, you agree your work is dual-licensed under MIT or Apache-2.0, the same terms
as the project. See [LICENSE-MIT](LICENSE-MIT) and [LICENSE-APACHE](LICENSE-APACHE).
