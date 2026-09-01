# Why Rust, why Linux only

Two decisions, one reason each — and both are about properties you can check, not tastes.

## Rust: one binary, one task per cell

meclaw is a single static binary. Installing it is copying a file; running a colony is
pointing that file at a directory. Every cell in the tree is one async task on the tokio
runtime, so a colony with hundreds of cells is one process with cheap, isolated,
supervised concurrency — not a container fleet, not a Python venv per tool, not a
message broker on the side. When a cell's task dies, the substrate knows which folder
that was and what routed there; nothing else is taken down with it.

Rust's part in this is unglamorous and constant: no GC pauses in the routing path, no
runtime to ship, data races ruled out at compile time, and a binary that behaves the same
on the machine it was tested on and the machine it was deployed to.

## Linux: the sandbox is the kernel's, not ours

The four cell types that start foreign code — `bash`, `code`, `harness`, `mcp` — run
their child under kernel enforcement:

- **Landlock** for the filesystem: the cell sees what its config grants, nothing else.
- **A fresh network namespace** under `network: "deny"`: nothing but a loopback interface
  in state DOWN — even `127.0.0.1` is out of reach, so no packet leaves and none arrives.
- **cgroup v2** for memory, pids and CPU.
- **seccomp-bpf** against ptrace, raw sockets and foreign signals.

Three properties make this a security model rather than a feature list:

1. **Deny by default.** A `bash`, `code` or `harness` cell instantiated from a template
   with no sandbox block of its own gets deny-by-default written into its `config.json` —
   visible, and therefore reviewable and editable.
2. **Closed key sets.** Every sandbox key set is closed, so a typo like
   `"netwrok": "deny"` is a boot error, not a silently unsandboxed cell.
3. **Fail-closed.** A restriction that cannot be enforced on this host makes the spawn
   fail. There is no path on which a restricted cell quietly keeps running unsandboxed —
   and `meclaw --sandbox-probe` tells you what your host can enforce before production
   does.

This is why there is no macOS build. Landlock, namespaces, cgroups and seccomp *are* the
model; a port without them would keep the word "sandboxed" and lose the property. A
claim of macOS support would not match reality.

## A daemon, the Linux way

meclaw binds a port and speaks HTTP. **There is no web authentication in the binary**,
and that is a design decision, not a gap: meclaw knows no identities, it knows paths, and
a substrate that mixes the two holds two truths about who may do what. Who may reach the
port is the reverse proxy's job — nginx and friends — exactly as it is for every other
Linux service. The binary opens no port by default; `--api 127.0.0.1:7777` is opt-in,
and binding `0.0.0.0` is you deciding what stands in front of it.

Inside the colony, the same separation holds: identity is stamped by the `operator` door
onto the envelope, capabilities are the broker's question, and secrets live in a
[vault with no `get`](names.md#the-authorities) — a credential leaves it only sealed,
under a key the requester minted for that one call.
