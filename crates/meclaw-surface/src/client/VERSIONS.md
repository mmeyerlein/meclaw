# Vendored client bundles

Copied in byte-for-byte and **never edited**. The moment we patch a bundle we own
the browser matrix again, which is the one thing adopting LiveView is meant to
avoid. Everything we need is an option, a hook or a JS command.

| File | Version | Bytes | SHA-256 |
|---|---|---|---|
| `phoenix_live_view.min.js` | 1.2.9 | 121746 | `732ce6e8fceeee3a96a92762c374fe9b81fca8d3e52b067172227924857bf2a7` |
| `phoenix.min.js` | 1.7.x | 25152 | `403d411d4f9c44170e248cb105cf630eaebf4cdad45f596d28ecdc0f5bb3b966` |

Both are MIT.
The licence texts sit next to this file as `LICENSE-phoenix_live_view` and
`LICENSE-phoenix`, transcribed from the upstream `LICENSE.md` of each project.

Both are plain IIFE bundles exposing the globals `LiveView` and `Phoenix`. No npm,
no bundler, no JS toolchain in a Rust repo — two `<script src>` tags.

`phoenix.min.js` is here because `LiveSocket`'s second constructor argument *is*
the `Phoenix.Socket` class. It is not optional and not replaceable.

morphdom is bundled into the LiveView dist, so the git dependency in its
`package.json` never reaches us.

## Upgrading

A three-line change: both files, and the `LIVEVIEW_VERSION` constant in
`crates/meclaw-surface/src/lib.rs`. The version we report on join and the
bundle we serve must move **together** — a mismatch is only a `console.warn` in
the client, which is exactly why it needs a rule and a test rather than a
watchdog. `1.2.9` is verifiable in the bundle itself: it is the string next to the
`liveview_version` reference.

The byte counts above are asserted by
`crates/meclaw-surface/tests/gh396_the_vendored_bundles_match_their_table.rs`,
which parses this table. A drifted count means somebody edited a bundle. The test
does not check the SHA-256, because no crate in the workspace provides a hash and
this feature adds none; the sums are here for a human to verify with `sha256sum`.

## Not vendored: the two files that are ours

Two files in this directory are **not** copies of anything upstream and are not
covered by the rule above. They are ours, edited like any other source file, and
they sit here only because `bundle()` serves this directory's closed list under
`@client/`:

| File | What it is |
|---|---|
| `boot.js` | The page shell's boot: reads the socket path from `<meta name="meclaw-live">` and an optional join timeout from `<meta name="meclaw-join-timeout">` (GH #1002), builds the `LiveSocket` with `window.SurfaceHooks`, sets `window.SurfaceSocket`. |
| `display-mic-worklet.js` | The display microphone's capture worklet (processor `mic`, 20 ms PCM16 frames, a `flush` message), loaded with `audioWorklet.addModule`. |

Both used to be inline -- the boot a `<script>` block in the shell, the worklet a
string turned into a `blob:` URL -- and a page that carries either does not run
under a Content-Security-Policy of `script-src 'self'` (GH #867). They have no
row in the table above and no byte-count check: nothing forbids editing them.
