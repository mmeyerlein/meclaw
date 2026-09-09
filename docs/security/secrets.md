# Secrets and capabilities

Every other boundary in meclaw is a property of the tree a reviewer reads off files. This one is
configured: a secret has to be put in, and somebody has to say who may spend it. Two cells do
that, and neither of them hands a credential to the agent that asks.

## The vault

`vault` is a secret store with no operation that returns a secret. `put` and `rotate` come
from the user channel, `use` signs on the broker's behalf so the secret does the work and
stays home, and `deliver` answers with a ciphertext sealed to a key the requester minted for
that one call. `get` is refused the way an unknown op is.

A secret goes in over the CLI, read from stdin, with no message and no log row:

```bash
meclaw --vault /main/access/vault --vault-add telegram_token
```

The vault mints its half of the sealing key per answer and drops it, so no key material
anywhere opens yesterday's box. [`templates/vault`](../../templates/vault/README.md).

## Handles instead of secrets

`access` is the capability broker. An agent asks for a capability, and what comes back is a
grant handle with a summary of what it covers, never the chat id, the endpoint or the
credential.

```json
{"capability": "chat.send", "subject": "member:example",
 "resource": {"channel": "example-chat"}, "purpose": "answer the incoming message"}
```

Policy is rows in a store, changed with an `insert` or an `update` while the colony runs:
requester, capability, subject, `scope_match`, verdict, `max_ttl_ms`, constraints, and a
`cred_ref` that names a credential and never carries one. Seven rows ship, five of them
disabled; the two enabled ones are the pair a fresh colony cannot start without.

Two rules make the broker an authority rather than a lookup table. The requester is read from
`context.requester`, which an edge sets and the colony wrote, never from the body, and an
absent one is a denial. The address of an invocation is built from the grant, and every
address key in the payload is removed before the message reaches the connector.
[`templates/access`](../../templates/access/README.md), wired to a consumer in
[`examples/vault-pilot`](../../examples/vault-pilot/README.md).
