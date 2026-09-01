# One assistant, two brains

The shipped assistant runs two LLMs on purpose: **talky**, the conversation surface, and
**cogny**, the reasoning core. This is not an accident of composition — it is the design
principle the whole assistant is built around:

> **One job, one brain, one menu.**

## The problem with one brain

A single model doing everything has to be prompted for everything: the persona, the
small talk, the tool catalogue, the escalation rules, the long-form reasoning. That has
two costs. In latency and money: every quick "good morning" pays for a prompt built to
solve hard problems. In quality: a context window stuffed with every tool and every rule
is a model that is mediocre at each of them. You can feel this in assistants that take
many seconds to say hello.

## The split

**talky** holds the conversation. It owns the window, answers fast, and carries the
session — its job is the person, not the problem. **cogny** is the problem solver and
only that: it has a single brain, declares its own errand, and gets a tool menu it
**asks for** rather than has typed into its prompt. When talky meets something that needs
real work, the work travels to cogny as a message and the answer travels back — over
ordinary edges, visible in the trace like everything else.

Two cells, two models, two budgets. The assistant level decides which model serves which
brain (`ctx.model_surface` vs. `ctx.model`), so the surface can run a fast, cheap model
while the core runs a strong one — or, local-first, two different local models. Changing
the split is a config edit, not a rewrite: **the harness decides, and the harness is a
file.**

## Menus are asked for

A detail that matters more than it looks: cogny's tools are not pasted into its system
prompt. A tool is a cell plus a name edge in the tools hive, and the menu is fetched when
needed. The prompt stays small, the tool surface is the topology — so growing a new tool
is a mutation, and the brain finds it without anyone editing a prompt.

## Why this beats "just use a better model"

Better models make both halves better; they do not remove the tension between them. Fast
and cheap versus deep and thorough is a budget decision per *job*, and jobs differ within
one conversation. Putting that decision into the topology — instead of into one model's
prompt — is what makes it inspectable, testable, and changeable at runtime.
