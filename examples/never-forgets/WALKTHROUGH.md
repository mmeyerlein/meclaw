# never-forgets, step by step

Every command below was run against this seed on a release build, against a
**real provider**, and every output block is what came back — shortened, never
edited. The whole walkthrough costs about **USD 0.00025** in provider tokens
(four calls, measured with `scripts/cost_report.py`; see step 8).

The claim under test: a sentence said in **February** is still there in
**August**, and the agent can be asked for it *by date* — not by similarity.

---

## Step 0 — what you need

A release build, an OpenRouter key (or any OpenAI-compatible endpoint), `jq`,
and a model that can call tools. The run below used
`openai/gpt-5.6-luna`.

```bash
cargo build --workspace --release
```

## Step 1 — the example's own copy of the template library

```bash
cp -r templates examples/never-forgets/templates
```

That is the whole step, and it is worth knowing what it is *not* for. The
collector's per-turn write is a **param**, and a param is named in the
declaration: `grow.json` carries `"override_params": {"collector/assemble":
{"turn_write": "1"}}` on the `talky` node, so nothing about this example needs an
edited library. Since
[#298](https://github.com/mmeyerlein/meclaw/issues/298) that value is also the
shipped default, so the line names the knob this example lives on rather than
switching it on. Since [#140](https://github.com/mmeyerlein/meclaw/issues/140) an
`override_params` on a subtree template is addressed by the cell's path inside it
(`collector/assemble` — the cell, not `collector`, which is the sub-unit's hive
and would swallow the key).

The copy exists for **Step 2**. A seed is a *file* in a template's `seed/`
directory, read once when the cell is first spawned, and there is no
`override_params` for a file — so giving the brain its `memory_recall` schema
means writing into a template library you are allowed to write into. Point
`--templates` at the copy and the shipped library stays untouched.

`turn_write` is the freshness lane: every stored turn and every answer hands out
what was said immediately, one message per turn. Without it the first row appears
when the session closes — a hole of up to a day, and a question about the last
exchange gets answered out of an empty store.

## Step 2 — give the brain the tool

**This step is easy to miss and the example does not work without it.** The
topology in `grow.json` wires the `memory_recall` lane, but a wired lane is not a
tool the model can see. Tool *schemas* are not topology — they live in the
brain's `system.tools`, seeded next to the cell. The composite ships none,
deliberately: identity, instructions and tools are the agent, not the graph.

```bash
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
```

Note the shape of the tool leaf: `system.tools.<name>.text` holds the
**provider-native tool object as a JSON string** — the full
`{"type":"function","function":{…}}` envelope, not just the inner schema. The
adapter parses that string at call time and hands it to the provider verbatim.

Without this file the model has no `memory_recall` in its menu, never asks
memory anything, and answers the March question out of thin air. That is not a
failure of the topology — every edge still fires correctly — and it is exactly
why it is worth seeing once.

## Step 3 — the key and the model

```bash
cat > examples/never-forgets/seed/.env <<'EOF'
OPENROUTER_API_KEY=sk-or-...
MODEL_BRAIN=openai/gpt-5.6-luna
EOF
```

## Step 4 — boot, and grow

```bash
./target/release/meclaw --root ./examples/never-forgets/seed \
                        --templates ./examples/never-forgets/templates \
                        --daemon --api 127.0.0.1:7788 &
```

```bash
curl -s http://127.0.0.1:7788/colony/registry
```

```
/memory/episodes   store
/memory/keep       code
/replay            code
```

Three cells. Nothing that talks. Now the declaration:

```bash
curl -s -X POST http://127.0.0.1:7788/colony/mutations \
     -H 'Content-Type: application/json' \
     -d @examples/never-forgets/grow.json
```

```json
{"mutation":{"id":"01a00650-e550-7902-92b8-4f4db5fbc5ad","outcome":"committed"}}
```

```
16 cells:
  /memory/episodes           store     /talky/errors            code
  /memory/keep               code      /talky/session-keeper/close      code
  /replay                    code      /talky/session-keeper/night      timer
  /sink                      code      /talky/session-keeper/sessions   store
  /surface                   code      /talky/session-keeper/stamp      code
  /talky/brain               llm       /talky/dispatcher             code
  /talky/collector/assemble  code      /talky/summarizer/prep   code
  /talky/collector/window    store     /talky/summarizer/writer llm
```

Three checked in, thirteen instantiated from templates by one POST. Verify the
seed landed while you are here:

```bash
python3 -c "
import sqlite3
c = sqlite3.connect('examples/never-forgets/seed/main/talky/brain/cell.db')
for r in c.execute('SELECT slot_path, substr(value,1,60) FROM system'): print(r)"
```

```
('instructions.memory', '{"text":"Today is 2026-08-15. You have a long-term memo')
('tools.memory_recall', '{"text":"{\\"type\\": \\"function\\", \\"function\\": {\\"name\\"')
```

---

## Step 5 — load the past

`past.jsonl` is nine turns across three months. One line, one turn, one `curl`:

```bash
while read -r line; do
  curl -s -X POST http://127.0.0.1:7788/messages \
       -H 'Content-Type: application/json' \
       -d "$(jq -c '{target: "/replay",
                     headers: {session_id: .session, happened_at: .at},
                     body: {messages: [{origin: .origin, type: "text", text: .text}]}}' \
             <<<"$line")"
  echo
done < examples/never-forgets/past.jsonl
```

```json
{"message_id":"01a0064e-37d3-7d72-9d84-c4cc84b2e92a"}
{"message_id":"01a0064e-37db-7c51-8680-511010767095"}
{"message_id":"01a0064e-37e1-7d83-93fb-c40019186369"}
… nine of them, one per line, in about a second
```

### What the organism just did — the import loop

Nine POSTs, no model call, no embedding, no queue, no batch job. Each line took
the same three-hop path: `@external → /replay → /memory/keep → /memory/episodes`.
That is the whole month-ingest, and three things about it are worth naming.

**It does not go through the conversation surface.** The obvious way to teach an
agent about January is to say it to the agent. That is wrong here, and the
reason is a clock: a turn re-spoken through `/surface` gets stamped with today.
`/replay` speaks the memory's episode port *directly* instead.

**It arrives at the same port the live turn uses.** After every real turn, the
talky puts one episode per turn on `/memory/keep` on route `in_episode` — which
cell inside the composite emitted it is the hive's business, not this lane's. The
import lane puts an episode on `/memory/keep` on route `in_episode`. Same
address, same shape, same lane name — and the memory behind it cannot tell them
apart. Neither is a special case of the other. That indistinguishability is what
makes a port a port rather than a function call with two callers.

Until GH #298 (ruling Q11) the live half of that sentence had a `memory-drain`
hive in it, which took the talky's batch and cut it into single turns. It is
gone from this walkthrough, not moved: the collector emits one message per turn
on `turn_write` itself, so the two producers are now literally the same shape and
there is nothing left in between.

**The event time rides in the headers, never on the turn.** Look at the `curl`
again: `happened_at` sits in `headers`, next to `session_id`, while the turn
itself carries only `origin`, `type` and `text`. A universal-body-format turn
has no timestamp field, and gluing one on gets the message refused at the
delivery boundary before any cell sees it. Per-message facts about a message
belong in the header; the body is the message.

Check where it landed:

```bash
python3 -c "
import sqlite3
c = sqlite3.connect('examples/never-forgets/seed/main/memory/episodes/cell.db')
for r in c.execute('SELECT happened_at, sender, substr(content,1,48) FROM episodes ORDER BY happened_at'):
    print(r[0], '|', r[1], '|', r[2])
print(list(c.execute('SELECT count(*), min(recorded_at), max(recorded_at) FROM episodes')))"
```

```
2026-01-09T18:40:00.000Z | user      | I switched the kitchen radiator off for the wint
2026-01-09T18:40:22.000Z | assistant | Noted. Stuck at three usually means the pin unde
2026-01-23T09:02:00.000Z | user      | Booked the dentist for the first week of March,
2026-02-14T11:05:00.000Z | user      | The plumber came by, he says the radiator valve
2026-02-14T11:06:10.000Z | assistant | So the fix is an insert, not a whole valve body.
2026-02-21T20:15:00.000Z | user      | Watched the second half of the game at Tom's pla
2026-03-02T08:12:00.000Z | user      | Bought a second bike lock, the old one is at the
2026-03-02T08:12:40.000Z | assistant | Two locks, two places. That is the setup that ac
2026-03-11T17:30:00.000Z | user      | Repotted the big monstera, it had grown straight

[(9, '2026-08-15T16:43:04.576784Z', '2026-08-15T16:43:04.709130Z')]
```

**That last line is the entire point of the two columns.** `happened_at` spans
January to March. `recorded_at` spans 132 milliseconds on the afternoon of the
import. A colony that stored only one of them would have flattened three months
into a third of a second, and no window over it could ever be right again.

---

## Step 6 — ask it

```bash
curl -s -X POST http://127.0.0.1:7788/messages \
     -H 'Content-Type: application/json' \
     -d '{"target": "/surface", "headers": {"channel": "chat-1"},
          "body": {"messages": [{"origin": "user", "type": "text",
                                 "text": "What did the plumber say about the radiator in February?"}]}}'
```

The answer arrives at `/sink` about fifteen seconds later:

```
The plumber said the radiator valve is a **Danfoss RA-N** and needs a
**new insert**.
```

## Step 7 — read the trace, because the answer is not the interesting part

```bash
TID=01a00651-25d8-7873-935e-ed827c13d96a
curl -s "http://127.0.0.1:7788/colony/trace?trace_id=$TID"
```

Dozens of hops. The recorded run had seventy-one, and that number is not
re-copied here: GH #298 took the drain's park/probe/mark round trip out of the
live path and a hop count is the least interesting thing in the listing. The
spine of them, with the header keys that decide each turn:

```
@external                   -> /surface                     {}
/surface                    -> /talky/session-keeper        {route: in_turn}
/talky/session-keeper       -> /talky/session-keeper/stamp  {route: in_turn}
…                                                           (session bookkeeping)
/talky/session-keeper/stamp -> /talky/session-keeper        {route: turn}
/talky/session-keeper       -> /talky/collector             {route: in_turn}
/talky/collector            -> /talky/collector/assemble    {route: in_turn}
…                                                           (window bookkeeping)
/talky/collector/assemble   -> /talky/collector             {route: turn_write, iter: 0,
                                                             turn_id: <session>#0,
                                                             turn_index: 0}
/talky/collector            -> /memory/keep                 {route: in_episode}
/memory/keep                -> /memory/episodes             {route: mstore}

/talky/collector/assemble   -> /talky/collector             {route: brain, iter: 0}
/talky/collector            -> /talky/brain                 {route: brain, iter: 0}  <- inference 1
/talky/brain                -> /talky/dispatcher            {finish_reason: tool_calls}
/talky/dispatcher           -> /talky/collector             {route: in_memory_call,
                                                             tool_name: memory_recall}
/talky/collector            -> /talky/collector/assemble    {route: in_memory_call}
/talky/collector/assemble   -> /talky/collector             {route: recall,
                                                             recall_query: "What did the plumber
                                                               say about the radiator in February?",
                                                             recall_window_from: 2026-02-01T00:00:00Z,
                                                             recall_window_to:   2026-02-28T23:59:59Z}
/talky/collector            -> /memory/keep                 {route: in_recall, …}
/memory/keep                -> /memory/episodes             {route: mstore}
/memory/episodes            -> /memory/keep                 {route: in_echo}
/memory/keep                -> /talky/collector             {route: in_bundle, iter: 0}
/talky/collector            -> /talky/collector/assemble    {route: in_bundle, iter: 0}

/talky/collector/assemble   -> /talky/collector             {route: brain, iter: 1}
/talky/collector            -> /talky/brain                 {route: brain, iter: 1}  <- inference 2
/talky/brain                -> /talky/dispatcher            {finish_reason: stop}
/talky/dispatcher           -> /talky/collector             {route: in_answer}
/talky/collector            -> /talky/collector/assemble    {route: in_answer}
/talky/collector/assemble   -> /talky/collector             {route: answer, iter: 1}
/talky/collector            -> /sink                        {route: answer, iter: 1}
```

### Read the hive hops before you read anything else

`/talky/session-keeper`, `/talky/collector` and `/talky` itself are **hives**,
not cells. Nothing runs in them -- no mailbox, no task, no `cell.db`. They are
still hops in this listing, because the colony logs the message that *arrives* at
a hive and logs the follow-up it forwards with the hive as the sender. So every
pass through a hive is two rows, hive on both sides of the cell that did the
work:

```
/talky/collector -> /talky/collector/assemble -> /talky/collector
```

That is worth knowing before you compare your own trace against this one. The
inverse also holds: a hive nobody addresses never appears at all. `/memory` is a
hive here too, and it is absent from the whole listing -- every edge points at
`/memory/keep`, so no message is ever routed to `/memory` itself.

And it is where the boundary rule becomes readable rather than theoretical: the
dispatcher's answer arrives at `/talky/collector`, not at `/talky/collector/assemble`.
Which cell behind the hive serves the lane is the hive's business, and the trace
shows the caller never needed to know.

### What the organism just did — the consumer asked for the window

The line to stare at is the tool call:

```json
{"query":"What did the plumber say about the radiator in February?",
 "window_from":"2026-02-01T00:00:00Z",
 "window_to":"2026-02-28T23:59:59Z"}
```

**Nobody upstream computed that range.** No parser looked for month names, no
edge carried a date, no cell guessed. The model read the question, decided that
"in February" meant a range, and named it. That is the whole shift: the
per-turn memory leg every agent can have is fired the moment a turn arrives,
*before* the model has read a word of it — which is a good floor and can never
answer a question about a time range, because nobody who had seen the question
was the one asking.

**From the dispatcher's side it is an ordinary tool.** `/talky/dispatcher` matched on
`hop.tool_name == 'memory_recall'` exactly the way it matches any other tool
name, and an edge knew which cell answers. From the collector's side it is the
one tool it serves itself, because it already owns the recall port — so the
round ends where it began, and memory never learns a word of dispatcher
vocabulary.

**And the round trip is visible as `iter`.** Inference 1 runs at `iter: 0`,
memory answers, inference 2 runs at `iter: 1`. Two provider calls with one
database round trip between them, and only the second one had February in its
context window.

Here is what came back through the port:

```
recall over 2026-02-01T00:00:00Z .. 2026-02-28T23:59:59Z:
- 2026-02-14T11:05:00.000Z user: The plumber came by, he says the radiator valve
  is a Danfoss RA-N and needs a new insert.
```

**One row.** The store held nine, and *two* of them talk about the radiator
valve — January's "the valve is stuck at three" is a perfectly good keyword hit
and it is the wrong answer. What excluded it was not ranking, not similarity and
not a model: `happened_at` is ISO-8601 UTC, ISO-8601 UTC sorts
lexicographically, and the window is therefore a string comparison over a
column. The query words only *narrow* what the window already holds.

---

## Now break it

The bundle above is small and correct, which is also what a lucky keyword search
would look like. So ask for a window where the answer does not exist. Not a
different topic — the **same question, a month that never happened**:

```bash
curl -s -X POST http://127.0.0.1:7788/messages \
     -H 'Content-Type: application/json' \
     -d '{"target": "/surface", "headers": {"channel": "chat-2"},
          "body": {"messages": [{"origin": "user", "type": "text",
                                 "text": "What did the plumber say about the radiator in April?"}]}}'
```

The model asks for April:

```json
{"query":"What did the plumber say about the radiator?",
 "window_from":"2026-04-01T00:00:00Z","window_to":"2026-05-01T00:00:00Z"}
```

The port answers:

```
recall over 2026-04-01T00:00:00Z .. 2026-05-01T00:00:00Z:
- nothing was said in this window
```

And the agent says so:

```
Nothing was said about the radiator by the plumber in April.
```

**This is the step that separates a window from a search.** Every similarity
store on earth would have returned the February sentence here — it is the
closest thing in the corpus, by a wide margin, and "closest thing" is exactly
what such a store is built to return. This one refuses, out loud, in one line,
and the agent repeats the refusal instead of dressing it up. A memory that
cannot say *nothing* is a memory whose *something* means very little.

Now swap the month the other way. `"What did I say about the radiator in
January?"` — same store, same tool, and the window moves:

```json
{"query":"What did the user say about the radiator?",
 "window_from":"2026-01-01T00:00:00Z","window_to":"2026-01-31T23:59:59Z"}
```

```
recall over 2026-01-01T00:00:00Z .. 2026-01-31T23:59:59Z:
- 2026-01-09T18:40:00.000Z user: I switched the kitchen radiator off for the
  winter, the valve is stuck at three.
- 2026-01-09T18:40:22.000Z assistant: Noted. Stuck at three usually means the
  pin underneath is seized, not the knob.
- 2026-01-23T09:02:00.000Z user: Booked the dentist for the first week of March,
  the one on the corner.
```

```
In January, you said: "I switched the kitchen radiator off for the winter;
the valve is stuck at three."
```

February is now the one that is absent. The two radiator sentences are
symmetrical — whichever window you name, you get that one and not the other.

**And here is the limit, in the same output.** The January bundle contains the
dentist, which has nothing to do with radiators. The query narrowing is a
lowercase substring test over words longer than three characters, and the
model's query ended in `radiator?` — with the question mark attached, so it
matched nothing, and by design the window then answers in full rather than
returning an empty set. That is the right failure direction for nine rows and
plainly the wrong one for nine million. The window is the load-bearing part
here; the query words are a courtesy, and this example does not pretend
otherwise.

---

## Step 8 — what it cost

```bash
python3 scripts/cost_report.py --db examples/never-forgets/seed/colony.db \
                               --prices scripts/prices-openrouter-2026-08-15.json
```

```
day         model                    calls    prompt  completion       USD
2026-08-15  openai/gpt-5.6-luna          4      1031         250   0.00025
total  : USD 0.00025 over 0.02 h
```

Four calls: two inferences per question, two questions. The nine-turn import and
both memory round trips cost nothing at all — no model is involved anywhere in
the storage or the recall path.

---

## Clean up

```bash
kill %1
rm -rf examples/never-forgets/seed/{colony.db*,log.jsonl,blobs,.staging,orphan-journal.jsonl,.env} \
       examples/never-forgets/seed/main/{surface,talky,sink} \
       examples/never-forgets/templates
find examples/never-forgets/seed -name 'cell.db*' -delete
```

The three instantiated node directories are the ones `grow.json` created; delete
them together with `colony.db` or the next boot will find cells the registry no
longer knows and refuse the mutation with `resume_requires_stopped_cell`.

## Pinned

`crates/meclaw-cells/tests/never_forgets_example.rs` boots **this** seed and
applies **this** declaration — the files, not copies of them — against a mock
provider, replays **this** `past.jsonl`, and drives the February question through
the whole tree. It checks that the second inference's prompt contains the
February sentence **with its instant** and contains neither the January nor the
March one, and the counter-test asks about April and requires the empty answer.
It runs step 2 as written and then asserts the wire carried `memory_recall` in
its tool list, because a mock returns a canned tool call whatever the brain was
told — the assertion, not the answer, is what says a live run would have worked.
And it pins the `turn_write` override in `grow.json` rather than trusting the
freshness assertion alone: a setup key that reaches nothing is exactly how
[#203](https://github.com/mmeyerlein/meclaw/issues/203) came up silently wrong.
