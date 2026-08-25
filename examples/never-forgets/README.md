# examples/never-forgets

Tell your agent something in passing in January. Ask it in March. It knows --
and it knows *when*.

That is the whole example. Not "it has a vector store": a vector store answers
*what is similar*, and the question a person actually asks is *what did I say
about the radiator in February*. Those are different questions, and only one of
them has a date in it.

If you intend to actually run this against a model, follow
[`WALKTHROUGH.md`](WALKTHROUGH.md) instead of this file -- it is the same example
end to end, with every command in the order it has to happen and the real output
next to it.

## The three moments

```
JANUARY   "I switched the kitchen radiator off for the winter,
           the valve is stuck at three."

FEBRUARY  "The plumber came by, he says the radiator valve is a
           Danfoss RA-N and needs a new insert."

MARCH     you: "What did the plumber say about the radiator in February?"
       agent: "A Danfoss RA-N, and it needs a new insert."   <- 2026-02-14
```

The March answer is not a lucky keyword hit. Both January and February talk
about the radiator valve, and *January is the wrong one*. What separates them is
the window the model asked for, and the model asked for it because it had read
the question.

## What is checked in

```
never-forgets/
├── seed/                          the --root of the colony
│   ├── colony.json                substrate defaults. two lines.
│   ├── main/config.json           type: "hive", and its graph is EMPTY
│   ├── main/replay/config.json    the import lane: one past turn -> the episode port
│   └── main/memory/              a one-table memory
│       ├── config.json            the hive marker + its two internal edges
│       ├── keep/config.json       the port: write a turn, answer a window
│       └── episodes/config.json   the table: one row per turn, two timestamps
├── past.jsonl                     nine turns across three months
├── grow.json                      the declaration. three nodes, seven edges.
└── README.md
```

Nothing that *talks* is in there. No door, no session keeper, no context
collector, no tool dispatcher, no brain, no summarizer -- thirteen cells' worth
of agent, none of it written here, all of it named in `grow.json` and
instantiated at runtime out of [`templates/`](../../templates/).

What *is* written here is the one thing a template library cannot ship for you:
**where your memory lives.** A library can hand you the shape of an agent; it
cannot decide which database your life goes into.

## What grows

| node | from template | what it brings |
|---|---|---|
| `/surface` | [`door@1.0.1`](../../templates/door/) | 1 cell. `POST /messages` becomes a turn on the ingress lane. |
| `/talky` | [`talky`](../../templates/talky/) | 12 cells. Session keeper, context collector, tool dispatcher, sidecar splitter, summarizer and an `llm` brain, sixteen internal edges pre-wired. |
| `/sink` | [`terminal@1.0.1`](../../templates/terminal/) | 1 cell. The stop for the answer lane, which this example does not decide. The `error` lane is deliberately unwired (GH #284): a refusal that ends in a swallowing cell is one nobody reads, so it dead-letters instead. |

Seventeen cells in the registry, three of them checked in -- the hive marker is a
scope, not a cell. That number is not counted by hand: it is
`CELLS_AFTER_GROW` in `crates/meclaw-cells/tests/never_forgets_example.rs`,
which boots this seed, applies this `grow.json` and asserts the registry it
gets. Adding a cell to any of the three templates moves it there first.

## The one shape worth reading: one port, two producers

```
  live turn ──> /talky ────────────────────turn_write────┐
                                                         ├──> /memory/keep ──> /memory/episodes
  past turn ──> /replay ───────────────────────episode───┘      (the port)         (the table)
```

The live lane knows what was *just* said. The import lane knows what was said in
January. They arrive at the **same port**, in the same shape, and the memory
behind it cannot tell them apart -- which is exactly the property that makes a
port a port rather than a function call.

**Retraction (GH #298, ruling Q11).** Until this version a `memory-drain` hive
sat in the middle of the live half of that picture: the talky handed its day out
as a batch, the drain cut it into single turns and kept a ledger so a second
delivery wrote nothing. That is gone, and it is gone rather than moved. The
collector now emits **one message per turn** on `turn_write`, with `hop.turn_id`,
`hop.turn_index` and `hop.happened_at` beside it -- which is exactly what the
episode port reads -- so there is nothing left in between to decompose. The
import lane never used the drain in the first place: `seed/main/replay` has
always spoken the episode port directly, one message per turn, and says so in its
own comment. Now the live lane does the same thing.

The difference between them is one field. A live turn carries no event time and
the memory stamps its own clock; an imported turn carries the instant it
happened and the memory keeps it. Both rows get a `recorded_at` either way:

| column | means | who sets it |
|---|---|---|
| `happened_at` | when it was said | the caller, if it knows; otherwise the memory's clock |
| `recorded_at` | when this colony learned it | always the memory's clock |

Keeping those two apart is the entire trick. A colony that only stores
`recorded_at` flattens a year of imported history into the minute you imported
it, and then no window over it can ever be right again.

**The event time rides in the request headers, never on the turn.** A turn in
the universal body format carries `origin`, `type`, `text` and `id` and nothing
else -- a timestamp glued onto it is refused at the delivery boundary before any
cell sees it. Headers are where per-message facts about a message belong.

## The other shape: the model asks for the window itself

The per-turn memory leg every agent can have is fired the moment a turn arrives,
before the model has read a word of it. It is a good floor and it can never
cover a question about a *time range*, because nobody who had seen the question
was the one asking.

`memory_recall` closes that. Two edges in `grow.json`, and neither is new
machinery:

```jsonc
// the dispatcher's memory lane -- the same shape as any tool edge
{"from": "./talky", "to": "./talky",
 "condition": "has(hop.route) && hop.route == 'tool' && has(hop.tool_name) && hop.tool_name == 'memory_recall'",
 "modifier": {"set_hop": {"route": "'in_memory_call'"}}},

// the recall port, carrying the window the model named
{"from": "./talky", "to": "./memory/keep",
 "condition": "has(hop.route) && hop.route == 'recall'",
 "modifier": {"set_hop": {"route": "'in_recall'"},
              "set_context": {"recall_query": "hop.recall_query",
                              "recall_window_from": "hop.recall_window_from",
                              "recall_window_to": "hop.recall_window_to",
                              "memory_call_id": "hop.memory_call_id", ...}}}
```

From the dispatcher's side this is a tool like any other: it names the tool, an
edge knows the cell. From the collector's side it is the one tool it serves
itself, because it already owns the recall port. So the round ends where it
began and memory never learns a word of dispatcher vocabulary.

The tool schema is a **seed**, not a contract of the topology -- what the model
may ask for is decided where the brain's `system.tools` is written, in
`templates/talky/brain/seed/system.jsonl`. **Wiring the lane is not enough**: a
wired edge routes a call that the model never makes, because a tool it cannot
see is a tool it will not ask for. The composite ships no tools deliberately --
identity, instructions and tools are the agent, not the graph.

The leaf `system.tools.memory_recall.text` holds the **provider-native tool
object as a JSON string** -- the full envelope, not just the inner schema:

```json
{
  "type": "function",
  "function": {
    "name": "memory_recall",
    "description": "Ask long-term memory about something, optionally restricted to a time range.",
    "parameters": {
      "type": "object",
      "properties": {
        "query":       {"type": "string", "description": "what to look for"},
        "window_from": {"type": "string", "description": "ISO-8601 start of the range (optional)"},
        "window_to":   {"type": "string", "description": "ISO-8601 end of the range (optional)"}
      },
      "required": ["query"]
    }
  }
}
```

The adapter parses that string at call time and hands it to the provider
verbatim. Seed only the inner half and the provider rejects it; seed nothing at
all and the model answers the March question out of thin air while every edge in
the graph still fires correctly. [`WALKTHROUGH.md`](WALKTHROUGH.md) Step 2 is
the executable version of this paragraph.

Inside `/memory/keep` the window is not machinery either. ISO-8601 UTC sorts
lexicographically, so a range is a string comparison over a column -- no index,
no model, no ranking. The query words only *narrow* what the window already
holds, and an empty window says so out loud instead of handing back the closest
thing it found.

## Run it

```bash
# from the repo root, on a fresh release build
cargo build --workspace --release

cat > examples/never-forgets/seed/.env <<'EOF'
OPENROUTER_API_KEY=sk-...
MODEL_BRAIN=openai/gpt-4o-mini
EOF

# A library you may write into. NOT for the collector's knobs -- those are params
# and grow.json sets them (see below). This copy exists for the next step: a seed
# is a FILE in the template's seed/ directory, and no override_params reaches a
# file. Pointing --templates here leaves the shipped library untouched.
cp -r templates examples/never-forgets/templates

# Give the brain the tool. WITHOUT THIS STEP THE EXAMPLE DOES NOT WORK: the lane
# is wired, but the model never sees a memory_recall to call, so it answers from
# its own recollection and the whole point is lost.
mkdir -p examples/never-forgets/templates/talky/brain/seed
python3 - <<'EOF'
import json
tool = {
  "type": "function",
  "function": {
    "name": "memory_recall",
    "description": "Ask long-term memory about something, optionally restricted to a time range.",
    "parameters": {
      "type": "object",
      "properties": {
        "query":       {"type": "string", "description": "what to look for"},
        "window_from": {"type": "string", "description": "ISO-8601 start of the range (optional)"},
        "window_to":   {"type": "string", "description": "ISO-8601 end of the range (optional)"}
      },
      "required": ["query"]
    }
  }
}
instructions = ("Today is 2026-08-15. You have a long-term memory you cannot see. "
                "When the user asks about something said in the past, call memory_recall "
                "FIRST -- never answer from your own recollection. If the question names a "
                "month or a period, pass it as window_from/window_to in ISO-8601 UTC. "
                "Answer only from what memory returns; if it returns nothing for that "
                "window, say so plainly.")

p = "examples/never-forgets/templates/talky/brain/seed/system.jsonl"
with open(p, "w") as f:
    f.write(json.dumps({"schema": {"slot_path": "text", "value": "json", "updated_at": "int"}}) + "\n")
    f.write(json.dumps({"slot_path": "instructions.memory",
                        "value": {"text": instructions}, "updated_at": 0}) + "\n")
    f.write(json.dumps({"slot_path": "tools.memory_recall",
                        "value": {"text": json.dumps(tool)}, "updated_at": 0}) + "\n")
EOF

./target/release/meclaw --root ./examples/never-forgets/seed \
                        --templates ./examples/never-forgets/templates \
                        --daemon --api 127.0.0.1:7788
```

The collector's knobs are params, not environment, and `grow.json` sets the one
this example needs on the `talky` node:

```jsonc
{"name": "talky", "template": "talky",
 "override_params": {"collector/assemble": {"turn_write": "1"}}}
```

Since [GH #140](https://github.com/mmeyerlein/meclaw/issues/140) an
`override_params` on a subtree template is **addressed by the cell's path inside
it**, so the knob is set where the declaration is read. The path is
`collector/assemble` and not `collector`: the latter is the sub-unit's hive,
which reads `graph`, `ports`, `required_drains` and `contract` and would take
this key without anyone consuming it.

`turn_write` is the per-turn lane: every stored turn and every
answer hands what was said out immediately, one message per turn, so the memory
is fresh *during* the session instead of at the nightly close. Without it the
first row appears when the session closes -- a freshness hole of up to a day, and
a question about the last exchange gets answered out of an empty store. **Since
[GH #298](https://github.com/mmeyerlein/meclaw/issues/298) the library ships that
lane ON**, so the override above no longer *turns it on* -- it says out loud, in
the declaration a reader reads, which knob this example depends on. Setting it to
`"0"` is what would switch the lane off, and this example would lose its point.
`memory_call_tier` needs no setting at all -- the shipped default is `"1"`.

Grow it:

```bash
curl -s -X POST http://127.0.0.1:7788/colony/mutations \
     -H 'Content-Type: application/json' \
     -d @examples/never-forgets/grow.json
```

`http://127.0.0.1:7788/ui/registry` now shows seventeen cells.

## Load the past

`past.jsonl` is nine turns across three months. One line, one turn, one `curl`:

```bash
while read -r line; do
  curl -s -X POST http://127.0.0.1:7788/messages \
       -H 'Content-Type: application/json' \
       -d "$(jq -c '{target: "/replay",
                     headers: {session_id: .session, happened_at: .at},
                     body: {messages: [{origin: .origin, type: "text", text: .text}]}}' \
             <<<"$line")"
done < examples/never-forgets/past.jsonl
```

That is the whole month-ingest: no model call, no embedding, no queue. Nine
`code` and `store` hops per turn and the colony has a past.

Check it landed under the right instants:

```bash
sqlite3 examples/never-forgets/seed/main/memory/episodes/cell.db \
  'SELECT happened_at, sender, substr(content,1,48) FROM episodes ORDER BY happened_at'
```

## Ask it

```bash
curl -s -X POST http://127.0.0.1:7788/messages \
     -H 'Content-Type: application/json' \
     -d '{"target": "/surface", "headers": {"channel": "chat-1"},
          "body": {"messages": [{"origin": "user", "type": "text",
                                 "text": "What did the plumber say about the radiator in February?"}]}}'
```

Then read the hop chain:

```bash
TID=$(curl -s 'http://127.0.0.1:7788/colony/trace?limit=1' | jq -r '.trace[0].trace_id')
curl "http://127.0.0.1:7788/ui/trace?trace_id=$TID"
```

The spine of it, with the window and session bookkeeping and the concurrent
turn-write leg into `/memory/keep` left out -- the trace has those too:

```
@external -> /surface -> /talky/session-keeper -> /talky/session-keeper/stamp ->
/talky/session-keeper -> /talky/collector -> /talky/collector/assemble ->
/talky/collector -> /talky/brain -> /talky/splitter -> /talky/dispatcher ->
/talky/collector -> /talky/collector/assemble -> /talky/collector ->
/memory/keep -> /memory/episodes -> /memory/keep -> /talky/collector ->
/talky/collector/assemble -> /talky/collector -> /talky/brain ->
/talky/splitter -> /talky/dispatcher -> /talky/collector ->
/talky/collector/assemble -> /talky/collector -> /sink
```

Two inferences, one round trip through memory in between, and the second
inference is the one that ran with February in its context window.

**A hive transit is a hop, and it is in the trace twice.** `/talky/collector` is
a hive, not a cell -- it has no mailbox and nothing runs in it. It still gets a
message-log row when a message arrives at it, and a second one as the sender of
what it forwards, which is why the chain reads `... -> /talky/collector ->
/talky/collector/assemble -> /talky/collector -> ...` around every pass through
the collector. Counting a hive out because "a trace names the cells that handled
the turn" is the single easiest way to lose an hour against your own trace.

A hive shows up when something **addresses** it, not because it exists. `/memory`
is a hive as well, and it never appears above: every edge here points straight at
`/memory/keep`, so nothing is ever routed to `/memory` itself.

Ask the same question about **April** and the memory says nothing was said in
that window -- rather than handing back February because it was the nearest
thing. That refusal is the reason the window is worth having.

## What this demonstrates, honestly

- **Time is a column, not a feature.** Two timestamps, kept apart, and a window
  is a string comparison over one of them.
- **The consumer asks.** The model that read the question is the one that names
  the range. Nothing upstream had to guess it, and nothing downstream had to
  interpret it.
- **A port takes any producer.** A live turn and a historical import write the
  same shape to the same address, and the memory is not aware there are two.
- **Freshness is a lane, not a promise.** `turn_write` is the whole
  difference between "retrievable now" and "retrievable tomorrow".

Two honest limits, both deliberate:

- **This memory is not the memory hive.** What is behind the port here is one
  table and one query -- it stores what was said and answers questions about
  *when*. Extraction, a fact model, ranking, consolidation and everything that
  reads *meaning* out of a turn are a much larger thing behind the same port.
  This example exists to show the port and the shape around it working end to
  end, not to be the retrieval engine.
- **The recall is not scored.** Window first, query words second, twenty rows,
  and no ranking whatsoever. That is honest for nine turns and would not be for
  nine million.

The topology above does not change when what sits behind the port does. That is
the point of writing it down as edges.

## Pinned

`crates/meclaw-cells/tests/never_forgets_example.rs` boots **this** seed and
applies **this** declaration -- the files, not copies of them -- against a mock
provider, replays **this** `past.jsonl` and drives the March question through
the whole tree. It measures what is checked in (six files, no edge in the root
hive, both timestamp columns present), that `grow.json` names only templates
that ship, that it sets `turn_write` on `collector/assemble` at instantiation,
that the recall edge carries the window keys -- and then the claim
itself: the second inference's prompt contains the February sentence **with its
instant**, and contains neither the January nor the March one. The counter-test
asks about April and requires the empty answer. If the example rots, those tests
go red first.
