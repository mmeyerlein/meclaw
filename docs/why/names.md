# The strange names

Every name in the tree is a role. One line each, so the vocabulary stops being strange —
the precise definitions live in the [glossary](../glossary.md).

## The shapes

| name | what it is |
|---|---|
| **cell** | the smallest actor: one folder, one `config.json`, one mailbox, one job |
| **hive** | a folder that groups cells into an organ — a scope, not an actor |
| **colony** | the whole running tree |
| **subcolony** | a child colony addressable as a single cell — the same shape, one size up |
| **edge** | a route a message may take; the harness is made of these |
| **template** | a class in the catalogue; instantiating it copies the subtree into your tree |
| **mutation** | the one operation that changes a running colony |
| **manifest** | an ordered list of mutation declarations — what the builder produces |

The biology is deliberate: a colony is a superorganism, and no single cell is the agent.
The agent is the shape.

## The authorities

| name | the role behind the name |
|---|---|
| **access** | the capability broker: agents ask, and get a *handle* — never the credential |
| **affinity** | who this colony knows, and what holds between them: relations, trust, disclosure, an append-only audit |
| **argus** | the control loop — named for the many-eyed watchman: it measures its own colony from the ledger, changes one parameter, keeps or reverts |
| **firewall** | screening: size, sender, forbidden literal, rate — every verdict a comparison, never a model |
| **session-keeper** | holds sessions the way a phone call is held: opened, kept, and ended by arithmetic |
| **vault** | the secret store with no `get` — `put`, `rotate`, `use`, `revoke`, and no operation that returns a secret |

## The assistant

| name | the role behind the name |
|---|---|
| **talky** | the conversation surface — the one that talks: fast answers, owns the window and the session |
| **cogny** | the reasoning core — the one that thinks: one brain, its own errand, a tool menu it asks for |
| **collector** | assembles the context window for the turn that is running, out of the record |
| **memory-hive** | a member's long-term memory as a hive — [why it works the way it does](memory.md) |

## The doors

| name | the role behind the name |
|---|---|
| **door** | where `POST /messages` becomes a turn on the ingress lane |
| **operator** | the one front door a person addresses the OS through — it adds *identity* to a request, and deliberately not authentication |
| **submit** | the only cell allowed to carry a manifest to the mutation door — after checking the digest you said yes to |
| **builder** | turns a wish into a manifest; can draft, can never apply |
| **terminal** | where an undecided lane ends, visibly, instead of vanishing |

If a name still feels odd after its one line, that is usually because the *role* is the
unusual thing — an agent stack that has no place for an `affinity` is a stack where
"who is this person to us" lives in a prompt.
