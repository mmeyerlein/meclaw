# Stability

Five surfaces are the public contract of this project: the HTTP API, the template DSL, the
template ports, the origin a `web` cell owns, and the documented `error_code` strings. If you
build on one of them, a `0.x` release will not take it away from you without saying so. If you
build on anything else in the tree, you are building on an internal.

## The five surfaces

The HTTP API means the `/colony/*` routes and `POST /messages`. They are specified in
[`meclaw-overview.md`](meclaw-overview.md).

The template DSL is the `template.json` and `config.json` schemas, including the mutation diff
format. [`config.md`](config.md) documents the `config.json` schema, and the overview the diff.

The template ports are the ingress and exit endpoints a template's README declares. Each
template's README names the ports that template has.

The origin a `web` cell owns is the `page.set` route grammar and its two reserved names, `@` and
`live`. [`cell-types.md`](cell-types.md) § `web`, a port-owning display substrate.

The documented `error_code` strings are the dead-letter reasons, the cell-type codes, and the
codes a `/colony` read can answer with. The overview lists them in the sections that emit them.

## What 0.x promises

Changes to those five are additive. A route, a key, a port or a code that is there today is there
tomorrow, and new ones arrive beside it.

A change that breaks an existing topology gets its own Breaking section in
[`CHANGELOG.md`](../CHANGELOG.md), with the migration named. That is the whole mechanism. meclaw
has no deprecation period and no compatibility flag, so the release note is where you find out.

Nothing under `crates/` carries a SemVer guarantee. The Rust crates are internals and move
without notice, which is why the binary and the HTTP API are the interface this file talks about.
