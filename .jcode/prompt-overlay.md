# Local Patch Maintenance Rules (kmou424 fork)

This working copy is the kmou424 fork of jcode. When doing self-dev work here,
follow the fork's patch management rules:

## Repository layout

- Fork remote (read-write): `git@github.com:kmou424/jcode.git` (`origin`).
- Upstream (read-only): `https://github.com/1jehuang/jcode.git` (`upstream`).
  Fetch tags from it.
- Patch branch naming: `kmou424/<upstream-tag>` (e.g. `kmou424/v0.84.0`).
  Each branch is based on a specific upstream tag — never on a moving
  `master`.

## Commit conventions

- **Docs commit first.** The first commit on every `kmou424/<tag>` branch is
  the fork-documentation commit with the fixed subject
  `docs(fork): kmou424 patch series`. It carries `.jcode/prompt-overlay.md`
  (this file), everything under `docs/custom/`, and all files under
  `.github/workflows/`. All later changes to docs, overlay, or workflow
  files are amended into that same first commit; the rest of the branch is
  then rebased on top and the branch force-pushed. Never scatter these
  changes across multiple commits. The fixed subject is how CI and agents
  locate the commit (`git log --fixed-strings --grep='docs(fork): kmou424 patch series'`).
- **Feature patches carry an ID prefix.** Every patch commit message starts
  with `[<ID>]` where `<ID>` is a two-digit number matching the feature's
  doc file (`docs/custom/<ID>-<slug>.md`). This is the only intentional
  deviation from conventional-commit style. Example:
  `[01] feat(providers): ...` pairs with `docs/custom/01-*.md`.
- **One-line commit messages.** Commit messages are a single subject line —
  no body. Anything worth explaining belongs in `docs/custom/<NN>-*.md`.
- Each patch commit is a self-contained feature/fix; they get cherry-picked
  and reordered, so avoid mega-commits mixing unrelated changes.
- **Never silently carry upstream bug fixes.** If a bug exists in upstream
  code (verify provenance with `git log -S` / a detached-tag worktree), do
  not fold a fix into the series on your own. When the bug noticeably hurts
  the experience or blocks progress, ask the user whether to fix it. Only a
  bug that genuinely blocks the feature being landed may be fixed, and then
  it is squashed into that feature's own `[NN]` patch — never a standalone
  "fix upstream" commit. Note upstream-fix candidacy in the doc either way.
- **Get approval before assigning work to a patch.** Before starting a new
  patch or folding the current request into an existing one, ask the user:
  name the patch number (`[NN]`) and describe the scope of changes, and only
  proceed after approval.
- When a change is a candidate for upstream (general feature/fix, not local
  config), note it in the doc and in the commit body so it can be PR'd and
  eventually dropped from the local series.

## Upgrade workflow

1. `git fetch upstream --tags`
2. Create the new patch branch: `git checkout -b kmou424/<new-tag> <new-tag>`
3. Cherry-pick the previous branch's commits in order — docs commit first,
   then numbered patches.
4. `git rebase -i` to group related patches: commits serving the same
   feature/requirement should be adjacent (and squashed when they are one
   logical change).
5. For every patch, check whether upstream now covers the feature or
   conflicts with its design. If upstream already implemented it, drop the
   patch and its doc. If the design diverged, flag it to the user and
   negotiate before rewriting.
6. Amend the docs commit if any doc or these rules changed, rebase, then
   force-push `kmou424/<new-tag>` to `origin`.

## Documentation layout

- `docs/custom/` holds one numbered doc per patch/feature:
  `<ID>-<slug>.md`, e.g. `01-named-provider-display-name.md`. The doc is the
  requirement-side reference for the patch: what it does, why, config
  surface, affected files, upstream status. Keep the set small — one doc
  per logical feature, not per commit or per file.

## Housekeeping

- **Force-push needs approval.** Before any `git push --force` /
  `--force-with-lease` (rebase cleanup, docs-commit amends, branch updates),
  stop and ask the user. Never push silently.

- **GC build caches after finishing a task.** Once the work for a request is
  done (commits pushed, daemon deployed if needed), prune dead Rust build
  caches so `target/` does not grow unbounded across sessions. Use the
  fork's precise GC (see `docs/custom/06-incremental-cache-gc.md`): it asks
  `cargo build --message-format=json` for the live unit set and deletes
  dead `incremental/` work-product dirs, dead `.fingerprint/` variants, and
  dead `deps/` artifacts — dormant-but-valid caches are kept. Run it right
  after a build so the oracle build is a no-op:

  ```bash
  # after selfdev builds — union the -p plan and the workspace plan so both
  # variant families stay live (feature unification differs between them)
  scripts/gc_incremental.py --profile selfdev --apply \
      --plan "--profile selfdev -p jcode --bin jcode" \
      --plan "--profile selfdev"
  scripts/gc_incremental.py --profile debug --apply     # after cargo test/build
  ```

  Dry-run first when unsure (omit `--apply`). A false positive only costs
  one crate's recompile, never correctness.
