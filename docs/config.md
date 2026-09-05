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
- **On a runtime registration (`add_templates`, GH #440)**: the `config.json` a declaration entry brings along is written into the instance-local library **byte for byte** as it stood in the body — registering does **not** substitute it. `${ctx.*}`, `${uuid7:*}` and `${VAR}` stay put and bind where they always bind: the instance class at **instantiation**, the environment class at **read** time. Registering files a class; only the `add_nodes` that names it turns that into an instance. The same separation as everywhere else — otherwise a library blueprint would carry the `ctx` of whichever mutation happened to deliver it.
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

- **`cell` block = colony substrate.** These fields control **how the colony** instantiates, registers, and supervises the cell. They are **never** handed to the cell. The cell sees only its `params` block plus the message it is currently processing. Allowed keys: `id`, `type`, `timeout`, `restart_limit`, `idle_timeout_ms`, `mailbox_size`, `message_timeout`, `provenance` (details in the `cell` table below) plus `template` — in a `ref` marker (§ Special case template reference): the subtree parser reads it inside a template, and it never stands in an **instantiated** `config.json` because the reference is already resolved by then. **In the root tree it is a declaration since GH #424**: on a **first** boot the colony resolves it and grows what it names (§ Startup algorithm, step 3a); on a **reboot** it is an unresolved reference in an already grown tree and is reported, never grown (A5b). A reference the first boot cannot fulfil stays a hard boot error and names both halves: the reference, and the directory the marker stands in. **A key declared in the `cell` block that is not on that list is a hard refusal — on every path that reads the block.** **Correction (GH #353), retracting the GH #277 correction:** that earlier note said this sentence was false and that an unknown key was silently ignored everywhere. It was half wrong when it was written, and it is obsolete now. Half wrong, because the boot *did* refuse an unknown `cell` key — through a hand-maintained allow-list inside `bootstrap.rs` that no other reader consulted. So the refusal existed at boot and was **missing on the mutation/staging path**: the very template a boot would have refused was staged through a mutation without a word, and the typo took effect as a default. Obsolete, because the list now lives on `CellHeader` itself (`#[serde(deny_unknown_fields)]`, `crates/meclaw-colony/src/config.rs`), which every read path deserializes — one list, one refusal, both paths. "Every read path" includes **every node of a multi-cell (subtree) template**: the subtree parser (`mutation/subtree.rs`) runs each node's `cell` block through the same barrier before it hands it on. Otherwise a mutation could commit a tree that the next restart refuses. At boot it is a `BootstrapError::InvalidJson` naming the offending key and the `config.json` it stands in; on the mutation path a normal pre-destructive validation refusal (`error_code: "schema"`), naming the same two. **Breaking:** a tree carrying an extra `cell` key — a typo, or a key someone used as a comment slot — stops booting until the key is removed. It never did anything; now it says so. **Addendum (GH #383):** `surface` stood on this list until then. The key is **removed** — not renamed, not moved — and now falls under exactly this sentence: `cell.surface` is an unknown key and therefore a hard refusal. The entry in the `cell` table below names the reason and says where a reader points instead (the `web` cell, `templates/canvy/MIGRATION.md`).
- **`params` block = handed 1:1 opaque to the cell.** After `${VAR}`/`${ctx.*}`/`${uuid7:*}` substitution, the colony passes it through to the cell **unchanged** and does **not** interpret its content. Cell-type-specific (each cell type defines its own `params` structure, see `cell-types.md`). **Sole exception:** at the hive scope marker, the colony reads `params.graph` as the initial desired graph (the hive is not an actor, so it does not get a `params` block "handed" to it).

**Only `id` and `type` are immutable.** They identify the node instance and its cell type across the entire lifetime. **Effectiveness rule** for all other fields: changes to `cell` or `params` fields (via new instantiation at the path or a new template) take effect **at the next spawn/wake** of the cell. The running cell task does not re-read `config.json` (see § Access, "Read-once").

**Special case hive scope marker** (`cell.type: "hive"`): only `cell` and `params` are relevant. `params` contains the optional `graph` block (initial desired graph, see `meclaw-overview.md` section "Graph schema") and the optional `ports` list (GH #133, see `cell-types.md` section `hive`) — whose entries come in **two** forms: the short name of a direct child as a string, or the **slot form** as an object (`{"name": "gen", "slot": true, "unbound": "park" | "drop" | "error"}`, GH #285). A slot is an address that may stand empty: `slot` and `unbound` are both mandatory, and the declaration buys exactly two exemptions — an edge onto the slot is no dangling endpoint at boot, and a mutation may wire it with `add_edges` before it is filled. A path that is not declared as a slot and has no occupant stays exactly what it is today: a hard error under `--validate-strict`. `unbound` says what happens to a message that reaches the unbound slot **over an edge** — `drop` discards it silently, `error` produces a `slot_unbound` dead letter, `park` holds it FIFO until the binding (`colony.json slot_park_max`, default 64; the newest arrival above the bound is refused as `slot_park_overflow`, and a shutdown discards the queue). A message that addresses the slot path directly from outside does not reach the declaration and stays `unresolved_path`. A slot is a valid `add_edges` endpoint but never a `remove_nodes` or `swap_nodes[].match` target; emptying it and rewiring it in the **same** diff commits, because the declaration outlives its occupant. Slots belong in a hive below the root — the root scope has no port boundary, and a slot declared there buys no exemption. Full description: `cell-types.md` § `hive`, **Slots**. Then there is the optional `required_drains` list (GH #147/#237: `{port, hop, because}` — which ports may only be wired from outside once their paired egress is consumed outside the hive — or, for a sealed hive, `{accepts, emits, because}`: a caller that sends this lane must subscribe to that one), no `dead_letters` override (the `HiveParams` deserializer is `deny_unknown_fields`; the DLQ is always `/colony/dead_letters`). Since GH #173 there is also the optional `params.contract` block — the hive's machine-readable **contract**, see § `params.contract` (hive). The **top-level** `contract` block stays unevaluated on a hive: that key belongs to the cell and carries a different shape there (`version`/`settings`/`consumes`/`emits`, where `emits` is an `EmitSpec` map per output). One word cannot carry two shapes, and the hive's wiring surface lives in `params` anyway (`graph`, `ports`, `required_drains`) — so the contract joined them. In the `cell` block, only `id` and `type` are relevant. `timeout`, `message_timeout`, `idle_timeout_ms`, and `mailbox_size` are ignored (no actor, no mailbox, no `handle()` call). A `description` is allowed, but only serves discovery by builders; `emits_meaning` and `consumes_meaning` are omitted. "Ignored" does **not** mean "anything goes": the closed key list of the `cell` block (§ Block definition) applies here too since GH #353 — an unknown key in a hive scope marker's `cell` block is the same hard error as on a cell, on every path that reads the marker, `move_nodes` included.

**Special case template reference** (`cell.type: "ref"`, GH #277): a directory that describes no cell but **places another template at this position**. **Inside a template** it carries a `config.json` and **nothing else** — any further file next to it would give one address two sources and is refused at parse time. **In the root tree it is different** (GH #424): there a marker may have **subdirectories** beside it, and those are deeper declarations — they address something else, not the same thing twice. The first boot grows the marker and **merges** into it: what the template brings and what the operator wrote beside it stand side by side. If the two collide on the **same** address, that is a named refusal, raised **before** any rename. `cell.template` is a template reference in exactly the form `TemplatesRegistry::resolve` accepts: `<name>` or `<name>@<version>`; SemVer ranges (`^`, `~`) exist here as little as anywhere else (`version.rs` keeps them post-roadmap, see `meclaw-overview.md` § Resolution `name@version`). `override_params` sits **top-level next to `cell`** — not inside the `cell` block (whose key list is closed) and not in `params`, which a `ref` does not have. It is optional, addresses the cells **of the referenced template** by their paths inside that template (`""` is its root), and sits in the layering **below** the mutation's own `override_params`: the reference sets the default, the caller overrides it key by key. A key that names no cell of the referenced template is an error and not a silent no-op — the message lists the cells that do exist. **The boot checks the `requires` declaration pre-destructively** (GH #465): `bootstrap_grow::grow_one` calls `validate_requires` — the very function `/colony/mutations` runs as its stage 3 — **before** `stage_subtree`. A marker naming a template whose `requires.ctx`/`requires.env` keys the colony does not hold is refused as `requirement_missing` **before a single byte is written**: nothing is staged, and the marker is still a marker. Until then the boot was the one instantiating path that did not read the declaration — the gap surfaced at the first turn, or, for a key with an empty default, not at all.

The whole file of a `ref` directory, with a default for the referenced template's root cell:

```json
{
  "cell": {
    "type": "ref",
    "template": "dispatcher@1.2.0"
  },
  "override_params": {
    "": { "external_timeout_ms": 30000 }
  }
}
```

**Birth state (GH #437).** Beside `override_params` — and top-level for the same
reason: the `cell` block has a closed key list and describes a *cell*, while a birth
state describes an *instantiation order* — a `ref` marker may declare `birth`:

```json
{
  "cell": {
    "type": "ref",
    "template": "unit@1.0.0"
  },
  "birth": "inactive"
}
```

The values are the same two as for `add_nodes[].birth`: `"active"` (the default) and
`"inactive"`. The declaration holds for **every** cell of the grown tree (a unit is
born whole); the tree is then registered, addressable and persisted inactive, and
nothing inside it runs until the ordinary reconnect wakes it
(`docs/meclaw-overview.md` § Reconnect) — that is, until a mutation **addresses** one of
its nodes itself. The declaration is durable (GH #491) and survives every restart and
every mutation elsewhere in the tree. An unknown value refuses the boot — a boot
that cannot fulfil a declaration must not start half a tree.

**The type is resolved at instantiation and never reaches disk.** What lands in the instance is the referenced template's content at the reference's position — there is no `ref` cell factory, no dispatcher path and no registry entry for one; `ref` is a template-time type, not a runtime type (see `cell-types.md` § Overview). At a resolved reference the referenced template root's `README.md` is dropped **together with** `template.json`: the two are the descriptor pair of a standalone template — its registry entry and its page — and neither of them belongs to the instance the reference places. The composite's own `README.md` is untouched (nothing was followed to reach it), so the instance is byte-identical to the copies the reference replaced. A ring of references is `template_ref_cycle` and renders the ring (`a@1.0.0 -> b@1.0.0 -> a@1.0.0`); a reference pointing at nothing is `template_missing` and names the versions the registry holds under that name (or `none`).

### `cell`

| Key | Content |
|---|---|
| `id` | `cell_id` (UUID v7). **Set during the copy operation template → instance**, the **only time** it is written. Instantiation reads it from the freshly written `config.json` and persists it into the **never-deleting `colony.db`**, which from then on is the **authoritative** source of the `cell_id` (`config.json` is only the bootstrap imprint). Afterwards **never reassigned**, not even on reconnect, resume, or reboot. (The re-dedicated `swap_nodes` graph swap pivots edges onto a different implementation with its **own** `id` and leaves the old cell with its `id` preserved but disconnected. It transfers **no** `cell_id`, see `meclaw-overview.md` § Mutation operations.) |
| `type` | Cell type (`hive`, `store`, `llm`, `bash`, `code`, `web_fetch`, `web_search`, `file`, `edit`, `proxy`, `timer`, `mcp`, `harness`, `subcolony`, `vault`, `web`). Together with `id`, the **immutable** part of the `cell` block. Plus `ref` — **never a runtime type** (see § Special case template reference above): it is resolved at instantiation and stands in no instantiated `config.json`. In the **root tree** it is a declaration the first boot fulfils (GH #424). |
| `restart_limit` | *(optional)* Maximum restart attempts by the supervisor before the cell is marked as `failed`. Default `5`. See `meclaw-overview.md` section "Restart strategy". |
| `timeout` | Hot/cold mode (see `meclaw-overview.md` section "Hot/cold cell model"): `0` = default (idle-timeout model, Awake↔Asleep), `>0` = one-shot (despawn after each message), `-1` = persistent (typically `proxy`/`timer`/`mcp`, never despawn). Phase-13 activation; before that, all cells are permanently a task. |
| `idle_timeout_ms` | *(optional, from Phase 13)* Idle duration in ms, after which a stateful cell with `cell.timeout: 0` despawns itself (Awake→Asleep). Overrides the colony default from `colony.json` `idle_timeout_default_ms`. Ignored if `cell.timeout != 0` (at `>0`, one-shot despawn after each message takes effect; at `-1`, the cell is persistent and never despawns). |
| `message_timeout` | *(optional)* Substrate backstop per `handle()` call in ms, see `meclaw-overview.md` section "Timeouts" (concept B). Overrides the colony default from `colony.json` `message_timeout_default_ms`. `0` or `-1` = no backstop (for long-running cells). **Not** the primary timeout for I/O operations. `params.external_timeout_ms` (concept A) is responsible for that. `cell.message_timeout` should be considerably more generous than `params.external_timeout_ms`, so that normally A takes effect first. |
| `mailbox_size` | *(optional, from Phase 5)* Bounded-mpsc capacity; overrides the colony default (`colony.json` `mailbox_default_capacity`, default 1000). See overview section "Mailbox size". |
| `provenance` | *(optional, GH #62)* **Instantiation origin stamp** — an object carrying `template` (the resolved template name from `template.json`, **not** the `name@version` reference form), `template_version` (the resolved version; **absent exactly when the template declares none** — "has no version" is a different statement from "version unknown") and `instantiated_at` (unix seconds, the same unit as every `created_at` in `colony.db`). Written **exactly once**, in the same write as the fresh `cell.id`, and never again. **Absent** for every node not born from a template: a hand-written tree, an `adopt` entry (the adopted node keeps its own origin unchanged — adoption does not change where a node came from), and anything instantiated before the field existed. **Correction (GH #277):** this said that for a **subtree template**, **every** node of the instance — nested cells and hive markers included — carries the **subtree template's** stamp; that is retracted. Every node carries the stamp of **the template it is an instance of** — a node that came in through a `cell.type: "ref"` sub-unit names the **referenced** template, not the composite above it. The additional key `template_chain` names the composites that placed it: an array of two-element arrays `[name, version]`, **outermost first**, the node's own template as the **last** element (`[["outer","1.0.0"],["inner","1.0.0"]]`), with `version` `null` when the template declares none. `template` and `template_version` are the projection of that last element; an instance of a ref-free template carries a one-element chain. The subtree template remains the unit an update addresses — the chain is how an update finds its instances. See § Origin below. |
| ~~`surface`~~ | **REMOVED (GH #383), was GH #159.** The key declared that a cell may be served under `/surface/<cell-path>` by the HTTP API — with `title`, `assets` and `boot_hint`. **What is retracted is the whole statement, not just the spelling:** the `/surface/*` route, the parser (`meclaw_colony::surface`) and the serving path in `--api` no longer exist. `cell.surface` is therefore an **unknown key** and falls under the closed key list above — a **hard boot refusal** naming the key and the file, on every read path (`BootstrapError::InvalidJson` at boot, `error_code: "schema"` on the mutation path). That is Breaking, and loud on purpose: a tree still carrying the key was served by a route that is gone, and ignoring it silently would let it boot into a colony where nothing answers. **Where to point instead:** a display is a cell of its own now, of type `web` (`templates/web`), owning its own port — `params.port`, no shared prefix, no declaration in the `cell` block. Migrating a 1.x canvas: `templates/canvy/MIGRATION.md`. Details: `cell-types.md` § `web`. |

### `params`

**`max_concurrency`** (*optional, only for stateless cells, from Phase 7*) lives in the **`params`** block, not in the `cell` block: maximum number of concurrently running worker tasks in the stateless-cell dispatcher (see `meclaw-overview.md` section "Stateless-cell dispatcher"). Default: high value (effectively unbounded for typical load paths). Configurable per cell: e.g. `web_fetch` with `32` (HTTP provider rate limits), `file` with `8` (disk I/O), `bash` one-shot with `4` (process resource limit). For stateful and long-running cells the value is ignored.

**`sandbox`** (*optional, from S4 / GH #35, completed in GH #85*) also lives in the **`params`** block, not in the `cell` block: the `cell` key list is closed and describes how the **colony** runs the cell, whereas the sandbox describes the rights with which the **cell** starts its child process, a property of execution just like `external_timeout_ms`. The block is read by the four cell types that start foreign code: **`bash`**, **`code`**, **`harness`** and **`mcp`** — the last since GH #96, with the same schema and the same parser (`crates/meclaw-cells/src/mcp/params.rs`). One difference remains, and it is deliberate: instantiation injects a default profile only for `bash`, `code` and `harness`; an `mcp` child without a declaration of its own keeps the rights of the daemon (GH #96, pinned by `crates/meclaw-cells/tests/gh96_mcp_sandbox_profile.rs`). Every other cell type ignores the block.

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
| `network` | `"deny"` \| `"allow"` | no, default `"deny"` | only under `restricted`. `deny` starts the child in a fresh network namespace (`unshare(CLONE_NEWUSER\|CLONE_NEWNET)`), which holds nothing but a `lo` in state DOWN, so even `127.0.0.1` is out of reach. `allow` leaves it in the daemon's network **and** puts the resolver configuration into the Landlock view (see below). |
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

**What `network: "allow"` additionally grants — name resolution (GH #144).** `/etc/resolv.conf` is inside the runtime set, but on a systemd-resolved host it is a **symlink** into `/run/systemd/resolve/`, and `/run` was in no set at all. An `allow` therefore opened the sockets and let every lookup die in `getaddrinfo` — measured: 953 of 953 embedding calls "endpoint unreachable" at `exit_code: 0`. Under `network: "allow"` Landlock now also grants read access to the **target** of `/etc/resolv.conf`: the resolved directory (`/run/systemd/resolve`, `/run/resolvconf`, … depending on the host), or the file itself when it sits directly in `/etc`. A boundary that promises a capability and then withholds it is worse than one that refuses it.

The grant rides on `network: "allow"` and on nothing else. Under `deny` the child sits in a fresh network namespace and has nothing to resolve for, so the path stays out. Pinned in `crates/meclaw-cells/tests/gh144_network_allow_resolves_names.rs`.

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

**`params.graph` (hive only) — the fields of an edge.** The hive scope marker is
the one place where the colony reads into `params`: `graph.edges[]` is the
initial target graph of its subtree (full form and semantics:
`meclaw-overview.md` § Graph schema and § Edge model). `from` and `to` are
mandatory; four optional fields join them:

| Key | Type | Default | Meaning |
|---|---|---|---|
| `condition` | CEL boolean as a string | `null` (= always matches) | decides whether the edge is responsible for this message; reads `context.*` and `hop.*`. A number out of a compartment binds as `int` (or `uint` above `i64::MAX`, `double` otherwise), so `hop.http_status == 200` is true on a message carrying 200 — GH #500, detail in `meclaw-overview.md` § Edge expression language. |
| `modifier` | object | `null` (= identity) | `set_context`/`delete_context`/`set_hop`/`delete_hop`/`restore_ttl` — the edge's sole header authority. |
| `default` | boolean | `false` | GH #283, since **v0.18.0**: `true` makes the edge a **default edge** — it is consulted only after no regular out-edge of the same sender fired, which makes it the declared consumer for what would otherwise dead-letter as `no_route` (or `hive_no_route`). It may carry a `condition` as well; without one the colony boots with a hint in its advisories, never with a refusal. |
| `lane` | string | `null` | GH #559: the name of the lane this edge runs — the declaration that turns a multi-segment deep edge into a **v-lane**. With the key absent the edge is an ordinary one and keeps exactly today's behaviour. Named rather than guessed: no validator reads the lane reliably out of a CEL guard. What the declaration permits and what it demands (connect point, mandatory hops, the three `error_code` strings) is in `meclaw-overview.md` § v-lanes. |

An edge in a mutation diff (`add_edges[]`) carries the same fields, and
`remove_edges[].match` can pattern on `condition`, `modifier` and `default` —
with `default` absent there the routing phase is unconstrained and the pattern
hits both (§ Mutation format). `lane` is **not** a match term: a v-lane is named
by the same terms as any other edge.

In both usages `from`/`to` are paths relative to the scope the declaration sits in,
and **`.` names that scope itself** — at boot the hive whose `config.json` carries
the `params.graph`, in a mutation diff the `scope` of the declaration. Since GH #487
that holds in both; before it, at boot only, although it is the spelling the
`{"from": "."}` doors below use.

### `params.contract` (hive only) — the contract, in lanes

GH #173. A template is a **class**: instantiate it, wire to its interface, swap
it later for another implementation with a different inside. For hive templates
none of that held. `contract` was a CELL property; a hive had `description`
prose, and the prose named cells three levels down ("Ingress: `./keeper/stamp`").
So every instantiation wrote the template's internal layout into the caller's own
topology.

**The contract is not an optional leaflet; it is the form in which a hive meets
the binding boundary rule** (`meclaw-overview.en.md` § The hive boundary — the
rule holds for all hives and all templates). Three requirements from there land
exactly here in the file:

1. **The address is the hive** — hence `"ports": []` next to the contract.
2. **A lane is named functionally** — hence `accepts[].route` and `emits[].route`
   are requests, not places (see "How a lane is named" below).
3. **The inner edge is the only place structure may be known** — hence the
   mapping from lane to cell lives in the `{"from": "."}` edges of `params.graph`,
   and nowhere else.

`params.contract` says the same thing in the only vocabulary that survives a
reimplementation — **lanes**, i.e. `hop.route` values:

```json
"params": {
  "ports": [],
  "contract": {
    "accepts": [
      {"route": "in_batch",
       "context": ["session_id"],
       "because": "one closed session as a single write batch"}
    ],
    "emits": [
      {"route": "episode",
       "because": "one message per turn of the batch"}
    ]
  },
  "graph": { "edges": [ … ] }
}
```

Read as: *send me a message at MY path whose `hop.route` is `in_batch`, and I
will hand you back messages at MY path whose `hop.route` is `episode`.* No cell
of the hive appears anywhere — which is what makes the inside free to change.

| Key | Content |
|---|---|
| `accepts[]` | Lanes a caller may send **into** the hive path. |
| `emits[]` | Lanes the hive sends back **out** through its own path. |
| `…[].route` | The `hop.route` value that **is** the lane. Never a cell name — the whole abstraction rests on this. |
| `…[].context` | `context` keys a caller must have promoted beforehand. **A requirement, checked** (see below). |
| `…[].at` | A list of scope-relative paths: **where** this lane connects at this hive — the connect point of a v-lane (GH #559). Optional; always a `./…` path strictly below the declaring hive — `"."` matches nothing. |
| `…[].because` | What the lane is for, in the hive's own words. Travels verbatim into a rejection. |

**`at` — the connect point of a v-lane** (GH #559). A v-lane is an edge that
skips levels and names its lane explicitly (`meclaw-overview.md` § v-lanes). `at`
is the half of it that belongs to the **target**: the permission to connect deep
on this lane, and the statement of where. Without `at` there is no connect point
for that lane — a v-lane onto it is refused with `v_lane_no_connect_point`, and a
sealed level in between refuses as before with `hive_port_boundary`. A crossed
level that declares the lane **without** a matching `at` is a mandatory hop and
may not be skipped (`v_lane_mandatory_hop`).

**A lane with `at` is not a lane of the rim** (GH #562). It docks below the hive
path by declaration, so two rules that are about rim traffic step aside for it:
the hive owes it **no door** out of `.` and no exit back into it — its door is
the connect point, and the lane-door check (`hive_contract::check_lane_doors`)
skips an entry that names one; the obligation is REPLACED rather than dropped,
because an `add_edges` entry that states the lane INTO the hive path is refused
`hive_contract` and told which connect point to end on instead — and a **parent
does not carry it** in the union of its occupants' lanes
(`development-rules.md` § 8b). Everything else is
unchanged: the connect points are policed per edge at mutation time, and a lane
without `at` is judged exactly as it always was. A connect point counts from instantiation
on: when the same mutation brings the hive into existence, its contract is read
out of the template's staged subtree — `ref` markers resolved, so it is found in
an occupant that comes from a different template too (GH #567).

```json
"contract": {
  "accepts": [
    {"route": "in_pack",
     "at": ["./talky", "./cogny"],
     "because": "the identity pack reaches both brains of this generation directly"}
  ]
}
```

Read as: *whoever sends me `in_pack` may draw the edge as far as my occupants
`talky` and `cogny` — and to no other.* The rest of the inside stays as hidden as
it was: `at` names exactly the points the contract opens for this one lane, and
the enumeration is the boundary rather than an example.

**How a lane is named.** A lane name **must** say what the caller wants, never
where it lands inside. This is the half of the boundary rule that `ports: []`
alone does not state — a port that becomes a lane of the same name is the same
interior cell name in a different field:

| instead of (structural) | functional | because |
|---|---|---|
| `writer` | `in_episode` | the caller hands over a turn; whether a cell called `writer` receives it is the hive's business |
| `recall` | `in_query` | what is asked for is memory, not a recall cell |
| `render` | `in_view` | what is wanted is a picture, not the invocation of a renderer |
| `policy` | `in_decide` | what is requested is a decision, not the route to one |

The test: **does the name survive a reimplementation of the inside?** If a rebuild
that breaks no promise makes the lane name wrong, the name was structural.

**Enforcement levels:**

| Check | When | Effect |
|---|---|---|
| An `add_edges` edge onto the hive path whose `set_hop.route` is **constant** must name an `accepts` lane | mutation | reject `hive_contract`, pre-destructive |
| Every `accepts` lane must route **inward** from the hive path (have a door) | mutation (post-state) | reject `hive_contract`, rollback |
| Every `emits` lane must route **outward** through the hive path from **some** interior cell — carried out, or produced by the door itself | mutation (post-state) | reject `hive_contract`, rollback |
| the same two checks | boot | `warn!` only — the birth topology is sovereign (as with GH #133/#147) |
| An edge naming an `accepts` lane **constantly** must carry that lane's `accepts[].context` keys — on the edge itself (`set_context`) or reachable backwards from its `from` | mutation (post-state) | reject `hive_contract`, pre-destructive |
| the same check | boot | report only — `warn!` per finding, `--validate --validate-strict` turns it into an error |

The check runs the **real router** (`apply_edges`) rather than comparing
condition strings: the migrated templates open a whole family of lanes with a
single `hop.route.startsWith('in_')`, and no text comparison finds `in_batch` in
that. The caller's stamped route is read the same way — the edge's own
`set_hop.route` expression is compiled and evaluated against **empty** headers. A
literal (`'in_batch'`) yields a string; anything reading the incoming message
(`hop.upstream_route`) fails and is skipped. **A check that cannot place an edge
must never reject it.**

**An exit may also CREATE the lane** (GH #176). A probe carrying only `hop.route`
finds exits that already *carry* the lane, and nothing else. A hive's failure
lane does not work that way: the door recognises something only the inside knows
— an `llm` cell's `hop.finish_reason` — and **translates** it into a lane on the
way out, which is exactly what the boundary is for. So the exit check also reads
the door's own `set_hop.route`: if it names the declared lane as a constant and
the edge crosses the hive path, that is an exit for the lane, even where its
condition is unreachable for a route probe. The condition is deliberately **not**
evaluated: whether the door ever fires is a statement about the messages the
inside produces, whether it names the lane is a statement about the door. A door
that names a **different** lane is no exit for this one, and a door that names
the lane on an edge that stays **inside** is not either — a caller cannot receive
what never crosses the boundary. If the expression is computed rather than
constant, the sentence above applies again: unplaceable is not rejectable.

**What is deliberately not checked:**

- **A hive with no edge at its path.** A contract is a statement about the hive
  *path*; if no edge touches that path, the hive is an island — freshly
  instantiated, or disconnected by `remove_nodes` — and its contract is dormant.
  Without this exception a contracted hive could not be removed at all.
- **`accepts[].context` — no longer.** This stood here until GH #291 and is
  retracted: the key **is** checked (table above). Two cases stay unchecked,
  both out of the same conservatism as the rest of `hive_contract`: an edge
  whose route is **computed** rather than stated — which lane it means is
  knowable only once a message exists, and what cannot be placed must not be
  rejected; and an edge whose caller side is a **hive path with no inbound
  edge** — nothing can be delivered there, so the requirement is **dormant**
  until one inbound edge lifts it.
- **How a lane is named.** `writer` is as valid a string as `in_episode`; no
  validator can see whether a name was chosen functionally or structurally. The
  requirement binds regardless — it rests on a reader, not on a check.
- **A caller's subscription condition.** The shipped topologies tell some lanes
  apart by a **second** hop key (`hop.round_capped`, and since `collector@3.5.0`
  `hop.partial` beside it — the first is raised by either cap, the byte one and
  the iteration one alike; the second only by the iteration cap that ended the
  round) that a route-only probe does not carry — that check would refuse
  correct wirings, so it does not exist.

**Relation to `params.ports`.** A port is the name of a lane, not the address of
a cell (`meclaw-overview.en.md` § The hive boundary). `ports: []` **and** a
`contract` are the two halves of **one** target shape, and every hive **is meant
to** arrive there: the address is the hive path, the lane is the port, and the
inside is nobody's business. A hive whose `ports` are still interior cell names
(`canvy`, `memory-hive`, `access`, `argus` — migration in GH #197) has not
finished getting there: it may carry a contract already, but a caller still has to
know a cell name. A hive with **no** `ports` key has not started — that is the
unsealed state, not an exemption from the rule.

### `contract`

The `contract` keys are organized by **enforcement level**; not all of them are substrate-enforced in v0.1.0:

| Key | Enforcement (v0.1.0) |
|---|---|
| `emits` | **substrate-enforced**: validated always-on at the `code` type (P13/D-017); remaining emitting cell types post-v0.1.0 (see § Schema format and validation; contract validation for the rest is a roadmap defer). |
| `version`, `settings`, `consumes` | **substrate-enforced**: presence + JSON type at config load (boot hard fail; mutation reject `contract_incomplete`). |
| `capabilities` | **discovery-only** *(specified, not built — see GH #254)*: hint for builder composer/audit tools, **no runtime check** until the hardening (see `capabilities` note below). Not merely unchecked but unread: `ContractBlock` (`crates/meclaw-colony/src/config.rs`) has no such field, the key is dropped silently at config load, and no API exposes it. |
| `write_surface` | **substrate-enforced, opt-in** (GH #260) — `"internal"` bounds the writes the substrate answers before `handle()` to the cell's parent scope; an absent key means `"open"`, i.e. no effect (see below). |
| `transfer` | **substrate-enforced, opt-in** (GH #314) — `"none"` exempts this cell's `cell.db` from the `transfer` body slot, export as well as import; an absent key means `"all"`, i.e. no effect (see below). |

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
  "type":        "string|number|boolean|object|array|blob_uuid",
  "values":      ["..."],
  "required":    true,
  "description": "..."
}
```

- `values` optional, only sensible for `type: string` (enum whitelist).
- `required` defaults to `true`.
- `description` optional, one sentence: **what** this slot means, for whoever reads the contract. The deserializer is not `deny_unknown_fields` — the key was always permitted and merely never written down; nothing evaluates it, it is documentation at the place of declaration. Worth it where the name alone does not carry: a cell-specific top-level body slot (`recall_diagnostic` in `memory-hive`'s `recall`) or a `hop` marker whose ABSENCE is the statement (`recall_empty`). An explanation of that length belongs here rather than in a comment JSON does not have.

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
- **Mutation/locality validator**: the build-time validator uses `emits.hop` (what the cell produces) together with `consumes.context` + `consumes.hop` (what the downstream cell expects) to statically check locality and reachability of a header value. A `hop` value is only available at the immediately following hop (unless an edge carries it forward via `set_context`), a `context` value across the entire lifecycle. Hive transits participate in the fan-in intersection: an edge with a hive `from` is a transit pass-through and contributes `set_hop` of this edge ∪ the intersection of the contributions of all inbound edges of the hive (recursively across multi-stage transits, cycle-safe). The same key walk the runtime performs at transit (`hop` expires only at a cell emission, not at the transit). **Participation/status filter at boot:** at bootstrap, the locality checker carries contract obligations **only for active nodes**, nodes that participate in the active graph. A registered but **disconnected/inactive** node (persisted `colony.db` status at reboot **or** island derived as inactive from t0 at first boot) is pure bookkeeping: it is rehydrated (stable `cell_id`), but at boot is subject to **no** contract enforcement. The full check resides at the **mutation moment** that connects it (participation rule + transit-aware intersection). Thus the check is uniform across both boot kinds: inactive ⇒ no boot obligation; active-and-wired ⇒ sharply checked. **Which graph is checked (GH #178):** the one the colony actually runs with. On a first boot those are the `params.graph` edges of the `config.json` files; on a reboot it is the persisted edge table — the same authority the boot loads its edges from. Since GH #186 the same cut also answers which paths are hives: on a reboot the persisted `hive_scopes` table, on a first boot the `config.json` walk. Otherwise the checker read a hive whose directory had been removed as a cell with no contract — a transit pass-through became a node that contributes nothing, and the fan-in intersection came out empty even though the topology delivers the key. Before, a reboot's checker saw only the files, i.e. a **partial** graph, and partial is worse than empty here: a hive that wrote down its doors gave an interior cell an incoming edge and thereby took away the lenient "no incoming edge ⇒ ingress-at-birth" branch, while the `set_context` setter sat on a mutation edge the checker could not see. **And how a finding lands:** on a first boot the file IS the topology and somebody is writing it right now — a violation is a loud boot failure. On a reboot the topology is committed state whose mutation edges already passed this same check when they were wired; a violation is **reported** (`tracing::warn` per finding, naming node + key + rule) and the colony starts. Refusing there would be a crash loop after the writes are already on disk. To see the finding before the restart, or to enforce it in CI, use `meclaw --validate --validate-strict` — there the same finding is a non-zero exit.

**A `consumes.body` key is mandatory by default — `required` withdraws the obligation, not the declaration.** With `required` absent it is `true`: `validate_consumes` (`crates/meclaw-core/src/contract.rs`) requires the key in the incoming body and otherwise reports `required consumes.body '{key}' missing`. `required: false` takes the key out of that check — and that is more than the field name suggests: `validate_consumes` walks **only** the required-key projection, so the **type check** falls away together with the presence obligation. An optional key that IS present is currently not validated in any way; its `type` token is documentation for it, not a gate. What remains is the declaration: the cell reads the slot, it merely does not demand it. That is the common form: most shipped templates declare `system`/`messages`/`op` optional. **What hangs off the declaration does not hang off the obligation** (GH #323): a capability switch (next paragraph) reads the declaration, not the obligation — otherwise `required: false` would silently withdraw a capability with nothing failing anywhere. **Consequent rule for `/colony` roundtrips:** a `/colony` endpoint answer (`{"mutation":{…}}`, `{"rescan":{"status":"ok"}}`) is **not** a UBF body, it carries no `messages[]` and therefore bounces off every contract that declares `messages`. A cell that runs a `/colony` roundtrip therefore declares an **empty** `consumes.body`.

**Declaration as a capability switch (GH #87):** a declared `consumes.body` key is not only a presence obligation, it can also **unlock a capability**. First case: `consumes.body.attachments`. Only a cell that declares the slot receives the read-only blob-store handle at spawn with which it resolves `attachments[]` refs itself at `handle()` time (`meclaw-overview.md` § "`attachments[]` schema", owner ruling GH #19; consumer detail in `cell-types.md` § `llm`). Without the declaration the handle does not exist, so the cell **cannot** read an attachment rather than merely not doing so. The switch is the **declaration**, not the presence obligation: `"attachments": {"type": "array", "required": false}` unlocks the handle just the same and still does not demand attachments on every message (GH #323). And since a missing handle is indistinguishable, to the cell, from "no blob store wired", this is no detail: a declaration the switch fails to see withdraws the capability silently.

**`consumes.topology` — a cell's own place in the graph, not the graph (GH #160).** A fourth field beside `body`/`context`/`hop`, and the only one that is **not a message compartment**: nothing in it is validated against an incoming message, a key declared here never makes a message invalid, and `validate_consumes` does not see it. It is a pure capability declaration in the same grammar.

```json
"consumes": { "topology": { "inbound_edges": { "type": "array", "required": true } } }
```

The only key the substrate knows is **`inbound_edges`**: the `from` paths of every edge pointing at the cell's **own** path. A declaring cell receives a read-only handle at spawn (`meclaw_colony::NeighbourhoodView`) that asks exactly that one question — live against colony's in-memory `EdgeTable`, bounded by the cell's own operation timeout, self-scoped. Not the graph (that is `/colony/graph`), not a scope, not its own outbound edges, and never another node's. Without the declaration the handle does not exist.

**`contract.ingress` — a cell declares that it is an entry point (GH #185).** A block beside `consumes`/`emits`, in the same grammar and for the same reason as `consumes.topology`: a capability that is declared rather than inferred from the shape of the graph.

```json
"contract": { "ingress": { "context": ["chat_id"] } }
```

The value is the **list of context keys** this cell may mint when a message is born — not a boolean. The list may only **narrow** the standard set (`INGRESS_CONTEXT_KEYS`), never widen it; a key outside it is refused by name. A boolean would have granted the whole standard set to anything saying "I am an entry", which is the same all-or-nothing generosity as the inference this block replaces.

**What it replaces:** until GH #185 the header check read "has no incoming edge" as "is the graph's entry". That held while the check saw only `config.json` edges. Now that it sees the running graph, a genuine entry that also receives replies — the ordinary shape of a proxy — loses exactly that branch. The declaration makes the question locally answerable: adding an unrelated edge no longer changes the answer.

**Why not under `emits`:** cells emit `hop`; `context` is edge authority alone (§ Access). The birth of a message is the sanctioned exception to that, not a cell emission — so the block sits beside it rather than under it.


**`contract.write_surface` — a cell bounds what the substrate writes on its behalf (GH #260).** A key beside `consumes`/`emits`/`ingress`, in the same grammar and for the same reason: a statement about this cell's **place**, not about a message.

```json
"contract": { "write_surface": "internal" }
```

`"open"` (the default, and what an absent key means) or `"internal"`. `"internal"` means: a write the **substrate** answers before `handle()` — today exactly one, the `import` of the `transfer` body slot (`cell-types.md` § Content transfer) — is refused when its sender lies outside this cell's **parent path**. The refusal carries `error_code: "write_denied"` and lands before the first row is written. **Fail-closed**: a message with no sender (a source message from an ingress or an event) is outside. An `export` is a **read** and is never bounded. A cell sitting directly under the colony root has `/` as its parent path, which contains every cell — the declaration is inert there.

**Why in the `contract` block and not in `params`:** because the substrate is type-agnostic. `params` belong to a cell type; the slot this rule bounds sits above **all eight** cell types with a `cell.db`. A rule the substrate enforces is declared where every cell type declares in the same grammar — anything else would make the substrate read a type's `params`, which is exactly what it must not do.

**Relation to `store`'s `params.write_surface` (GH #132):** two halves of the same boundary, deliberately **not** derived from one another. `params.write_surface` bounds the ops a `store`'s `handle()` runs; `contract.write_surface` bounds the ones the substrate runs before `handle()` is ever reached. The scope arithmetic is the same in both, so a cell declaring both gets **one** boundary rather than two that disagree. A cell declaring only one has only one.

This replaces the last direct `colony.db` read in the tree (the `vault` unlock attestation); § Database isolation has had **no** exception since. First and only consumer today: `vault` — a cell that cannot verify its neighbourhood stays LOCKED, which is why a `vault` `config.json` without this declaration never unlocks.

**`contract.transfer` — a cell declares that its database does not travel (GH #314).** Same place, same grammar, same reasoning as `write_surface` — except this key does not answer *who* may write, it answers *whether* this cell responds to the seam at all.

```json
"contract": { "transfer": "none" }
```

`"all"` (the default, and what an absent key means) or `"none"`. `"none"` means: the `transfer` body slot (`cell-types.md` § Content transfer) is refused for this cell with `error_code: "transfer_exempt"` — `export` **and** `import`, because a store that may not leave may not be overwritten through the same seam either. The refusal lands **before** the arguments are read: it names no table, because a refusal that sounds different per table name is an inventory. A typo in the value is a parse error, never a silent fallback to `"all"` — a misspelled exemption that quietly means "travels after all" would be the worst outcome this key can have.

**Why a declaration rather than a list inside the substrate:** an exclusion list of cell-type names in `db_transfer.rs` would be invisible in the `config.json` of the cell it applies to, invisible in a diff, and would have to be edited again for the next cell type with the same need. A declaration binds a cell type nobody has written yet.

**Relation to `contract.write_surface`:** two independent statements about the same seam. `write_surface` bounds the write half to the parent scope and leaves an `export` untouched (no write surface has ever bounded a read) — and that gap is exactly why #314 was opened, because the `vault`'s disclosure was a **read**. A cell that wants both declares both; one does not switch on the other.

**Correction (GH #336, `access@2.0.4`):** this used to read "first and today only consumer: `vault`" — that is retracted, and it was only true for as long as the `vault` was the sole declarant. It is **two** cell types across **three** shipped configs: `vault` (`templates/vault`, `templates/access/vault`) and the capability broker's `store` (`templates/access/store`), whose `grants` are live bearer handles — an export is a read, which `contract.write_surface` explicitly does not bound, which is why migration there means re-granting at the target rather than importing. `cell-types.md` § Content transfer carries the same retraction.

**`params.transfer.base_path` — the fence a cell manages its own files inside (GH #555).** The counterpart to `contract.transfer`, and deliberately on the **other** surface: the owner's ruling of 2026-09-04 reads, verbatim, *"cells manage their own files, nobody else does"*, and *where* an instance keeps its files is a statement about that instance — not about its role. Two cells of the same template must be able to write into different directories; with the fence in the `contract` they would share one. Hence `params`, exactly as for `file`'s `base_path`.

```json
"params": { "transfer": { "base_path": "/srv/meclaw/export" } }
```

An **absolute** path, or none at all. It bounds the file half of the `transfer` body slot (`cell-types.md` § Content transfer): `{"operation": "export", "to": "<dir>"}` and `{"operation": "import", "from": "<dir>"}` resolve `<dir>` **relative** to this directory. With the key absent the cell has no directory and falls back to none — every named `to`/`from` is refused with `error_code: "transfer_path_out_of_bounds"`, and that is exactly the default: a cell that says nothing writes nothing. Absolute is required because the cell task knows no colony root (`root` lives in the colony struct and never reaches `spawn_cell`) — a relative fence would resolve against the process's working directory and would not be a boundary. A relative value, a non-string, or a `transfer` block that is not an object are therefore **loud boot errors**, like a broken `emits` schema declaration.

**Nothing here is canonicalised and nothing is checked for existence — neither at boot nor at `--validate`.** The parse checks the string and `is_absolute`, and nothing else. That is a decision with a receipt: a `file` cell canonicalises its `base_path` in `validate_params`, which is why the shipped interim export sink was a `code` cell rather than a `file` cell — a member whose export directory did not exist yet would otherwise fail `--validate` and fail to boot, for a lane nobody had used. A fence is a promise about **where**, and a promise about where does not require the where to exist yet. If the directory is missing, the first `to`/`from` finds out — as `error_code: "transfer_io_error"` on the message.

**Relation to `contract.write_surface` and `contract.transfer`:** three independent statements about the same seam. `transfer` says **whether** the database answers the seam at all (and it strikes first, before any path is resolved), `write_surface` says **who** may write (and it holds for `from:` exactly as for a message `import` — reading the document off a disk does not make it a different operation), `params.transfer.base_path` says **where**. None switches on another.

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

**Note on permissions until the hardening** *(specified, not built — see GH #254)*: The capabilities in this phase are **discovery hints** for builder composer and audit tools, **not a runtime check** — and today not even that: the list above describes a block no parser reads and no API exposes, one that exists only in `config.json` files on disk. This applies in particular to `mutate-graph`: whether a cell actually _can_ mutate depends solely on the topology (does an edge to `/colony/mutations` exist?). Post-roadmap hardening may add capability tokens that are checked at runtime. See overview section "Permissions" in the mutation format.

#### `ToolSpec`

Declares which tools the cell offers to its LLM (or external consumers) *(specified, not built — see GH #254)*. **Not a routing endpoint**. Where tool calls are routed is decided by the topology. Today nobody reads `contract.tools` (`ContractBlock` does not know the key) and no shipped `config.json` writes it; the tools an `llm` cell actually offers live in `params.tools` (see `cell-types.md` § `llm`).

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

Six keys, **builder-enforced**, not substrate-enforced: the structure takes effect as soon as the builder/composer consumes it (the same discovery contract surface for the LLM builder that writes edges, and reviewer/operator), not as boot validation in the substrate. As a top-level block **of a cell's `config.json`** this is *(specified, not built — see GH #254)*: `ParsedConfig` (`crates/meclaw-colony/src/config.rs`) carries exactly `cell`, `params` and `contract`, and a `description` beside them is dropped silently. Today the block is read only on `template.json`, by the template scanner (`crates/meclaw-colony/src/templates/scanner.rs`) — there it is built.

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
- **The index** is the four `registry` columns `template`, `template_version`, `instantiated_at` and `template_chain` in `colony.db` (since schema v6). The first three answer "which nodes came from `sink-tpl@1.0.0`?" with **one** SQL statement instead of a tree walk; the fourth carries the chain as JSON and answers the question the leaf stamp alone cannot: "which instances does a bump of an **inner** template touch?"

The index is filled in two places: at instantiation (the mutation knows the template) and **at every boot**, from the `config.json` that was read. The second one is the important one: a config-only copy brings no `colony.db`, so its index starts empty — and without the boot pass it would silently claim "no origin" while the files next to it say otherwise. A node without `cell.provenance` sends nothing and keeps `NULL`; origin is **recorded, never invented**.

**The chain (`template_chain`, GH #277).** A composite template may place foreign templates as `cell.type: "ref"` sub-units; the placed tree is **copied along** at staging, like everything else. The leaf stamp alone then answers "what is this node?", but no longer "which instances does a bump of `inner` touch?" — before GH #277 the composite's name was recorded there and the inner template's was missing, and the leaf stamp alone would merely reverse that loss. `template_chain` holds both ends: an update addressing the composite finds the node through the **first** entry, one addressing the referenced template through the **last**. A missing key means "written before the field existed", not "no chain"; `instantiated_at` stays the **same** timestamp for every node of one instance. The index in `colony.db` carries the chain along, in `registry.template_chain` — a JSON list, written at instantiation and at every boot. A `NULL` **and** an unparseable value both read as "no chain was recorded": the instance's own `config.json` remains the source, the table is the index, and a broken index entry costs a query hit, never the truth.

**What the stamp is not.** It is not a live binding to the template: the template may change, move or disappear without the instance noticing or changing (§ Access, "After instantiation"). It is also not a reference to a `templates` row — it names the name and the version, not the `template_id`. (Since GH #62 the `template_id` is **stable** across rescans, but it remains a colony-local surrogate key and is meaningless in an exported tree.)

## Snapshot versus live-read

Not every artifact under `{root}` is read again when a colony boots. Knowing which is which decides what a backup has to move and what a restore actually restores.

**Live-read at every boot.** `config.json` is re-read and re-parsed on every boot, and `${VAR}` tokens are substituted in memory against `{root}/.env` each time. The on-disk file is never rewritten at boot -- instantiation is its only writer. `.env` is therefore live: changing a value and rebooting changes the effective params. Instantiation does not freeze the environment class either: `${VAR}` and `${VAR:-default}` survive the write **literally**, so an exported tree carries the tokens and not the values -- secrets stay in `.env`. The escaped form `$${VAR}` survives instantiation unchanged too and becomes the literal text `${VAR}` when read; it binds to nothing. The price of that late binding is a standing dependency: a `${VAR}` with neither value nor default fails the boot loudly (`env_var_missing`, naming the variable) instead of quietly yielding an empty value.

**The empty value** is the case next to it: `VAR=` in `.env` and `${VAR:-}` both yield `""`, and what that means is decided by the cell at parse time, not by the substrate. The rule is the same across the tree (GH #268, GH #270): an empty **optional** credential is **no** credential (`web_search.api_key`, `mcp.auth.bearer` -- the cell sends no `Authorization` header at all rather than a header with nothing after it), while an empty **required** value is a **configuration error** and is rejected at parse time exactly like a missing one, with the same message and the name of the key (`mcp.endpoint`, `mcp.command`, `proxy.bot_token`, and the Slack variant's `app_token`/`bot_token`). The reason for the second half: `${VAR}` without a default only catches the **unset** variable -- a half-filled copy of `.env.example` sets it empty, and without this check that would produce a cell which looks healthy and fails at a third party on every call. `llm.api_key` is the third case, and it shows what the rule actually turns on: not an optional flag, but what an absence **means** (GH #271). The **declaration** is required there -- a key missing altogether is a configuration error at spawn -- while the **value** may be empty, and then says exactly what it says: this endpoint needs no credential. An OpenAI-compatible server on localhost ignores the header anyway, and the cell sends no `Authorization` header at all, on **both** dialects. A `${…_API_KEY}` without a default whose variable is set empty in `.env` is therefore the explicit statement "no key is needed here" -- and not the same thing as a forgotten entry. Refusal stays reserved for values whose absence has no working meaning: a `proxy` without a bot token can do nothing at all, an `llm` against an anonymous endpoint can do everything.

**Birth snapshot in `cell.db`.** A cell's `cell.db` is created at its first spawn and seeded exactly once, on `OpenStatus::Created`. `timer` copies `params.schedules` and `store` loads `seed/*.jsonl` at that moment and never again. After the first boot, `cell.db` is the truth: editing `params.schedules` in `config.json` changes neither an existing schedule nor adds a new one. The runtime params overlay (`cell.db` `params` table) likewise wins over the `config.json` birth params for the keys it holds. To make a `config.json` edit of a birth-snapshot field take effect, the cell must start without a `cell.db`.

**Two seeders, not one — and, since GH #456, a third writer.** The sentence above describes the spawn path. Beside it stands the **mutation staging seeder** (`mutation::stage::seed_cell_db_if_present`), and the two read different things. The `store`'s spawn path walks the tables declared in `params.schema` (`seed::check_seed_files` → `load_seed_if_present`): a `seed/<file>.jsonl` whose name matches no table declared there is neither checked nor loaded by it. The staging seeder reads **every** `seed/*.jsonl` in the directory — it does not know `params.schema` at all — and writes it into a fresh `cell.db` at instantiation, that is **before** the first spawn. It keeps out only of cell types that own their own schema (`CellFactory::owns_schema`, GH #398). **So a table `params.schema` does not name is seedable too** — the alias and rejected-pair tables of a `canonical` declaration, for instance: they are built from the seed's header line and therefore without their key, and `apply_canonical_ddl` rebuilds them with it at the first spawn through `ensure_keyed_table`, carrying the rows over (GH #255). A seed for such a table therefore takes effect through the staging path and **only** through it; a hand-written tree that the boot finds has no reader for it.

**The third writer is not a file but a declaration** (GH #456). Both seeders above run once, at the birth of a `cell.db`. The diff operation `seed_rows` writes into a `cell.db` that already stands — including that of an **awake** cell — and it is not a third mechanic: the same JSON→SQL binding, the table built from the declared column list, `ensure_keyed_table` supplying a missing key at the next wake. What distinguishes it is not the writing but the door: a digest, a gate, an access verdict and a `mutation_log` row (overview § Mutation operations). It is therefore meant for the rows that are **permissions and keys** — a policy row, a grant, a firewall rule — and not for bulk data. Two consequences the two seeders do not have: a `seed_rows` **is** substituted (it is part of the diff), and a `params.write_surface: "internal"` does not bound it — that key binds messages to a cell, while `seed_rows` goes through the write authority itself and is bounded by the mutation scope plus the access verdict over it.

**Birth snapshot in `colony.db`.** A node's `cell_id` is minted once and carried in the `registry` table, stable across reboots. `params.graph.edges` and hive scopes are applied on the first boot and persisted; on a reboot they are hydrated from `colony.db` and the `params.graph` hints are ignored — since GH #168 by the bootstrap **planner** too, which used to validate them and die on an edge a mutation had long since removed. On a reboot, a cell directory absent from the registry is reported but never adopted — registration happens only through instantiation or mutation. Both hold for **edges**: the boot instantiates no node at all, and the `nodes` block described in `meclaw-overview.md` § Graph schema is marked "specified, not built" there (GH #277) — in a `config.json` it aborts the boot today. This paragraph describes current behaviour, that section the target picture. The `templates` index is a scan snapshot holding absolute filesystem paths; a boot re-scans only when the table is empty, otherwise an explicit `--rescan-templates` is required.

**`hive_scopes` has no delete path, and that is the policy rather than a gap in it.** There is no `DELETE FROM hive_scopes` anywhere in the crate: the first apply writes the rows, a mutation adds one for every hive it creates, and nothing ever removes one. The first reason is structural: a hive has no registry row. The `registry` holds cells — mailbox, status, `cell_id` — and a hive is not a cell, so the colony needs a second list to know that an address holds a hive rather than something it can deliver to. That distinction is what routing turns on: an edge into a cell delivers into its mailbox; an edge into a hive delivers nothing, the colony reads the hive's own edges and routes on. Transit versus delivery, and the colony must know which it is looking at.

The missing delete is therefore not a special case: `remove_nodes` is disconnect-instead-of-delete. The edges go, the registry entry stays, the directory stays. Nothing is deleted anywhere, so a scope row outliving its hive is the same no-delete policy applied consistently, not an omission in one table. The consequence is accepted deliberately: a path that was once a hive keeps being a hive to every reader of the table, the boot's fan-in walk included.

**And it is load-bearing.** GH #186 made `hive_scopes` the boot authority for which nodes are hives on a reboot *because* the table is append-only. A stale row is helpful there: a hive whose directory was wiped is still read as a transit rather than as a contract-less cell (§ `contract`, mutation/locality validator). The question only becomes a real decision at a hive **relocation**, which `move_nodes` refuses today; under this policy the answer is already framed — a hive moves by its scope row moving with it, not by one being deleted and another created.

**Consequences for backup and restore.** The restore unit of a cell is its directory, not its `config.json`: restoring the config restores the declaration, never the birth-snapshot state. The restore unit of a colony is `{root}` as a whole, including `blobs/` and the SQLite WAL sidecars `*.db-wal` / `*.db-shm` — a backup that matches `*.db` alone can restore a colony that boots cleanly and runs the state from before the last writes. A restored tree whose `colony.db` came along keeps its identity; a config-only copy is a new colony with re-minted `cell_id`s. After relocating a tree, run `--rescan-templates` so the template index points at the new root, and `--validate --validate-strict` to surface cell directories the restored registry does not know.
