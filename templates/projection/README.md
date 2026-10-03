# `projection@1.0.0`

One workspace of a [`file-space`](../file-space/README.md) laid out as a directory, a permitted program run in it, and what the program changed taken over into the workspace ([#905](https://github.com/mmeyerlein/meclaw/issues/905)). It is the child hive `./projection` of every file space; the space hands it the projection operations that arrive on its `in_ws`. The hive is sealed (`params.ports: []`). Cells: `store` (store, `write_surface: internal`), `mat` (code: lay out, scan, take over), `run` (code: one program, no shell), `git` (code: git as the exchange format, [#906](https://github.com/mmeyerlein/meclaw/issues/906)).

## Operations (on `in_ws` of the file space, hop `op`, `op_id`, `ws`)

| Op | Args | Answer |
|---|---|---|
| `ws_materialize` | `force?` | `{ws, dir, written, removed, dirty: [path]}`; `failed`/`skipped` when a file could not be laid out |
| `ws_exec` | `argv` (list of strings), `timeout_ms?` | `{ws, run, exit, out_tail, err_tail, adopted: [{path, version}], proposals: [{id, path, kind}]}`; `failed`, `skipped` when present |
| `ws_adopt` | `run`, `ids` (list) or `'all'` | `{ws, run, adopted: [{path, file, version}]}`; `failed: [{id, path, code}]`, `unknown: [id]` when present |

`ws` is a workspace name or id; the directory is always `<base_path>/<name>/`, laid out relative to the workspace root (`<root>/a/b.md` is `<base_path>/<name>/a/b.md`); manifest keys and every path in an answer are space paths. Every answer carries `messages: []`. `exit` is the program's exit code or `'timeout'`; `out_tail`/`err_tail` are the last 8 KiB of each stream.

## Behaviour

| Step | Rule |
|---|---|
| lay out | `ws_tree`, then `raw` for every file whose version differs from the manifest; paths the tree no longer names are removed. A file on disk that differs from what was laid out (a program's change nobody adopted) is `dirty`: never overwritten or removed unless `force` |
| run | argv[0] must be in `exec_allow` (else `not_allowed`, nothing is laid out or started); the workspace is laid out first (`dirty` → `dirty_projection`, nothing runs); no shell, cwd the workspace directory, environment `PATH`, `HOME`, `LANG=C.UTF-8` only, cap `min(timeout_ms, exec_timeout_ms)` |
| scan | after the run, the directory is read once (`ignore` and every `.git` segment left out): `modified`, `created`, `deleted` against the manifest; a symlink counts only when it points at a file inside the directory |
| take over | argv starting with an `auto_adopt` prefix and exit 0 → every `modified` file is written at once (`overwrite` against the laid-out version); everything else is a numbered proposal in `runs.changes`. `ws_adopt`: `modified` → `overwrite`, `created` → `create`, `deleted` → `remove`, each with hop `ws`; `base_moved` leaves the proposal open. An adopted proposal is never taken over twice |

Refusals: `bad_request`, `unknown_op`, `no_base_path`, `bad_base_path`, `not_allowed`, `dirty_projection`, `run_unknown`, `outside_root` (per file in `failed`: a path that would leave `<base_path>/<ws>/` or a symlink on the way), the other per-file codes in `failed` `corrupt`, `fetch_failed`, `gone`, `write_failed`, `stage_failed`, `exec_failed`, `materialize_failed`, `store_error`, and the space's own codes (`ws_unknown`, `ws_closed`, …) passed through.

## Knobs (params of `mat` and `run`, set by the owner's override)

| Cell | Param | Default | Meaning |
|---|---|---|---|
| `mat`, `run` | `base_path` | `''` | absolute directory outside the colony root; empty or relative → `no_base_path`; inside a directory tree that holds a `colony.json`, or below the cell's cwd when that cwd is a colony root → `bad_base_path`; the directory must exist before the cells spawn (the write grant opens it) |
| `mat`, `run` | `sandbox` | restricted, no network, runtime set, **no write** | the owner writes the whole block again with `filesystem.write: [<base_path>]` (an override replaces a param, it does not merge into it) |
| `mat` | `exec_allow` | `[]` | program names `ws_exec` may start (argv[0], exactly) |
| `mat` | `auto_adopt` | `["cargo fmt", "rustfmt"]` | argv prefixes whose `modified` files are taken over at once |
| `mat` | `ignore` | `[".git/**", "target/**"]` | globs the scan leaves out |
| `run` | `exec_timeout_ms` | 600000 | cap of one run; `external_timeout_ms` (660000) and `cell.message_timeout` (720000) lie above it |
| `run` | `env_path` | `/usr/local/bin:/usr/bin:/bin` | PATH of the program |
| `run` | `env_home` | `''` | HOME of the program; empty = `base_path` |
| `git` | `base_path`, `sandbox` | as `mat` | the same override, on `projection/git` |
| `git` | `git_author` | `meclaw <meclaw@example.invalid>` | author and committer of an export, `Name <mail>` |
| `git` | `remotes` | `{}` | name → URL or absolute path; any other `remote`/`source` is taken only as an absolute local path; a URL with a user or secret → `credentials_unsupported` |
| `git` | `import_max_files` | 2000 | more tree entries → `too_many_files`, nothing written |
| `git` | `import_max_bytes` | 26214400 | a larger file is skipped as `too_large` |
| `git` | `git_timeout_ms` | 60000 | cap of one git call (`git_timeout`) |

The override is path-keyed on the ref that instantiates the space, one level per ref: `projection/mat` on a `file-space` ref, `files/projection/mat` on a ref to a template that holds the space at `./files`. `sandbox.filesystem.write` takes absolute literals only, so `base_path` and the grant are written twice, the same path both times.

## Store

| Table | Columns | Rule |
|---|---|---|
| `proj` | `ws`, `dir`, `manifest` (json `{path: {file, version, sha256}}`), `at` | one row per workspace name |
| `runs` | `run`, `ws`, `argv` (json), `exit`, `out_tail`, `err_tail`, `changes` (json `[{id, path, kind, file?, base?, sha256?, state}]`), `at` | one row per run; `state` `open`/`adopted` |
| `pending` | `op_id`, `cell`, `phase`, `body`, `at` | a job of `mat` or `git` (`cell`) and the answers it waits for; a job older than an hour is cleared by the next request of the same cell |

## Internal lanes

| Lane | Between | Carries |
|---|---|---|
| `mat_store` | `mat` → `store` | store operations; the edge lifts `reply_cell`/`phase` into `cur_origin`/`cur_phase` |
| `in_run` / `ran` | `mat` → `run` → `mat` | `{argv, cwd, timeout_ms?}` / `{exit, out_tail, err_tail}` |
| `in_ws`, `in_read`, `in_write` / `in_answer` | `mat` → space → `mat` | `caller` 'projection', `op_id` `mat:<job>:<n>` |
| `git_store` | `git` → `store` | as `mat_store` |
| `in_proj` / `answer` → `in_answer` | `git` → `mat` → `git` | `ws_materialize` with `caller` 'git'; the answer is turned back to `in_answer` on its way |
| `in_ws`, `in_write` / `in_answer` | `git` → space → `git` | `caller` 'projection', `op_id` `git:<job>.<n>` |

Every file step into the space (`in_ws`, `in_read`, `in_write`) arrives with the full colony TTL: the space's edges out of `./projection` restore it, one door per step, and the job's file list bounds the steps (`import_max_files` for an import or a pull) ([#975](https://github.com/mmeyerlein/meclaw/issues/975)).

## git -- the exchange format ([#906](https://github.com/mmeyerlein/meclaw/issues/906))

| Op | Args | Answer |
|---|---|---|
| `ws_export_git` | `note?` | `{commit, files}`; `nothing_to_commit` |
| `ws_import_git` | `source`, `ref?` (`HEAD`) | `{created, modified, removed, skipped: [{path, reason}], upstream}` (`upstream` empty when a write was refused) |
| `ws_push` | `remote`, `branch?` (`main`) | `{remote, branch, commit}` |
| `ws_pull` | `remote`, `branch?` (`main`) | as `ws_import_git` |

`ws` is the workspace NAME; the repository is `<base_path>/<ws>/.git`, beside the laid-out files. The repository tree is root-relative like the directory: in a workspace rooted at `/proj`, the space path `/proj/a/b.md` is the repository path `a/b.md`. An export materializes first (`dirty_projection` when the directory holds changes nobody adopted, `materialize_failed` when a file could not be laid out), asks `ws_tree` for the root, stages ONLY the manifest's paths (a tool's `target/` beside them never lands in a commit) and commits with the old head and -- while it is no ancestor -- `refs/meclaw/upstream` as parents, so a push after a pull fast-forwards. An import fetches into that repository (the commit held as `refs/meclaw/incoming`) and writes the difference into the workspace through `in_write` (`create`, `overwrite` against the working version, `remove`), only under the root; the caller commits the workspace. Only when every write went through does the commit become `refs/meclaw/upstream` (the answer's `upstream`; empty when a write was refused): a pull that fails or stops halfway leaves the upstream where it was, so the next push stays `push_rejected` instead of fast-forwarding over a change the workspace never received. The upstream belongs to the directory `<base_path>/<ws>`, not to the workspace: a caller that discards a pulled workspace while the directory lives on pulls again before the next export. Symlinks, submodules and every path with a `.git` segment are skipped (`skipped` reasons `symlink`, `submodule`, `bad_path`, `outside_root`, `too_large`, or the code `write` refused with), and a skipped path is never removed from the workspace; more than `import_max_files` entries is `too_many_files` before anything is written. A push is never forced (`push_rejected`, `not_exported` before the first export).

Every git call is an argv list without a shell, under an environment of its own (no host variables, no system or global configuration, `core.autocrlf=false`, literal pathspecs) and with `core.hooksPath=/dev/null` and `core.fsmonitor=false`: no hook or fsmonitor of a repository ever runs. There is no credential path: a remote URL with a user or secret is `credentials_unsupported`, and no answer ever shows one. Bytes enter the space unchanged; a `.gitattributes` in the workspace (`eol=`, filters) still applies to an export as git applies it. A network remote needs `sandbox.network: allow` by override.

Refusals: `bad_request`, `unknown_op`, `ws_unknown`, `no_base_path`, `remote_unknown`, `credentials_unsupported`, `not_exported`, `nothing_to_commit`, `dirty_projection`, `materialize_failed`, `not_materialized`, `too_many_files`, `push_rejected`, `git_failed`, `git_timeout`, `store_error`, and the space's codes passed through.
