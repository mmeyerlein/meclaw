# simple-agent-colony

A colony of four: the colony's `access`, `affinity` and `llm-registry`, and one
`simple-agent@2` grown beside them by one builder block. The agent keeps no key and no record
of people; every lane to the three neighbours, its grants and its params come from the block.

| file | what it is |
|---|---|
| `wish.json` | the block: the builder's recipe `simple_agent` with its params |
| `grow.json` | the recipe's rendering of `wish.json`, one declaration at the level `/` |
| `seed/main` | the level with the three neighbours as template references |

`grow.json` is what the recipe renders, and a test holds the two together
(`sa2_builder_block_renders_one_agent_the_door_accepts`); the same test runs the door's
checks against it. `meclaw --validate --validate-strict` passes on this seed with the block
laid into it (`the_simple_agent_block_validates_strict_in_the_example_colony`). A third test grows this colony with a stub model and answers one turn
(`sa2_example_colony_answers_a_turn_with_the_stub`): the brain's key arrives sealed from the
colony's access under the grant the block seeded.

```bash
meclaw --root <colony> --templates templates --apply examples/simple-agent-colony/grow.json
```

A fourth test runs exactly this command on a colony grown from `seed/main`, and the door commits
the block (`the_example_readme_apply_commits_the_block`). Every bare `${NAME}` of the templates
the agent reaches needs a value in the colony's `.env` at that moment.

The key lives in the colony's vault (`cred:openrouter`, see [`vault-pilot`](../vault-pilot/)),
never in `.env`. The model id in `wish.json` is a placeholder: name the model your provider
serves, or let the llm-registry push one.

What the block draws:

- the node `./agent` from `simple-agent@2.2.7`, `ctx.model` its brain's start value;
- per consumer cell the ask to `./access` and the sealed answer back by its grant, the grants
  and their birth events in `./access/store`;
- the brief, proposal and identity-pack roads to `./affinity`, the subscriber of the
  enrolment's `subscribe` stamped on the edge;
- the commit receipt into the agent (its enrolment and its menu), the llm-registry's pushes
  under `/agent/` as `in_model`, refusals back as `in_refused`, and the announcement of
  `brain` with its need on every receipt.
