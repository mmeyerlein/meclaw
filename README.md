<div align="center">

# meclaw

_Where agents build agents._

<p align="center">
  <a href="https://github.com/mmeyerlein/meclaw/actions/workflows/ci.yml"><img src="https://github.com/mmeyerlein/meclaw/actions/workflows/ci.yml/badge.svg" alt="ci"></a>
  <a href="https://github.com/mmeyerlein/meclaw/releases"><img src="https://img.shields.io/github/v/release/mmeyerlein/meclaw" alt="release"></a>
  <a href="#license"><img src="https://img.shields.io/badge/license-MIT%2FApache--2.0-blue" alt="license"></a>
  <a href="https://github.com/mmeyerlein/meclaw/discussions"><img src="https://img.shields.io/github/discussions/mmeyerlein/meclaw?label=Discussions&color=blueviolet" alt="Discussions"></a>
</p>

</div>

meclaw is a framework for composing complex agentic systems, not just agents: fast, secure, ontology-grounded, auditable and made for agents to build with, in one Rust binary. Use it to try a new harness structure in an afternoon, to grow an agentic OS of your own, or to build the next thing before it has a name.

meclaw-os is the reference implementation: a complete agentic OS, grown on that substrate and nothing else.

Three things that make a meclaw agent unique. A voice cell that listens and speaks on a wire protocol built for it. A web cell that gives the assistant a display on its own origin, where it shows you instead of telling you. And a vault that keeps your secrets secret, with every cell inside a kernel sandbox.

meclaw talks to any OpenAI-compatible endpoint and to MCP servers.

# Quick start

```bash
curl -fsSL https://github.com/mmeyerlein/meclaw/releases/latest/download/start.sh | sh
```

It asks for an OpenRouter key, grows the assistant and asks it one question. Set `OPENROUTER_API_KEY` first and it does not ask.

# Quick links

- [Getting started](docs/getting-started.md)
- [Configure your model](docs/model.md)
- [meclaw](docs/meclaw.md)
- [meclaw-os](docs/meclaw-os.md)
- [Security](docs/security.md)
- [Templates and apps](docs/templates-and-apps.md)
- [Why it is built this way](docs/why/README.md)
- [All docs](docs/README.md)

[Roadmap](ROADMAP.md) · [Contributing](CONTRIBUTING.md) · [Discussions](https://github.com/mmeyerlein/meclaw/discussions) · [Blog](https://meclaw.ai/blog/)

## License

MIT ([LICENSE-MIT](LICENSE-MIT)) or Apache 2.0 ([LICENSE-APACHE](LICENSE-APACHE)), whichever you like.
