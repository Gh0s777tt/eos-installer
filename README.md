# eos-installer

**E-OS fork of [`redox-os/installer`](https://gitlab.redox-os.org/redox-os/installer).** Part of the [**E-OS**](https://github.com/Gh0s777tt/E-OS) ecosystem — a hardened, Crimson-branded downstream of [Redox OS](https://www.redox-os.org).

This repository is the **Redox OS installer**.

## E-OS changes vs upstream

_None yet_ — pinned to a clean upstream commit. E-OS branding and config for this component are applied via **recipe patches in the [main repo](https://github.com/Gh0s777tt/E-OS)**, not fork commits.

## How it's pinned

The E-OS build pins this fork in [`recipes/core/installer/recipe.toml`](https://github.com/Gh0s777tt/E-OS/blob/main/recipes/core/installer/recipe.toml):

- branch **`master`** · rev **`05bf2eb42956`**
- up to date with upstream

## Build standalone

This fork is normally built by the E-OS cookbook (`make CI=1 …` in the [main repo](https://github.com/Gh0s777tt/E-OS)). To build it on its own you need the Redox toolchain; see the main repo's [build guide](https://github.com/Gh0s777tt/E-OS/blob/main/docs/building.md).

## Hosting

**GitLab (source of truth):** https://gitlab.com/e-os/eos-installer  
**GitHub (read-only mirror):** https://github.com/Gh0s777tt/eos-installer

## License

MIT (inherited from upstream Redox). The E-OS project as a whole is AGPL-3.0; see the [main repo](https://github.com/Gh0s777tt/E-OS/blob/main/LICENSE).

---
[E-OS main repo](https://github.com/Gh0s777tt/E-OS) · [Docs](https://github.com/Gh0s777tt/E-OS/tree/main/docs) · [Upstream](https://gitlab.redox-os.org/redox-os/installer)
