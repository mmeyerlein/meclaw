# Start here

These pages explain the concepts of meclaw and how to work with them. They do not document the
source: the code is in the repository, and reading it is the way to every implementation detail.
Questions belong in [Discussions](https://github.com/mmeyerlein/meclaw/discussions), problems in
[Issues](https://github.com/mmeyerlein/meclaw/issues).

## The pages

- [Getting started](getting-started.md) installs the binary, boots meclaw-os, and adds an organisation, a member and an agent of your own, one command per step.
- [Configure your model](model.md) says where the key and the model go, which variables the colony reads, and how to point it at a local endpoint.
- [meclaw](meclaw.md) puts the primitives on one page: colony, cell, hive, edge, hop, mutation, template and instance.
- [Cells](cells.md) goes through every cell type in a few lines each, with a snippet for `llm`, `store` and `code`.
- [meclaw-os](meclaw-os.md) describes the reference implementation: the four levels, who lives on each, and what each level owns.
- [Security](security.md) covers the sandbox around every cell, the hive boundary, the vault, access and the firewall.
- [Templates and apps](templates-and-apps.md) tells a template, a subtree that gets copied, apart from an app, a sealed hive at the rim of a member.
- [Why it is built this way](why/README.md) holds the architecture decisions, one question per page.

## Reference

- [meclaw-overview.md](meclaw-overview.md) is the full system spec. On conflict with any other file, this one wins.
- [cell-types.md](cell-types.md) lists every cell type with its params, its contract and its failure modes.
- [config.md](config.md) describes the blocks of a `config.json`, variable substitution, and what a cell is allowed to know.
- [rewiring.md](rewiring.md) adds, moves and disconnects cells against a colony that keeps running.
- [stability.md](stability.md) names the five public surfaces and what `0.x` promises about them.
- [status.md](status.md) names the platform, the runner and the other limits a colony has today.
- [voice-wire-protocol.md](voice-wire-protocol.md) documents every frame of a `meclaw-voice/1` WebSocket, close codes included.
- [store-backed-tool-loop.md](store-backed-tool-loop.md) fans tool calls out, collects every result, and re-enters inference exactly once.
- [costs.md](costs.md) measures provider spend out of `colony.db` and compares it with numbers from one running colony.
- [glossary.md](glossary.md) defines the words the other pages assume.
- [installation.md](installation.md) walks through every knob of `install.sh` and `start.sh`, and the same steps by hand.

[Roadmap](../ROADMAP.md) · [Contributing](../CONTRIBUTING.md) · [Changelog](../CHANGELOG.md) · [Discussions](https://github.com/mmeyerlein/meclaw/discussions) · [Issues](https://github.com/mmeyerlein/meclaw/issues)

## If you are new

1. [Getting started](getting-started.md), until a colony answers you.
2. [meclaw](meclaw.md), for the words the rest of these pages use.
3. [meclaw-os](meclaw-os.md), to see what those words compose into.
4. [Why it is built this way](why/README.md), when you want to know why a boundary sits where it sits.
