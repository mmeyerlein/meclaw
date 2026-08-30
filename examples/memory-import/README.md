# examples/memory-import

A person's agent is replaced. A colony is rebuilt on another machine. A benchmark
has to run twice against the same remembered state.

In every one of those what must not be re-earned is not only the memory. A
member is four things a colony cannot recompute: what was said to this person,
the curated record that decides who may be told what, the screen every inbound
turn passes, and — inside each of its generations — the ledger that says which
conversation is still open. All four LEAVE now (`in_export` writes
`<hive>/seed/<table>.jsonl`, one directory per holder,
[#471](https://github.com/mmeyerlein/meclaw/issues/471) and
[#475](https://github.com/mmeyerlein/meclaw/issues/475)), and until this example
there was no declared way to put any of them back: there is no `seed` field on a
mutation diff and none on a `ref` marker, the only manifest key that carries
files is `add_templates[].files`, and the shipped `member` reaches its three
holders through a reference. Nothing can put a seed into a reference.

So the way in was a file copy by hand, or — worse, and it happened — harvesting
the parts back out of the dead-letter queue.

This example is the way in. It is one manifest.

## The shape

```
                        ┌─ colony A ───────────────────────────┐
                        │ member ▸ memory-hive                 │
   in_export ──────────▶│        ▸ affinity                    │
   (+ context.assistant)│        ▸ firewall                    │
                        │        ▸ assistants/<gen> ▸ … ▸ keeper│
                        │        ▸ export-sink                 │
                        └──────────┬───────────────────────────┘
                                   │  memory-hive/seed/<table>.jsonl
                                   │  affinity/seed/<table>.jsonl
                                   │  firewall/seed/rules.jsonl
                                   │  session-keeper/seed/sessions.jsonl
                                   ▼  export_final.json (one per hive, one for the member)
                            build_import.py
                                   │  one manifest        one message per late part
                                   ▼                      ▼ (--after-boot)
                        ┌─ colony B ──────────────┐
   add_templates ──────▶│ member-<name>           │  the three references
   add_nodes    ──────▶ │   memory-hive/store/seed│  written out, each
                        │   affinity/store/seed   │  export inside its own
                        │   firewall/rules/seed   │
                        └──────────┬──────────────┘
                                   │  grow a generation, then
                     in_import ────┘  hop.import_hive = 'session-keeper'
```

Nothing in that picture touches a `cell.db`. There is no `sqlite3`, no file
copied into a running tree, no schema header lifted by hand.

## What is checked in

```
memory-import/
├── build_import.py     export directory + template library -> one manifest
│                       (+ --after-boot: the `in_import` messages for what a
│                        birth cannot seed)
└── README.md
```

No seed of its own, and that is the point: this example is a **step**, not a
colony. It grows a member into whatever org shell you already have —
[`organism`](../organism/) is the one that builds such a shell from nothing.

## The order is the mechanism

**A seed is read exactly once, when the `cell.db` is created.** After that it is
not merged, not appended, not diffed — it is not read. There is no message, no
operation and no flag that makes a running cell load one. So:

1. **Everything the export carried goes in at BIRTH.** The manifest registers
   the derived template and instantiates it in the SAME diff. `add_templates`
   runs first inside a diff and its registrations are visible to that diff's
   `add_nodes` ([#443](https://github.com/mmeyerlein/meclaw/issues/443)), which
   is what makes "declare the tree, then grow it" one operation.
2. **Everything that happened afterwards goes in through `in_import`**, against
   the holder that is now running. A delta, a correction, a second source: one
   export part per message, and applying the same part twice leaves the same
   state. That lane is on the shipped `member` and since #475 it has four doors:
   `hop.import_hive` names `affinity`, `firewall` or `session-keeper`, and a part
   that names nothing goes to the memory hive, which is where every part written
   before #471 came from. It is addressed at the member's own path — the org
   above does not carry the lane, and the manifest draws no edge for it. A
   `session-keeper` part additionally names the GENERATION on
   `context.assistant`: a member with two of them has two session ledgers, and
   they are not one document.

There is no third option and no second chance. **A member that is already
running cannot be given a past** — grow it again under another name, or do the
first step before the first message.

## Running it

```bash
# 1. walk the memory out of the member that has it
curl -sS localhost:8080/messages \
  -H 'content-type: application/json' \
  -d '{"target": "/os/orgs/<org>/members/<name>", "header": {"hop": {"route": "in_export"}},
       "body": {"messages": []}}'
# the sink writes under MEMBER_EXPORT_DIR, one directory per holder; a holder is
# finished when <hive>/seed/export_final.json is there, and MEMBER_EXPORT_DIR's own
# export_final.json names every holder that finished

# 2. turn that directory into one manifest
python3 examples/memory-import/build_import.py \
    --export "$MEMBER_EXPORT_DIR" \
    --templates ./templates \
    --scope /os/orgs/<org> \
    --name <name> \
    --after-boot carry-in.json \
    --assistant <generation> > import.json

# 3. grow the member that is born with it
./target/release/meclaw --root <root> --templates ./templates --validate
./target/release/meclaw --root <root> --templates ./templates --apply import.json

# 4. grow that member's generation (templates/member/README.md, 'Addressing an
#    assistant through a channel'), then post what a birth could not seed --
#    one message per part, at the member's own path
python3 - <<'PY'
import json, urllib.request
for msg in json.load(open("carry-in.json")):
    urllib.request.urlopen(urllib.request.Request(
        "http://localhost:8080/messages", method="POST",
        headers={"content-type": "application/json"},
        data=json.dumps(msg).encode()))
PY
```

Step 4 exists only when the export carries something a birth cannot seed — today
that is `session-keeper` and nothing else. Without `--after-boot` the tool behaves
exactly as it always did and says on stderr what it left out.

`--scope` is the org the member belongs to; `--name` is the member's name, and
the local template is called `member-<name>` unless `--template-name` says
otherwise.

## What the tool does, and why each part of it is a decision

**It writes the references out.** `member/memory-hive`, `member/affinity` and
`member/firewall` are each `{"cell": {"type": "ref"}}`, and a reference has no
files. The tool splices every referenced hive the export carries into the
member's own tree at its own path — the same tree the substrate would have
staged, only now it is a place a seed can live. A holder the export does NOT
carry stays a reference: writing one out for nothing would freeze a shipped
template at the shape it had on the day of the import.

**It removes the shipped placeholder seeds of every hive it splices.** `affinity`
ships a fictional person and `firewall` ships eight example rules, and both are
there so a fresh instance is neither empty nor an open gate. Beside an imported
record they are something else: a second person nobody imported, and a rule set
nobody wrote. What the RECEIVING hive configures for itself is the exception and
survives — today that is `memory-hive`'s `emb_models.jsonl` and nothing else.

**It refuses a hive directory without `export_final.json`.** A walk that aborted
leaves a PREFIX of a document, and a prefix looks exactly like a whole one from
the outside. A member born from one has no way to discover what it is missing.
The check is per holder, because three walks finish independently.

**It reads the pre-#471 flat shape too.** An export directory whose `seed/` sits
directly under it, with no per-hive level, is read as a memory-hive-only export.
That is what every document written before the sink learned to file by hive looks
like, and refusing them would strand the backups already on disk.

**It says out loud what it cannot place, and it writes out the way in.**
`session-keeper` carries its sessions as well, and its hive stands inside a
GENERATION rather than under the member — four levels down, and the container
that holds generations ships EMPTY, so a derived member template has nowhere to
put its seed. That part is named on stderr and left out of the manifest, because
a document that quietly went nowhere reads exactly like one that was never
exported. It is **late, not lost**: since
[#475](https://github.com/mmeyerlein/meclaw/issues/475) the member forwards
`in_import` to it, and `--after-boot FILE` writes the ready-to-post messages that
carry it in once the member is up and its generation is grown. `--assistant NAME`
says which generation; without it the tool refuses rather than guessing, because
a session ledger delivered to the wrong generation is a conversation grafted onto
somebody else's.

**It refuses a directory carrying `emb_models.jsonl`.** That table is the
RECEIVING hive's own configuration — which embedding generation is live, behind
which endpoint. The export deliberately never writes it, so a directory that has
one was edited by hand, and merging it would leave the new member with two live
generations or none.

**It does not copy the shipped prose.** A derived template that carries the
original's README claims things about itself that are no longer true of it.

**It does not add `in_import` — the shipped level already carries it.** The hive
has accepted the lane since 2.2.0, and the door through the member was the piece
that was missing; `member@1.4.0` ships it, so the derived template inherits it
along with everything else this tool copies. That is step 2 above, and it is now
one lane on the shipped level rather than a per-derivation patch, which is what
makes the second step available to a member that was grown the ordinary way.

## The alias tables, and a correction

The identity families (`predicate_aliases`, `subject_aliases`, `claim_aliases`
and their three refusal logs) are the only tables in this hive that carry a real
`PRIMARY KEY` — the store creates them itself out of `params.canonical`, and
`set_alias` / `reject_pair` are upserts on that key.

A seed header cannot express a key. It carries column names and a coarse type
and nothing else, so the staging seeder builds those tables **without** one, and
an upsert against a keyless table does not duplicate — it FAILS, every time, and
the judgement it carried is never recorded.

That used to make the alias families untransferable by seed, and both template
READMEs said so. It is no longer true:
[#255](https://github.com/mmeyerlein/meclaw/issues/255) made the store **assert**
the key at its first wake and rebuild the table if it finds it missing, carrying
every row over and collapsing duplicates onto the key. So the whole document
travels as a seed set, alias families included, and the repair is measured on a
table whose rows came out of another colony in
`crates/meclaw-cells/tests/gh467_a_member_is_born_with_its_history.rs`.

## What this example is not

- **Not a colony transfer.** What moves is a member's remembered CONTENT. The
  topology, the edges and the other cells' databases are a different axis
  ([#37](https://github.com/mmeyerlein/meclaw/issues/37)).
- **Not the chat state at BIRTH, and that is a placement problem rather than a
  missing mechanism.** The open sessions live in `session-keeper/sessions`,
  inside a generation, not in the memory hive. Since
  [#471](https://github.com/mmeyerlein/meclaw/issues/471) that hive has the same
  transfer lane as the other three, and a keeper that receives one CONTINUES the
  conversation the source had open — pinned in
  `crates/meclaw-cells/tests/gh471_a_keeper_carries_its_sessions.rs`. Since
  [#475](https://github.com/mmeyerlein/meclaw/issues/475) a member's own
  `in_export` reaches it and its own `in_import` carries a part back, so what is
  left is the ORDER: the container that holds generations is empty until one is
  instantiated, so no derived member template has a path for that seed.
  `--after-boot` is the answer to that, and it is step 2 rather than step 1. Hive
  to hive, outside a member, it is still one edge: `old -> new` on
  `hop.route == 'dump'` with `set_hop route: 'in_import'`.
- **Not a merge of two people.** Nothing dedupes across identities: two hives
  that learned the same thing from different turns hold two facts about it
  afterwards, and the nightly identity round is what decides whether they are
  one.
- **Not paged.** A part is a whole table. A hive whose largest table outgrows one
  message needs a keyset-paged part, which does not exist yet
  ([#243](https://github.com/mmeyerlein/meclaw/issues/243) follow-up).

## Pinned

`crates/meclaw-cells/tests/gh467_a_member_is_born_with_its_history.rs` boots a
colony that remembers, walks it out through the shipped sink, runs
`build_import.py` on the result, applies the manifest to a colony that never
heard any of it, and asks that colony's store the question — the lexical leg of
a recall — that only a memory it never saw written can answer. It also drives
step 2 twice, because idempotency is the whole repair procedure for a partial
transfer.

`crates/meclaw-cells/tests/gh475_a_member_reaches_the_keeper_it_holds.rs` drives the
fourth holder: a member with one generation is told `in_export` with that generation
named, the sink files `session-keeper/seed/sessions.jsonl` beside the other directories,
`--after-boot` turns it back into one `in_import` message, and the member's own door
carries it into the keeper's store.

`crates/meclaw-cells/tests/gh471_a_member_carries_all_of_itself.rs` does the same
walk for all three holders at once: one distinctive row is written into each of
them in colony A, one `in_export` produces three documents, and the member grown
in colony B arrives with the memory, the record AND the screen. The last claim is
the one a row count cannot reach — B's firewall refuses a turn on a rule that
only ever existed in A.
