# examples/display-colony-view

Two owners, one screen.

A screen in this library is a **channel**, not a picture: it belongs to whoever the
colony grew it for, and everybody who has something to show writes onto it. This
example grows one — [`display`](../../templates/display/) — and puts two producers
in front of it. One is an application that draws the colony's own topology
([`colony-view`](../../templates/colony-view/)); the other is a `code` cell with
a paragraph and no application at all.

Neither of them can touch the other's view, and that is not a rule either of them
follows. It is where the owner comes from: the screen writes a view under the
`reply_to` the substrate stamped on the emission, so a body that claims a
different owner is refused rather than believed.

## What is checked in

```
display-colony-view/
├── seed/                            the --root of the colony
│   ├── colony.json                  substrate defaults, plus the one line that
│   │                                opts into `mutation_receipts` (GH #553)
│   └── main/
│       ├── config.json              type: "hive", and its graph is EMPTY
│       ├── tick/config.json         a timer, once a minute
│       └── scribe/
│           ├── config.json          the second owner: one code cell
│           └── scribe.py            its source, copied into script_inline
├── grow.json                        the declaration. three nodes, six edges.
└── README.md
```

The screen, the application and the sink are **not** in there: they are
instantiated from the template library when `grow.json` is applied, and every lane
of this colony is written in that one file. What a library cannot ship for you is a
producer with something to say, so that is the part that is checked in.

## The two producers

**`colony-view`** is an app. It hears that the graph moved, asks the colony's
read-only graph endpoint, and emits one view whose content is a component tree —
plus the components that draw it, declared in the same body. It has no port and no
display of its own; which screen it lands on is the edge's decision, not its own.
What tells it the graph moved is the **mutation receipt** (GH #553): `colony.json`
opts in with `"mutation_receipts": {"to": "/"}`, `grow.json` draws
`. -> ./colony-view` on `hop.route == 'mutation_committed'`, and the receipt of
that very mutation is what puts the first picture up. Until `colony-view@1.1.0`
this was a one-minute poll timer inside the app.

**`scribe`** is the floor of the same model. `kind: "prose"`, a title and a
paragraph, no components, no port, no markup. That is what an ordinary agent's view
looks like before anybody builds an application, and it is why the library has no
"minimum viable app": there is nothing to build.

Both send on the lane `view`, and the edge that carries them to the screen turns it
into `in_view`:

```json
{"from": "./scribe", "to": "./display",
 "condition": "has(hop.route) && hop.route == 'view'",
 "modifier": {"set_hop": {"route": "'in_view'"}}}
```

That shape is the whole wiring contract. An agent is wired exactly like the app.

## The sink is not decoration

Two lanes leave the screen — `event` (a person did something in the browser) and
`receipt` (a refusal, addressed to whoever asked) — and both name the `owner` they
belong to, because in a real colony the level above routes them back to that one
agent. This example has no such level, so both go to a `terminal`: a lane with a
destination that discards is honest, and a lane with no destination at all
dead-letters. Neither is silent, which is the point.

## Running it

**1. Grow the colony.**

```bash
./target/release/meclaw --root ./examples/display-colony-view/seed \
                        --templates ./templates \
                        --apply ./examples/display-colony-view/grow.json
```

**2. Start it.**

```bash
./target/release/meclaw --root ./examples/display-colony-view/seed \
                        --templates ./templates \
                        --daemon --api 127.0.0.1:7788
```

**3. Open the screen.**

```
http://127.0.0.1:7899/
```

The topology picture is there at once: `colony.json` opts this colony into
`mutation_receipts`, so the mutation that grew the screen left a receipt at the
root hive, the root hive carried it into `./colony-view`, and the app drew the
colony it had just become part of. The scribe's own view follows within a minute
— that one really is a timer, and it is the example's PRODUCER, not the app.
Drag a node of the topology picture: nothing enters the router — the write lands
in the screen's own database and is diffed to every open browser — and it is
still where you put it after the next redraw, because the application's node
declares `keep` and the screen leaves those props alone on an object it already
holds.

**Ask for a fresh picture without changing anything:**

```bash
curl -s -X POST http://127.0.0.1:7788/messages \
  -H 'Content-Type: application/json' \
  -d '{"target": "/colony-view", "hop": {"route": "in_refresh"}, "body": {"messages": []}}'
```

## The port

One port per screen, and it is chosen at instantiation — `grow.json` names `7899`.
A second screen in the same colony is a second `display` on a different port, which
is what makes one reverse-proxy rule a complete access statement for one of them.
Authentication and TLS are external, permanently (R-W8-2): the screen binds
loopback by default and everything in front of it is somebody else's job.

## What this example is not

- **Not a window manager.** Nothing here decides what deserves attention. The
  screen stacks by `updated_at` and stops; a producer that has something new says
  it again, and that is the whole focus model in v1.
- **Not multi-screen.** One region, called `main`.
- **Not a model.** There is no `llm` cell in this colony at all, so it costs
  nothing to run and needs no provider key.
