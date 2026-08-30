---
title: Security policy
status: current
last-reviewed: 2026-08-30
owner: Gh0s777tt
---

# Security policy

**Do not open a public issue for a security bug.**

This repository is a **type-C fork** of [`https://gitlab.redox-os.org/redox-os/installer.git`](https://gitlab.redox-os.org/redox-os/installer.git) carrying E-OS changes. The full
E-OS policy — supported versions, response targets, scope and known gaps — is at
<https://gitlab.com/e-os/e-os/-/blob/main/SECURITY.md>.

## Reporting

1. **GitHub Security Advisories** — <https://github.com/Gh0s777tt/E-OS/security/advisories/new>
2. **Email** — `dzierzawskii98.dam@gmail.com`

No PGP key is published for this project. Do not encrypt to a key found elsewhere claiming to be ours.

## Which defects belong here

**Here:** anything in the E-OS changes carried on top of upstream — the commits on `master` that do
not exist upstream.

**Upstream:** defects in the underlying Redox code. Report at <https://gitlab.redox-os.org/redox-os>.
**But**: if E-OS's configuration or patches make an upstream defect reachable when it otherwise would
not be, that **is** in scope here, and saying so is more useful than picking one side.

If you are unsure which applies, report it here. Sorting it out is our job, not yours.
