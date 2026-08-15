# `config.json` format

Detailed spec of the `config.json` format per cell and per hive scope marker. In case of conflict between this file and `meclaw-overview.md`, the overview wins. It is the single source of truth.

> New here? [`README.md`](README.md) is the map of this directory and [`glossary.md`](glossary.md) defines the vocabulary this file assumes.

## Supreme rule

**A cell does not know what happens before or after it.** It knows only its own contract (input/output schema), its params, and the message it is currently processing. It has **no** knowledge of sender paths, receiver paths, hop history, routing strategies, or other cells.

Messages are atomic. Trace reconstruction lives in the central message log in `colony.db` (filterable by path prefix), not in the message.

**Envelope fields are read-only from the cell's perspective.** `id`, `trace_id`, `parent_message_id`, `correlation_id`, `target`, `reply_to`, `ttl`, `created_at` are set exclusively by colony during routing. A cell can neither write them in its content JSON nor manipulate them via an edge (see `meclaw-overview.md` section "Envelope setter authority"). Anyone wanting a reply target other than the sender solves it application-specifically via header-based routing.

**From the cell's perspective, the world is single-threaded.** A `handle()` call runs to completion before the next starts. The cell task pulls sequentially from the mpsc mailbox. Cell code therefore contains no `Mutex`, no `RwLock`, no atomics, no reentrancy defense. The system's parallelism lives outside the cell, see `meclaw-overview.md`, section "Concurrency and parallelism".

## Access

- **Authority**: Only the colony reads and writes `config.json`. The **only writer is instantiation** (exactly once). **Read-once:** the running cell task **never re-reads** `config.json` after startup; `config.json` is the **instantiation snapshot**, not a live document.
- **At instantiation**: colony copies the template, assigns a new UUID v7, stamps the **origin** (`cell.provenance` — template name, template version, instantiation time), resolves the **instance class** (`${ctx.*}`, `${uuid7:*}`) and writes the result into the instance's `config.json`. The **environment class** (`${VAR}`, `${VAR:-default}`) is **not** resolved here: it stays a token in the file and binds late, at every read (see `meclaw-overview.md`, section Variable substitution, and Snapshot versus live-read below). A secret a template references as `${VAR}` therefore never reaches the disk. **The node reference is the filesystem directory name** (the path segment under `{root}`), **not** a `cell.name` field. The `config.json` carries no `name`. When resolving the root chain, the `${...}` substitution wins over the `template.json` template name. Naming collisions with siblings inside the same hive scope are rejected by colony in the single-stage mutation validation, see `meclaw-overview.md` section "Naming collisions".
- **After instantiation**: `config.json` is semantically frozen, the bootstrap snapshot. No one writes into it anymore, neither colony nor the cell itself. **Dynamic cell state** (changed params) lives exclusively in `cell.db`; **colony state** (registry, edge table, `cell_id`, message log, mutations) lives in `colony.db`. After the snapshot, `config.json` carries neither of the two forward (see `meclaw-overview.md` section "Lifecycle of `config.json` and `cell.db`"). The graph of a topology lives centrally in colony's registry and `colony.db`, not in the `config.json` of the hive scope marker (its `params.graph` is only an initial bootstrap hint).
- **Cells do not read `config.json`.** The colony hands the cell the `params` block at startup. Param updates come afterwards via message and are persisted by the cell in its `cell.db` (`config.json` diverges from the live state, by design; cell reset = `cell.db` wipe → cell starts again from the bootstrap state).

## Structure

```json
{
  "cell":        { ... },
  "params":      { ... },
  "contract":    { ... },
  "description": { ... }
}
```

### Block definition (canonical)

Two of the four blocks have fundamentally different authority. This separation is the root of the entire file:

- **`cell` block = colony substrate.** These fields control **how the colony** instantiates, registers, and supervises the cell. They are **never** handed to the cell. The cell sees only its `params` block plus the message it is currently processing. Allowed keys: `id`, `type`, `timeout`, `restart_limit`, `idle_timeout_ms`, `mailbox_size`, `message_timeout`, `provenance` (details in the `cell` table below). A key declared in the `cell` block that is not allowed is a boot error.
- **`params` block = handed 1:1 opaque to the cell.** After `${VAR}`/`${ctx.*}`/`${uuid7:*}` substitution, the colony passes it through to the cell **unchanged** and does **not** interpret its content. Cell-type-specific (each cell type defines its own `params` structure, see `cell-types.md`). **Sole exception:** at the hive scope marker, the colony reads `params.graph` as the initial desired graph (the hive is not an actor, so it does not get a `params` block "handed" to it).

**Only `id` and `type` are immutable.** They identify the node instance and its cell type across the entire lifetime. **Effectiveness rule** for all other fields: changes to `cell` or `params` fields (via new instantiation at the path or a new template) take effect **at the next spawn/wake** of the cell. The running cell task does not re-read `config.json` (see § Access, "Read-once").

**Special case hive scope marker** (`cell.type: "hive"`): only `cell` and `params` are relevant. `params` contains the optional `graph` block (initial desired graph, see `meclaw-overview.md` section "Graph schema") and the optional `ports` list (GH #133, see `cell-types.md` section `hive`), no `dead_letters` override (the `HiveParams` deserializer is `deny_unknown_fields`; the DLQ is always `/colony/dead_letters`). A `contract` block is not evaluated, because hive scope markers are not actors and do not participate in the message flow (see `cell-types.md` section `hive`). In the `cell` block, only `id` and `type` are relevant. `timeout`, `message_timeout`, `idle_timeout_ms`, and `mailbox_size` are ignored (no actor, no mailbox, no `handle()` call). A `description` is allowed, but only serves discovery by builders; `emits_meaning` and `consumes_meaning` are omitted.

### `cell`

| Key | Content |
|---|---|
| `id` | `cell_id` (UUID v7). **Set during the copy operation template → instance**, the **only time** it is written. Instantiation reads it from the freshly written `config.json` and persists it into the **never-deleting `colony.db`**, which from then on is the **authoritative** source of the `cell_id` (`config.json` is only the bootstrap imprint). Afterwards **never reassigned**, not even on reconnect, resume, or reboot. (The re-dedicated `swap_nodes` graph swap pivots edges onto a different implementation with its **own** `id` and leaves the old cell with its `id` preserved but disconnected. It transfers **no** `cell_id`, see `meclaw-overview.md` § Mutation operations.) |
| `type` | Cell type (`hive`, `store`, `llm`, `bash`, `code`, `web_fetch`, `web_search`, `file`, `edit`, `proxy`, `timer`, `mcp`, `harness`, `subcolony`). Together with `id`, the **immutable** part of the `cell` block. |
| `restart_limit` | *(optional)* Maximum restart attempts by the supervisor before the cell is marked as `failed`. Default `5`. See `meclaw-overview.md` section "Restart strategy". |
| `timeout` | Hot/cold mode (see `meclaw-overview.md` section "Hot/cold cell model"): `0` = default (idle-timeout model, Awake↔Asleep), `>0` = one-shot (despawn after each message), `-1` = persistent (typically `proxy`/`timer`/`mcp`, never despawn). Phase-13 activation; before that, all cells are permanently a task. |
| `idle_timeout_ms` | *(optional, from Phase 13)* Idle duration in ms, after which a stateful cell with `cell.timeout: 0` despawns itself (Awake→Asleep). Overrides the colony default from `colony.json` `idle_timeout_default_ms`. Ignored if `cell.timeout != 0` (at `>0`, one-shot despawn after each message takes effect; at `-1`, the cell is persistent and never despawns). |
| `message_timeout` | *(optional)* Substrate backstop per `handle()` call in ms, see `meclaw-overview.md` section "Timeouts" (concept B). Overrides the colony default from `colony.json` `message_timeout_default_ms`. `0` or `-1` = no backstop (for long-running cells). **Not** the primary timeout for I/O operations. `params.external_timeout_ms` (concept A) is responsible for that. `cell.message_timeout` should be considerably more generous than `params.external_timeout_ms`, so that normally A takes effect first. |
| `mailbox_size` | *(optional, from Phase 5)* Bounded-mpsc capacity; overrides the colony default (`colony.json` `mailbox_default_capacity`, default 1000). See overview section "Mailbox size". |
| `provenance` | *(optional, GH #62)* **Instantiation origin stamp** — an object carrying `template` (the resolved template name from `template.json`, **not** the `name@version` reference form), `template_version` (the resolved version; **absent exactly when the template declares none** — "has no version" is a different statement from "version unknown") and `instantiated_at` (unix seconds, the same unit as every `created_at` in `colony.db`). Written **exactly once**, in the same write as the fresh `cell.id`, and never again. **Absent** for every node not born from a template: a hand-written tree, an `adopt` entry (the adopted node keeps its own origin unchanged — adoption does not change where a node came from), and anything instantiated before the field existed. For a **subtree template**, **every** node of the instance — nested cells and hive markers included — carries the **subtree template's** stamp: the subtree template is the unit an update addresses. See § Origin below. |

### `params`

**`max_concurrency`** (*optional, only for stateless cells, from Phase 7*) lives in the **`params`** block, not in the `cell` block: maximum number of concurrently running worker tasks in the stateless-cell dispatcher (see `meclaw-overview.md` section "Stateless-cell dispatcher"). Default: high value (effectively unbounded for typical load paths). Configurable per cell: e.g. `web_fetch` with `32` (HTTP provider rate limits), `file` with `8` (disk I/O), `bash` one-shot with `4` (process resource limit). For stateful and long-running cells the value is ignored.

**`sandbox`** (*optional, from S4 / GH #35, completed in GH #85*) also lives in the **`params`** block, not in the `cell` block: the `cell` key list is closed and describes how the **colony** runs the cell, whereas the sandbox describes the rights with which the **cell** starts its child process, a property of execution just like `external_timeout_ms`. The block is read by the three cell types that start foreign code: **`bash`**, **`code`** and **`harness`**. Every other cell type ignores it.

```json
"params": {
  "sandbox": {
    "trust": "restricted",
    "network": "deny",
    "filesystem": {
      "read":    ["/srv/data"],
      "write":   ["/srv/work"],
      "runtime": true
    }
  }
}
```

| Key | Type | Required | Meaning |
|---|---|---|---|
| `trust` | `"restricted"` \| `"trusted"` | yes | `restricted` = the sandbox is enforced. `trusted` = the explicit escape hatch for local cells, **no** enforcement. |
| `network` | `"deny"` \| `"allow"` | no, default `"deny"` | only under `restricted`. `deny` starts the child in a fresh network namespace (`unshare(CLONE_NEWUSER\|CLONE_NEWNET)`), which holds nothing but a `lo` in state DOWN, so even `127.0.0.1` is out of reach. |
| `filesystem` | object | **yes** under `restricted` | the allowed filesystem view, enforced via Landlock. |
| `filesystem.read` | array of absolute paths | no, default `[]` | readable and executable, recursively. |
| `filesystem.write` | array of absolute paths | no, default `[]` | readable, writable and creatable, recursively. |
| `filesystem.runtime` | bool | no, default `true` | adds the runtime set (see below). |
| `limits` | object | no | resource caps via cgroup v2, only under `restricted` — see below. |
| `limits.memory_max_bytes` | integer > 0 | no | `memory.max` in bytes. Swap is pinned to `0` alongside it: a cap a process can escape into swap is not a cap. |
| `limits.pids_max` | integer > 0 | no | `pids.max` — how many tasks the child and its descendants may hold together. The answer to a fork bomb. |
| `limits.cpu_max_percent` | integer > 0 | no | `cpu.max` as a percentage of **one** core against a fixed 100 ms period, so `200` means two whole cores. |
| `syscalls` | object | no | syscall filter via seccomp-bpf, only under `restricted` — see below. |
| `syscalls.ptrace` | `"deny"` \| `"allow"` | no, default `"deny"` | `ptrace` plus `process_vm_readv`/`process_vm_writev`, the same capability under three names. |
| `syscalls.raw_sockets` | `"deny"` \| `"allow"` | no, default `"deny"` | `AF_PACKET` of any kind and `SOCK_RAW` of any family. An ordinary TCP or UDP socket is untouched. |
| `syscalls.foreign_signals` | `"deny"` \| `"allow"` | no, default `"deny"` | every signal whose target is not the sandboxed process itself. |

**All four key sets are closed** (`sandbox`, `sandbox.filesystem`, `sandbox.limits`, `sandbox.syscalls`): an unknown key is a boot error. A `"netwrok": "deny"` must not pass as "no value given, so use the default". At a security boundary a forgiving parser is the worst property a parser can have.

**Default-deny for template-sourced cells (GH #85, the migration cut).** When a `bash`, `code` or `harness` cell is **instantiated from a template** and declares **no** `params.sandbox`, instantiation writes this block into its `config.json`:

```json
"sandbox": { "trust": "restricted", "network": "deny", "filesystem": { "runtime": true } }
```

**The cut is prospective only.** It applies at instantiation time and only to the node being born, the same shape as the secret cut from GH #20. A tree already on disk keeps running unchanged: there, an absent `sandbox` block still means "no sandbox". What "template-sourced" means is answered by `cell.provenance` (§ `cell`): the stamp is written in the same write as the default, and an `adopt` entry carries neither.

**The default deliberately names no path.** A default that filled in the cell's own directory would bake an absolute host path into the instantiated `config.json`, and an exported tree would then carry a boundary pointing at a directory on somebody else's machine — exactly the failure class GH #20 was opened about. What stays reachable is the runtime set, which is what an interpreter needs to start at all; anything beyond that the template declares itself. The block is visible in the instance `config.json` and therefore editable.

**The escape hatch stays explicit.** A template that needs full rights writes `"sandbox": {"trust": "trusted"}`, nothing is inserted and nothing is enforced, and whoever reads the instance sees the decision.

Recommended baseline for a template that has to write (`<cell-workspace>` is the only directory it should write):

```json
"sandbox": {
  "trust": "restricted",
  "network": "deny",
  "filesystem": { "read": [], "write": ["<cell-workspace>"], "runtime": true }
}
```

**The runtime set** (`filesystem.runtime: true`) grants read and execute on `/usr`, `/lib`, `/lib64`, `/bin`, `/sbin`, `/etc`, `/proc`, `/sys` and read/write on `/dev/null`, `/dev/zero`, `/dev/full`, `/dev/random`, `/dev/urandom`. Without it no interpreter starts, because even the dynamic loader would be unreachable. It is a convenience, **not a security statement**: it contains `/etc` (hence `/etc/passwd`) and `/proc` (hence `/proc/<pid>/cmdline` of other processes of the same user). Set `runtime: false` and enumerate the paths yourself if that is not acceptable.

**Resource caps (`limits`, GH #85).** Enforced through a **delegated sub-cgroup** (cgroup v2) created per child process, filled, entered before the `exec` and removed afterwards — including after a crash and a restart, because the directory name carries the daemon's pid and the next run sweeps away whatever pid no longer exists. A `limits` block that caps **nothing** is a boot error: it would read as "capped" and be no such thing.

The delegated root is looked up in two steps: the topmost writable ancestor of the daemon's own cgroup (which is what delegation looks like for a systemd user service with `Delegate=yes` and inside a container), otherwise `/sys/fs/cgroup/user.slice/user-<uid>.slice/user@<uid>.service`. **Operating requirement, measured:** creating the directory is not enough — *moving* a process additionally requires write access to `cgroup.procs` of the **common ancestor** of source and destination. A daemon started from an ssh login lives in `user-<uid>.slice/session-<n>.scope`, so the common ancestor is the root-owned `user-<uid>.slice` and the move fails with `EACCES`. The same daemon under `systemctl --user` lives below `user@<uid>.service` and is allowed. **Run the daemon as a user unit if you want `limits`.** Otherwise the spawn fails loudly (fail-closed) instead of running uncapped.

Teardown writes `cgroup.kill` first: the sub-cgroup belongs to exactly one child, so anything that outlived it is a leftover — a cap a detached descendant escapes is not a cap.

**Syscall filter (`syscalls`, GH #85).** A seccomp-bpf program assembled in this tree (no new crate, hard rule 6) that closes what Landlock, being a filesystem LSM, does not cover. **Naming the block means "filter this process"**: an axis that is not mentioned is **denied**, and an axis you want to keep open you spell out as `"allow"` and can see that you did. A block in which all three axes are `"allow"` is a boot error, because it would install no filter at all.

A denial is `EPERM`, not a kill: the program sees an ordinary permission error and can report it. Only an architecture the filter was not built for ends the process, because a filter keyed to the wrong syscall table is not a filter.

**The limit of `foreign_signals`, stated plainly:** a BPF program cannot consult the process table, so it compares the target pid against exactly one constant, its own, patched in after the fork. What stays allowed is `kill(self)` and `tgkill(self, tid)`, which is what `raise()` and `abort()` compile down to. Denied are `kill(0, …)` (the own process group — for a cell's child that is the **daemon's** group), `kill(-1, …)` and every foreign pid. The cost: a shell script under this axis cannot end its own background job with `kill $!`. That is a real restriction; the opposite reading, "allow every positive pid", would protect nothing.

**Fail-closed.** A `restricted` profile that cannot be enforced (no Landlock in the kernel, no namespaces on this host, a declared path that does not exist) makes the spawn fail; the cell emits `error_code: "io_error"` with `sandbox not applied: <reason>`. There is no path on which a `restricted` cell quietly keeps running unsandboxed.

**Ask before it hurts: `meclaw --sandbox-probe` (GH #97).** Fail-closed means an unenforceable profile only shows up *in production*, as the `io_error` of a live cell. So that this need not be the first contact, the flag answers the same question up front, about **the host**, without running a cell: it needs no colony root, creates neither `colony.db` nor `log.jsonl`, and **always exits 0** — the report *is* the answer, and a host that can enforce nothing is not a failure of the asking. One line per `params.sandbox` property, a verdict from the closed set `yes` / `no` / `skipped`, then the reason:

```
sandbox probe: which params.sandbox properties this host can enforce
  filesystem  yes      Landlock ABI 4
  network     yes      an unprivileged CLONE_NEWUSER|CLONE_NEWNET child ran
  limits      no       the sub-cgroup was created but moving a child into it was refused
                       (Permission denied (os error 13)). The kernel can do this, the launch
                       cannot: the daemon must run as a systemd user unit (user@<uid>.service);
                       an ssh session scope cannot move processes, because the common ancestor
                       user-<uid>.slice is root-owned
  syscalls    yes      seccomp filter mode is present for this architecture
```

The `limits` line is the reason the flag exists: its answer is **not a property of the kernel but of the launch** (see the operating requirement above). A bare "no" would send the operator hunting for a kernel feature that is already there, so the text separates two cases: an **absent mechanism** ("this host delegates no writable cgroup v2 directory …", which no other launch changes) and a **wrong launch** (`EACCES` at the common ancestor, naming the user-unit requirement).

The same report is appended to **`--validate`** (on stderr, like every other validate diagnostic). There it is **strictly informative** — it never changes the validate verdict: `--validate` checks the tree, enforceability is a question about the machine, and the fail-closed refusal happens at spawn time. Two of the four probes (`network`, `limits`) fork `/bin/sh -c :`; in the validate appendix they run **only when the tree declares a `restricted` profile at all** — otherwise the line reads `skipped` with the reason `no restricted profile in tree`. A `trust: "trusted"` is no cause; an unreadable `sandbox` block is (whoever wrote it wanted enforcement).

**`sandbox` is not runtime-changeable.** The block is read from the birth params only. For `bash` and `code` that holds structurally: both are stateless, have no `cell.db` and therefore no runtime param overlay at all. `harness` has one, and there `sandbox` is listed immutable explicitly: an update touching it is rejected as `Immutable` rather than swallowed as an unknown key. A security boundary that a message can move is not a boundary.

Cell-type-specific. Each cell type defines its own `params` structure (see `cell-types.md`). The colony hands this block to the cell at startup; afterwards param updates via message are possible (last-write-wins, persisted in `cell.db`). **Form** (W4b): the update message carries a **top-level `params` body slot** (1:1 this `params` block, partial), pure cell content, no header gate; the cell merges + persists it itself and replays the overlay at wake/respawn over the birth params (`config.json` stays untouched). Which fields are runtime-changeable or immutable (e.g. credentials, security boundaries) is cell-type-specific (see `cell-types.md`, e.g. `llm` § Runtime param updates).

`${VAR}` substitution from `.env` is performed by the colony before handover to the cell. `${ctx.<key>}` and `${uuid7:<label>}` are resolved at mutation application (see overview section "Variable substitution").

**Convention for I/O cells**: every cell that performs I/O operations of indeterminate duration (HTTP, DB, subprocess, filesystem, MCP calls) declares a `params.external_timeout_ms` field (or a semantically more fitting name like `query_timeout_ms` for `store`). The cell implementation wraps **every** such operation with `tokio::time::timeout` and, on elapsed, emits a regular error message (`header.finish_reason: "error"`, cell-type-specific `error_code` like `provider_timeout` / `query_timeout` / `script_timeout`). This is concept A in `meclaw-overview.md` section "Timeouts", the primary protection, set precisely per operation, manageable by the operator. **`cell.message_timeout`** (in the `cell` block) is the coarse backstop for cell hangs and lies considerably above `external_timeout_ms` (concept B in the same section).

### `contract`

The `contract` keys are organized by **enforcement level**; not all of them are substrate-enforced in v0.1.0:

| Key | Enforcement (v0.1.0) |
|---|---|
| `emits` | **substrate-enforced**: validated always-on at the `code` type (P13/D-017); remaining emitting cell types post-v0.1.0 (see § Schema format and validation; contract validation for the rest is a roadmap defer). |
| `version`, `settings`, `consumes` | **substrate-enforced**: presence + JSON type at config load (boot hard fail; mutation reject `contract_incomplete`). |
| `capabilities` | **discovery-only**: hint for builder composer/audit tools, **no runtime check** until the hardening (see `capabilities` note below). |

**`version` format:** non-empty string, freely choosable (no semver requirement). **`settings` format:** object `{ "<key>": SettingSpec }` (see § SettingSpec), empty object permitted. **`consumes` format:** object (see § consumes), empty object permitted.

Optional: `tools`, `multi_send_capable`.

**Body follows the universal body format**: top-level slots are primarily `system` and `messages[]` (see `meclaw-overview.md` section "Body format (universal)"). Cells may declare their own top-level slots (`meta`, `delta`, `event` etc.). `emits.body` and `consumes.body` declare the slots this cell writes or reads. Unknown top-level slots in an incoming message are ignored by the consumer.

#### `emits`: what the cell writes into its output message

Split into `body` (the actual content) and `hop` (the isolated cell output to routing metadata). Cells emit **only** `hop`. `context` is solely edge authority and does not appear in `emits` (see overview section "Headers vs. body: write model"). The cell produces content JSON; colony interprets `content.header` as `hop` and takes the rest as `message.body`.

```json
"emits": {
  "body": {
    "<key>": <EmitSpec>,
    ...
  },
  "hop": {
    "<key>": <EmitSpec>,
    ...
  }
}
```

**`EmitSpec`**:
```json
{
  "type":     "string|number|boolean|object|array|blob_uuid",
  "values":   ["..."],
  "required": true
}
```

- `values` optional, only sensible for `type: string` (enum whitelist).
- `required` defaults to `true`.

#### `consumes`: what the cell reads from the incoming message

Split into `body` (content slots) and **the two header compartments** `context` (persistent) and `hop` (exactly this hop). Cells read all three read-only; they have **no** knowledge of who set the value when. That is a topology matter. The lifetime of a header is determined **purely structurally** by the compartment name (`context` = persistent, `hop` = hop-local/expires). There is **no** per-key lifetime annotation.

```json
"consumes": {
  "body": {
    "<key>": <ConsumeSpec>,
    ...
  },
  "context": {
    "<key>": <ConsumeSpec>,
    ...
  },
  "hop": {
    "<key>": <ConsumeSpec>,
    ...
  }
}
```

**`ConsumeSpec`**:
```json
{
  "type":     "string|number|boolean|object|array|blob_uuid",
  "required": true
}
```

- If a required value is missing: cell is not called, error message to `reply_to` (if set), otherwise dead letter.
- **Mutation/locality validator**: the build-time validator uses `emits.hop` (what the cell produces) together with `consumes.context` + `consumes.hop` (what the downstream cell expects) to statically check locality and reachability of a header value. A `hop` value is only available at the immediately following hop (unless an edge carries it forward via `set_context`), a `context` value across the entire lifecycle. Hive transits participate in the fan-in intersection: an edge with a hive `from` is a transit pass-through and contributes `set_hop` of this edge ∪ the intersection of the contributions of all inbound edges of the hive (recursively across multi-stage transits, cycle-safe). The same key walk the runtime performs at transit (`hop` expires only at a cell emission, not at the transit). **Participation/status filter at boot:** at bootstrap, the locality checker carries contract obligations **only for active nodes**, nodes that participate in the active graph. A registered but **disconnected/inactive** node (persisted `colony.db` status at reboot **or** island derived as inactive from t0 at first boot) is pure bookkeeping: it is rehydrated (stable `cell_id`), but at boot is subject to **no** contract enforcement. The full check resides at the **mutation moment** that connects it (participation rule + transit-aware intersection). Thus the check is uniform across both boot kinds: inactive ⇒ no boot obligation; active-and-wired ⇒ sharply checked.

**Every key declared in `consumes.body` is mandatory.** There is no optional `consumes.body` field: `validate_consumes` (`crates/meclaw-core/src/contract.rs`) requires every declared key in the incoming body and otherwise reports `required consumes.body '{key}' missing`. **Consequent rule for `/colony` roundtrips:** a `/colony` endpoint answer (`{"mutation":{…}}`, `{"rescan":{"status":"ok"}}`) is **not** a UBF body, it carries no `messages[]` and therefore bounces off every contract that declares `messages`. A cell that runs a `/colony` roundtrip therefore declares an **empty** `consumes.body`.

**Declaration as a capability switch (GH #87):** a declared `consumes.body` key is not only a presence obligation, it can also **unlock a capability**. First case: `consumes.body.attachments`. Only a cell that declares the slot receives the read-only blob-store handle at spawn with which it resolves `attachments[]` refs itself at `handle()` time (`meclaw-overview.md` § "`attachments[]` schema", owner ruling GH #19; consumer detail in `cell-types.md` § `llm`). Without the declaration the handle does not exist, so the cell **cannot** read an attachment rather than merely not doing so. Because declaring is binding, this is a deliberate coupling: whoever wants to read attachments thereby also requires them on every inbound message.

**Enforcement state:** The substrate-side required-`consumes` check runs at the delivery boundary (before `handle()`): missing/type-wrong required key → error message to `reply_to` (`error_code: "consumes_violation"`), otherwise dead letter (same token). **The error reply is delivered DIRECTLY to `reply_to`** (registry lookup via `route()`), not routed via the consumer's out-edges. It is feedback to a known sender, not a routing target (W2b ruling 2026-06-12; see `meclaw-overview.md` § Routing errors "Outputs arm: three disjoint cases", case 2). A catch-all out-edge of the consumer does not redirect the error reply.

#### Schema format and validation

- Schemas follow **JSON Schema Draft 2020-12** (Rust: `jsonschema` crate).
- **`code` = always-on trust boundary (no opt-out):** the `emits` validation of the `code` output runs **unconditionally** (`validate_emits = true`), independent of the build profile **and** of `colony.json` `strict_validation`. `code` is the only user-script-driven output whose correctness does not follow from cell discipline; therefore it is always checked.
- **Remaining emitting cell types:** `emits` validation runs centrally at the colony's outputs arm following the debug-on/`strict_validation` model: in the debug build always active, in the release build per `colony.json` `strict_validation: true|false` (default `false`, schema see `meclaw-overview.md` section "`colony.json` schema").
- **`strict_validation` role:** thus controls **only** the future non-`code` emits validation in the release build. The flag has **no** influence on the always-on `code` path.

**Enforcement state:** `code` always-on (in-cell, two-pass, unchanged); all remaining emitting cell types are validated **centrally at the colony's emission boundary** (outputs arm), flag-gated following the debug-on/`strict_validation` model. **Asymmetry by design:** `code` checks in-cell always-on with all-or-nothing two-pass; the rest runs centrally, flag-gated and per-emission. This is intended and not drift. Violation: emission is discarded; with `input_reply_to` error reply (`error_code: "contract_violation"`), otherwise dead letter (same token). **Two registered boundaries of the central check (debug net, not a trust boundary, ratification 2026-06-10):** (a) error replies to an `input_reply_to` that points to a `/colony/*` endpoint or a hive path are silently discarded (only the cell-path cascade is followed); (b) a cell that emits in the µs window between task spawn and the landing of its `SetNodeContract` entry (self-emitting types at boot) passes the check fail-open (absent entry ⇒ vacuous check).

#### `capabilities`: fixed list

| Capability | Meaning |
|---|---|
| `network:llm` | may contact LLM providers |
| `network:http` | may make arbitrary HTTP calls |
| `network:search` | may contact search providers |
| `network:mcp` | may contact MCP providers |
| `network:proxy` | may contact chat-platform providers |
| `fs:read` | may read the filesystem (within boundary) |
| `fs:write` | may write the filesystem |
| `shell:exec` | may execute shell commands |
| `db:own` | may read/write its own `cell.db` |
| `mutate-graph` | wants to trigger graph mutations (discovery hint, no runtime check until the hardening) |

Extensible as needed, documented centrally in `meclaw-core`.

**Note on permissions until the hardening**: The capabilities in this phase are **discovery hints** for builder composer and audit tools, **not a runtime check**. This applies in particular to `mutate-graph`: whether a cell actually _can_ mutate depends solely on the topology (does an edge to `/colony/mutations` exist?). Post-roadmap hardening may add capability tokens that are checked at runtime. See overview section "Permissions" in the mutation format.

#### `ToolSpec`

Declares which tools the cell offers to its LLM (or external consumers). **Not a routing endpoint**. Where tool calls are routed is decided by the topology.

```json
{
  "name":   "<tool-name>",
  "schema": { ... }
}
```

#### `SettingSpec`

```json
{
  "type":        "string|number|boolean|object|array",
  "secret":      false,
  "default":     "<value>",
  "description": "<text>"
}
```

#### Flags

- `multi_send_capable`: cell can produce multiple output messages from a single input. Activates the cell-type-specific multi-send wire format, for `code` e.g. the JSON-array format on stdout (see `cell-types.md`). Each emitted message runs independently through the outgoing edges; colony evaluates freshly per message. The value comes from `contract.multi_send_capable` (bool, default `false`). The former `params.multi_send_capable` bridge is removed. A `params` value is ignored by the `code` factory.

### `description`

Six keys, **builder-enforced**, not substrate-enforced: the structure takes effect as soon as the builder/composer consumes it (the same discovery contract surface for the LLM builder that writes edges, and reviewer/operator), not as boot validation in the substrate.

| Slot | Content |
|---|---|
| `purpose` | Why does this cell exist? What problem does it solve? (1-2 sentences) |
| `use_when` | When does the composer reach for this template? Preconditions, alternatives. |
| `not_in_scope` | What does this cell deliberately **not** do? Helps the builder exclude the cell when it does not fit. |
| `emits_meaning` | Semantics of the `contract.emits` entries: what do they mean beyond type info? |
| `consumes_meaning` | Semantics of the `contract.consumes` entries. |
| `examples` | Concrete input/output examples; at least one. |

**At hive scope markers** (`cell.type: "hive"`): `description` describes the scope purpose (what does this hive bundle? when does the builder use it? what does not belong in it?). `emits_meaning` and `consumes_meaning` are omitted, since hive scope markers do not participate in the message flow.

## Origin (`cell.provenance`)

An instantiated cell is a **detached copy**: `template.json` is dropped at staging, and the tree can be exported, backed up or moved to another machine. For a template to find its instances later (an app-store update), the node has to carry its origin **itself** — there is no colony to ask when only the tree arrives.

**Two homes, one truth.**

- **The source** is `cell.provenance` in the instance's `config.json`. It travels with the directory; a backup, a copy and an export carry it verbatim.
- **The index** is the three `registry` columns `template`, `template_version`, `instantiated_at` in `colony.db` (schema v5). They answer "which nodes came from `sink-tpl@1.0.0`?" with **one** SQL statement instead of a tree walk.

The index is filled in two places: at instantiation (the mutation knows the template) and **at every boot**, from the `config.json` that was read. The second one is the important one: a config-only copy brings no `colony.db`, so its index starts empty — and without the boot pass it would silently claim "no origin" while the files next to it say otherwise. A node without `cell.provenance` sends nothing and keeps `NULL`; origin is **recorded, never invented**.

**What the stamp is not.** It is not a live binding to the template: the template may change, move or disappear without the instance noticing or changing (§ Access, "After instantiation"). It is also not a reference to a `templates` row — it names the name and the version, not the `template_id`. (Since GH #62 the `template_id` is **stable** across rescans, but it remains a colony-local surrogate key and is meaningless in an exported tree.)

## Snapshot versus live-read

Not every artifact under `{root}` is read again when a colony boots. Knowing which is which decides what a backup has to move and what a restore actually restores.

**Live-read at every boot.** `config.json` is re-read and re-parsed on every boot, and `${VAR}` tokens are substituted in memory against `{root}/.env` each time. The on-disk file is never rewritten at boot -- instantiation is its only writer. `.env` is therefore live: changing a value and rebooting changes the effective params. Instantiation does not freeze the environment class either: `${VAR}` and `${VAR:-default}` survive the write **literally**, so an exported tree carries the tokens and not the values -- secrets stay in `.env`. The escaped form `$${VAR}` survives instantiation unchanged too and becomes the literal text `${VAR}` when read; it binds to nothing. The price of that late binding is a standing dependency: a `${VAR}` with neither value nor default fails the boot loudly (`env_var_missing`, naming the variable) instead of quietly yielding an empty value.

**Birth snapshot in `cell.db`.** A cell's `cell.db` is created at its first spawn and seeded exactly once, on `OpenStatus::Created`. `timer` copies `params.schedules` and `store` loads `seed/*.jsonl` at that moment and never again. After the first boot, `cell.db` is the truth: editing `params.schedules` in `config.json` changes neither an existing schedule nor adds a new one. The runtime params overlay (`cell.db` `params` table) likewise wins over the `config.json` birth params for the keys it holds. To make a `config.json` edit of a birth-snapshot field take effect, the cell must start without a `cell.db`.

**Birth snapshot in `colony.db`.** A node's `cell_id` is minted once and carried in the `registry` table, stable across reboots. `params.graph.edges` and hive scopes are applied on the first boot and persisted; on a reboot they are hydrated from `colony.db` and the `params.graph` hints are ignored. On a reboot, a cell directory absent from the registry is reported but never adopted — registration happens only through instantiation or mutation. The `templates` index is a scan snapshot holding absolute filesystem paths; a boot re-scans only when the table is empty, otherwise an explicit `--rescan-templates` is required.

**Consequences for backup and restore.** The restore unit of a cell is its directory, not its `config.json`: restoring the config restores the declaration, never the birth-snapshot state. The restore unit of a colony is `{root}` as a whole, including `blobs/` and the SQLite WAL sidecars `*.db-wal` / `*.db-shm` — a backup that matches `*.db` alone can restore a colony that boots cleanly and runs the state from before the last writes. A restored tree whose `colony.db` came along keeps its identity; a config-only copy is a new colony with re-minted `cell_id`s. After relocating a tree, run `--rescan-templates` so the template index points at the new root, and `--validate --validate-strict` to surface cell directories the restored registry does not know.
