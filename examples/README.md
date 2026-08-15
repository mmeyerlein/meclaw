# examples

Six colonies, in the order they are worth reading. Every one of them is a directory tree plus
edges -- no example here adds a line of Rust.

| example | cells | what it is for |
|---|---:|---|
| [`hello`](hello/) | 2 | The smallest colony that does something. One `llm`, one edge. If you understand this folder, you understand the model. |
| [`swarm`](swarm/) | 7 | The tool loop as a shape: fan-out to tools, fan-in through a store, and a loopback edge that re-enters the `llm`. |
| [`telegram-research`](telegram-research/) | 12 | A real multi-tool agent on a real surface, written out node by node. Needs credentials. |
| [`meclaw-os`](meclaw-os/) | 17 | The same class of agent, **not** written out: an EMPTY seed -- two config files, zero cells -- plus one declaration that instantiates the whole tree from the template library at runtime. |
| [`never-forgets`](never-forgets/) | 18 | Tell it something in January, ask it in March. Bi-temporal episodes, an import lane for a past you already have, and a model that names the time range it wants. |
| [`hard-shell`](hard-shell/) | 3 | An agent you are allowed to attack. `kill -9` it mid tool run, point it at the cloud metadata endpoint, start it twice on one directory -- with a seed that configures no security at all. |

The first four are about *shape*; the last two are about a property you can try to break.

`hello`, `swarm` and `hard-shell` validate and boot without any key. `telegram-research` needs a
Telegram bot token and a provider key; `meclaw-os` and `never-forgets` need a provider key to
answer, and boot and grow without one.

Every colony here runs the guarded CEL form (`has(hop.x) && hop.x == ...`), which the sweep in
`crates/meclaw-colony/tests/gh80_shipped_conditions_are_guarded.rs` enforces over every shipped
`config.json`. Copy from these, not from an old README.
