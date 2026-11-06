# 0009. Required checks without the up-to-date requirement

- Status: Accepted (2026-11-06)
- Related: SAGA-549, SAGA-546, SAGA-178 (Plane),
  [#545](https://github.com/Magnus-Gille/sagascript/pull/545)–[#547](https://github.com/Magnus-Gille/sagascript/pull/547)

## Context

The "Protect main" ruleset requires a pull request and the checks `check-macos` and
`check-windows`. Until 2026-11-06 it also required branches to be up to date with `main`
(`strict_required_status_checks_policy = true`). Every merge then made every other open pull
request stale. Each one needed `update-branch` and a full CI rerun before it could merge, one
after another.

On 2026-11-04 and 2026-11-05 a green pull request waited about 25 minutes after each merge
ahead of it. Two hotfixes and a version bump took about 1.5 hours to land although nothing
failed (SAGA-546). The sprint that started on 2026-11-06 has about 27 pull requests to merge.

A merge queue tests the combination without making authors wait in line, but it is not
available: GitHub offers it only for repositories owned by an organization, and this one
belongs to a personal account. The owner's constraint is that delivery costs 0 kr. The
repository is public, so standard GitHub-hosted runners are free either way, while larger
runners and Actions cache storage above 10 GB are not.

## Decision

1. `main` requires `check-macos` and `check-windows` to pass on the pull request's head
   commit. It does not require the branch to be up to date with `main`.
2. A green pull request may merge as it is. Merge with
   `gh pr merge N --auto --merge --match-head-commit <sha>`, so that only the head that was
   checked can merge.
3. The push CI on `main` is the integration check. Whoever merges watches it. When it fails,
   fixing `main`, by a fix or a revert, comes before the next merge.
4. Update a branch from `main` before merging when:
   - it has conflicts
   - it changes code that a pull request merged after its last CI run also changed
   - it is a release pull request
5. A release tag goes only on a `main` commit whose push CI is green
   ([RELEASING.md](../../RELEASING.md), steps 3 and 5).

## Consequences

- A green pull request merges without a rerun, so N pull requests no longer cost N CI runs in
  a row. Fewer runs also mean fewer runner minutes and fewer cache restores.
- Two pull requests that pass separately can break `main` together. A typical case is a
  function renamed in one and still called in the other. This is now caught after the merge, by
  the push CI on `main`, instead of before it. With one main author and few concurrent pull
  requests the owner judged this rare and cheap to fix.
- The ruleset still requires a pull request and the two checks, and it has no bypass actors.
  Only the up-to-date requirement changed.
- Revisit if the repository moves to an organization, where a merge queue becomes available,
  or if `main` turns red after merges more than occasionally.
