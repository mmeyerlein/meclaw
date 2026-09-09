# Why it is built this way

These pages carry the architecture decisions behind meclaw, one question per page, at the level
of the decision rather than the code.

- [Everything is a file](everything-is-a-file.md) answers why the harness lives in the file system and why there is no SDK.
- [An operating system for agents](an-os-for-agents.md) answers what meclaw-os is and what its four nested levels are for.
- [Memory that outlives the window](memory.md) answers where a conversation is kept once the context window is not the place.
- [One assistant, two brains](two-brains.md) answers why the shipped assistant runs two models.
- [Why Rust, why Linux only](rust-and-linux.md) answers what the single binary and the kernel sandbox buy, and what they cost.
- [Ontology, in the meclaw sense](ontology.md) answers which typed vocabulary a colony is composed out of, and which meaning of the word is not meant here.
- [Prepared for self-improvement](rsi.md) answers which primitives for rebuilding a running colony exist today, and why no loop closes them unattended.
- [You talk, it shows](you-talk-it-shows.md) answers what the voice cell and the screen are aimed at together.

Two files here are redirects for older links: [names.md](names.md) points at the glossary, and
[self-modification.md](self-modification.md) points at `rsi.md`.
