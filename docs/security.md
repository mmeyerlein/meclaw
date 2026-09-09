# Security

meclaw draws three boundaries: a kernel sandbox around every cell that starts a child process,
a hive that is an address boundary so an edge reaches the hive and never a cell inside it, and
one database per cell that no other cell may read. Secrets live in a `vault` cell that has no
operation returning one.

None of it is switched on by hand. A cell born from a template carries its sandbox profile in
its own `config.json`, and `examples/hard-shell` is a three cell colony whose seed holds no
security configuration at all. The boundaries are properties a reviewer reads off files, which
is what lets a colony be audited instead of trusted.

## The kernel sandbox

`params.sandbox` declares the rights a cell's child process starts with, enforced at spawn by
four Linux mechanisms: Landlock for the filesystem view, `unshare(CLONE_NEWUSER|CLONE_NEWNET)`
for `network: "deny"`, a delegated cgroup v2 sub-cgroup for memory, pids and CPU, and a
seccomp-bpf filter against `ptrace`, raw sockets and signals to processes outside the sandbox.

Four cell types read the block: `bash`, `code`, `harness` and `mcp`. Instantiation writes a
default into the first three, so a template that declares nothing still gets one:

```json
"sandbox": { "trust": "restricted", "network": "deny", "filesystem": { "runtime": true } }
```

An `mcp` cell gets no default and keeps the daemon's rights until its template declares a
profile. `"trust": "trusted"` is the escape hatch and stands readable in the instance. The
block is fail-closed: a `restricted` profile that cannot be applied fails the spawn with
`io_error`, so no path leaves a cell quietly running unsandboxed. `meclaw --sandbox-probe`
asks the host up front which of the four it can enforce. Every key and default:
[`config.md`](config.md) § `params.sandbox`.

## The hive boundary

A hive is the authority and address boundary of its subtree. An edge from outside ends at the
hive path and asks for a lane on `hop.route`; which cell serves that lane is stated once, on
the hive's own door edge inside.

```json
{"from": "./agent", "to": "./access",
 "modifier": {"set_hop": {"route": "'in_request'"}}}
```

`params.ports: []` seals a hive. A mutation drawing an edge past a declared port is refused
with `hive_port_boundary`, and since 0.33.0 a message entering the colony from outside and
naming a cell inside such a hive is refused with `hive_boundary` rather than delivered past
the doors. A hive with no `ports` key is one the substrate does not check, and unfinished.

The rule, its three requirements and which templates have arrived:
[`meclaw-overview.md`](meclaw-overview.md) § The hive boundary.

## One cell, one database

A cell touches its own `cell.db` and nothing else; another cell's `cell.db` and `colony.db`
are closed to it, and a read counts as access. Whoever needs a fact out of another cell's
state sends a message, which is versioned, logged, authorised by an edge and consistent at one
point in time. This binds the substrate too, and `ATTACH` appears nowhere in the tree.

## Secrets

The vault and the capability broker are the one place in a colony that is configured rather than
composed: a secret goes in over the CLI, a policy row decides who may spend it, and what travels
on the wire is a handle. [`security/secrets.md`](security/secrets.md) has both.

## Identities and permissions

`affinity` is the curated record of who a colony knows: one AIeOS document per entity, plus
relations, trust and disclosure as tables. A field reaches an audience because a `disclosure`
row named it, never because no rule forbade it, and the round it is asked in has to be
declared or the read does not happen.

`firewall` screens an ingress channel before the agent sees anything. Every inbound turn ends
on `pass`, byte identical, or on `reject`, naming the reason and the rule that fired. Above
the rows stands a hardline in the template's code that no `update` can reach.

`submit` holds the only edge onto `/colony/mutations` in the whole tree, and no mutation may
draw that endpoint at any scope. Every structural change takes one road, and the drafter never
applies because the edge between drafting and submitting is absent.

## What the daemon does not do

Linux x86_64 only. The release is a static musl build and the installer refuses every other
platform, because the four sandbox mechanisms are kernel mechanisms and a macOS port would
keep the word `sandboxed` in the schema and lose the property behind it
([`why/rust-and-linux.md`](why/rust-and-linux.md)).

The HTTP server installs no authentication, no TLS and no session. meclaw knows paths and
knows no identities, and who may reach the port is the reverse proxy's business, the same as
for any other Linux daemon. The binary opens no port unless you pass `--api`.

## See it refuse

Three commands against a seed that contains no security configuration:

```bash
meclaw --root examples/hard-shell/seed --templates templates --daemon --api 127.0.0.1:7799 &
curl -sX POST http://127.0.0.1:7799/colony/mutations -d @examples/hard-shell/grow.json
curl -sX POST http://127.0.0.1:7799/messages -d '{"target":"/door","body":{"messages":[
  {"origin":"assistant","type":"tool_call","id":"c1",
   "text":"{\"url\":\"http://169.254.169.254/\"}"}]}}'
```

The cloud metadata fetch comes back as `hop.error_code: target_blocked` on a lane of its own,
without an `http_status`, because the address was judged before the connect and no packet left
the machine. Every command with the output it produced:
[`examples/hard-shell/WALKTHROUGH.md`](../examples/hard-shell/WALKTHROUGH.md).

## Where to read more

- [`../SECURITY.md`](../SECURITY.md) reports a vulnerability privately
- [`why/rust-and-linux.md`](why/rust-and-linux.md) why the kernel does the isolating
