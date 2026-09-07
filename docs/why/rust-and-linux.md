# Why Rust, why Linux only

meclaw is one static binary that runs a whole colony as one process, and the isolation it
gives a cell that runs foreign code belongs to the kernel. That second half is why there is
no macOS build, and it is the only reason for it.

## One process, one task per cell

Installing meclaw is copying a file. The release artefact is a stripped static musl binary
for `x86_64-unknown-linux-musl`, built by `.github/workflows/release.yml`, and the installer
refuses every other platform with a `cargo install` line in place of a broken download
(`scripts/install.sh`).

Every cell in the tree is one async task on the tokio runtime, and its state lives inside
that task. A colony of a few hundred cells is one process with a few hundred tasks, no
container per tool and no broker on the side. When a task dies the substrate knows which
directory it was and what routed there, and the supervisor restarts that one cell. Rust
earns its place here without much drama: no runtime to ship, no pause in the routing path,
and data races ruled out at compile time in a program whose whole job is message passing.

## The sandbox is four kernel mechanisms

A `bash`, `code` or `harness` cell starts a child process with the daemon's rights.
`crates/meclaw-cells/src/sandbox/mod.rs` is the boundary that takes those rights away again,
declared per cell in `params.sandbox` and enforced at spawn:

- Landlock for the filesystem view, so the child reaches only the paths its config grants.
- `unshare(CLONE_NEWUSER | CLONE_NEWNET)` for `network: "deny"`, which lands the child in a
  fresh namespace whose only interface is a down `lo`.
- A delegated cgroup v2 sub-cgroup for memory, pids and CPU.
- A seccomp-bpf filter assembled in this tree against `ptrace`, raw sockets and signals to
  processes outside the sandbox.

Two properties make that a boundary. It is written down: a `bash`, `code` or `harness` cell
from a template that declares no sandbox gets `trust: "restricted"`, `network: "deny"` and a
runtime-only filesystem written into its own `config.json`
(`default_sandbox_block` in `crates/meclaw-colony/src/mutation/stage.rs`), where a reviewer
can read it. And it is fail-closed: a profile that cannot be applied fails the spawn, and off
Linux a `restricted` profile is refused with an error naming the four mechanisms
(`crates/meclaw-cells/src/sandbox/other.rs`). `meclaw --sandbox-probe` reports per property
what your host can enforce, by trying each one instead of reading a knob.

I chose Linux only because those four are Linux mechanisms. A macOS port would keep the word
"sandboxed" in the config schema and lose the property behind it. The `mcp` cell type reads
and enforces the same profile and gets no default injected, so its sandbox is opt-in and an
`mcp` cell declaring nothing runs with the daemon's rights; `SANDBOX_ENFORCING_CELL_TYPES`
lists three types and the comment above it says why `mcp` is absent.

## Authentication is the reverse proxy's job

The HTTP server registers 21 routes and installs no authentication, no TLS and no session
(`crates/meclaw-api/src/router.rs`). meclaw knows paths and knows no identities, and a
substrate that mixed the two would hold two answers to the question of who may do what. Who
may reach the port is the reverse proxy's business, the same as for any other Linux daemon.
The binary opens no port unless you pass `--api`, and `--api 0.0.0.0:7777` is you deciding
what stands in front of it.

Inside the colony the same separation holds: identity is stamped onto the envelope by the
`operator` door, capabilities are the broker's question, and the `vault` cell type has no
route that returns a secret.
