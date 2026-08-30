---
title: Working contract — type C fork
status: obowiązujący
last-reviewed: 2026-08-30
owner: Gh0s777tt
---

# CLAUDE.md — `eos-installer`

To jest **fork typu C**: kod upstreamu Redox z łatkami E-OS. Pełny kontrakt ekosystemu jest
w <https://gitlab.com/e-os/e-os/-/blob/main/CLAUDE.md>; tutaj tylko to, co dotyczy tego repozytorium.

## 1. Czym to jest

Fork [`https://gitlab.redox-os.org/redox-os/installer.git`](https://gitlab.redox-os.org/redox-os/installer.git) niosący zmiany E-OS. Gałąź, którą buduje E-OS, to **`master`**,
przypięta rewizją w `repos.toml` orkiestratora.

**Nie jest to lustro.** Lustro (typ B) nie niesie własnego kodu; ten fork niesie, i dlatego podlega
pełnym standardom projektu.

## 2. Reguła nadrzędna: utrzymuj rebaseowalność

Upstream się rusza. Każda nasza zmiana to koszt przy **każdym** kolejnym rebasie.

- **Łatki małe i tematyczne.** Jedna zmiana logiczna na commit.
- **Każda łatka z uzasadnieniem w treści commita** — dlaczego, i co zmierzono.
- **Każda ze statusem wobec upstreamu**: *zgłoszona / przyjęta / lokalna na stałe*. Łatka bez
  statusu jest długiem, którego nikt nie umie spłacić.
- **Nie refaktoryzuj kodu upstreamu dla estetyki.** Zmiana formatowania kosztuje konflikt przy
  każdym rebasie i nie daje nic.
- **Nie przenoś plików.** Układ katalogów jest własnością upstreamu; przeniesienie zamienia każdy
  przyszły rebase w konflikt na każdym pliku.

Sprawdza to `scripts/eos-rebase-check.sh` w orkiestratorze.

## 3. Zmiana trafia na urządzenie dopiero po podbiciu przypięcia

Sam commit tutaj **niczego nie zmienia w obrazie**. Trzeba podbić rewizję w `repos.toml`
i w recepturze orkiestratora, a potem przebudować.

To repozytorium **nie ma lustra automatycznego** — push wymaga dwóch poleceń, na GitLab i na GitHub.

## 4. Protokół weryfikacji — reguły twarde

1. **Każda zmiana ma testy** — nowe albo zaktualizowane. Brak testu wymaga uzasadnienia i zgody.
2. **Zmiana jest skończona**, gdy build i testy przechodzą, a **artefakt został uruchomiony**,
   nie przemyślany.
3. **Weryfikuj artefakt, nie kod wyjścia.** Do opisu MR-a wklejasz prawdziwe wyjście.
4. **Bramka sprawdzająca obecność nie jest bramką** — każda kontrola potrzebuje testu negatywnego.
5. **Zmiany dotykające rozruchu, kryptografii, aktualizacji lub granic uprawnień** wymagają
   **pisemnej analizy ryzyka i planu wycofania** w opisie MR-a.
6. **Bez commitów na gałąź przypiętą bez MR-a. Bez `force-push`. Bez sekretów.**
7. **Dokumentację aktualizuj w tym samym MR** co zmianę.

## 5. Definicja ukończenia

- [ ] Build przechodzi (na cel Redox, przez cookbook orkiestratora)
- [ ] Testy przechodzą; nowe testy towarzyszą zmianie
- [ ] Prawdziwe wyjście poleceń w opisie MR-a
- [ ] Commit niesie uzasadnienie **i status wobec upstreamu**
- [ ] `CHANGELOG.md` ma wpis
- [ ] Commit podpisany, Conventional Commits, jedna zmiana logiczna
- [ ] Przy obszarach z §4.5 — analiza ryzyka i plan wycofania
- [ ] Po scaleniu: podbita rewizja w `repos.toml` orkiestratora
