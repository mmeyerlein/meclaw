# `browser@1.0.0`

A web page on a screen, as a cell. One browser per member, a browser context per
identity, a page per card, and each page's picture streamed onto a topic of the
display's own socket -- so a person looks at the page itself rather than at
somebody's summary of it, and can reach into it.

## What it is

The cell holds a Chromium-based browser and talks to it over the DevTools
Protocol on a pair of pipes. What the topology sends it are four verbs about
pages. What comes back are three lanes: `page` when a page changes, `receipt`
per message, `error` when something failed. **A frame is never a message**: the
picture goes out on the link a display joined, and pointers, wheels, keys and
text come back on the same link.

## Requirements

**The browser is a prerequisite, not a delivery.** Install a Chromium-based
browser through your distribution's package system and name its binary in
`params.chromium_path`. There is no default and no search path, and the cell
never goes looking in some tool's cache: a substrate that found a binary by
itself would be running one nobody vetted.

Take the **full** browser rather than a headless shell. The shell renders no
PDF, and a page that is a PDF is a page a person will want to look at.

A packaged browser brings its own confinement -- your distribution's AppArmor or
SELinux policy, plus the browser's namespace and seccomp sandbox around every
renderer. This cell adds nothing to it and takes nothing away: no sandbox flag,
no `--no-sandbox` (it is refused in `extra_args`), and no switch to turn any of
it off. After the spawn the cell **looks**, at the processes below the pid it
remembers, and a browser whose children never left the cell's own user namespace
gets no page at all.

## One browser, many windows

A **context** is an identity. Two contexts are two logged-in states, and closing
one is what logging out means. Contexts are ephemeral on purpose: a browser
restart is a logged-out browser, because a cell that persisted a session without
saying so would be keeping a credential nobody wrote down.

A **page** is a view. Its id is the application's own id for the card it belongs
to, and that one word is the page, the topic suffix and the prop the component
carries. `max_pages` caps how many are open at once; above it the oldest
**suspended** page is closed to make room, and if none is suspended the request
comes back `too_many_pages` -- a page somebody might be looking at is never
given up quietly.

## The topic

A display joins `page:<page>` on the socket it already holds, the mount table
hands the join to this cell under `params.mount`, and frames travel as an
`image` broadcast:

| direction | shape |
|---|---|
| cell to viewer | binary: 16 bytes of head -- width, height, scroll x, scroll y as big-endian `u32`, in CSS pixels -- then the JPEG |
| viewer to cell | text, one JSON object: `pointer`, `touch`, `wheel`, `key`, `text` or `navigate` |

`navigate` takes only `about:back`, `about:forward` and `about:reload`: a free
address travels as `in_navigate`, from the application. A viewer who could send
a browser anywhere is a viewer who could send it somewhere the member never
asked for.

The picture runs when somebody is watching and stops when nobody is -- `viewers`
empty or not empty is the only switch there is. The pace is the
acknowledgement: the browser sends the next frame when the last one was
acknowledged, and the cell acknowledges after the frame reached everybody and
after `1000 / max_fps` milliseconds. A viewer that stops reading is given up
with close code `4408`; the cell never waits for one.

## The lifecycle

```text
opening -> active  <-> background -> suspended -> opening ...
             |                                      ^
             +-> throttled                          |
in_close -> closed        a new life -> reopened ----+
```

`suspend_after_ms` decides when a page nobody is watching gives up its target;
the context and the row stay, so the next join or navigation brings it back.
After a restart the cell reads its own `pages` table and opens what stood there,
reporting `reopened` -- nobody is asked, and there is no state for an
application to have to answer.

## `params`

| key | default | what it is |
|---|---|---|
| `mount` | `browser` | the name a `page:` join finds this cell under |
| `chromium_path` | **required** | the installed browser's binary |
| `user_data_dir` | the cell's own | the profile directory, created and removed with the cell |
| `extra_args` | `[]` | extra flags; the ones the cell owns and `--no-sandbox` are refused |
| `max_pages` | `8` | open pages at once |
| `suspend_after_ms` | `300000` | idle time before a page gives up its target; `0` never |
| `throttle_after_ms` | `30000` | unbroken frame production before the brake |
| `screencast` | jpeg, 60, 1280x800, 20 fps | how the picture is cut and paced |
| `sandbox` | a cgroup cap | required; the CELL's ceiling, never the browser's sandbox |
| `external_timeout_ms` | `10000` | the A-timeout around every round trip |
| `startup_timeout_ms` | `20000` | how long the browser has to answer |

## Error codes

`invalid_input`, `unknown_page`, `too_many_pages`, `spawn_failed`,
`startup_timeout`, `browser_crashed`, `navigate_failed`, `cdp_timeout`,
`client_too_slow`. Closed set.

## What it is not

- **Not a browser.** It holds one. Install it yourself, from your packages.
- **Not a place for a login you want to keep.** A context is ephemeral, and a
  restart is a logged-out browser.
- **Not a video stream.** The picture is a JPEG screencast; a codec is a later
  wave.
- **Not an opinion about what to open.** The addresses come from the
  application, and this cell holds pages rather than choosing them.
