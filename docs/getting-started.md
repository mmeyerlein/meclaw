# Getting started

Five steps put a colony on your machine and grow an organisation into it while it answers.
A colony is one tree of cells under one daemon. After the first command, every step is a JSON
declaration posted to `POST /colony/mutations`, the same endpoint an agent goes through when it
changes the tree.

Nothing is deployed and nothing restarts between the steps: you watch a running system gain
three levels it did not have.

## 1 and 2: install meclaw, start meclaw-os

```bash
curl -fsSL https://github.com/mmeyerlein/meclaw/releases/latest/download/start.sh \
    | MECLAW_EXAMPLE=organism sh
```

On a terminal the run asks for an OpenRouter key before it installs anything, and does not echo
it while you type. It writes the key and the model tokens the shipped declarations read into one
file, the colony's `.env`, mode `0600`. `MECLAW_EXAMPLE=organism` picks the seed whose root tree
declares the `meclaw-os` shell: the first boot grows it, thirty-five cells, and one more
declaration adds the colony's front door and the terminal its answers stop in. Leave the variable
out and you get the flat assistant of the [quick start](../README.md) instead, which answers one
question and has no room below it.

The run prints the port it took, the colony directory and the pid. The steps below assume
port `7777`; the browser view is at <http://127.0.0.1:7777/ui/>.

```bash
lib=~/.local/share/meclaw/"$(meclaw --version | cut -d' ' -f2)"   # templates/ and examples/
post() { curl -s -X POST 127.0.0.1:7777/colony/mutations \
              -H 'Content-Type: application/json' -d @"$1"; }
```

## 3: an organisation

```bash
post "$lib/examples/organism/grow-org.json"
```

An organisation is a name and a boundary and holds nothing else, so it grows into the open
container the shell ships for it. The same declaration draws the lanes that cross into it,
because a hive nothing routes into is an island.

## 4: a member

```bash
post "$lib/examples/organism/grow-member.json"
```

A member owns what its agents must share, the memory, the curated record, the screen and the
channels. So `alex` arrives before any agent of alex does.

## 5: your own agent

```bash
post "$lib/examples/organism/grow-assistant.json"
```

`scribe` is one generation of alex's agent: a conversation surface that answers, a reasoning core
that thinks, a tool surface. A `${VAR}` in a declaration is read from the colony's own `.env` at
every mutation and never from your shell, so a missing line is refused as `env_var_missing`
instead of committing a half-wired cell.

## Talk to it

```bash
meclaw ask --api 127.0.0.1:7777 --target /door "Say hello in one short sentence."
```

`ask` posts one turn and reads the answer out of `GET /colony/trace`, where every other hop of
that turn is waiting too. `/door` puts the turn on the `in_turn` lane and stamps which agent it
is for, which is what a channel does for the person using it; the shipped door names `scribe`
unless the caller already said otherwise. The colony is ninety-three cells by then, and the
trace shows every hop the turn took through them.

## Where to read next

- [`examples/organism`](../examples/organism/) for the six declarations this page took three
  from, the Telegram channel, and the count of hand-written edges.
- [A colony refuses an attack](../examples/hard-shell/WALKTHROUGH.md), where a blocked fetch
  turns into a typed event on a route, out of a seed that configures no security.
- [An answer from months-old memory](../examples/never-forgets/WALKTHROUGH.md), where a sentence
  said in February is asked for by date in August.
- [Installation](installation.md) for the same steps by hand, and every knob `start.sh` reads.

## If it does not start

- The port was busy. `start.sh` takes the next free one above `7777` and prints it; `MECLAW_PORT`
  picks a different first try.
- The very first start can stay silent for about 40 seconds while the binary comes off cold disk.
  Every later start answers in under a second.
- A wrong key gets through the growing and fails at the first turn, with `code=auth`.
