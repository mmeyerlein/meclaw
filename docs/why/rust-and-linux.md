# Why Rust, why Linux only

Why one Rust binary, and why does it run on Linux and nowhere else? The binary is the whole
runtime: a colony of a few hundred cells is one process with one task per cell, no container per
tool and no broker beside it. Linux is the whole isolation: the sandbox around a cell that runs
foreign code is four kernel mechanisms, and there is no portable stand-in for them.

## One process

Installing meclaw is copying a file. The release artefact is a stripped static musl binary for
`x86_64-unknown-linux-musl`, built by `.github/workflows/release.yml`, and `scripts/install.sh`
answers every other platform with a `cargo install` line instead of a download that would not run.

Every cell is one async task on the tokio runtime and its state lives inside that task. When a
task panics the supervisor re-instantiates that one cell `one_for_one` and the rest of the colony
keeps routing ([`meclaw-overview.md`](../meclaw-overview.md) § Cell model). Rust earns its place
quietly here: no runtime to ship, no pause in the routing path, and data races ruled out at
compile time in a program whose whole job is message passing.

## The isolation belongs to the kernel

A `bash`, `code` or `harness` cell starts a child process with the daemon's rights.
`crates/meclaw-cells/src/sandbox/mod.rs` takes those rights away again at spawn, out of the cell's
own `params.sandbox`, using Landlock, network namespaces, a delegated cgroup v2 sub-cgroup and a
seccomp-bpf filter. Not one of the four is something meclaw enforces itself. Each is enforced by
the kernel against a process the daemon started, which is what makes the boundary hold for code a
model wrote a minute ago. What each one covers, and which cell types get a default profile written
into their `config.json`, is on [security](../security.md).

## What that costs

There is no macOS build. Off Linux not one of the four mechanisms exists, so a `restricted`
profile is refused at spawn rather than quietly dropped
(`crates/meclaw-cells/src/sandbox/other.rs`). A port would keep the word `sandboxed` in the config
schema and lose the property behind it, which is a worse answer than no port.
`meclaw --sandbox-probe` reports per property what the host in front of you can enforce, by
trying each one instead of reading a knob.

## Where to read on

- [security](../security.md) for the boundaries this one is part of
- [`installation.md`](../installation.md) for building from source on a host without the release binary
