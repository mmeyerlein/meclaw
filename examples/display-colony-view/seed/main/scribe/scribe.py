"""The smallest thing that can put something on a screen.

THIS FILE IS THE SOURCE. `config.json` carries a byte-identical copy in
`params.script_inline`, and a test pins that the two agree.

A prose view is the floor of the view model: no application, no components, no
port, no markup. This cell holds one paragraph and a title, and every tick it
says them again on the `view` lane. The edge that leaves this colony's seed hive
turns that lane into the screen's `in_view`, and the screen writes the row under
THIS cell's path -- because the owner of a view is the envelope's `reply_to` and
never anything the body claims.

Saying it again on every tick is not a mistake and not a cost. `(owner, view_id)`
is the identity of a view, so a second arrival of the same pair REPLACES the row
rather than adding one; what changes is `updated_at`, which is the key the screen
stacks by. A producer that has nothing new to say and says it anyway therefore
walks to the top of the stack, which is the honest reading of "this is still
current" -- and it is why this example ticks slowly.

What it deliberately does NOT do: look at the message it was woken by. A tick
carries no content, and a cell that read one would be pretending the timer knew
something. The only input this cell has is its own text.
"""
import json
import sys

TITLE = "A screen is a channel"

BODY = (
    "This paragraph is a view. It was written by a code cell with no model, no "
    "components and no port of its own -- the smallest thing an agent can put on "
    "a screen. Below it, if the colony has been growing for a minute, stands a "
    "second view written by an application. Two owners, one screen, and neither "
    "of them can touch the other's row."
)


def main():
    return {
        "header": {"route": "view"},
        "messages": [],
        "view_id": "scribe",
        "region": "main",
        "kind": "prose",
        "content": {"title": TITLE, "body": BODY},
    }


if __name__ == "__main__":
    sys.stdout.write(json.dumps(main()))
