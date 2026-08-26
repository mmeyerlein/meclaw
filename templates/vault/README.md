# `vault@1.1.0`

A secret store with **no operation that returns a secret**.

That sentence is the whole template. Everything below is either a consequence of
it or an honest note about what it does not cover.

| path | type | role |
|---|---|---|
| *(this template is one cell)* | `vault` | encrypted store + executor |

## The claim, and why it is a type and not a policy

The usual shape of this problem is a store plus a rule: *the secret is in there,
and something checks whether you may read it.* Every such rule is an argument
waiting to be won — by a prompt, by a bug, by a future maintainer who needs an
exception "just for the migration".

The `vault` cell type has no read. Its route surface is:

| op | who may call it | what comes back |
|---|---|---|
| `put` / `rotate` | the user channel only | the version number |
| `use` | the broker only | the **result** of using the secret |
| `revoke` | user channel, broker | how many versions were revoked |
| `status` | user channel, broker | names, versions, key fingerprint |
| `unlock` / `lock` | the user channel only | locked true/false |
| `deliver` | the broker only | the secret, SEALED to a key only the requester holds |

There are eight rows, and the eighth hands out nothing in the clear. `get` is
still refused the same way `frobnicate` is — as an unknown op — because the
absence of a READ is structural, not a special case somebody could argue away
later. `deliver` is not `get` with a coat on: what comes back is a ciphertext
whose key lives in exactly one task's memory and dies with it.

`use` v1 signs: HMAC-SHA256 over a caller-supplied payload. That is the
ssh-agent shape deliberately — the secret does work and stays home. The one
case that genuinely needs the value itself, a connector authenticating to a
platform, is served by `deliver` (see *Sealed delivery* below).

## The two callers

**The user channel** is a message with no `reply_to`. No edge can produce one,
because the colony stamps `reply_to` on everything a cell emits — so "came from
the operator" is substrate truth here, not a claim in a body. This is the only
way a secret gets *in*:

```
meclaw --vault /main/access/vault --vault-add telegram_token
```

The value is read from stdin (never from an argument — that lands in `ps` output
and in shell history) and written straight into the vault's own database. No
message is built, no edge is travelled, no message-log row is written, no
context window ever sees it. It works with the daemon stopped.

**The broker** is the one path named in `params.broker`. It may ask the vault to
*use* a secret. It may **not** put one — otherwise an agent that captured the
broker could swap the vault's contents for its own.

Everyone else is refused before the operation is even looked at, and the refusal
is written to `vault_audit` with the offending path. A vault whose refusals are
invisible cannot be reviewed.

### Why the vault does not check the grant itself

A cell cannot query another cell inside one `handle()` — that is the actor model,
not a limitation of this template. So the work is split where it can actually be
done: the **broker** validates the grant against the grants store on its own
lane, and the **vault** does the one thing only it can do — check who is
talking — and records the `grant_id` it was handed. The ACL is on the vault
side; the lookup belongs to the broker.

## Sealed delivery

`deliver` answers with a **sealed box**: the secret encrypted to a key that
only the requester ever held. The broker forwards it, the message log records
it, and neither can read it.

```text
recipient, once per request
  e_sk, e_pk := X25519 keygen        # a fresh pair for this one request
  ask the broker:
    {"op": "deliver", "name": ..., "grant_id": ...,
     "recipient_key": hex(e_pk)}     # only the public half travels

vault, once per response
  r_sk, r_pk := X25519 keygen        # the vault's own ephemeral half
  shared     := X25519(r_sk, e_pk)
  box_key    := HMAC-SHA256(key: "meclaw-sealed-box-v1",
                            msg: shared || e_pk || r_pk)
  nonce      := 24 random bytes
  ct         := XChaCha20-Poly1305(box_key, nonce, secret)
  answer     := {"epk": hex(r_pk), "nonce": hex(nonce),
                 "ciphertext": hex(ct)}
  r_sk is dropped here               # nothing is kept that could reopen it

recipient
  shared  := X25519(e_sk, r_pk)      # the same shared value, from the other side
  box_key := HMAC-SHA256(same label, same transcript)
  secret  := open(box_key, nonce, ct)
  e_sk dies with the task that minted it
```

**No long-term vault key.** The vault mints its half per answer and forgets it.
There is no key material anywhere that could open yesterday's box, so a stolen
disk, a subpoenaed message log and a captured broker all buy the same thing:
ciphertext. Forward secrecy is not a feature here, it is what is left when you
refuse to keep a key.

**The box proves nothing about who sealed it.** That is deliberate, not an
oversight. Authenticity is carried by the topology — only the path in
`params.broker` is ever answered — plus the policy the broker enforced before
the request reached the vault. Putting a second, weaker answer to the same
question inside the crypto would invite callers to trust the weaker one.

**Both public keys are in the transcript.** The key agreement alone fixes the
peers but not the context: the same shared value could be reached under some
other protocol that happens to reuse one of the halves. Binding `e_pk` and
`r_pk` into the derivation means a box is only openable as *this* protocol's
box, and cross-protocol recycling of a key produces garbage rather than a
plaintext.

**HMAC, not HKDF.** The output needed is exactly 32 bytes — one
XChaCha20-Poly1305 key. HKDF's expand step exists to stretch a PRK to arbitrary
length, and at exactly one block it does nothing but add a construction to
explain. The extract step is an HMAC. So this is the extract step, named
honestly.

A request and its answer:

```json
{"op": "deliver", "name": "telegram_token",
 "grant_id": "g-7f3a...",
 "recipient_key": "3b6a...c1"}
```

```json
{"name": "telegram_token", "version": 3, "grant_id": "g-7f3a...",
 "sealed": {"epk": "9d21...4e", "nonce": "5c0f...a7",
            "ciphertext": "e18b...2d"}}
```

Only the broker may call it — a delivery is a **spend**, and the user channel
that deposits secrets has no business drawing them back out. A locked vault
answers `vault_locked`; a name that was never deposited or has been revoked
answers `unknown_secret`; a missing or malformed `recipient_key` (anything that
is not 64 hex characters of X25519 public key) answers `invalid_input`. No new
`error_code` was added — the documented list stays closed. Every delivery and
every refusal is written to `vault_audit`.

## Injection at unlock (deprecated)

**Deprecated since GH #421.** This path pushes the secret to the connector as a
message body, which means it travels through the `message_log` — precisely the
exposure sealed delivery exists to remove. It is not being removed now, because
removing it would break running colonies; it logs a warning and will disappear
with the first release that bundles breaking changes. Use `deliver` instead.

`use` signs and returns a signature. That covers everything a vault can do
*without* the secret going anywhere. But a connector that has to authenticate to
Telegram needs the token itself, and pretending otherwise would make the vault
decorative.

So this path was shaped so that no message could steer it:

```json
"inject_map": {"telegram_token": {"to": "./connector", "key": "bot_token"}}
```

At **unlock** — once, not per request — the vault hands each named secret to the
cell in the map, as a params update: a body with a `params` slot and no
`messages`, which the receiving cell merges, persists and answers with silence.
No inference, no cost, no echo.

Three properties make this narrow rather than a hole:

- **The map is configuration.** A caller that names its own target in the body
  changes nothing, because the field is never read — the same reasoning that
  makes `reply_to` the only identity this cell trusts.
- **It happens at unlock**, so the hot path never carries a credential and the
  requester never triggers a delivery.
- **The requester learns THAT, not WHAT.** The unlock answer lists which name
  went where under which key. The value appears in exactly one place: the
  connector's params.

A secret named in the map that has not been deposited yet is skipped with a
warning rather than failing the unlock — a vault that refuses to open because
one of five credentials is missing is a vault nobody can commission.

## Unlock attestation

Before it accepts key material, the vault verifies its own inbound edges against
`params.broker` plus `params.sealed_neighbors`. If anything else is wired to it,
it stays **locked** and says which path it found.

The check is also strict about what must be *present*. A vault whose broker edge
is missing does **not** attest — it stays locked and reports
`broker_unwired: <path>` (still under `error_code: "attestation_failed"`; no new
code). Earlier versions let a vault with no inbound edges at all attest, on the
grounds that nothing suspicious was wired to it. That was backwards: if the
topology is what stands in for a signature — and with sealed delivery it
explicitly is — then the absence of that topology cannot be allowed to attest.
An unwired vault is an unverifiable one.

This closes a specific hole. The hive-port boundary is enforced on *mutations*,
and the birth topology is deliberately exempt (author sovereignty). A `code` cell
has filesystem access, so an agent could rewrite the grow file or the colony
database on disk and let the *next boot* wire an edge into the vault that no
mutation would ever have been allowed to add — laundering the gate through a
reboot.

It still can. It just never gets the key. Making the unlock the checkpoint means
a tampered topology may exist and is simply useless: an unverifiable
neighbourhood fails closed the same way a wrong one does.

## The key

`params.key_source` names a *source*, never material — same layering ssh uses:

| source | where the passphrase comes from |
|---|---|
| `auto` (default) | `systemd-cred` under systemd, else `prompt` |
| `prompt` | the terminal, echo off (`/dev/tty`, like `ssh-keygen`) |
| `systemd-cred` | `$CREDENTIALS_DIRECTORY/<credential_name>` — use `LoadCredentialEncrypted=`, TPM2-bound where the machine has one, and the key never lies readable on disk |
| `plainfile` | a file, protected by its permissions alone. Refused unless group and others cannot read it — the same answer ssh gives for a loose private key. This is the honest unattended-boot option, not a recommendation |

Whatever a source delivers is treated as a passphrase and run through argon2id
against a per-store salt; each secret is then sealed with XChaCha20-Poly1305 and
its own random 24-byte nonce.

**A woken vault is always locked.** The key lives in the cell's task and dies
with it. A vault that could resume its unlocked state across a sleep would have
to keep the key somewhere that survives the sleep, and there is no such place
that is not a worse version of the problem the vault exists to solve.

## No-delete, as the rest of the substrate means it

A `put` onto an existing name *is* a rotation: it appends a version. A `revoke`
flips a status. Yesterday's ciphertext stays on disk, which is what makes a
revocation auditable rather than a hole.

`revoke` needs no passphrase, deliberately. Being locked out of a vault must
never stop you from disabling a credential that leaked.

## Wiring

Instantiate it **inside** the hive that owns the credentials, next to the cell
that validates grants:

```
access/
  policy/    code   -- decides
  invoke/    code   -- validates the grant
  vault/     vault  -- this template; params.broker = "./invoke"
  store/     store
```

`access@2.1.0` ships with exactly this (`access@1` did too -- the property has been
true of every version): the vault is an interior cell of the hive
and deliberately **not** one of its ports, so the generic boundary refuses any
edge into it from outside the scope. Its `params.inject_map` is empty on a fresh
instance — a template that shipped with delivery addresses would deliver to
somebody else's.

`params.broker` may be absolute (`/main/access/invoke`) or hive-relative
(`./invoke`, resolved against the vault's own path) — the relative form is what
lets one template be instantiated anywhere.

**Leave the vault OUT of the hive's `params.ports`** — that list is exactly the
set a non-port interior cell must be absent from, so there is nothing to declare
and declaring it is what would break the protection. A sealed hive writes
`"ports": []` and names its lanes in `params.contract` instead; the generic
boundary then refuses any edge from outside the scope into any interior cell,
the vault included. The vault is protected simply by not being a port. No cell
name appears in substrate code for this — it is the same mechanism every sealed
hive uses (`crates/meclaw-colony/src/mutation/port_boundary.rs`).

## Honest limits

- **Same-process placement**: a determined `code` cell in the same process can
  read the vault's memory while it is unlocked. Stated openly. The designed
  answer is placement — the vault cell in its own process or under its own
  user — which is a deployment property and changes no edge.
- **Operator always wins.** True for every local system.
- **An agent that develops the substrate itself** is out of scope by definition.
  No vault holds against that, and pretending otherwise would be the more
  dangerous claim.
- **The key while unlocked** is zeroed on drop, which closes the freed-page
  window and not the live-memory one. Labelled rather than advertised.
- **A sealed box does not prove who sealed it.** It proves only that whoever
  did held the requester's public key. Who that was is carried by the topology
  (`params.broker` is the only path answered) and by the policy the broker
  enforced — not by the ciphertext. A signature can be added later as a fourth
  field alongside `epk`, `nonce` and `ciphertext`, without breaking the wire
  form.

## Storage

Three tables in the cell's own `cell.db`:

| table | holds |
|---|---|
| `vault_meta` | the salt (not secret; it exists so two vaults with one passphrase do not share a key) |
| `vault_secrets` | one row per `(name, version)`: nonce, ciphertext, status, created_at |
| `vault_audit` | every operation, refusals included: at, op, actor, name, outcome, reason |

And none of the three travels. Since GH #314 this template declares
`contract.transfer: "none"`, which exempts its `cell.db` from the `transfer` body slot
(`docs/cell-types.md` § Content transfer) in both directions -- `export` and `import`, refused
with `error_code: "transfer_exempt"`. The slot is answered by the **substrate**, above every
cell type and before `handle()`, so the two-caller ACL below never sees one; and the disclosure
it would have made needs no passphrase, because `name`, `version`, `status` and `created_at` are
plaintext columns and `vault_audit` is the complete call history. `contract.write_surface` does
not cover this: an export is a read, and no write surface has ever bounded a read.
