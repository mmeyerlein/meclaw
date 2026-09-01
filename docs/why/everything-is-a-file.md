# Everything is a file

Every agent framework asks you to express your agent in its code: subclass this, register
that, wire the loop, redeploy. meclaw asks you to express it in a directory tree — and
then it runs the tree.

## The tree runs

A colony is a directory. Every folder in it is a **cell**: an actor with a mailbox, an
async task of its own, and a `config.json` that says what it is — an `llm` brain, a
`store`, a `bash` runner, a `web_fetch` probe, one of 15 built-in types
([cell types](../cell-types.md)). Folders that group cells are **hives**. The **edges**
between them are the routes a message can take, and a message that travels writes a typed
`hop` into the trace at every step.

That is the whole model. There is no second config format, no DSL file beside the tree,
no registry the tree must match. The tree *is* the topology.

## Why that buys flexibility

Because the harness is files, everything you already know how to do with files works on
your agent system:

- `ls` shows the topology, `grep` searches it, `diff` reviews a change, `git` versions it.
- A harness pattern — tool loop, plan-and-execute, fan-out — is not a library feature.
  It is a shape: tools are cells, the loop is an edge that routes back.
- An `llm` cell makes **one** provider call and emits **one** message. No inner loop,
  ever. Whatever loops in your system is visible as edges on disk.

And because the harness is files, an agent can rebuild it. Not by writing Rust — by
submitting the same kind of change a human submits.

## One door for every change

A running colony changes through exactly one operation: a **mutation**, `POST`ed to
`/colony/mutations`. A mutation is a diff in a closed vocabulary — add nodes, add edges,
remove them, register templates, adjust params. It is validated against the tree,
applied atomically while everything keeps running, and recorded in the mutation ledger
(`GET` on the same address). There is no second way. That single door is what makes
"agents rebuild their own harness" auditable instead of terrifying: every change has a
record, every record has a requester.

## No SDK, no plugin API — deliberately

The interface is HTTP and files. You do not import meclaw into your program; you point
the binary at a tree. Extension does not mean writing a plugin against an API surface
that must stay stable — it means adding cells and templates. The proof is shipped: **38
templates** in [`templates/`](../../templates/README.md), from a ten-line door to a
complete operating shell, and not one of them contains a line of Rust. Instantiating a
template **copies** it into your tree; from that moment it is yours, with no link back to
the library.

Foreign tools still connect — the `mcp` cell type speaks MCP to external tool servers —
but they connect as cells, over routes, inside the sandbox, like everything else.

## What this is not

It is not "config over code" as a slogan. Code exists — `bash`, `code` and `harness`
cells run real programs, under a real kernel sandbox
([Why Rust, why Linux](rust-and-linux.md)). The point is narrower and harder: the
*harness* — who talks to whom, what loops, what escalates — is data, changed through one
recorded door, readable by every tool you already have.
