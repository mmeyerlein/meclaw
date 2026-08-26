# Migrating a `canvy` instance from 1.x to `canvy@2.1.4`

**A 1.x instance is not upgraded in place.** Every address the template offered
was removed — the server-rendered markup, the `store` cell that held the
positions, and the `/surface/…` path the HTTP API served it on — which is why
the first digit moved. Swapping the template under a running instance would
leave a hive whose cells no longer exist, wired to lanes that no longer mean
anything.

So the shape of this migration is: **instantiate 2.0.0 beside the old one, carry
the one thing a person made by hand across, and retire the old hive by
disconnect.** Nothing is deleted anywhere.

**This repository ships the recipe and the tool; running either is the
operator's act.** No test here touches a deployed tree, no script here is wired
into a boot, and nothing in this document happens by itself. Read it through
before you start — steps 2 and 3 are ordered for a reason that only bites
afterwards.

---

## What crosses, and what does not

| 1.x row | 2.0.0 | Why |
|---|---|---|
| `node` | **carried** — becomes the `x`/`y` of the cell's `canvy-node` object | the one thing a person made by hand |
| `hive_shift` | **dropped** | a frame is derived from the cells it holds; moving a hive is moving its members, so there is no row to move ([#170](https://github.com/mmeyerlein/meclaw/issues/170)) |
| `camera` | **dropped** | pan and zoom are local browser state and are neither sent nor stored |
| the topology snapshot | **dropped** | the display's own object tree holds the picture; the next tick refills it |

The dropped rows are **reported**, not skipped quietly: the export writes a
receipt beside the bundle naming every row it left behind and why. Your hive
frames will land wherever their members land, and that is the correct outcome
rather than a loss — but it is a thing to know before the first screenshot, not
after.

## Before you start

- **The two instances do not collide.** 1.x owned no port; it was served by the
  HTTP API under `/surface/<cell path>`. 2.0.0's display owns a port of its own
  (the template ships `7810`), so both can run side by side for as long as you
  want to compare them.
- **RETRACTED (GH #410, `canvy@2.1.0`): the display's port is immutable once
  the cell exists.** Up to `canvy@2.0.1` this step said: *"Pick a free one at
  instantiation; a later params update naming it is refused without partial
  apply."* **That refusal is withdrawn.** Pick a free one anyway — but if you
  pick wrongly, or if you later want the new canvas on the address the old one
  had, send the display cell `{"params": {"port": 7810}}` instead of rebuilding
  it. Positions survive, because the `cell.db` is untouched. Until the old
  canvas is retired, a second location block on the reverse proxy is still the
  simpler answer.
- **Find the old store.** The positions live in the `cell.db` of the 1.x
  instance's `store` cell — the directory named `store` inside the old hive's
  directory, under the colony root. Not the hive's directory, and not another
  cell's: the export refuses anything that carries no `canvas` table.
- **Copy it first if the old colony is up.** The export opens the database
  read-only, but a copy is one command and removes the question entirely.

---

## 1. Instantiate `canvy@2.1.4` beside the old hive

A mutation, into a running colony. Give the node a name that does not collide
with the old one and the display a free port:

```json
{
  "scope": "/ops",
  "diff": {
    "add_nodes": [
      {
        "name": "canvy2",
        "template": "canvy@2.1.4",
        "override_params": {"web": {"port": 7811}}
      }
    ]
  }
}
```

```bash
curl -s -X POST http://127.0.0.1:7777/colony/mutations \
     -H 'Content-Type: application/json' -d @grow-canvy2.json
```

`override_params` is keyed by the cells of the instantiated template, and `web`
is the display. `examples/meclaw-os/grow-canvy.json` ships the same shape as a
file you can post as it stands — into a colony with no canvas yet, so its node
is simply called `canvy`.

**Nothing has to point at it.** The hive declares no ports, so no edge reaches a
cell inside it and the way in is the HTTP port the display owns. If you had an
edge asking the old canvas for a fresh snapshot, draw the same lane at the new
hive path — the lane is `in_refresh` and the hive path is the address.

## 2. Let it draw itself once — **before** step 4

The timer takes a topology snapshot on the minute and the layout cell turns it
into objects. Wait for one tick, or ask for one immediately by posting at the
hive path with the lane on `hop.route`:

```bash
curl -s -X POST http://127.0.0.1:7777/messages \
     -H 'Content-Type: application/json' \
     -d '{"target": "/ops/canvy2", "hop": {"route": "in_refresh"},
          "body": {"messages": []}}'
```

Open `http://127.0.0.1:7811/`. You should see the whole colony, laid out by the
flow layout — every box in a computed spot, none of them yours yet.

**This step is not optional and it is not cosmetic.** A migration patch is an
`object.update`, and an update names an object that has to exist: replaying the
bundle into a display that has never drawn refuses every leg with
`unknown_object` and changes nothing.

## 3. Export the saved positions

```bash
python3 scripts/canvy_export_positions.py \
        /path/to/colony/ops/canvy/store/cell.db > canvy-positions.json
```

The script reads exactly one table, read-only, and exactly the three kinds this
document accounts for:

```sql
SELECT kind, id, x, y, z FROM canvas
 WHERE kind IN ('node', 'hive_shift', 'camera')
```

On stdout it writes the patch bundle — one `object.update` per carried position,
as `tool_call` turns of a message body. On stderr it writes the summary: how
many positions were carried, how many `hive_shift` and `camera` rows were
dropped and why, and any row it could not read at all. Keep both; the bundle
carries its own receipt in a `canvy_migration` slot the display ignores.

The output is a pure function of the database, so a re-run of the export is
byte-identical to the file you kept and can be diffed against it.

Exit codes: `0` a bundle was written, `2` the source is not a canvy 1.x store,
`3` the store holds no `node` row and there is nothing to replay. On `2` and `3`
**nothing** reaches stdout, so an empty bundle cannot be applied by accident.

**Sanity-check the ids before you send them.** A carried id is `n/` plus the
cell's path with its slashes stripped — `n/talky/brain` for `/talky/brain`. If
the colony has been rewired since those positions were made, the ids that no
longer name a cell are exactly the ones step 4 will refuse.

## 4. Replay the bundle into the new display

The bundle **is** a message body. Post it at the new display cell:

```bash
curl -s -X POST http://127.0.0.1:7777/messages \
     -H 'Content-Type: application/json' \
     -d "$(jq -c '{target: "/ops/canvy2/web", body: .}' canvy-positions.json)"
```

Two or more `tool_call` turns in one body are answered as **one bundle**: each
leg is applied in call order, a failed leg does not roll back its siblings, and
the reply's header carries `bundle_errors` — stamped unconditionally, so a `0`
means *checked and clean* rather than *nobody looked*.

**Where that reply goes.** A message posted from the HTTP ingress names no
reply address, so the display's answer matches no out-edge and dead-letters as
`no_route`. That is where you read the outcome:

```bash
curl -s 'http://127.0.0.1:7777/colony/dead_letters?limit=5' | jq '.'
```

Look for `bundle_errors`. Then reload the page: your boxes are where you put
them.

**On the next tick they stay there.** The layout cell reads back what the
display already holds before it writes, and leaves those coordinates alone —
only a cell the display has never seen gets a computed spot. That is the same
mechanism that makes a drag stick, and a migrated position is indistinguishable
from a dragged one from here on.

### If a leg is refused

| `error_code` | What it means | What to do |
|---|---|---|
| `unknown_object` | the id names no object — that cell is gone, or was renamed, or step 2 was skipped | if step 2 ran, this row is history: drop it, or re-point it at the cell's new path and re-send just that one |
| `invalid_input` | a prop the component does not declare | the export and the layout have drifted; do not hand-edit the bundle, report it |

A refused leg wrote nothing. Re-sending a corrected bundle is safe: an
`object.update` merges per key and is idempotent for the same values.

## 5. Retire the old hive — by disconnect

**A hive cannot be addressed by `remove_nodes`.** The match is resolved against
the cell registry and a hive has no row there, so a match on the hive path is
`match_no_hit` and, because validation is all or nothing, the whole mutation
fails on it. Two operations in one diff, then: `remove_nodes` for the cells the
old hive holds, `remove_edges` for the pairs whose end is the hive itself. That
shape, with the full reasoning and the edge arithmetic, is `docs/rewiring.md`
§ *Disconnect the old hive*.

Read the real edges first — every pattern must hit at least one edge in the
prior state, or the mutation is refused as `match_no_hit`:

```bash
curl -s http://127.0.0.1:7777/colony/graph | jq '.edges[] | select(.from | test("canvy"))'
```

### Pre-flight: does canvy own the only edge that keeps your tree awake?

**Do this before the mutation below, not after.** canvy 1.x ships
`<hive>/probe -> /colony/graph`. In a tree where the canvas is the only thing
that talks to `/colony`, that edge is the **only boundary-crossing edge of the
whole subtree** — and this step removes it as a side effect of removing
`probe`.

What follows is not a canvy behaviour, it is the substrate's, working exactly as
`docs/meclaw-overview.md` § *Connectivity and activity* describes it: a subtree
that loses its last boundary-crossing edge flips to `active = false` and every
task below it ends. The LLM cells, the connectors, the timers, all of them. **The
mutation commits cleanly and reports nothing unusual**, and `/health` keeps
answering `status: ok` — a colony with no running cells is not an unhealthy
process. This has been measured on a real instance: 47 active cells to **0**, on
a mutation whose stated purpose was to retire a canvas
([#403](https://github.com/mmeyerlein/meclaw/issues/403)).

Count what would be left. Substitute your own member path for `/main`:

```bash
curl -s http://127.0.0.1:7777/colony/graph \
  | jq '[.edges[] | select((.from | startswith("/main")) != (.to | startswith("/main")))
                  | select((.from + .to) | test("canvy") | not)] | length'
```

**Zero means stop.** Draw an anchor edge first, in its own mutation, and only
then run the retirement. It is a connectivity-only edge: it carries no traffic,
because no cell ever emits the route it is conditioned on. It exists so the
subtree has a boundary-crossing edge that does not belong to canvy.

```json
{
  "scope": "/",
  "diff": {"add_edges": [
    {"from": "/main", "to": "/colony/graph",
     "condition": "has(hop.route) && hop.route == '__never__'"}
  ]}
}
```

Note what that edge starts from: the **hive path**, not a cell inside it. That
is not a stylistic choice — see step 6 — and it is the shape the substrate's own
refusal message points at.

Then, with the old hive at `/ops/canvy` and its cells named by their full paths:

```json
{
  "scope": "/ops",
  "diff": {
    "remove_nodes": [
      {"match": {"name": "./canvy/refresh"}},
      {"match": {"name": "./canvy/probe"}},
      {"match": {"name": "./canvy/render"}},
      {"match": {"name": "./canvy/store"}}
    ],
    "remove_edges": [
      {"match": {"from": ".", "to": "./canvy"}},
      {"match": {"from": "./canvy", "to": "."}}
    ]
  }
}
```

Those four names are the ones canvy 1.x shipped — `refresh`, `probe`, `render`
and `store`, verified against the last 1.x `config.json` before the re-cut.
Take them from `/colony/graph` anyway rather than from this list: a 1.x instance
that was hand-edited may hold different ones, and a name that is not there is a
`match_no_hit` that fails the whole diff.

`remove_nodes` disconnects the cells and stops their tasks, which is what
silences the old timer. The `remove_edges` entries take the lanes the
instantiating mutation drew between the parent scope and the hive; a pattern
without a `condition` takes every edge between the named pair, so one entry per
direction is enough however many lanes there are.

**Disconnected, not deleted.** The directory, the `cell_id` and the `cell.db` of
every old cell stay exactly where they are, and the old hive stays as an empty
scope marker: edgeless, without occupants, without traffic. That is the
No-Delete policy, and it is what makes step 6 possible. If you want the
directories gone as well, that is a stopped-colony act with its own checklist —
`docs/rewiring.md` § *Removing a cell for real*.

## 6. The way back

Nothing above is one-way while the old tree is still on disk — but the
retirement is the one step whose undo has real limits, and both of them were
found the hard way ([#403](https://github.com/mmeyerlein/meclaw/issues/403)).

- **Undo the retirement**: `add_edges` with the same pairs, and the connectivity
  pass makes the old cells active again — **with one exception, and it is the
  edge that matters most.**

  **Correction.** This used to promise the undo without qualification. That is
  retracted: `<hive>/probe -> /colony/graph` **cannot be re-drawn**. It predates
  hive sealing, so it exists in a running 1.x graph but the current substrate
  refuses to create it:

  ```
  error_code: hive_port_boundary
  add_edges[].from='./<member>/canvy/probe' resolves to an interior node of the
  sealed hive '.../canvy', while the edge's other endpoint '/colony/graph' lies
  outside it — that wires past the port. Declared ports of '.../canvy': none.
  ```

  So the one edge whose removal can deactivate your tree is the one the way back
  does not cover. Use the hive-path anchor from the pre-flight in step 5 instead
  — that is what restores activity, and on the measured instance it brought the
  count back to its pre-mutation value immediately. The pre-flight is what keeps
  you out of this situation; this paragraph is what gets you out of it.

- **The retirement does not survive a reconnect within the same colony run.**
  Once the old cells have been reconnected, a second attempt at step 5 is
  refused outright:

  ```
  error_code: stop_wiring_unavailable
  disconnect of Awake cell .../canvy/probe without live stop-wiring (interim guard)
  ```

  That is the guard behaving as designed — it is pinned by
  `crates/meclaw-colony/tests/phase_13_5_lifecycle_3b_reconnect.rs` — not a
  defect to work around. It does mean step 5 is **one-shot** against a running
  colony: go back, and going forward again needs a restart first.
- **Undo the migration**: the old `cell.db` still holds the `canvas` table, so
  the export can be re-run at any time. The new display's positions are just
  props; re-sending an older bundle overwrites them.
- **Undo the instantiation**: `remove_nodes` on the new hive's cells, the same
  shape as step 5. The port is released when the display's task stops.

---

## What this migration deliberately does not do

- **It does not guess.** The export carries every `node` row it finds, including
  rows for cells the colony no longer has. It cannot tell a deleted cell from a
  renamed one — 1.x could not either, which is why sweeping stale rows was an
  operator's button there ([#184](https://github.com/mmeyerlein/meclaw/issues/184))
  — so it hands you the refusals in step 4 instead of quietly dropping rows.
- **It does not move the camera.** Where a person is looking is theirs, and in
  2.0.0 it never leaves their browser. A fresh load starts from the picture's
  own `viewBox`, which frames the whole drawing.
- **It does not carry the frames.** See the table at the top: they are derived
  now, and derived from data this migration does carry.
- **It does not run itself.** R-W8-9: every step here is an operator's act
  against their own tree.

## What is pinned

`crates/meclaw-cells/tests/canvy2_position_migration.rs` builds a **synthetic**
1.x `canvas` table, runs the shipped `scripts/canvy_export_positions.py` over
it, and puts the emitted bundle into a real `web` cell that the shipped layout
has bootstrapped — then reads the migrated coordinates back out of that cell's
own database and out of the page a browser would be served. It also pins the
half that would otherwise rot silently: that the props the export writes are the
props `templates/canvy/layout/layout.py` declares `editable` on `canvy-node`,
and that the ids it writes are ids the layout creates.
