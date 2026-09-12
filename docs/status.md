# Status and limits

What a colony you install today can and cannot do, and how far a `0.x` release binds the
project to what it already ships.

## What is limited today

Linux x86_64 only. The release is a static musl build, and the installer refuses any other
platform. A `code` cell runs `python3` and no other runner. The daemon installs no
authentication and no TLS; put a reverse proxy in front of it, like any Linux daemon. HTTP and
files are the whole interface, and there is no SDK to import.

meclaw is under heavy development, and I would not leave it unattended in production. The
0.36.1 release gate ran 7093 tests. One measured colony spent 0.32 EUR on a day of
conversation.

## What 0.x binds

Five surfaces are the public contract of this project: the HTTP API, the template DSL, the
template ports, the mount a `web` cell owns, and the documented `error_code` strings. On `0.x`
those five change additively, and a change that breaks an existing topology gets its own
Breaking section in [`../CHANGELOG.md`](../CHANGELOG.md).

## Where to read on

- [stability.md](stability.md) writes out what each of the five surfaces covers.
- [costs.md](costs.md) is the method behind the measured day, and how to measure your own.
- [why/rust-and-linux.md](why/rust-and-linux.md) says what the Linux-only build buys.
- [`../ROADMAP.md`](../ROADMAP.md) is what is being worked on.
