# A window cut to the model

How large should the context window of a conversation be? As large as trust needs and no larger,
for latency: trust is the goal and latency the variable. The record of a conversation grows
without end and is never deleted; for each call the curator cuts the window out of it.

[`curator`](../../templates/curator/) keeps a ledger in front of a model. Every element is a block,
stored once byte for byte, and a row of the wall, the append-only order of what was said. A block
the window shortens or lets go keeps its short id and the way back. The history tools search the
wall by phrase, exact text or pattern, read a block or a span of turns, and outline sessions and
topics, so "didn't I ask you about the weather today?" can be answered from the record when the
window no longer holds the turn. A search reads a bounded number of rows and says so.

## Cut by the numbers of the model

From the catalogue row and every answer the curator reads `input_soft`, up to which a window is
worth filling, `input_hard`, above which the llm cell refuses a call, the input, cached and
cache-write prices, and a price ladder by prompt length; a new model is one catalogue row. A rebuild
is due at the soft line, near the hard line, and below both when holding the cached window costs
more over some turns than one rebuild. It plans the window anew: older segments take a summary or
drop out by a horizon (a day, a task, everything), the newest rounds stay raw. Over the aim, old
tool results keep one line and unused tool declarations become stubs; with garbage collection on,
every rebuild shortens old tool results and a memory result a younger one repeats. The model
releases or pins a block through a section of its answer, and a block with a time of validity
expires. Between two rebuilds only the end of the window moves, a changed tool menu aside.

## The system part and the dynamic part

Identity, instructions and the tool menu are set by others and stay; the curator counts them, as
they share one window and one cache. A pin another hive sets between two rebuilds stands as a
message where it arrived, so the cached prefix holds, and moves into the system part at the next
rebuild (a cold cache is one cause); notes open the person's newest message for that reason. For a
new tool this is a goal; today it enters the system part at the next call. The curator can write the
turn's memory question from the wall and looks up a gap the model names for a later turn. A turn's
route stamp can send a memory turn to the model that sees the recall, or leave the recall out when
the turn surely needs none; these switches ship off. The task behind these is to hold the most
relevant information in the window before the turn asks for it, by the state of the conversation
that the answer's sidecar sections and the judges report, rather than keeping the window bare and
searching the record on every turn. How well this works is to be measured.

## What is still a goal

Relevance is read from structure today (age, kind, repetition, topic); that the time, asked twice a
day, stops mattering is still to come. The aim is that every window starts from a whole day or
session and is cut only when a hard limit is crossed: the window, the soft line, the role's target
size, or the cost rule with its price ladder. Where that limit lies, and whether a whole day serves
trust better than a curated one, the answer's sidecar sections included, is to be measured, not set.
Today the horizons of a task and of everything cut by other measures than the day; a preset whose
horizon is the day, such as `talky`'s, already starts there.
