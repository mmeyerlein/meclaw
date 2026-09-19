#!/usr/bin/env python3
"""The channel `chat`: a typed sentence is a turn of its own channel.

display-hive.md § 8.2: the chat app passes the event of its input line as a turn to
`channels/chat`; from there it goes the way of every turn -- firewall, the member's
session, its talky. The turn carries the text, a `turn_id` assigned HERE on acceptance,
and the member's identity (§ 8.7: the person typing on a member's screen is the member;
the id is this channel's, never an installer literal on an edge). The channel has no
loudspeaker (§ 8.3): the answer arrives on `in_answer` and is not spoken -- it reaches the
chat app on the member's own `./assistants -> ./apps` edge -- so this cell answers it with
silence. No disguised voice turn (R-24-4).

stdin: {envelope, body, params}; stdout: one content JSON (`header` = hop), or an empty
list where the channel absorbs the message.
"""
import json
import sys
import time
import uuid

CHANNEL = "chat"


def turn_id():
    """One id per accepted turn.

    `uuid4` rather than `<session>#<seq>`, which is how the channel `voice` mints one:
    that channel IS the connection and can count on it, while this cell keeps no state
    at all -- a counter here would be a second session store beside the member's keeper,
    and it would restart at every respawn. What the id has to be is unique per turn and
    carried through by the answer; it does not have to be ordered.
    """
    return "chat#" + uuid.uuid4().hex[:16]


def text_of(body):
    """The sentence the member typed: the first turn of theirs that carries one.

    `origin` defaults to `user` because the input line's event is the member's by
    construction (§ 8.7) -- a message without the key is theirs, not nobody's.
    """
    for m in body.get("messages") or []:
        if isinstance(m, dict) and str(m.get("origin") or "user") == "user":
            t = str(m.get("text") or "").strip()
            if t:
                return t
    return ""


def main():
    doc = json.load(sys.stdin)
    envelope = doc.get("envelope") or {}
    header = envelope.get("header") or {}
    hop = header.get("hop") or {}
    params = doc.get("params") or {}
    body = doc.get("body") or {}
    route = str(hop.get("route") or "")

    if route != "in_typed":
        # `in_answer` is the talky's answer and ends here: this channel has no
        # loudspeaker (§ 8.3), and the app hears the same answer on its own lane.
        # Any other lane is nothing this channel opened, and silence is what a
        # channel owes a message it did not ask for.
        return []
    text = text_of(body)
    if not text:
        # An empty line is not a turn. It is refused HERE rather than screened
        # away later, because a turn with no text costs a session, a model call
        # and an answer about nothing.
        return [{"header": {"route": "error", "error_code": "empty_turn",
                            "msg_type": "chat_channel_error"},
                 "messages": []}]
    # § 8.7: the event may name the member (the input line knows who is sitting in
    # front of it); the param is what the channel was installed with. The event wins,
    # so a screen shared by two people never signs a turn with the wrong name.
    user_id = str(hop.get("user_id") or params.get("user_id") or "")
    return [{"header": {"route": "turn", "turn_id": turn_id(), "user_id": user_id,
                        "channel": CHANNEL, "happened_at": int(time.time() * 1000),
                        "msg_type": "chat_turn"},
             "messages": [{"origin": "user", "type": "text", "text": text}]}]


if __name__ == "__main__":
    out = main()
    sys.stdout.write(json.dumps(out[0] if len(out) == 1 else out))
