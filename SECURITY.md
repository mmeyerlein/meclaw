# Security policy

## Reporting a vulnerability

Report it privately through GitHub Private Vulnerability Reporting: the **Security** tab of
this repository, "Report a vulnerability". Please do not open a public issue for something
exploitable.

A useful report names the version (`meclaw --version`), the cell types or templates involved,
the topology or `config.json` that reproduces it, what you observed and what you expected, and
the impact you see. A minimal colony seed or mutation manifest is the fastest form.

## Supported versions

meclaw is on `0.x`. Fixes go into the latest release and there is no backport branch, so
reproduce on the newest release before reporting.

## What the sandbox does and does not cover

An agent with tools runs code, reaches the network and touches files on the machine you gave
it. Every cell that starts a child process runs inside a kernel sandbox (Landlock, network
namespace, cgroup v2, seccomp), which bounds that risk without removing it. What the
boundaries are, and where each one ends, is written out in
[docs/security.md](docs/security.md).
