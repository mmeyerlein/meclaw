# hard-shell, step by step

Every command below was run against this seed on a release build, and every
output block is what came back — shortened, never edited. Nothing here needs a
key, a model or an account. One step needs the open internet; it is marked.

The claim under test: **the shell is the state this colony ships in, not a
feature somebody enabled.** `seed/main/probe/config.json` contains a timeout and
a concurrency cap and nothing else. If the refusals below happen anyway, they
are not configuration.

Total runtime of the whole walkthrough: under two minutes. Total cost: zero.

---

## Step 1 — build and boot

```bash
cargo build --workspace --release

./target/release/meclaw --root ./examples/hard-shell/seed \
                        --templates ./templates \
                        --daemon --api 127.0.0.1:7799 &
```

The seed carries exactly one cell, so that is what the registry has:

```bash
curl -s http://127.0.0.1:7799/colony/registry
```

```json
{"registry":[{"active":true,"cell_id":"01a0064c-07c6-7b02-a5ad-d3a0504d153b",
              "cell_type":"web_fetch","failed":false,
              "lifecycle_status":"Awake","path":"/probe"}]}
```

One cell. No door yet, no terminal, no lane. Those arrive next, and they arrive
as a message.

## Step 2 — grow the colony

```bash
curl -s -X POST http://127.0.0.1:7799/colony/mutations \
     -H 'Content-Type: application/json' \
     -d @examples/hard-shell/grow.json
```

```json
{"mutation":{"id":"01a0064c-2824-70c1-98de-25b9f92fbf76","outcome":"committed"}}
```

```bash
curl -s http://127.0.0.1:7799/colony/registry
```

```
/probe     web_fetch   Awake
/door      code        Awake
/sink      code        Awake
```

`committed` is the whole mutation protocol you need for this example: the
declaration was validated, staged, spawned and wired in one transaction. A
rejection would have named a code instead and changed nothing on disk.

---

## Step 3 — the first attack

`169.254.169.254` is where AWS, GCP and Azure hand out instance credentials over
plain HTTP to anything asking from inside the machine. It is the first address a
prompt-injected agent is told to fetch.

```bash
curl -s -X POST http://127.0.0.1:7799/messages \
     -H 'Content-Type: application/json' \
     -d '{"target": "/door",
          "body": {"messages": [{"origin": "assistant", "type": "tool_call", "id": "c1",
                                 "text": "{\"url\": \"http://169.254.169.254/latest/meta-data/iam/security-credentials/\"}"}]}}'
```

```json
{"message_id":"01a0064c-38c2-7f40-b9d8-6a06cda6e050"}
```

Now read the whole trace, not just the answer:

```bash
TID=01a0064c-38c2-7f40-b9d8-6a06cda6e050
curl -s "http://127.0.0.1:7799/colony/trace?trace_id=$TID"
```

Three hops, trimmed to the headers that matter:

```
@external -> /door      hop {}
/door     -> /probe     hop {"route":"turn","exit_code":0,"duration_ms":19}
/probe    -> /sink      hop {"route":"denied","error_code":"target_blocked",
                             "finish_reason":"error","operation":"web_fetch",
                             "duration_ms":0}
                        body "web_fetch refuses 169.254.169.254:
                              link-local 169.254.0.0/16 (cloud metadata)"
```

### What the organism just did — the deny path

This is the part worth slowing down for, because three separate design decisions
are visible in those three lines and only one of them is the refusal itself.

**The judgement happened before the socket.** `duration_ms` on the deny hop is
`0` and there is **no `http_status` key at all**. A refusal that came back from
the network would carry a status; this one has nothing to report because nothing
was sent. The address was screened, the fetch never started, and no packet left
the machine.

**The refusal became a message, not an exception.** `/probe` did not crash, did
not log-and-swallow, and did not return an empty body. It emitted a normal
message with `finish_reason: "error"` and a typed `error_code`. From the
substrate's point of view a deny is an ordinary emission — which is precisely
why the next thing can happen at all.

**The lane was chosen by the code, not by the prose.** The edge in `grow.json`
reads:

```json
{"from": "./probe", "to": "./sink",
 "condition": "has(hop.error_code) && hop.error_code == 'target_blocked'",
 "modifier": {"set_hop": {"route": "'denied'"}}}
```

It matches `hop.error_code`, and it *stamps* `hop.route = "denied"` on the way
past. So the deny arrives at the terminal on a lane of its own, distinguishable
from `fetched` and from `failed` without anyone parsing the message text. A deny
matched on wording breaks the day somebody rewrites the sentence; a deny with no
lane at all dead-letters and nobody sees it. The code is the contract.

Confirm the third property directly:

```bash
curl -s http://127.0.0.1:7799/colony/dead_letters
```

```json
{"dead_letters":[]}
```

Every lane in this tree has an address, including the ones that fail.

---

## Step 4 — the neighbours, and each one names its own range

```bash
# same POST as above, one per url
http://127.0.0.1:8080/admin
http://10.0.0.5/internal
http://192.168.1.1/
http://[::1]:9200/_cluster/health
```

All four denied, all four `target_blocked`, and the message names the range that
caught it:

```
denied | target_blocked | web_fetch refuses 127.0.0.1: loopback 127.0.0.0/8
denied | target_blocked | web_fetch refuses 10.0.0.5: private RFC 1918 10.0.0.0/8
denied | target_blocked | web_fetch refuses 192.168.1.1: private RFC 1918 192.168.0.0/16
denied | target_blocked | web_fetch refuses ::1: loopback ::1
```

The range in the text is for the human reading the trace. The `error_code` is
for the edge. They are deliberately different channels.

---

## Step 5 — the lane that succeeds

*(the one step that needs the open internet)*

```bash
curl -s -X POST http://127.0.0.1:7799/messages \
     -H 'Content-Type: application/json' \
     -d '{"target": "/door",
          "body": {"messages": [{"origin": "assistant", "type": "tool_call", "id": "ok1",
                                 "text": "{\"url\": \"https://example.com/\"}"}]}}'
```

```
/probe -> /sink  hop {"route":"fetched","http_status":200,"content_type":"text/html",
                      "bytes":559,"duration_ms":118,"operation":"web_fetch"}
                 body "<!doctype html><html lang=\"en\"><head><title>Example Domain</title>…"
```

Same cell, same edges, different lane — chosen by `has(hop.http_status)`. Here
`duration_ms` is `118` and there *is* a status, which is the mirror image of the
deny hop and the cheapest way to tell a refusal from a failure at a glance.

---

## Step 6 — one daemon per root

With the first daemon still running, start a second one on the same directory:

```bash
./target/release/meclaw --root ./examples/hard-shell/seed --daemon --api 127.0.0.1:7800
echo "exit=$?"
```

```
Error: another meclaw colony is already running on this root: pid 1516518 holds
./examples/hard-shell/seed/colony.db-lease (start_id 166418485). Two daemons on
one root duplicate every cell and every child process — stop pid 1516518 first,
or boot a different --root.
exit=1
```

It refuses **before** `colony.db` is opened and creates no database of its own,
so a mistyped restart cannot leave you with two colonies quietly duplicating
every cell, every timer and every tool child. The holder record names a pid
**and** its start time (`start_id`), which is what makes the next step safe.

---

## Step 7 — kill it the rude way

```bash
kill -9 1516518
./target/release/meclaw --root ./examples/hard-shell/seed \
                        --templates ./templates --daemon --api 127.0.0.1:7799 &
```

`SIGKILL` gives a daemon no chance to clean up. What it left behind is in
`seed/log.jsonl`, which the new boot writes to:

```json
{"level":"WARN","target":"meclaw_cli::lease",
 "fields":{"message":"root lease left behind by a DEAD holder (hard kill or crash); reclaiming",
           "lease":"./examples/hard-shell/seed/colony.db-lease","holder_pid":1516518}}
{"level":"INFO","target":"meclaw_cli",
 "fields":{"message":"orphan journal: boot reap clean","examined":0,"gone":0,
           "owned_by_live_daemon":0}}
```

Two facts in those two lines. The lease was **reclaimed, loudly** — the holder
was verified dead by pid *and* start time before anything was taken over, and a
recycled pid would have been recognised as recycled instead. And the orphan
journal was walked: this run had no tool child alive at kill time, so the reap
is `clean`, but it ran, and the counters say what it examined rather than
staying silent.

The topology survived:

```bash
curl -s http://127.0.0.1:7799/colony/registry
```

```
3 cells
```

No re-grow. The declaration from step 2 is in `colony.db`, not in the process.

The reaper's harder cases — a child that really did outlive its parent, and a
recycled pid that must *not* be killed — are pinned in
`crates/meclaw-cells/tests/gh116_orphan_reap.rs`, which hard-kills a real daemon
and drives a real boot reap. The lease cases are in
`crates/meclaw-cli/tests/gh121_root_lease.rs`.

---

## Now break it

The refusals so far were all plain, well-formed addresses. The obvious attack is
to write the same address so it does not look like itself. Try all four at once:

| what you send | why it might work |
|---|---|
| `http://2130706433/` | `127.0.0.1` as a single decimal integer |
| `http://0177.0.0.1/` | the first octet in octal |
| `http://[::ffff:169.254.169.254]/latest/meta-data/` | the metadata endpoint as an IPv4-mapped IPv6 address |
| `http://169.254.169.254./latest/meta-data/` | trailing dot — a different string, the same host |

```bash
for u in "http://2130706433/" "http://0177.0.0.1/" \
         "http://[::ffff:169.254.169.254]/latest/meta-data/" \
         "http://169.254.169.254./latest/meta-data/"; do
  curl -s -o /dev/null -X POST http://127.0.0.1:7799/messages \
       -H 'Content-Type: application/json' \
       -d "{\"target\": \"/door\", \"body\": {\"messages\": [{\"origin\": \"assistant\",
            \"type\": \"tool_call\", \"id\": \"x\", \"text\": \"{\\\"url\\\": \\\"$u\\\"}\"}]}}"
done
```

Every one of them arrives at `/sink` on the `denied` lane:

```
denied | target_blocked | web_fetch refuses 127.0.0.1: loopback 127.0.0.0/8
denied | target_blocked | web_fetch refuses 127.0.0.1: loopback 127.0.0.0/8
denied | target_blocked | web_fetch refuses ::ffff:169.254.169.254: link-local 169.254.0.0/16 (cloud metadata)
denied | target_blocked | web_fetch refuses 169.254.169.254: link-local 169.254.0.0/16 (cloud metadata)
```

Read the refusal texts, not just the verdicts: `2130706433` and `0177.0.0.1`
both come back as **`127.0.0.1`**. The address was parsed and normalised into a
number before anything was compared, so there is no string form of a blocked
address left to be clever with. The judgement is not a substring match on the
URL — which is exactly why it holds against forms nobody enumerated in advance.

Two things this walkthrough does *not* prove, and should not pretend to:

- **A hostname that resolves inward.** The claim is that a name is resolved and
  **every** address it answers with is screened, with the screened resolver
  handed to the HTTP client so there is no window between "checked" and
  "connected". Demonstrating it needs DNS you control; it is pinned in the test
  suite instead.
- **`allow_private_networks: true`.** That knob is a real hole and is meant to
  be one — a colony talking to a model on its own GPU needs it. It opens the
  private ranges and *not* link-local: `169.254.0.0/16` and `fe80::/10` stay
  refused even then.

And the thing worth repeating at the end: none of the above is in
`seed/main/probe/config.json`. Go look. It is a timeout and a concurrency cap.

---

## Clean up

```bash
kill %1
rm -rf examples/hard-shell/seed/{colony.db*,log.jsonl,blobs,.staging,orphan-journal.jsonl} \
       examples/hard-shell/seed/main/{door,sink}
```

Everything a run creates is either gitignored or listed above; the seed goes
back to four files.

## Pinned

`crates/meclaw-cells/tests/hard_shell_example.rs` boots **this** seed and applies
**this** declaration — the files, not copies of them — offline, and checks that
the seed contains no security configuration, that `grow.json` names only
templates that ship, and that the refusal arrives as `target_blocked` on its own
lane without an `http_status` and without a dead letter. Steps 6 and 7 are
process-level facts about the daemon and are pinned where they live:
`crates/meclaw-cli/tests/gh121_root_lease.rs` and
`crates/meclaw-cells/tests/gh116_orphan_reap.rs`, both with real processes.
