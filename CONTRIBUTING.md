---
title: Contributing
status: current
last-reviewed: 2026-08-30
owner: Gh0s777tt
---

# Contributing

This is a **type-C fork** of [`https://gitlab.redox-os.org/redox-os/installer.git`](https://gitlab.redox-os.org/redox-os/installer.git). Read [`CLAUDE.md`](CLAUDE.md) first — the
rebasability rules there are the point of this repository, not decoration.

The full contribution guide is in the orchestrator:
<https://gitlab.com/e-os/e-os/-/blob/main/CONTRIBUTING.md>.

## Before you change anything here

**Ask whether the change belongs upstream instead.** A fix accepted by Redox costs nothing to
maintain; the same fix carried here costs a conflict at every rebase, forever. Send it upstream when
you can, and carry it here only while it is pending or when it is genuinely E-OS-specific.

## Rules specific to this repository

- The branch E-OS builds is **`master`**.
- **Do not move files and do not reformat upstream code.** Both turn every future rebase into a
  conflict on every touched file.
- Every commit states **why**, what was measured, and its **status upstream**
  (*reported / accepted / permanently local*).
- This repository has **no push mirror** — push to GitLab and GitHub by hand.
- A merged change reaches no device until its revision is bumped in the orchestrator's `repos.toml`.

## Verification

Built for `*-unknown-redox` by the E-OS cookbook, not on a host. To prove a change:

```bash
# in the orchestrator
bash scripts/eos-build.sh x86_64
bash scripts/ci-boot-smoke.sh ~/eos-artifacts/eos-x86_64-harddrive.img 300 --arch x86_64
```

Paste the real output into the merge request.
