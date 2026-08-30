# examples/vault-pilot

A model that holds no key.

`main/brain/config.json` names a provider, a model and a grant. It does **not**
name a credential: `api_key` is `${EXAMPLE_PROVIDER_KEY:-}` and that variable is
meant to stay unset, so the string resolves to empty and an empty string is not a
bearer. The value it authenticates with arrives from the vault, **sealed**, on an
ordinary broker invocation, and it is opened in the cell's own task and lives
nowhere else.

This is the first example that wires [`templates/access`](../../templates/access/)
to a consumer. Everything else in the colony is grown from the library.

## What is checked in

```
vault-pilot/
├── seed/                                     the --root of the colony
│   ├── colony.json                           substrate defaults. two lines.
│   ├── main/config.json                      type: "hive", and its graph is EMPTY
│   ├── main/brain/config.json                the consumer: one llm cell, no key
│   └── main/access/
│       ├── config.json                       the hive marker, graph EMPTY
│       └── store/
│           ├── config.json                   the shipped broker store, verbatim
│           └── seed/
│               ├── policy.jsonl              one rule, and it is switched ON
│               ├── cred_refs.jsonl           WHICH credential this colony holds
│               ├── grants.jsonl              the long-term grant
│               └── grant_events.jsonl        its `granted` event
├── grow.json                                 the declaration. two nodes, seven edges.
└── README.md
```

Five of the broker's six cells are **not** in there — `policy`, `invoke`, `sweep`,
`clock` and `vault` are instantiated from `templates/access` when `grow.json` is
applied, and the fourteen edges inside the hive come with them.

## Why two of them are checked in anyway

**The store, because of a chicken and an egg.** `params.credential_grant_id` is
immutable: no message may repoint it, so no message can mint it either, and the
`grants` row it names has to exist *before* the `llm` cell boots. Nothing in a
manifest can write that row — the mutation diff vocabulary is seven topology
operations and none of them writes to a store. What *can* put rows into a store is
a `seed/<table>.jsonl`, and it lands exactly once, on a fresh `cell.db`. So the
store is checked in with its seed, and the instantiation **merges** around it: a
subtree template's cells that already exist on disk are left untouched, and the
missing ones come from the library. One `add_nodes` therefore grows five cells
around a sixth it did not write.

**The hive marker, because the store has to live under it.** It is the shipped
hive config in every respect but one: `params.graph.edges` is empty, because the
instantiation draws those edges and drawing them twice is drawing them once plus a
duplicate. `params.ports` and `params.contract` are the shipped ones, so the
boundary is the template's boundary — the hive path is the address, and no edge
reaches a cell inside it.

## The grant handle

It is a literal, and it is the same literal in two files. To keep it reproducible
rather than invented, it is **derived**:

```
grant:<cred_ref without its "cred:" prefix, ":" → "-">@<subject, ":" → "-">

cred:example-provider:primary  +  member:example
                    ↓
grant:example-provider-primary@member-example
```

One credential per subject gets one handle, the same one every time, and two
people writing the seed by hand write the same string. Nothing computes it at run
time — a seed file is text, and that is the point: the handle exists before
anything runs.

## Running it

Four gestures, in this order. The third one is the awkward step, and it is
awkward for a reason worth knowing.

**1. Grow the colony.**

```bash
./target/release/meclaw --root ./examples/vault-pilot/seed \
                        --templates ./templates \
                        --apply ./examples/vault-pilot/grow.json
```

**2. Start it once and say something.** Cells spawn lazily, so at this point the
vault has a `config.json` and no database. This turn is *supposed* to fail — the
vault holds nothing yet — and what it is for is waking the vault so it creates
its `cell.db`.

```bash
VAULT_PILOT_PASSPHRASE='…' ./target/release/meclaw --root ./examples/vault-pilot/seed \
                                                   --templates ./templates
# type one line, watch it come back as an error, then ctrl-D
```

**3. Put the credential in the vault.** It goes straight into the vault's own
database, with no colony running, and it is read from **stdin** — never from an
argument, which would land it in `ps` output and in shell history. A credential
never becomes a message.

```bash
./target/release/meclaw --root ./examples/vault-pilot/seed \
                        --vault /main/access/vault \
                        --vault-add cred:example-provider:primary
```

The name it is stored under is the `ref` in `cred_refs.jsonl`, and the grant reads
it from there. A payload cannot ask for a credential it was not granted.

`--vault` takes the cell's path with the root cell directory in it
(`/main/access/vault`), because this mode talks to a directory rather than to a
running colony — the same cell is `/access/vault` to a message.

**4. Start it for real.**

```bash
VAULT_PILOT_PASSPHRASE='…' ./target/release/meclaw --root ./examples/vault-pilot/seed \
                                                   --templates ./templates --api 127.0.0.1:7788
```

`grow.json` sets `params.unlock_env` on the vault to `VAULT_PILOT_PASSPHRASE`. It
has to: a vault inside a sealed hive cannot be unlocked over the user channel — the
user channel is a source message, a source message reaches no hive-internal cell,
and everything that can reach one is an edge. So it opens itself from the
environment or not at all. The param names a **variable**, never a value.

## What happens on the first message

```
you    → /brain    "hello"
brain  → access    access.invoke, operation vault.deliver, one ephemeral X25519 public key
you    ← (nothing yet)                                 ← this turn is PARKED
access → vault     the grant said WHICH credential
vault  → access    {"epk", "nonce", "ciphertext"}
access → brain     ack + the sealed box, opened in RAM
brain  → provider  Authorization: Bearer <the vault's value>
brain  → you       the answer                            ← the SAME turn, answered
```

**The first turn after every wake asks, and is answered anyway (GH #457).** The
`llm` cell parks the turn that triggered the vault round instead of dropping it,
fires the request, and drains the parking lot once the sealed box arrives; the
credential lives in the task and dies with it, so a restart puts the cell back at
the beginning of the asking, not of the conversation. Two bounds keep the parking
honest, both `params`: `credential_wait_max` (how many turns fit — the one that
does not gets its `credential_pending` receipt immediately) and
`credential_wait_ms` (how long the round may take before every parked turn gets
that receipt, in order). `credential_pending` is therefore the code for a vault
that refused or never answered, and no longer the price of a wake. Measured in
`crates/meclaw-cells/tests/gh457_a_parked_turn_is_answered_not_dropped.rs`.

## The policy row, and what it is not for

`seed/policy.jsonl` carries one rule, and it is the only row in this tree that
ships `enabled: 1` — a template's rows all ship switched off, and a fresh broker
grants nothing until an operator turns on exactly what they meant.

That rule is **not** what lets the delivery above happen. `./invoke` reads
`grants`, never `policy`; the seeded grant is what it spends. The rule is what lets
this grant be **minted again** when the seeded one runs out: an `access.request`
for `credential.read` on behalf of `member:example` answers `granted` instead of
`capability_unknown`. `scope_match.actions` is `["vault.deliver"]`, so a grant cut
from it can spend nothing else.

## What this example does not show

No connector, so no `connect` lane is ever taken — the broker's other half, where a
grant becomes a message to a chat, is `templates/access`'s territory and not this
one's. No approval lane. No revocation walk-through, though one `revoked` row in
`grant_events` would close this door on the very next call, `expires_at`
notwithstanding.

And no second consumer: `proxy` and `web_search` hold their credentials in their
own params today, and giving them this shape is a follow-up with a trigger on it
(`docs/defer-register.md` § *Lane-3-Nachtrag*).
