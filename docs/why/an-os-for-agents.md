# An operating system for agents

"AI OS" is a hyped phrase, so here is the un-hyped version of what meclaw-os is: the part
of every agentic product you were going to build anyway, shipped as templates, grown at
runtime. It is rudimentary and experimental — and it is already opinionated about the
things that hurt when you get them wrong.

## Why an OS at all

Build one assistant and you need a door, a screen for hostile input, a memory, a way to
hold sessions, somewhere for secrets. Build the *second* assistant and the real questions
start: do the two share a memory? Who owns the screen they both draw on? Does the rate
limit reset because you redeployed one of them? A classical single-agent stack has no
place to put those answers, so they end up duplicated, inconsistent, or forgotten.

meclaw-os is that place. Its design rule is one sentence:

> **A level owns what its siblings must share.**

## The four levels

| level | owns | because |
|---|---|---|
| **shell** (`meclaw-os`) | the capability broker, the control loop, the authoring path (builder + submit), the one front door | every organisation asks the same broker |
| **organisation** (`org`) | a name and a boundary — no cell at all | a group is an audience, not a holder |
| **member** | the memory, the curated record, the screening, the channels, the screens | two assistants of one person must know the same person — and must meet one attacker |
| **assistant** | one generation: conversation surface, reasoning core, tool surface | an agent generation should be replaceable without touching what the person owns |

The consequences are concrete. Memory belongs to the **member**, not the assistant — so
replacing your assistant does not amnesia your history. The firewall sits **outside** the
assistant — so a new generation still meets the same attacker record and the rate window
does not restart. Channels belong to the member — so one Telegram bot can reach two of
your agents, and a screen both draw on has one owner.

## The authorities

Standing across the levels are hives that decide **without a model** — every verdict is a
comparison or a clock:

- **access** — the capability broker. An agent may ask in natural language; what travels
  is a *handle*, never a credential ([more under Rust & Linux](rust-and-linux.md)).
- **firewall** — size, sender, forbidden literal, rate; each verdict names the rule that
  fired, plus a hardline layer no rule row can lift.
- **session-keeper** — a session as a channel generation, modelled on a phone call, ended
  by arithmetic rather than by judgement.
- **affinity** — the curated record of the people and agents a colony knows: relations,
  trust, disclosure, an append-only audit.
- **argus** — the control loop: measure from the colony's own ledger, change one
  parameter, keep or revert after a window. A receipt for **every** tick, including the
  tick that had nothing to do.

Every authority ships **inert**: every seeded policy row and every charter goal is
disabled. A fresh shell grants nothing and changes nothing until an operator turns on
exactly what they mean.

## Apps

An **app** is a composed use of templates for one task, tagged `app` in its manifest and
grown under a member. The first shipped app is
[`colony-view`](../../templates/colony-view/): it draws the colony's own topology onto a
display — a screen that belongs to the person, on which several agents can hold views and
none can touch another's. One screen, one app today; this is the youngest part of the
tree, and [where it is going](../../ROADMAP.md) is voice in, vision out.

## What "grown, not installed" means

The OS is a template (`meclaw-os`). A seed with a single `ref` declaration grows the
whole shell on first boot, through the same validation any mutation gets — and after
that, adding an organisation, a member or an assistant is one JSON declaration each
([`examples/organism`](../../examples/organism/)). No redeploy at any step. That is the
practical advantage over a classical agent: **a new agent is a grow, not a project.**
