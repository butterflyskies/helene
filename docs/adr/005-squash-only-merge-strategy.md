# ADR-005: Squash-Only Merge Strategy

## Status

Accepted

## Context

Pull requests to helene can accumulate many small commits during review — fixups,
rebases, CI retries. Three merge strategies were considered:

1. **Merge commits** — preserves full branch history. `main` accumulates noise from
   WIP commits, fixups, and iterative changes. `git log` becomes harder to scan.

2. **Rebase and merge** — linearizes history but preserves every commit. Same noise
   problem as merge commits, just without the merge node.

3. **Squash and merge** — collapses the entire PR into a single commit on `main`.
   Each commit on `main` maps 1:1 to a PR. History is clean and bisectable.

## Decision

Squash merge only. Enforced at two layers:

- **GitHub org ruleset** — disables merge commits and rebase merges at the org level.
- **Repo settings** — redundant enforcement at the repo level as defense-in-depth.

PR titles become commit messages on `main`, so they should follow conventional commit
format (`feat:`, `fix:`, `refactor:`, `docs:`, `chore:`).

## Consequences

- `main` has a clean, bisectable, linear history.
- Each commit on `main` corresponds to exactly one PR.
- Branch history is preserved in GitHub's PR UI but not in `main`'s git log.
- Contributors must write meaningful PR titles — these become the permanent record.
- Force-push to feature branches during review is free (the branch history is discarded
  on merge anyway).
