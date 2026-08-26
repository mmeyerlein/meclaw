# examples/hard-shell

Three cells, no configuration, and an agent you are allowed to attack.

`kill -9` the colony in the middle of a tool run: nothing leaks, nothing is
orphaned, and the next boot cleans up after the one you killed. Point it at the
cloud metadata endpoint: it refuses, and it says which range the address belongs
to. Start a second daemon on the same directory: it refuses, and it names the
process that holds it.

None of that is configured here. **The seed contains no policy file, no allow
list and no security block** -- that is the claim, and it is why this example is
three cells instead of thirty.

[`WALKTHROUGH.md`](WALKTHROUGH.md) runs the whole thing command by command with
the real output next to each one: under two minutes, no key, no account.

## What is checked in

```
hard-shell/
├── seed/                          the --root of the colony
│   ├── colony.json                substrate defaults. two lines.
│   ├── main/config.json           type: "hive", and its graph is EMPTY
│   └── main/probe/config.json     one web_fetch cell: a timeout and a concurrency cap
├── grow.json                      the declaration. two nodes, four edges.
└── README.md
```

`main/probe/config.json` is worth reading for what is *missing* from it:

```json
"params": { "external_timeout_ms": 5000, "max_concurrency": 4 }
```

There is no `allow_private_networks`. That knob exists, it defaults to `false`,
and opening the inside of your network is something a human has to type.

## What grows

| node | from template | what it brings |
|---|---|---|
| `/door` | [`door@1.0.2`](../../templates/door/) | 1 cell. `POST /messages` becomes a turn on the ingress lane. |
| `/sink` | [`terminal@1.0.1`](../../templates/terminal/) | 1 cell. Three lanes end here: fetched, denied, failed. |

```
  POST /messages
        |
        v
    /door ──turn──> /probe ──denied───> /sink
                          │ ──fetched──────^
                          └──failed────────┘
```

The refusal gets **a lane of its own**, routed on the error *code* rather than
on the message text:

```json
{"from": "./probe", "to": "./sink",
 "condition": "has(hop.error_code) && hop.error_code == 'target_blocked'",
 "modifier": {"set_hop": {"route": "'denied'"}}}
```

A deny that dead-letters is a deny nobody sees. A deny matched on prose breaks
the day somebody rewords the refusal. The code is the contract.

## Run it

No key, no model, no network needed -- this colony answers no questions, it only
reaches outwards.

```bash
cargo build --workspace --release

./target/release/meclaw --root ./examples/hard-shell/seed \
                        --templates ./templates \
                        --daemon --api 127.0.0.1:7799

curl -s -X POST http://127.0.0.1:7799/colony/mutations \
     -H 'Content-Type: application/json' \
     -d @examples/hard-shell/grow.json
```

---

## Moment 1 -- the metadata endpoint is refused, and nobody said so

`169.254.169.254` is where AWS, GCP and Azure hand out instance credentials,
over plain HTTP, to anything that asks from inside the machine. It is the first
address a prompt-injected agent is told to fetch.

```bash
curl -s -X POST http://127.0.0.1:7799/messages \
     -H 'Content-Type: application/json' \
     -d '{"target": "/door",
          "body": {"messages": [{"origin": "assistant", "type": "tool_call", "id": "c1",
                                 "text": "{\"url\": \"http://169.254.169.254/latest/meta-data/iam/security-credentials/\"}"}]}}'
```

Read what arrived at the terminal:

```bash
TID=$(curl -s 'http://127.0.0.1:7799/colony/trace?limit=1' | jq -r '.trace[0].trace_id')
curl "http://127.0.0.1:7799/ui/trace?trace_id=$TID"
```

```
hop.finish_reason  error
hop.error_code     target_blocked
hop.route          denied
text               web_fetch refuses 169.254.169.254:
                   link-local 169.254.0.0/16 (cloud metadata)
```

There is **no `http_status`**, and that absence is the interesting part: the
address is judged before the connect, so no packet left the machine. Try the
neighbours and each one names its own range:

| url | refusal |
|---|---|
| `http://127.0.0.1:8080/admin` | `loopback 127.0.0.0/8` |
| `http://10.0.0.5/internal` | `private RFC 1918 10.0.0.0/8` |
| `http://192.168.1.1/` | `private RFC 1918 192.168.0.0/16` |
| `http://[::1]:9200/_cluster/health` | `loopback ::1` |

The judgement is not a substring match on the URL. Decimal, octal and hex forms
of an address are normalised first; a *hostname* is resolved and **every**
address it answers with is screened, so a name that resolves inwards is refused
the same way; the fetch follows no redirect it has not screened, and the
resolver the HTTP client is given is the screened one -- so there is no window
between "checked" and "connected" for DNS to change its mind in.

## Moment 2 -- one daemon per root

```bash
./target/release/meclaw --root ./examples/hard-shell/seed --daemon --api 127.0.0.1:7799 &
./target/release/meclaw --root ./examples/hard-shell/seed --daemon --api 127.0.0.1:7800
```

The second one does not start:

```
another meclaw colony is already running on this root: pid 4711 holds
.../seed/colony.db-lease (start_id 8829). Two daemons on one root duplicate
every cell and every child process -- stop pid 4711 first, or boot a
different --root.
```

It refuses **before** `colony.db` is opened, and it creates no database of its
own -- so a mistyped restart cannot leave you with two colonies quietly
duplicating every cell, every timer and every tool child. The lease is a
directory published by an atomic rename, and the holder record names a pid *and*
its start time: a recycled pid is recognised as recycled and reclaimed, a dead
holder is reclaimed with a log line that says why, and an unreadable record
refuses the boot rather than guessing.

Pinned in `crates/meclaw-cli/tests/gh121_root_lease.rs`, which boots real
daemons and checks each of those cases.

## Moment 3 -- a hard kill leaves nothing behind

Every tool child a colony spawns -- a shell, a script runner, an MCP server --
is written to `orphan-journal.jsonl` next to `colony.db` **before** it starts,
and marked retired when it exits. `SIGKILL` gives a daemon no chance to clean
up; the journal is what survives it.

```bash
# start a colony that runs a long tool child, then kill it the rude way
kill -9 <the daemon pid>

# the child outlived its parent
ps -o pid,ppid,cmd -p <the child pid>

# boot again on the same root
./target/release/meclaw --root ./examples/hard-shell/seed --daemon --api 127.0.0.1:7799
```

```
orphan journal: previous run left tool children behind
  examined=1 reaped=1 gone=0 skipped=0
```

The child is gone. What makes that safe rather than reckless is that the reaper
**never** kills a bare pid: each record carries the pid, the process start time
and the executable name, and all three have to still match. A pid that has been
recycled since is skipped loudly (`pid reuse: recorded start_id …, live process
started at …`), so the worst case is a leftover process, never somebody else's.
There is no pattern matching anywhere in it -- no `pkill`, no name sweep.

Pinned in `crates/meclaw-cells/tests/gh116_orphan_reap.rs`, which hard-kills a
real daemon, confirms the child outlived it, and drives the boot reap. Its
sibling cases point a record at the *test runner itself* with a mutated start
time: a careless reaper would kill the test binary, and that is exactly what
those cases are for.

## What this demonstrates, honestly

- **The shell is the default state, not a feature you enable.** Nothing in this
  seed asks for any of it.
- **A refusal is a routable event.** Typed code, own lane, in the trace -- not a
  log line and not a crash.
- **Failure is judged, not guessed.** A lease names a pid *and* its start time;
  an orphan record names a pid, a start time and an executable. Identity is
  verified before anything is killed or taken over.
- **Nothing dead-letters.** Every lane in this tree has an address, including
  the ones that fail.

Two honest limits:

- **The deny is a range matrix, not a policy engine.** It is deny-by-default
  over the private and special-use ranges plus a single two-tier opt-out; it has
  no per-host allow list, no per-cell policy and no notion of "this one internal
  service is fine". A tree that needs that puts a proxy in front and points the
  cell at it.
- **`allow_private_networks: true` is a real hole, and it is meant to be.** A
  colony talking to a model on its own GPU needs it. It opens the private ranges
  -- and *not* link-local: `169.254.0.0/16` and `fe80::/10` stay refused even
  then, because there is no legitimate reason for an agent to read the metadata
  endpoint.

## Pinned

`crates/meclaw-cells/tests/hard_shell_example.rs` boots **this** seed and
applies **this** declaration -- the files, not copies of them -- and sends the
metadata fetch through the whole colony, offline, with no network involved. It
measures that the seed contains no security configuration (and that the knob
which would open it is genuinely absent from the params), that `grow.json` names
only templates that ship, and that the refusal arrives as `target_blocked`, on
its own lane, naming the address and its range, without an `http_status` and
without a dead letter. Moments 2 and 3 are process-level facts about the daemon
rather than properties of this tree, and they are pinned where they live --
`crates/meclaw-cli/tests/gh121_root_lease.rs` and
`crates/meclaw-cells/tests/gh116_orphan_reap.rs`, both with real processes.
