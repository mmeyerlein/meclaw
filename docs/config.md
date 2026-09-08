# `config.json` format

The `config.json` format in detail, per cell and per hive scope marker. Where this file and `meclaw-overview.md` disagree, the overview wins: it is the single source of truth.

> New here? [`README.md`](README.md) is the map of this directory and [`glossary.md`](glossary.md) defines the vocabulary this file assumes.

## Supreme rule

A cell does not know what happens before or after it. It knows its own contract (input and output schema), its params, and the message it is currently processing. Sender paths, receiver paths, hop history, routing strategies and other cells lie outside its knowledge.

Messages are atomic. Trace reconstruction lives in the central message log in `colony.db`, filterable by path prefix. The message itself carries no history.

Envelope fields are read-only from the cell's perspective. `id`, `trace_id`, `parent_message_id`, `correlation_id`, `target`, `reply_to`, `ttl` and `created_at` are set by the colony during routing, and by nobody else. A cell can neither write them in its content JSON nor manipulate them over an edge (see `meclaw-overview.md` § Envelope setter authority). A reply target other than the sender is an application-level problem, solved with header-based routing.

From the cell's perspective the world is single-threaded. A `handle()` call runs to completion before the next one starts, and the cell task pulls sequentially from its mpsc mailbox. Cell code therefore contains no `Mutex`, no `RwLock`, no atomics and no reentrancy defense. The parallelism of the system lives outside the cell (`meclaw-overview.md` § Concurrency and parallelism).

## Access

Only the colony reads and writes `config.json`, and instantiation is its only writer, exactly once. The running cell task never re-reads the file after startup, which makes `config.json` the instantiation snapshot and not a live document.

At instantiation the colony copies the template, assigns a fresh UUID v7, stamps the origin (`cell.provenance`, with template name, template version and instantiation time), resolves the instance class (`${ctx.*}`, `${uuid7:*}`) and writes the result into the instance's `config.json`. The environment class (`${VAR}`, `${VAR:-default}`) is left alone here: it stays a token in the file and binds late, at every read (see `meclaw-overview.md` § Variable substitution, and § Snapshot versus live-read below). A secret a template references as `${VAR}` therefore never reaches the disk.

The node reference is the filesystem directory name, the path segment under `{root}`. The `config.json` has no `cell.name` field and carries no `name` at all. When the root chain is resolved, the `${...}` substitution wins over the template name from `template.json`. Naming collisions with siblings inside the same hive scope are rejected by the colony in the single-stage mutation validation (`meclaw-overview.md` § Naming collisions).

After instantiation `config.json` is semantically frozen, the bootstrap snapshot. Nobody writes into it any more, neither the colony nor the cell itself. Dynamic cell state (changed params) lives in `cell.db`, colony state (registry, edge table, `cell_id`, message log, mutations) lives in `colony.db`. After the snapshot, `config.json` carries neither of the two forward (see `meclaw-overview.md` § Lifecycle of `config.json` and `cell.db`). The graph of a topology lives centrally in the colony's registry and in `colony.db`, never in the `config.json` of the hive scope marker, whose `params.graph` is an initial bootstrap hint.

On a runtime registration (`add_templates`, GH #440) the `config.json` a declaration entry brings along is written into the instance-local library byte for byte as it stood in the body. Registering does not substitute it. `${ctx.*}`, `${uuid7:*}` and `${VAR}` stay put and bind where they always bind, the instance class at instantiation and the environment class at read time. Registering files a class; only the `add_nodes` that names it turns that into an instance. Any other split would let a library blueprint carry the `ctx` of whichever mutation happened to deliver it.

Cells do not read `config.json` at all. The colony hands the cell its `params` block at startup. Param updates arrive afterwards by message, and the cell persists them in its `cell.db`, so `config.json` and the live state diverge. A cell reset wipes `cell.db`, and the cell starts again from the bootstrap state.

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

Two of the four blocks carry different authority.

The `cell` block is colony substrate. Its fields control how the colony instantiates, registers and supervises the cell, and none of them is ever handed to the cell. The cell sees its `params` block and the message it is currently processing, nothing else. Allowed keys are `id`, `type`, `timeout`, `restart_limit`, `idle_timeout_ms`, `mailbox_size`, `message_timeout` and `provenance` (details in the `cell` table below), plus `template` in a `ref` marker (§ Template reference). The subtree parser reads `template` inside a template, and it never stands in an instantiated `config.json`, because the reference is already resolved by then. In the root tree it is a declaration since GH #424: on a first boot the colony resolves it and grows what it names (`meclaw-overview.md` § Startup algorithm, step 3a), and on a **reboot** it is an unresolved reference in an already grown tree and is reported, never grown (A5b). A reference the first boot cannot fulfil stays a hard boot error and names both halves, the reference and the directory the marker stands in.

A key declared in the `cell` block that is not on that list is a hard refusal, on every path that reads the block. GH #353 retracts the GH #277 correction that once stood here and claimed the opposite. The refusal used to exist at boot only, through a hand-maintained allow-list inside `bootstrap.rs` that no other reader consulted, so the very template a boot would have refused was staged through a mutation without a word and the typo took effect as a default. The list now lives on `CellHeader` itself (`#[serde(deny_unknown_fields)]`, `crates/meclaw-colony/src/config.rs`), which every read path deserializes: one list, one refusal, both paths.

"Every read path" includes **every node of a multi-cell (subtree) template**. The subtree parser (`mutation/subtree.rs`) runs each node's `cell` block through the same barrier before it hands it on, so a mutation cannot commit a tree that the next restart refuses. At boot the refusal is a `BootstrapError::InvalidJson` naming the offending key and the `config.json` it stands in, on the mutation path a normal pre-destructive validation refusal (`error_code: "schema"`) naming the same two.

This is breaking. A tree carrying an extra `cell` key, a typo or a key someone used as a comment slot, stops booting until the key is removed. It never did anything; now it says so.

`surface` stood on that list until GH #383. The key is removed, neither renamed nor moved, so `cell.surface` is an unknown key and therefore a hard refusal like any other. The `cell` table below names the reason and says where a reader points instead (the `web` cell, `templates/canvy/MIGRATION.md`).

The `params` block is handed to the cell 1:1 and opaque. After `${VAR}`, `${ctx.*}` and `${uuid7:*}` substitution the colony passes it through unchanged and does not interpret its content. The shape is cell-type-specific: every cell type defines its own `params` structure (see `cell-types.md`). The sole exception is the hive scope marker, where the colony reads `params.graph` as the initial desired graph. A hive is no actor, so nothing is handed to it.

Only `id` and `type` are immutable. They identify the node instance and its cell type across the entire lifetime. Every other field follows the effectiveness rule: a change to a `cell` or `params` field, through a new instantiation at the path or through a new template, takes effect at the next spawn or wake of the cell. The running cell task does not re-read `config.json` (§ Access).

#### Hive scope marker

At a hive scope marker (`cell.type: "hive"`) only `cell` and `params` are relevant. `params` carries the optional `graph` block, the initial desired graph (`meclaw-overview.md` § Graph schema), and the optional `ports` list (GH #133, `cell-types.md` § `hive`).

A `ports` entry comes in two forms: the short name of a direct child as a string, or the slot form as an object (`{"name": "gen", "slot": true, "unbound": "park" | "drop" | "error"}`, GH #285). A slot is an address that may stand empty. `slot` and `unbound` are both mandatory, and the declaration buys exactly two exemptions: an edge onto the slot is no dangling endpoint at boot, and a mutation may wire it with `add_edges` before it is filled. A path that is not declared as a slot and has no occupant stays what it is today, a hard error under `--validate-strict`.

`unbound` says what happens to a message that reaches the unbound slot over an edge. `drop` discards it silently, `error` produces a `slot_unbound` dead letter, `park` holds it FIFO until the binding (`colony.json slot_park_max`, default 64; the newest arrival above the bound is refused as `slot_park_overflow`, and a shutdown discards the queue). A message that addresses the slot path directly from outside does not reach the declaration and stays `unresolved_path`. A slot is a valid `add_edges` endpoint and never a `remove_nodes` or `swap_nodes[].match` target; emptying it and rewiring it in the same diff commits, because the declaration outlives its occupant. Slots belong in a hive below the root: the root scope has no port boundary, and a slot declared there buys no exemption. Full description: `cell-types.md` § `hive`, Slots.

Next to them sits the optional `required_drains` list (GH #147/#237). Its entries are `{port, hop, because}`, which ports may only be wired from outside once their paired egress is consumed outside the hive, or, for a sealed hive, `{accepts, emits, because}`, a caller that sends this lane must subscribe to that one. A `dead_letters` override does not exist: the `HiveParams` deserializer is `deny_unknown_fields`, and the DLQ is always `/colony/dead_letters`. Since GH #173 there is also the optional `params.contract` block, the hive's machine-readable contract (§ `params.contract`).

The top-level `contract` block stays unevaluated on a hive. That key belongs to the cell and carries a different shape there (`version`/`settings`/`consumes`/`emits`, where `emits` is an `EmitSpec` map per output). One word cannot carry two shapes, and the hive's wiring surface lives in `params` anyway (`graph`, `ports`, `required_drains`), so the contract joined them.

In the `cell` block of a hive scope marker only `id` and `type` are relevant. `timeout`, `message_timeout`, `idle_timeout_ms` and `mailbox_size` are ignored, because there is no actor, no mailbox and no `handle()` call. A `description` is allowed and serves discovery by builders; `emits_meaning` and `consumes_meaning` are omitted. "Ignored" is not "anything goes": the closed key list of the `cell` block applies here too since GH #353, and an unknown key in a hive scope marker's `cell` block is the same hard error as on a cell, on every path that reads the marker, `move_nodes` included.

#### Template reference

A template reference (`cell.type: "ref"`, GH #277) is a directory that places another template at this position instead of describing a cell of its own.

Inside a template it carries a `config.json` and nothing else. Any further file next to it would give one address two sources and is refused at parse time. In the root tree it is different (GH #424): there a marker may have subdirectories beside it, and those are deeper declarations, addressing something else rather than the same thing twice. The first boot grows the marker and merges into it, so what the template brings and what the operator wrote beside it stand side by side. A collision on the same address is a named refusal, raised before any rename.

`cell.template` is a template reference in exactly the form `TemplatesRegistry::resolve` accepts, `<name>` or `<name>@<version>`. SemVer ranges (`^`, `~`) exist here as little as anywhere else (`version.rs` keeps them post-roadmap, see `meclaw-overview.md` § Resolution `name@version`).

`override_params` sits top-level next to `cell`. It belongs neither in the `cell` block, whose key list is closed, nor in `params`, which a `ref` does not have. The block is optional, addresses the cells of the referenced template by their paths inside that template (`""` is its root), and sits in the layering below the mutation's own `override_params`: the reference sets the default, the caller overrides it key by key. A key that names no cell of the referenced template is an error, never a silent no-op, and the message lists the cells that do exist.

`meclaw --validate` asks the block both questions before anything grows (GH #586): the cell path, and one nesting level down the param key, with the very check `/colony/mutations` gives an `add_nodes[].override_params` (GH #294), in the same wording and with the same `error_code: "schema"`. Both are a hard error without `--validate-strict`, as sharp as the unresolvable reference: a param the referenced template does not declare is no legal topology somebody might have meant. Until GH #586 the pre-flight check asked neither half, and a typo in the marker cost a whole boot cycle, because the marker is where a template's params are reachable before a builder exists.

The boot checks the `requires` declaration pre-destructively (GH #465). `bootstrap_grow::grow_one` calls `validate_requires`, the very function `/colony/mutations` runs as its stage 3, before `stage_subtree`. A marker naming a template whose `requires.ctx`/`requires.env` keys the colony does not hold is refused as `requirement_missing` **before a single byte is written**: nothing is staged, and the marker is still a marker. Until then the boot was the one instantiating path that did not read the declaration, and the gap surfaced at the first turn, or, for a key with an empty default, not at all.

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

A `ref` marker may also declare a birth state under the top-level key `birth` (GH #437). It sits top-level for the same reason as `override_params`: the `cell` block has a closed key list and describes a cell, while a birth state describes an instantiation order.

```json
{
  "cell": {
    "type": "ref",
    "template": "unit@1.0.0"
  },
  "birth": "inactive"
}
```

The values are the same two as for `add_nodes[].birth`, `"active"` (the default) and `"inactive"`. The declaration holds for every cell of the grown tree, since a unit is born whole. The tree is then registered, addressable and persisted inactive, and nothing inside it runs until the ordinary reconnect wakes it (`meclaw-overview.md` § Reconnect), that is, until a mutation addresses one of its nodes itself. The declaration is durable (GH #491) and survives every restart and every mutation elsewhere in the tree. An unknown value refuses the boot, because a boot that cannot fulfil a declaration must not start half a tree.

The type is resolved at instantiation and never reaches disk. What lands in the instance is the referenced template's content at the reference's position. No `ref` cell factory exists, no dispatcher path and no registry entry for one: `ref` is a template-time type, never a runtime type (see `cell-types.md` § Overview). At a resolved reference the referenced template root's `README.md` is dropped together with `template.json`. The two are the descriptor pair of a standalone template, its registry entry and its page, and neither of them belongs to the instance the reference places. The composite's own `README.md` is untouched, since nothing was followed to reach it, so the instance is byte-identical to the copies the reference replaced. A ring of references is `template_ref_cycle` and renders the ring (`a@1.0.0 -> b@1.0.0 -> a@1.0.0`); a reference pointing at nothing is `template_missing` and names the versions the registry holds under that name (or `none`).

### `cell`

| Key | Content |
|---|---|
| `id` | `cell_id` (UUID v7). Set during the copy operation from template to instance, the only time it is written. Instantiation reads it from the freshly written `config.json` and persists it into the never-deleting `colony.db`, which from then on is the authoritative source of the `cell_id` (`config.json` is only the bootstrap imprint). Afterwards never reassigned, not on reconnect, not on resume, not on reboot. (The re-dedicated `swap_nodes` graph swap pivots edges onto a different implementation with its own `id` and leaves the old cell with its `id` preserved and disconnected. It transfers no `cell_id`, see `meclaw-overview.md` § Mutation operations.) |
| `type` | Cell type (`hive`, `store`, `llm`, `bash`, `code`, `web_fetch`, `web_search`, `file`, `edit`, `proxy`, `timer`, `mcp`, `harness`, `subcolony`, `vault`, `web`, `voice`). Together with `id` the immutable part of the `cell` block. Plus `ref`, which is never a runtime type (§ Template reference): it is resolved at instantiation and stands in no instantiated `config.json`. In the root tree it is a declaration the first boot fulfils (GH #424). |
| `restart_limit` | *(optional)* Maximum restart attempts by the supervisor before the cell is marked as `failed`. Default `5`. See `meclaw-overview.md` § Restart strategy. |
| `timeout` | Hot/cold mode (see `meclaw-overview.md` § Hot/cold cell model): `0` = default (idle-timeout model, Awake and Asleep), `>0` = one-shot (despawn after each message), `-1` = persistent (typically `proxy`/`timer`/`mcp`/`web`/`voice`, never despawn). Phase-13 activation; before that, all cells are permanently a task. |
| `idle_timeout_ms` | *(optional, from Phase 13)* Idle duration in ms after which a stateful cell with `cell.timeout: 0` despawns itself (Awake to Asleep). Overrides the colony default `idle_timeout_default_ms` from `colony.json`. Ignored if `cell.timeout != 0`: at `>0` the one-shot despawn after each message takes effect, at `-1` the cell is persistent and never despawns. |
| `message_timeout` | *(optional)* Substrate backstop per `handle()` call in ms, see `meclaw-overview.md` § Timeouts (concept B). Overrides the colony default `message_timeout_default_ms` from `colony.json`. `0` or `-1` = no backstop, for long-running cells. It is not the primary timeout for I/O operations; `params.external_timeout_ms` (concept A) is responsible for that. `cell.message_timeout` should be considerably more generous than `params.external_timeout_ms`, so that normally A takes effect first. |
| `mailbox_size` | *(optional, from Phase 5)* Bounded-mpsc capacity; overrides the colony default (`colony.json` `mailbox_default_capacity`, default 1000). See `meclaw-overview.md` § Mailbox size. |
| `provenance` | *(optional, GH #62)* The instantiation origin stamp, an object carrying `template` (the resolved template name from `template.json`, never the `name@version` reference form), `template_version` (the resolved version, absent exactly when the template declares none, which says something different from "version unknown") and `instantiated_at` (unix seconds, the same unit as every `created_at` in `colony.db`). Written exactly once, in the same write as the fresh `cell.id`, and never again. Absent for every node not born from a template: a hand-written tree, an `adopt` entry (the adopted node keeps its own origin unchanged, since adoption does not change where a node came from), and anything instantiated before the field existed. GH #277 retracts what stood here for a subtree template, that every node of the instance carried the subtree template's stamp. Every node carries the stamp of the template it is an instance of, and a node that came in through a `cell.type: "ref"` sub-unit names the **referenced** template, not the composite above it. The additional key `template_chain` names the composites that placed it: an array of two-element arrays `[name, version]`, outermost first, with the node's own template as the last element (`[["outer","1.0.0"],["inner","1.0.0"]]`) and `version` `null` when the template declares none. `template` and `template_version` are the projection of that last element, and an instance of a ref-free template carries a one-element chain. The subtree template remains the unit an update addresses; the chain is how an update finds its instances. See § Origin below. |
| ~~`surface`~~ | REMOVED (GH #383), was GH #159. The key declared that a cell may be served under `/surface/<cell-path>` by the HTTP API, with `title`, `assets` and `boot_hint`. The whole statement is retracted, down to the mechanism: the `/surface/*` route, the parser (`meclaw_colony::surface`) and the serving path in `--api` no longer exist. `cell.surface` therefore falls under the closed key list above, a hard boot refusal naming the key and the file (`BootstrapError::InvalidJson` at boot, `error_code: "schema"` on the mutation path). That is breaking, and loud on purpose: a tree still carrying the key was served by a route that is gone, and ignoring it silently would let it boot into a colony where nothing answers. A display is a cell of its own now, of type `web` (`templates/web`), owning its own port through `params.port`, with no shared prefix and no declaration in the `cell` block. Migrating a 1.x canvas: `templates/canvy/MIGRATION.md`. Details: `cell-types.md` § `web`. |

### `params`

The `params` block is cell-type-specific. Every cell type defines its own structure (see `cell-types.md`). The colony hands the block to the cell at startup, and afterwards param updates arrive by message (last write wins, persisted in `cell.db`). The form (W4b): the update message carries a top-level `params` body slot, 1:1 this `params` block and partial, pure cell content with no header gate. The cell merges and persists it itself and replays the overlay at wake or respawn over the birth params, while `config.json` stays untouched. Which fields are runtime-changeable and which are immutable (credentials, security boundaries) is cell-type-specific, see `cell-types.md`, for example `llm` § Runtime param updates.

`${VAR}` substitution from `.env` is performed by the colony before handover to the cell. `${ctx.<key>}` and `${uuid7:<label>}` are resolved at mutation application (see `meclaw-overview.md` § Variable substitution).

Every cell that performs I/O of indeterminate duration (HTTP, DB, subprocess, filesystem, MCP calls) declares a `params.external_timeout_ms` field, or a semantically more fitting name such as `query_timeout_ms` for `store`. The cell implementation wraps every such operation with `tokio::time::timeout` and, on elapsed, emits a regular error message (`header.finish_reason: "error"` with a cell-type-specific `error_code` such as `provider_timeout`, `query_timeout` or `script_timeout`). This is concept A in `meclaw-overview.md` § Timeouts, the primary protection, set precisely per operation and manageable by the operator. `cell.message_timeout` in the `cell` block is the coarse backstop for cell hangs and lies considerably above `external_timeout_ms` (concept B in the same section).

#### `params.max_concurrency`

*(optional, only for stateless cells, from Phase 7)* The maximum number of concurrently running worker tasks in the stateless-cell dispatcher (see `meclaw-overview.md` § Stateless cell dispatcher). It lives in `params` and not in the `cell` block. Default: a high value, effectively unbounded for typical load paths. Configurable per cell, for example `web_fetch` with `32` (HTTP provider rate limits), `file` with `8` (disk I/O), `bash` one-shot with `4` (process resource limit). For stateful and long-running cells the value is ignored.

#### `params.sandbox`

*(optional, from S4 / GH #35, completed in GH #85)* The sandbox block lives in `params` and not in the `cell` block. The `cell` key list is closed and describes how the colony runs the cell, whereas the sandbox describes the rights with which the cell starts its child process, a property of execution just like `external_timeout_ms`.

Four cell types read it, the four that start foreign code: `bash`, `code`, `harness` and `mcp`. The last one since GH #96, with the same schema and the same parser (`crates/meclaw-cells/src/mcp/params.rs`). One difference remains, and it is meant: instantiation injects a default profile only for `bash`, `code` and `harness`, while an `mcp` child without a declaration of its own keeps the rights of the daemon (GH #96, pinned by `crates/meclaw-cells/tests/gh96_mcp_sandbox_profile.rs`). Every other cell type ignores the block.

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
| `trust` | `"restricted"` \| `"trusted"` | yes | `restricted` = the sandbox is enforced. `trusted` = the explicit escape hatch for local cells, with **no** enforcement. |
| `network` | `"deny"` \| `"allow"` | no, default `"deny"` | only under `restricted`. `deny` starts the child in a fresh network namespace (`unshare(CLONE_NEWUSER\|CLONE_NEWNET)`), which holds nothing but a `lo` in state DOWN, so even `127.0.0.1` is out of reach. `allow` leaves it in the daemon's network **and** puts the resolver configuration into the Landlock view (see below). |
| `filesystem` | object | **yes** under `restricted` | the allowed filesystem view, enforced via Landlock. |
| `filesystem.read` | array of absolute paths | no, default `[]` | readable and executable, recursively. |
| `filesystem.write` | array of absolute paths | no, default `[]` | readable, writable and creatable, recursively. |
| `filesystem.runtime` | bool | no, default `true` | adds the runtime set (see below). |
| `limits` | object | no | resource caps via cgroup v2, only under `restricted`, see below. |
| `limits.memory_max_bytes` | integer > 0 | no | `memory.max` in bytes. Swap is pinned to `0` alongside it: a cap a process can escape into swap is no cap. |
| `limits.pids_max` | integer > 0 | no | `pids.max`, how many tasks the child and its descendants may hold together. The answer to a fork bomb. |
| `limits.cpu_max_percent` | integer > 0 | no | `cpu.max` as a percentage of **one** core against a fixed 100 ms period, so `200` means two whole cores. |
| `syscalls` | object | no | syscall filter via seccomp-bpf, only under `restricted`, see below. |
| `syscalls.ptrace` | `"deny"` \| `"allow"` | no, default `"deny"` | `ptrace` plus `process_vm_readv`/`process_vm_writev`, the same capability under three names. |
| `syscalls.raw_sockets` | `"deny"` \| `"allow"` | no, default `"deny"` | `AF_PACKET` of any kind and `SOCK_RAW` of any family. An ordinary TCP or UDP socket is untouched. |
| `syscalls.foreign_signals` | `"deny"` \| `"allow"` | no, default `"deny"` | every signal whose target is not the sandboxed process itself. |

All four key sets are closed (`sandbox`, `sandbox.filesystem`, `sandbox.limits`, `sandbox.syscalls`): an unknown key is a boot error. A `"netwrok": "deny"` must not pass as "no value given, so use the default".

When a `bash`, `code` or `harness` cell is instantiated from a template and declares no `params.sandbox`, instantiation writes this block into its `config.json` (GH #85, the migration cut):

```json
"sandbox": { "trust": "restricted", "network": "deny", "filesystem": { "runtime": true } }
```

The cut is prospective only. It applies at instantiation time and only to the node being born, the same shape as the secret cut from GH #20. A tree already on disk keeps running unchanged, and there an absent `sandbox` block still means "no sandbox". What "template-sourced" means is answered by `cell.provenance` (§ `cell`): the stamp is written in the same write as the default, and an `adopt` entry carries neither.

The default names no path, and that is a decision. A default that filled in the cell's own directory would bake an absolute host path into the instantiated `config.json`, and an exported tree would then carry a boundary pointing at a directory on somebody else's machine, exactly the failure class GH #20 was opened about. What stays reachable is the runtime set, which is what an interpreter needs to start at all. Anything beyond that the template declares itself. The block is visible in the instance `config.json` and therefore editable.

The escape hatch stays explicit. A template that needs full rights writes `"sandbox": {"trust": "trusted"}`, nothing is inserted and nothing is enforced, and whoever reads the instance sees the decision.

Recommended baseline for a template that has to write, where `<cell-workspace>` is the only directory it should write:

```json
"sandbox": {
  "trust": "restricted",
  "network": "deny",
  "filesystem": { "read": [], "write": ["<cell-workspace>"], "runtime": true }
}
```

The runtime set (`filesystem.runtime: true`) grants read and execute on `/usr`, `/lib`, `/lib64`, `/bin`, `/sbin`, `/etc`, `/proc`, `/sys` and read/write on `/dev/null`, `/dev/zero`, `/dev/full`, `/dev/random`, `/dev/urandom`. Without it no interpreter starts, because even the dynamic loader would be unreachable. It is a convenience and **not a security statement**: it contains `/etc` (hence `/etc/passwd`) and `/proc` (hence `/proc/<pid>/cmdline` of other processes of the same user). Set `runtime: false` and enumerate the paths yourself if that is not acceptable.

`network: "allow"` additionally grants name resolution (GH #144). `/etc/resolv.conf` is inside the runtime set, but on a systemd-resolved host it is a symlink into `/run/systemd/resolve/`, and `/run` was in no set at all. An `allow` therefore opened the sockets and let every lookup die in `getaddrinfo`. Measured: 953 of 953 embedding calls "endpoint unreachable" at `exit_code: 0`. Under `network: "allow"` Landlock now also grants read access to the target of `/etc/resolv.conf`, the resolved directory (`/run/systemd/resolve`, `/run/resolvconf`, depending on the host), or the file itself when it sits directly in `/etc`.

The grant rides on `network: "allow"` and on nothing else. Under `deny` the child sits in a fresh network namespace and has nothing to resolve for, so the path stays out. Pinned in `crates/meclaw-cells/tests/gh144_network_allow_resolves_names.rs`.

Resource caps (`limits`, GH #85) are enforced through a delegated sub-cgroup (cgroup v2) created per child process, filled, entered before the `exec` and removed afterwards. Removal survives a crash and a restart, because the directory name carries the daemon's pid and the next run sweeps away whatever pid no longer exists. A `limits` block that caps nothing is a boot error: it would read as "capped" and be no such thing.

The delegated root is looked up in two steps: the topmost writable ancestor of the daemon's own cgroup, which is what delegation looks like for a systemd user service with `Delegate=yes` and inside a container, otherwise `/sys/fs/cgroup/user.slice/user-<uid>.slice/user@<uid>.service`. One operating requirement is measured, and it bites. Creating the directory is not enough. Moving a process additionally requires write access to `cgroup.procs` of the common ancestor of source and destination. A daemon started from an ssh login lives in `user-<uid>.slice/session-<n>.scope`, so the common ancestor is the root-owned `user-<uid>.slice` and the move fails with `EACCES`. The same daemon under `systemctl --user` lives below `user@<uid>.service` and is allowed. Run the daemon as a user unit if you want `limits`. Otherwise the spawn fails loudly (fail-closed) instead of running uncapped.

Teardown writes `cgroup.kill` first. The sub-cgroup belongs to exactly one child, so anything that outlived it is a leftover.

The syscall filter (`syscalls`, GH #85) is a seccomp-bpf program assembled in this tree (no new crate, hard rule 6) that closes what Landlock, being a filesystem LSM, does not cover. Naming the block means "filter this process": an axis that is not mentioned is denied, and an axis you want to keep open you spell out as `"allow"` and can see that you did. A block in which all three axes are `"allow"` is a boot error, because it would install no filter at all.

A denial is `EPERM` and no kill: the program sees an ordinary permission error and can report it. Only an architecture the filter was not built for ends the process, because a filter keyed to the wrong syscall table would let the wrong calls through.

The limit of `foreign_signals`, stated plainly: a BPF program cannot consult the process table, so it compares the target pid against exactly one constant, its own, patched in after the fork. What stays allowed is `kill(self)` and `tgkill(self, tid)`, which is what `raise()` and `abort()` compile down to. Denied are `kill(0, …)` (the own process group, which for a cell's child is the daemon's group), `kill(-1, …)` and every foreign pid. The cost: a shell script under this axis cannot end its own background job with `kill $!`. That is a real restriction. The opposite reading, "allow every positive pid", would protect nothing.

The whole block is fail-closed. A `restricted` profile that cannot be enforced (no Landlock in the kernel, no namespaces on this host, a declared path that does not exist) makes the spawn fail, and the cell emits `error_code: "io_error"` with `sandbox not applied: <reason>`. No path lets a `restricted` cell quietly keep running unsandboxed.

`meclaw --sandbox-probe` (GH #97) asks before it hurts. Fail-closed means an unenforceable profile only shows up in production, as the `io_error` of a live cell. So that this need not be the first contact, the flag answers the same question up front, about the host, without running a cell. It needs no colony root, creates neither `colony.db` nor `log.jsonl`, and always exits 0, since the report is the answer even when the host can enforce nothing. One line per `params.sandbox` property, a verdict from the closed set `yes` / `no` / `skipped`, then the reason:

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

The `limits` line is the reason the flag exists. Its answer is a property of the launch and not of the kernel (see the operating requirement above). A bare "no" would send the operator hunting for a kernel feature that is already there, so the text separates two cases: an absent mechanism ("this host delegates no writable cgroup v2 directory …", which no other launch changes) and a wrong launch (`EACCES` at the common ancestor, naming the user-unit requirement).

The same report is appended to `--validate`, on stderr like every other validate diagnostic. There it is strictly informative and never changes the validate verdict: `--validate` checks the tree, enforceability is a question about the machine, and the fail-closed refusal happens at spawn time. Two of the four probes (`network`, `limits`) fork `/bin/sh -c :`; in the validate appendix they run only when the tree declares a `restricted` profile at all, and otherwise the line reads `skipped` with the reason `no restricted profile in tree`. A `trust: "trusted"` is no cause; an unreadable `sandbox` block is, because whoever wrote it wanted enforcement.

`sandbox` is not runtime-changeable. The block is read from the birth params only. For `bash` and `code` that holds structurally: both are stateless, have no `cell.db` and therefore no runtime param overlay at all. `harness` has one, and there `sandbox` is listed immutable explicitly, so an update touching it is rejected as `Immutable` instead of being swallowed as an unknown key.

#### `params.graph` (hive only)

The hive scope marker is the one place where the colony reads into `params`. `graph.edges[]` is the initial target graph of its subtree (full form and semantics: `meclaw-overview.md` § Graph schema and § Edge model). `from` and `to` are mandatory, and four optional fields join them.

| Key | Type | Default | Meaning |
|---|---|---|---|
| `condition` | CEL boolean as a string | `null` (= always matches) | decides whether the edge is responsible for this message; reads `context.*` and `hop.*`. A number out of a compartment binds as `int` (or `uint` above `i64::MAX`, `double` otherwise), so `hop.http_status == 200` is true on a message carrying 200 (GH #500, detail in `meclaw-overview.md` § Edge expression language). |
| `modifier` | object | `null` (= identity) | `set_context`/`delete_context`/`set_hop`/`delete_hop`/`restore_ttl`, the edge's sole header authority. |
| `default` | boolean | `false` | GH #283, since **v0.18.0**: `true` makes the edge a default edge. It is consulted only after no regular out-edge of the same sender fired, which makes it the declared consumer for what would otherwise dead-letter as `no_route` (or `hive_no_route`). It may carry a `condition` as well; without one the colony boots with a hint in its advisories and never with a refusal. |
| `lane` | string | `null` | GH #559: the name of the lane this edge runs, the declaration that turns a multi-segment deep edge into a **v-lane**. With the key absent the edge is an ordinary one and keeps exactly today's behaviour. The lane is named and never guessed, because no validator reads it reliably out of a CEL guard. What the declaration permits and what it demands (connect point, mandatory hops, the three `error_code` strings) is in `meclaw-overview.md` § v-lanes. |

An edge in a mutation diff (`add_edges[]`) carries the same fields, and `remove_edges[].match` can pattern on `condition`, `modifier` and `default`. With `default` absent there the routing phase is unconstrained and the pattern hits both (`meclaw-overview.md` § Mutation format). `lane` is no match term: a v-lane is named by the same terms as any other edge.

In both usages `from`/`to` are paths relative to the scope the declaration sits in, and `.` names that scope itself, at boot the hive whose `config.json` carries the `params.graph`, in a mutation diff the `scope` of the declaration. Since GH #487 that holds in both. Before it, at boot only, although it is the spelling the `{"from": "."}` doors below use.

#### `params.contract` (hive only)

GH #173. A hive template is a class: instantiate it, wire to its interface, swap it later for another implementation with a different inside. Before the contract, none of that held for hives. `contract` was a cell property, a hive had `description` prose, and the prose named cells three levels down ("Ingress: `./keeper/stamp`"), so every instantiation wrote the template's internal layout into the caller's own topology.

`params.contract` is the form in which a hive meets the boundary rule of `meclaw-overview.md` § The hive boundary, which holds for all hives and all templates. Three things follow from that rule and land here in the file: the address is the hive, hence `"ports": []` next to the contract; a lane is named functionally, hence `accepts[].route` and `emits[].route` are requests and never places; the inner edge is the only place structure may be known, hence the mapping from lane to cell lives in the `{"from": "."}` edges of `params.graph` and nowhere else.

The contract says all of that in the only vocabulary that survives a reimplementation, lanes, which are `hop.route` values:

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

Read as: *send me a message at my own path whose `hop.route` is `in_batch`, and I will hand you back messages at my own path whose `hop.route` is `episode`.* No cell of the hive appears anywhere, which is what leaves the inside free to change.

| Key | Content |
|---|---|
| `accepts[]` | Lanes a caller may send **into** the hive path. |
| `emits[]` | Lanes the hive sends back **out** through its own path. |
| `…[].route` | The `hop.route` value that **is** the lane. Never a cell name, which would put the hive's interior into the caller's topology. |
| `…[].context` | `context` keys a caller must have promoted beforehand. A requirement, and it is checked (see below). |
| `…[].at` | A list of scope-relative paths, where this lane connects at this hive, the connect point of a v-lane (GH #559). Optional; always a `./…` path strictly below the declaring hive, and `"."` matches nothing. |
| `…[].required` | `true`: the instantiating mutation **must** wire this lane, onto the hive path for a rim lane, onto one of the `at` connect points for a lane that docks below the rim. Without that edge the birth is refused with `hive_contract`. Checked once, at birth; a later `remove_edges` on a standing hive is not judged. Only meaningful on `accepts`, since nobody wires an exit from outside. Optional, default `false` (apps rim, 2026-09-05). |
| `…[].because` | What the lane is for, in the hive's own words. Travels verbatim into a rejection. |

`at` is the connect point of a v-lane (GH #559). A v-lane is an edge that skips levels and names its lane explicitly (`meclaw-overview.md` § v-lanes). `at` is the half of it that belongs to the target: the permission to connect deep on this lane, and the statement of where. Without `at` there is no connect point for that lane, a v-lane onto it is refused with `v_lane_no_connect_point`, and a sealed level in between refuses as before with `hive_port_boundary`. A crossed level that declares the lane without a matching `at` is a mandatory hop and may not be skipped (`v_lane_mandatory_hop`).

A lane with `at` is no lane of the rim (GH #562). It docks below the hive path by declaration, so two rules that are about rim traffic step aside for it. The hive owes it no door out of `.` and no exit back into it, because its door is the connect point, and the lane-door check (`hive_contract::check_lane_doors`) skips an entry that names one. The obligation is replaced and not dropped: an `add_edges` entry that states the lane into the hive path is refused `hive_contract` and told which connect point to end on instead, and a parent does not carry the lane in the union of its occupants' lanes. Everything else is unchanged. The connect points are policed per edge at mutation time, and a lane without `at` is judged exactly as it always was. A connect point counts from instantiation on: when the same mutation brings the hive into existence, its contract is read out of the template's staged subtree, with `ref` markers resolved, so it is found in an occupant that comes from a different template too (GH #567).

```json
"contract": {
  "accepts": [
    {"route": "in_pack",
     "at": ["./talky", "./cogny"],
     "because": "the identity pack reaches both brains of this generation directly"}
  ]
}
```

Read as: *whoever sends me `in_pack` may draw the edge as far as my occupants `talky` and `cogny`, and to no other.* The rest of the inside stays as hidden as it was. `at` names exactly the points the contract opens for this one lane, and the enumeration is the boundary rather than an example.

A lane name must say what the caller wants, never where it lands inside. This is the half of the boundary rule that `ports: []` alone does not state, because a port that becomes a lane of the same name is the same interior cell name in a different field:

| instead of (structural) | functional | because |
|---|---|---|
| `writer` | `in_episode` | the caller hands over a turn; whether a cell called `writer` receives it is the hive's business |
| `recall` | `in_query` | what is asked for is memory, not a recall cell |
| `render` | `in_view` | what is wanted is a picture, not the invocation of a renderer |
| `policy` | `in_decide` | what is requested is a decision, not the route to one |

The test: does the name survive a reimplementation of the inside? If a rebuild that breaks no promise makes the lane name wrong, the name was structural.

Enforcement levels:

| Check | When | Effect |
|---|---|---|
| An `add_edges` edge **from outside** onto the hive path whose `set_hop.route` is **constant** must name an `accepts` lane | mutation | reject `hive_contract`, pre-destructive |
| An `add_edges` edge from a node **strictly inside** the hive onto the hive path is an **exit** and must name an `emits` lane (GH #602) | mutation | reject `hive_contract`, pre-destructive |
| Every `accepts` lane must route **inward** from the hive path (have a door) | mutation (post-state) | reject `hive_contract`, rollback |
| Every `emits` lane must route **outward** through the hive path from **some** interior cell, carried out or produced by the door itself | mutation (post-state) | reject `hive_contract`, rollback |
| the same four checks | boot | `warn!` only, since the birth topology is sovereign (as with GH #133/#147); one line per half, the **first** violation — the rim-direction checks and the own-graph checks are two halves of one rule and speak alike at boot |
| An edge naming an `accepts` lane **constantly** must carry that lane's `accepts[].context` keys, on the edge itself (`set_context`) or reachable backwards from its `from` | mutation (post-state) | reject `hive_contract`, pre-destructive |
| the same check | boot | report only: `warn!` per finding, and `--validate --validate-strict` turns it into an error |

Which of the two lane lists applies to an edge onto the hive path is decided by direction, not by the target (GH #602). An edge ending on the hive path is both halves of the interface: from outside it ENTERS, from a node strictly inside it LEAVES, because the message crosses the rim outwards. Reading `to` alone made every exit look like an entry, so the catch-all the member ships (`./channels -> .` stamping `hop.route = 'error'`, GH #598), a lane the member **emits**, was refused as a lane it does not accept, while the boot instantiated the same edge with a warning at most. An edge whose `from` is the hive path itself leaves nothing and stays an entry. An `emits` lane with `at` is not measured against the rim address on the way out: `at` says where a lane DOCKS on this hive, which is a statement about arriving traffic; whatever carries the lane from inside out through the hive path names a declared lane and passes. The way back of such a lane belongs to the level that declared it, not to the caller `at` gives permission to.

The check runs the real router (`apply_edges`) instead of comparing condition strings. The migrated templates open a whole family of lanes with a single `hop.route.startsWith('in_')`, and no text comparison finds `in_batch` in that. The caller's stamped route is read the same way: the edge's own `set_hop.route` expression is compiled and evaluated against empty headers. A literal (`'in_batch'`) yields a string; anything reading the incoming message (`hop.upstream_route`) fails and is skipped. A check that cannot place an edge must never reject it.

An exit may also create the lane (GH #176). A probe carrying only `hop.route` finds exits that already carry the lane, and nothing else. A hive's failure lane does not work that way: the door recognises something only the inside knows, an `llm` cell's `hop.finish_reason`, and translates it into a lane on the way out, which is exactly what the boundary is for. So the exit check also reads the door's own `set_hop.route`. If that names the declared lane as a constant and the edge crosses the hive path, the edge is an exit for the lane, even where its condition is unreachable for a route probe. The condition stays unevaluated, and that is meant: whether the door ever fires is a statement about the messages the inside produces, whether it names the lane is a statement about the door. A door that names a different lane is no exit for this one, and neither is a door that names the lane on an edge staying inside, because a caller cannot receive what never crosses the boundary. If the expression is computed rather than constant, the sentence above applies again: the check cannot place the edge and therefore does not reject it.

Four things are deliberately not checked.

- A hive with no edge at its path. A contract is a statement about the hive *path*; if no edge touches that path, the hive is an island, freshly instantiated or disconnected by `remove_nodes`, and its contract is dormant. Without this exception a contracted hive could not be removed at all.
- `accepts[].context` up to GH #291. This entry said the key was unchecked, and that is retracted: the key is checked (table above). Two cases stay unchecked, both out of the same conservatism as the rest of `hive_contract`. An edge whose route is computed rather than stated, where which lane it means is knowable only once a message exists, and what cannot be placed must not be rejected. And an edge whose caller side is a hive path with no inbound edge, where nothing can be delivered, so the requirement stays dormant until one inbound edge lifts it.
- How a lane is named. `writer` is as valid a string as `in_episode`, and no validator can see whether a name was chosen functionally or structurally. The requirement binds regardless; it rests on a reader and not on a check.
- A caller's subscription condition. The shipped topologies tell some lanes apart by a second hop key (`hop.round_capped`, and since `collector@3.5.0` `hop.partial` beside it; the first is raised by either cap, the byte one and the iteration one alike, the second only by the iteration cap that ended the round) that a route-only probe does not carry. That check would refuse correct wirings, so it does not exist.

Since GH #612 `ports` seals delivery too, not only wiring: a message from outside the colony that names a cell inside this hive is refused with `hive_boundary`, unless the address is listed here in `ports` or is the connect point of a `contract` `accepts` lane and the message carries exactly that lane. A hive with no `ports` key is untouched by this.

A port is the name of a lane and never the address of a cell (`meclaw-overview.md` § The hive boundary). `ports: []` and a `contract` are the two halves of one target shape, and every hive is meant to arrive there: the address is the hive path, the lane is the port, and the inside is nobody's business. Which shipped hives are already there, which are mid-migration and which have not started is listed in `meclaw-overview.md` § Current state and migration. A hive with no `ports` key has not started, which is the unsealed state and no exemption from the rule.

### `contract`

The `contract` keys are organized by enforcement level. Not all of them are substrate-enforced in v0.1.0.

| Key | Enforcement (v0.1.0) |
|---|---|
| `emits` | **substrate-enforced**: validated always-on at the `code` type (P13/D-017); remaining emitting cell types post-v0.1.0 (see § Schema format and validation; contract validation for the rest is a roadmap defer). |
| `version`, `settings`, `consumes` | **substrate-enforced**: presence and JSON type at config load (boot hard fail; mutation reject `contract_incomplete`). |
| `capabilities` | **discovery-only** *(specified, not built — see GH #254)*: hint for builder composer/audit tools, **no runtime check** until the hardening (see the `capabilities` note below). The key is unchecked and also unread: `ContractBlock` (`crates/meclaw-colony/src/config.rs`) has no such field, the key is dropped silently at config load, and no API exposes it. |
| `write_surface` | **substrate-enforced, opt-in** (GH #260). `"internal"` bounds the writes the substrate answers before `handle()` to the cell's parent scope; an absent key means `"open"`, so no effect (see below). |
| `transfer` | **substrate-enforced, opt-in** (GH #314). `"none"` exempts this cell's `cell.db` from the `transfer` body slot, export as well as import; an absent key means `"all"`, so no effect (see below). |

The `version` format is a non-empty string, freely choosable, with no semver requirement. The `settings` format is an object `{ "<key>": SettingSpec }` (§ `SettingSpec`), and an empty object is permitted. The `consumes` format is an object (§ `consumes`), and an empty object is permitted. Optional keys: `tools`, `multi_send_capable`.

The body follows the universal body format. Top-level slots are primarily `system` and `messages[]` (see `meclaw-overview.md` § Body format (universal)). Cells may declare their own top-level slots (`meta`, `delta`, `event` and others). `emits.body` and `consumes.body` declare the slots this cell writes or reads. Unknown top-level slots in an incoming message are ignored by the consumer.

#### `emits`

`emits` declares what the cell writes into its output message. It splits into `body` (the actual content) and `hop` (the isolated cell output to routing metadata). Cells emit only `hop`. `context` is solely edge authority and does not appear in `emits` (see `meclaw-overview.md` § Headers vs. body: write model). The cell produces content JSON; the colony interprets `content.header` as `hop` and takes the rest as `message.body`.

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

An `EmitSpec`:

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
- `description` optional, one sentence, saying what this slot means for whoever reads the contract. The deserializer is not `deny_unknown_fields`: the key was always permitted and merely never written down, nothing evaluates it, and it is documentation at the place of declaration. It is worth writing where the name alone does not carry, for a cell-specific top-level body slot (`recall_diagnostic` in `memory-hive`'s `recall`) or a `hop` marker whose absence is the statement (`recall_empty`). An explanation of that length belongs here rather than in a comment JSON does not have.

#### `consumes`

`consumes` declares what the cell reads from the incoming message. It splits into `body` (content slots) and the two header compartments `context` (persistent) and `hop` (exactly this hop). Cells read all three read-only and have no knowledge of who set the value when, which is a topology matter. The lifetime of a header is determined purely structurally by the compartment name: `context` is persistent, `hop` is hop-local and expires. No per-key lifetime annotation exists.

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

A `ConsumeSpec`:

```json
{
  "type":     "string|number|boolean|object|array|blob_uuid",
  "required": true
}
```

If a required value is missing, the cell is not called and an error message goes to `reply_to`, or to the dead-letter queue when `reply_to` is unset.

The build-time mutation and locality validator uses `emits.hop` (what the cell produces) together with `consumes.context` and `consumes.hop` (what the downstream cell expects) to check locality and reachability of a header value statically. A `hop` value is available only at the immediately following hop, unless an edge carries it forward via `set_context`; a `context` value is available across the entire lifecycle. Hive transits participate in the fan-in intersection: an edge with a hive `from` is a transit pass-through and contributes `set_hop` of this edge ∪ the intersection of the contributions of all inbound edges of the hive, recursively across multi-stage transits and cycle-safe. That is the same key walk the runtime performs at transit, where `hop` expires only at a cell emission and not at the transit.

At bootstrap the locality checker carries contract obligations only for active nodes, the nodes that participate in the active graph. A registered but disconnected or inactive node (persisted `colony.db` status at reboot, or an island derived as inactive from t0 at first boot) is pure bookkeeping: it is rehydrated with a stable `cell_id`, and at boot it is subject to no contract enforcement. The full check resides at the mutation moment that connects it (participation rule plus transit-aware intersection). The check is therefore uniform across both boot kinds. Inactive means no boot obligation, active and wired means sharply checked.

Which graph is checked (GH #178): the one the colony actually runs with. On a first boot those are the `params.graph` edges of the `config.json` files, on a reboot it is the persisted edge table, the same authority the boot loads its edges from. Since GH #186 the same cut also answers which paths are hives: on a reboot the persisted `hive_scopes` table, on a first boot the `config.json` walk. Otherwise the checker read a hive whose directory had been removed as a cell with no contract, a transit pass-through became a node that contributes nothing, and the fan-in intersection came out empty even though the topology delivers the key. Before that, a reboot's checker saw only the files, so a partial graph, and partial is worse than empty here: a hive that wrote down its doors gave an interior cell an incoming edge and thereby took away the lenient branch for "no incoming edge, so ingress at birth", while the `set_context` setter sat on a mutation edge the checker could not see.

How a finding lands depends on the boot kind. On a first boot the file is the topology and somebody is writing it right now, so a violation is a loud boot failure. On a reboot the topology is committed state whose mutation edges already passed this same check when they were wired, so a violation is reported (`tracing::warn` per finding, naming node, key and rule) and the colony starts. Refusing there would be a crash loop after the writes are already on disk. To see the finding before the restart, or to enforce it in CI, use `meclaw --validate --validate-strict`, where the same finding is a non-zero exit.

A `consumes.body` key is mandatory by default, and `required` withdraws the obligation rather than the declaration. With `required` absent it is `true`, and `validate_consumes` (`crates/meclaw-core/src/contract.rs`) requires the key in the incoming body and otherwise reports `required consumes.body '{key}' missing`. `required: false` takes the key out of that check, and that is more than the field name suggests: `validate_consumes` walks only the required-key projection, so the type check falls away together with the presence obligation. An optional key that is present is currently not validated in any way, and its `type` token documents the slot and gates nothing. What remains is the declaration: the cell reads the slot, it merely does not demand it. Most shipped templates declare `system`, `messages` and `op` optional, which is the common form. What hangs off the declaration does not hang off the obligation (GH #323). A capability switch (the next paragraph) reads the declaration and never the obligation, since otherwise `required: false` would silently withdraw a capability with nothing failing anywhere. One consequence for `/colony` roundtrips: a `/colony` endpoint answer (`{"mutation":{…}}`, `{"rescan":{"status":"ok"}}`) is no UBF body, it carries no `messages[]` and therefore bounces off every contract that declares `messages`. A cell that runs a `/colony` roundtrip therefore declares an empty `consumes.body`.

A declared `consumes.body` key can also work as a capability switch (GH #87). The first case is `consumes.body.attachments`. Only a cell that declares the slot receives the read-only blob-store handle at spawn with which it resolves `attachments[]` refs itself at `handle()` time (`meclaw-overview.md` § `attachments[]` schema, owner ruling GH #19; consumer detail in `cell-types.md` § `llm`). Without the declaration the handle does not exist, so the cell cannot read an attachment at all rather than merely not doing so. The switch is the declaration and not the presence obligation: `"attachments": {"type": "array", "required": false}` grants the handle just the same and still does not demand attachments on every message (GH #323). Since a missing handle is indistinguishable, to the cell, from "no blob store wired", this is no detail: a declaration the switch fails to see withdraws the capability silently.

`consumes.topology` declares a cell's own place in the graph, never the graph (GH #160). It is a fourth field beside `body`, `context` and `hop`, and the only one that is no message compartment. Nothing in it is validated against an incoming message, a key declared here never makes a message invalid, and `validate_consumes` does not see it at all. The field is a pure capability declaration in the same grammar.

```json
"consumes": { "topology": { "inbound_edges": { "type": "array", "required": true } } }
```

The only key the substrate knows is `inbound_edges`, the `from` paths of every edge pointing at the cell's own path. A declaring cell receives a read-only handle at spawn (`meclaw_colony::NeighbourhoodView`) that asks exactly that one question, live against the colony's in-memory `EdgeTable`, bounded by the cell's own operation timeout, self-scoped. It is neither the graph (that is `/colony/graph`) nor a scope nor its own outbound edges, and never another node's. Without the declaration the handle does not exist.

#### `contract.ingress`

A cell declares with `contract.ingress` that it is an entry point (GH #185). The block sits beside `consumes` and `emits`, in the same grammar and for the same reason as `consumes.topology`: a capability that is declared rather than inferred from the shape of the graph.

```json
"contract": { "ingress": { "context": ["chat_id"] } }
```

The value is the list of context keys this cell may mint when a message is born, and no boolean. The list may only narrow the standard set (`INGRESS_CONTEXT_KEYS`), never widen it, and a key outside it is refused by name. A boolean would have granted the whole standard set to anything saying "I am an entry", which is the same all-or-nothing generosity as the inference this block replaces.

Until GH #185 the header check read "has no incoming edge" as "is the graph's entry". That held while the check saw only `config.json` edges. Now that it sees the running graph, a genuine entry that also receives replies, the ordinary shape of a proxy, loses exactly that branch. The declaration makes the question locally answerable, and adding an unrelated edge no longer changes the answer.

The block sits beside `emits` and not under it, because cells emit `hop` while `context` is edge authority alone (§ Access). The birth of a message is the sanctioned exception to that, and no cell emission.

#### `contract.write_surface`

A cell bounds with `contract.write_surface` what the substrate writes on its behalf (GH #260). The key sits beside `consumes`, `emits` and `ingress`, in the same grammar and for the same reason: a statement about this cell's place, and not about a message.

```json
"contract": { "write_surface": "internal" }
```

The values are `"open"` (the default, and what an absent key means) and `"internal"`. `"internal"` means a write the substrate answers before `handle()`, today exactly one, the `import` of the `transfer` body slot (`cell-types.md` § Content transfer), is refused when its sender lies outside this cell's parent path. The refusal carries `error_code: "write_denied"` and lands before the first row is written. It is fail-closed: a message with no sender, a source message from an ingress or an event, is outside. An `export` is a read and is never bounded. A cell sitting directly under the colony root has `/` as its parent path, which contains every cell, so the declaration is inert there.

The key sits in the `contract` block and not in `params` because the substrate is type-agnostic. `params` belong to a cell type, while the slot this rule bounds sits above all eight cell types with a `cell.db`. A rule the substrate enforces is declared where every cell type declares in the same grammar. Anything else would make the substrate read a type's `params`, which is exactly what it must not do.

`store`'s `params.write_surface` (GH #132) is the other half of the same boundary, and the two are not derived from one another. `params.write_surface` bounds the ops a `store`'s `handle()` runs, `contract.write_surface` bounds the ones the substrate runs before `handle()` is ever reached. The scope arithmetic is the same in both, so a cell declaring both gets one boundary instead of two that disagree. A cell declaring only one has only one.

This replaces the last direct `colony.db` read in the tree, the `vault` unlock attestation, and `meclaw-overview.md` § Database isolation has had no exception since. The first and only consumer today is `vault`: a cell that cannot verify its neighbourhood stays LOCKED, which is why a `vault` `config.json` without this declaration never unlocks.

#### `contract.transfer`

A cell declares with `contract.transfer` that its database does not travel (GH #314). Same place, same grammar and same reasoning as `write_surface`, except that this key answers whether this cell responds to the seam at all instead of who may write.

```json
"contract": { "transfer": "none" }
```

The values are `"all"` (the default, and what an absent key means) and `"none"`. `"none"` means the `transfer` body slot (`cell-types.md` § Content transfer) is refused for this cell with `error_code: "transfer_exempt"`, `export` as well as `import`, because a store that may not leave may not be overwritten through the same seam either. The refusal lands before the arguments are read, and it names no table, because a refusal that sounds different per table name is an inventory. A typo in the value is a parse error and never a silent fallback to `"all"`: a misspelled exemption would quietly mean "travels after all".

The exemption is a declaration and no list inside the substrate, for a reason. An exclusion list of cell-type names in `db_transfer.rs` would be invisible in the `config.json` of the cell it applies to, invisible in a diff, and would have to be edited again for the next cell type with the same need. A declaration binds a cell type nobody has written yet.

`contract.write_surface` and `contract.transfer` are two independent statements about the same seam. `write_surface` bounds the write half to the parent scope and leaves an `export` untouched, since no write surface has ever bounded a read, and that gap is exactly why GH #314 was opened: the `vault`'s disclosure was a read. A cell that wants both declares both; one does not switch on the other.

GH #336 (`access@2.0.4`) retracts what used to read "first and today only consumer: `vault`", which was true only for as long as the `vault` was the sole declarant. It is two cell types across three shipped configs: `vault` (`templates/vault`, `templates/access/vault`) and the capability broker's `store` (`templates/access/store`), whose `grants` are live bearer handles. An export is a read, which `contract.write_surface` explicitly does not bound, which is why migration there means re-granting at the target and not importing. `cell-types.md` § Content transfer carries the same retraction.

#### `params.transfer.base_path`

`params.transfer.base_path` is the fence a cell manages its own files inside (GH #555). It is the counterpart to `contract.transfer` and sits on the other surface. The owner's ruling of 2026-09-04 reads, verbatim, *"cells manage their own files, nobody else does"*, and where an instance keeps its files is a statement about that instance, not about its role. Two cells of the same template must be able to write into different directories, and with the fence in the `contract` they would share one. Hence `params`, exactly as for `file`'s `base_path`.

```json
"params": { "transfer": { "base_path": "/srv/meclaw/export" } }
```

An absolute path, or none at all. It bounds the file half of the `transfer` body slot (`cell-types.md` § Content transfer): `{"operation": "export", "to": "<dir>"}` and `{"operation": "import", "from": "<dir>"}` resolve `<dir>` relative to this directory. With the key absent the cell has no directory and falls back to none, every named `to`/`from` is refused with `error_code: "transfer_path_out_of_bounds"`, and that is exactly the default: a cell that says nothing writes nothing. Absolute is required because the cell task knows no colony root (`root` lives in the colony struct and never reaches `spawn_cell`), so a relative fence would resolve against the process's working directory and would not be a boundary. A relative value, a non-string, or a `transfer` block that is not an object are therefore loud boot errors, like a broken `emits` schema declaration.

Nothing here is canonicalised and nothing is checked for existence, neither at boot nor at `--validate`. The parse checks the string and `is_absolute`, and nothing else. That is a decision with a receipt: a `file` cell canonicalises its `base_path` in `validate_params`, which is why the shipped interim export sink was a `code` cell instead of a `file` cell. A member whose export directory did not exist yet would otherwise fail `--validate` and fail to boot, for a lane nobody had used. A fence says where, and it does not require the where to exist yet. If the directory is missing, the first `to`/`from` finds out, as `error_code: "transfer_io_error"` on the message.

`contract.transfer`, `contract.write_surface` and `params.transfer.base_path` are three independent statements about the same seam. `transfer` says whether the database answers the seam at all, and it strikes first, before any path is resolved. `write_surface` says who may write, and it holds for `from:` exactly as for a message `import`, because reading the document off a disk does not make it a different operation. `params.transfer.base_path` says where. None switches on another.

The substrate-side required-`consumes` check runs at the delivery boundary, before `handle()`. A missing or type-wrong required key produces an error message to `reply_to` (`error_code: "consumes_violation"`), or a dead letter with the same token when `reply_to` is unset. The error reply is delivered directly to `reply_to` through a registry lookup via `route()`, never through the consumer's out-edges. It is feedback to a known sender and no routing target (W2b ruling 2026-06-12; see `meclaw-overview.md` § Behavior on routing errors, "Outputs arm: three disjoint cases", case 2). A catch-all out-edge of the consumer does not redirect the error reply.

#### Schema format and validation

- Schemas follow JSON Schema Draft 2020-12 (Rust: `jsonschema` crate).
- `code` is an always-on trust boundary with no opt-out. The `emits` validation of the `code` output runs unconditionally (`validate_emits = true`), independent of the build profile and of `colony.json` `strict_validation`. `code` is the only user-script-driven output whose correctness does not follow from cell discipline, so it is always checked.
- For the remaining emitting cell types, `emits` validation runs centrally at the colony's outputs arm following the debug-on and `strict_validation` model: in the debug build always active, in the release build per `colony.json` `strict_validation: true|false` (default `false`, schema see `meclaw-overview.md` § `colony.json`: schema).
- `strict_validation` therefore controls only the future non-`code` emits validation in the release build. The flag has no influence on the always-on `code` path.

Enforcement state: `code` is always-on, in-cell, two-pass and unchanged; all remaining emitting cell types are validated centrally at the colony's emission boundary (outputs arm), flag-gated following the debug-on and `strict_validation` model. The asymmetry is intended and no drift: `code` checks in-cell always-on with all-or-nothing two-pass, the rest runs centrally, flag-gated and per emission. On a violation the emission is discarded; with `input_reply_to` an error reply follows (`error_code: "contract_violation"`), otherwise a dead letter with the same token. The central check has two registered boundaries, a debug net and no trust boundary (ratification 2026-06-10): error replies to an `input_reply_to` that points to a `/colony/*` endpoint or a hive path are silently discarded, so only the cell-path cascade is followed; and a cell that emits in the microsecond window between task spawn and the landing of its `SetNodeContract` entry (self-emitting types at boot) passes the check fail-open, since an absent entry makes the check vacuous.

#### `capabilities`

The capability list is fixed.

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

It is extensible as needed and documented centrally in `meclaw-core`.

The capabilities in this phase are discovery hints for builder composer and audit tools, and no runtime check. **Note on permissions until the hardening** *(specified, not built — see GH #254)*: today they are not even that. The list above describes a block no parser reads and no API exposes, one that exists only in `config.json` files on disk. This applies in particular to `mutate-graph`: whether a cell actually _can_ mutate depends solely on the topology, that is, on whether an edge to `/colony/mutations` exists. Post-roadmap hardening may add capability tokens that are checked at runtime. See `meclaw-overview.md` § Permissions in the mutation format.

#### `ToolSpec`

Declares which tools the cell offers to its LLM (or external consumers) *(specified, not built — see GH #254)*. It is no routing endpoint: where tool calls are routed is decided by the topology. Today nobody reads `contract.tools` (`ContractBlock` does not know the key) and no shipped `config.json` writes it. The tools an `llm` cell actually offers live in `params.tools` (see `cell-types.md` § `llm`).

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

`multi_send_capable` says that the cell can produce multiple output messages from a single input. It activates the cell-type-specific multi-send wire format, for `code` the JSON-array format on stdout (see `cell-types.md`). Each emitted message runs independently through the outgoing edges, and the colony evaluates freshly per message. The value comes from `contract.multi_send_capable` (bool, default `false`). The former `params.multi_send_capable` bridge is removed, and a `params` value is ignored by the `code` factory.

### `description`

Six keys, **builder-enforced**, not substrate-enforced: the structure takes effect as soon as the builder or composer consumes it, the same discovery contract surface for the LLM builder that writes edges and for a reviewer or operator, and it is no boot validation in the substrate. As a top-level block **of a cell's `config.json`** this is *(specified, not built — see GH #254)*. `ParsedConfig` (`crates/meclaw-colony/src/config.rs`) carries exactly `cell`, `params` and `contract`, and a `description` beside them is dropped silently. Today the block is read only on `template.json`, by the template scanner (`crates/meclaw-colony/src/templates/scanner.rs`), and there it is built.

| Slot | Content |
|---|---|
| `purpose` | Why does this cell exist? What problem does it solve? (1-2 sentences) |
| `use_when` | When does the composer reach for this template? Preconditions, alternatives. |
| `not_in_scope` | What does this cell deliberately **not** do? Helps the builder exclude the cell when it does not fit. |
| `emits_meaning` | Semantics of the `contract.emits` entries: what do they mean beyond type info? |
| `consumes_meaning` | Semantics of the `contract.consumes` entries. |
| `examples` | Concrete input/output examples; at least one. |

At hive scope markers (`cell.type: "hive"`) the `description` describes the scope purpose: what does this hive bundle, when does the builder use it, what does not belong in it. `emits_meaning` and `consumes_meaning` are omitted, since hive scope markers do not participate in the message flow.

## Origin (`cell.provenance`)

An instantiated cell is a detached copy. `template.json` is dropped at staging, and the tree can be exported, backed up or moved to another machine. For a template to find its instances later, in an app-store update, the node has to carry its origin itself. No colony is there to ask when only the tree arrives.

The origin has two homes and one truth. The source is `cell.provenance` in the instance's `config.json`. It travels with the directory, and a backup, a copy and an export carry it verbatim. The index is the four `registry` columns `template`, `template_version`, `instantiated_at` and `template_chain` in `colony.db` (since schema v6). The first three answer "which nodes came from `sink-tpl@1.0.0`?" with one SQL statement instead of a tree walk. The fourth carries the chain as JSON and answers the question the leaf stamp alone cannot: which instances does a bump of an inner template touch?

The index is filled in two places, at instantiation, where the mutation knows the template, and at every boot, from the `config.json` that was read. The second one is the important one. A config-only copy brings no `colony.db`, so its index starts empty, and without the boot pass it would silently claim "no origin" while the files next to it say otherwise. A node without `cell.provenance` sends nothing and keeps `NULL`.

A composite template may place foreign templates as `cell.type: "ref"` sub-units, and the placed tree is copied along at staging like everything else. The leaf stamp alone then answers "what is this node?" and no longer "which instances does a bump of `inner` touch?". Before GH #277 the composite's name was recorded there and the inner template's was missing, and the leaf stamp alone would merely reverse that loss. `template_chain` holds both ends: an update addressing the composite finds the node through the first entry, one addressing the referenced template through the last. A missing key means "written before the field existed" and never "no chain", and `instantiated_at` stays the same timestamp for every node of one instance. The index in `colony.db` carries the chain along in `registry.template_chain`, a JSON list written at instantiation and at every boot. A `NULL` and an unparseable value both read as "no chain was recorded": the instance's own `config.json` remains the source, the table is the index, and a broken index entry costs a query hit and nothing more.

The stamp is no live binding to the template. The template may change, move or disappear without the instance noticing or changing (§ Access). It is also no reference to a `templates` row: it names the name and the version, not the `template_id`. Since GH #62 the `template_id` is stable across rescans, but it remains a colony-local surrogate key and is meaningless in an exported tree.

## Snapshot versus live-read

Not every artifact under `{root}` is read again when a colony boots. Knowing which is which decides what a backup has to move and what a restore actually restores.

### Live-read at every boot

`config.json` is re-read and re-parsed on every boot, and `${VAR}` tokens are substituted in memory against `{root}/.env` each time. The on-disk file is never rewritten at boot, since instantiation is its only writer. `.env` is therefore live: changing a value and rebooting changes the effective params. Instantiation does not freeze the environment class either. `${VAR}` and `${VAR:-default}` survive the write literally, so an exported tree carries the tokens and not the values, and secrets stay in `.env`. The escaped form `$${VAR}` survives instantiation unchanged too and becomes the literal text `${VAR}` when read, binding to nothing. The price of that late binding is a standing dependency: a `${VAR}` with neither value nor default fails the boot loudly (`env_var_missing`, naming the variable) instead of quietly yielding an empty value.

The empty value is the case next to it. `VAR=` in `.env` and `${VAR:-}` both yield `""`, and what that means is decided by the cell at parse time and not by the substrate. The rule is the same across the tree (GH #268, GH #270). An empty optional credential is no credential (`web_search.api_key`, `mcp.auth.bearer`, where the cell sends no `Authorization` header at all rather than a header with nothing after it), while an empty required value is a configuration error and is rejected at parse time exactly like a missing one, with the same message and the name of the key (`mcp.endpoint`, `mcp.command`, `proxy.bot_token`, and the Slack variant's `app_token`/`bot_token`). The reason for the second half: `${VAR}` without a default only catches the unset variable, a half-filled copy of `.env.example` sets it empty, and without this check that would produce a cell which looks healthy and fails at a third party on every call.

`llm.api_key` is the third case, and it shows what the rule actually turns on: what an absence means, and not whether a flag says optional (GH #271). The declaration is required there, so a key missing altogether is a configuration error at spawn, while the value may be empty and then says exactly what it says: this endpoint needs no credential. An OpenAI-compatible server on localhost ignores the header anyway, and the cell sends no `Authorization` header at all, on both dialects. A `${…_API_KEY}` without a default whose variable is set empty in `.env` is therefore the explicit statement "no key is needed here", and something else than a forgotten entry. Refusal stays reserved for values whose absence has no working meaning: a `proxy` without a bot token can do nothing at all, an `llm` against an anonymous endpoint can do everything.

### Birth snapshot in `cell.db`

A cell's `cell.db` is created at its first spawn and seeded exactly once, on `OpenStatus::Created`. `timer` copies `params.schedules` and `store` loads `seed/*.jsonl` at that moment and never again. After the first boot, `cell.db` is the truth: editing `params.schedules` in `config.json` changes no existing schedule and adds no new one. The runtime params overlay (`cell.db` `params` table) likewise wins over the `config.json` birth params for the keys it holds. To make a `config.json` edit of a birth-snapshot field take effect, the cell must start without a `cell.db`.

Two seeders run at the birth of a `cell.db`, and since GH #456 a third writer joins them. The paragraph above describes the spawn path. Beside it stands the mutation staging seeder (`mutation::stage::seed_cell_db_if_present`), and the two read different things. The `store`'s spawn path walks the tables declared in `params.schema` (`seed::check_seed_files`, then `load_seed_if_present`), and a `seed/<file>.jsonl` whose name matches no table declared there is neither checked nor loaded by it. The staging seeder reads every `seed/*.jsonl` in the directory, without knowing `params.schema` at all, and writes it into a fresh `cell.db` at instantiation, so before the first spawn. It keeps out only of cell types that own their own schema (`CellFactory::owns_schema`, GH #398). So a table `params.schema` does not name is seedable too. The alias and rejected-pair tables of a `canonical` declaration are the example: they are built from the seed's header line and therefore without their key, and `apply_canonical_ddl` rebuilds them with it at the first spawn through `ensure_keyed_table`, carrying the rows over (GH #255). A seed for such a table therefore takes effect through the staging path and only through it, and a hand-written tree that the boot finds has no reader for it.

The third writer is a declaration and no file (GH #456). Both seeders above run once, at the birth of a `cell.db`. The diff operation `seed_rows` writes into a `cell.db` that already stands, including that of an awake cell, and it is no third mechanic: the same JSON to SQL binding, the table built from the declared column list, `ensure_keyed_table` supplying a missing key at the next wake. What distinguishes it is the door and not the writing, namely a digest, a gate, an access verdict and a `mutation_log` row (`meclaw-overview.md` § Mutation operations). It is therefore meant for the rows that are permissions and keys, a policy row, a grant, a firewall rule, and not for bulk data. Two consequences the two seeders do not have: a `seed_rows` is substituted, since it is part of the diff, and a `params.write_surface: "internal"` does not bound it, since that key binds messages to a cell while `seed_rows` goes through the write authority itself and is bounded by the mutation scope plus the access verdict over it.

### Birth snapshot in `colony.db`

A node's `cell_id` is minted once and carried in the `registry` table, stable across reboots. `params.graph.edges` and hive scopes are applied on the first boot and persisted; on a reboot they are hydrated from `colony.db` and the `params.graph` hints are ignored, since GH #168 by the bootstrap planner too, which used to validate them and die on an edge a mutation had long since removed. On a reboot, a cell directory absent from the registry is reported and never adopted, since registration happens only through instantiation or mutation. Both hold for edges: the boot instantiates no node at all, and the `nodes` block described in `meclaw-overview.md` § Graph schema is marked "specified, not built" there (GH #277), so in a `config.json` it aborts the boot today. This paragraph describes current behaviour, that section the target picture. The `templates` index is a scan snapshot holding absolute filesystem paths, and a boot re-scans only when the table is empty, otherwise an explicit `--rescan-templates` is required.

`hive_scopes` has no delete path, and that is the policy rather than a gap in it. No `DELETE FROM hive_scopes` exists anywhere in the crate: the first apply writes the rows, a mutation adds one for every hive it creates, and nothing ever removes one. The first reason is structural. A hive has no registry row. The `registry` holds cells with mailbox, status and `cell_id`, and a hive is no cell, so the colony needs a second list to know that an address holds a hive rather than something it can deliver to. That distinction is what routing turns on: an edge into a cell delivers into its mailbox, an edge into a hive delivers nothing, and the colony reads the hive's own edges and routes on.

The missing delete is therefore no special case. `remove_nodes` is disconnect instead of delete: the edges go, the registry entry stays, the directory stays. Nothing is deleted anywhere, so a scope row outliving its hive is the same no-delete policy applied consistently. The consequence is accepted: a path that was once a hive keeps being a hive to every reader of the table, the boot's fan-in walk included.

GH #186 made `hive_scopes` the boot authority for which nodes are hives on a reboot because the table is append-only. A stale row is helpful there: a hive whose directory was wiped is still read as a transit rather than as a contract-less cell (§ `contract`, the mutation and locality validator). The question only becomes a real decision at a hive relocation, which `move_nodes` refuses today, and under this policy the answer is already framed: a hive moves by its scope row moving with it, and not by one being deleted and another created.

### Backup and restore

The restore unit of a cell is its directory and not its `config.json`: restoring the config restores the declaration, never the birth-snapshot state. The restore unit of a colony is `{root}` as a whole, including `blobs/` and the SQLite WAL sidecars `*.db-wal` and `*.db-shm`. A backup that matches `*.db` alone can restore a colony that boots cleanly and runs the state from before the last writes. A restored tree whose `colony.db` came along keeps its identity, while a config-only copy is a new colony with re-minted `cell_id`s. After relocating a tree, run `--rescan-templates` so the template index points at the new root, and `--validate --validate-strict` to surface cell directories the restored registry does not know.
