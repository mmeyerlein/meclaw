# `vault@1.0.0`

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

There is no eighth row. `get` is refused the same way `frobnicate` is — as an
unknown op — because the absence is structural, not a special case somebody
could argue away later.

`use` v1 signs: HMAC-SHA256 over a caller-supplied payload. That is the
ssh-agent shape deliberately — the secret does work and stays home. The one
case that genuinely needs the value itself, a connector authenticating to a
platform, is served by injection at unlock (below) rather than by an operation,
so no request can ever ask for one.

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

## Injection at unlock — the one way a secret leaves

`use` signs and returns a signature. That covers everything a vault can do
*without* the secret going anywhere. But a connector that has to authenticate to
Telegram needs the token itself, and pretending otherwise would make the vault
decorative.

So there is exactly one path out, and it is shaped so that no message can steer
it:

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

`access@1` ships with exactly this: the vault is an interior cell of the hive
and deliberately **not** one of its ports, so the generic boundary refuses any
edge into it from outside the scope. Its `params.inject_map` is empty on a fresh
instance — a template that shipped with delivery addresses would deliver to
somebody else's.

`params.broker` may be absolute (`/main/access/invoke`) or hive-relative
(`./invoke`, resolved against the vault's own path) — the relative form is what
lets one template be instantiated anywhere.

Declare the vault as a **non-port interior cell** in the hive's `params.ports`.
The generic boundary then refuses any edge from outside the scope to it; the
vault is protected simply by not being a port. No cell name appears in substrate
code for this — it is the same mechanism every sealed hive uses.

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

## Storage

Three tables in the cell's own `cell.db`:

| table | holds |
|---|---|
| `vault_meta` | the salt (not secret; it exists so two vaults with one passphrase do not share a key) |
| `vault_secrets` | one row per `(name, version)`: nonce, ciphertext, status, created_at |
| `vault_audit` | every operation, refusals included: at, op, actor, name, outcome, reason |
