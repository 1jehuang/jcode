---
name: visual-qa-loop
description: Run a screenshot-backed visual QA loop for web pages or components, comparing against reference designs, testing mobile/desktop interactions, iterating fixes until a target quality bar is met, then validating, committing, and preparing PR or post-merge follow-up.
allowed-tools: bash, browser, mcp__firefox-devtools__*, read, write, edit, multiedit, apply_patch, agentgrep, batch, todo, bg, pr_watch, webfetch
---

# Visual QA Loop

Use this skill when the user asks for visual QA, screenshot comparison, design parity, mobile QA, drawer/modal QA, badge/sticker placement checks, or repeated quality loops across pages.

The workflow is intentionally autonomous: gather evidence, implement safe fixes, validate, and repeat until the target score or acceptance criteria is met. Do not stop after the first pass unless the next action would be destructive or needs user credentials/payment.

## Goals

- Compare live UI against a reference design, screenshot, HTML prototype, PDF export, or previously accepted page.
- Inspect both static design parity and interactive behavior: open/close, scrolling, focus, tap targets, hover/focus states, drawers, modals, forms, sticky headers, and route navigation.
- Produce concrete findings with screenshots or DOM evidence.
- Apply scoped fixes, then re-run the same checks.
- Validate with the project’s normal checks and HTTP smoke routes.
- Commit the work and prepare PR or post-merge steps when appropriate.

## Default quality bar

If the user does not specify a target, use:

- **90+ visual parity score** for high-value pages and components.
- No obvious overlap, clipping, inaccessible tap targets, unreadable text, broken scrolling, or non-functional interactive controls.
- Mobile and desktop variants both pass for shared components.
- Generated screenshots and temporary artifacts are not committed unless the repo explicitly tracks QA artifacts.

## Workflow

### 1. Re-orient and protect existing work

1. Confirm the repo, branch, and target URL or local dev server.
2. Run `git status --short --branch`.
3. Identify concurrent edits. Do not overwrite files modified by another agent unless you first inspect and intentionally preserve their changes.
4. Create or switch to a task branch if the current branch is merged, deleted, or unrelated.
5. Check whether generated folders like `.omx/`, `audit/`, screenshots, Playwright caches, or design exports are ignored or intentionally tracked.

### 2. Define scope and acceptance criteria

For each page/component, record:

- URL/route and viewport(s), usually `390x844`, `430x932`, tablet if relevant, and desktop if shared.
- Reference asset: design HTML/PDF/screenshot/prototype path.
- Critical sections and states to inspect.
- Interactive states: menu open, cart/search drawer open, seeded cart, form with input, empty state, scroll midpoint, bottom of page.
- Target score or stop condition.

When the user says “all pages,” build a route matrix from app routes, navigation links, and obvious Shopify/resource URLs.

### 3. Capture evidence

Prefer real screenshots over DOM-only judgment.

Recommended order:

1. Use Firefox DevTools MCP screenshots when available.
2. If MCP viewport control is unreliable, use the browser bridge plus CSS/DOM inspection for interaction, and capture whatever the bridge can see.
3. If Playwright browsers are missing or unsupported, do not stall. Use system Firefox/Chromium if possible, or run an autonomous visual agent such as `omx --madmax --high` when available.
4. Save temporary screenshots under `/tmp/<project>-visual-qa/<timestamp>/` unless repo policy says to track them.

For every pass, capture at least:

- Reference design top and full-page, or relevant component states.
- Live page top and full-page, or component closeups.
- Interactive screenshots before and after opening drawers/menus/forms.
- A concise comparison table: expected, actual, severity, fix candidate.

### 4. Score and prioritize

Use a simple rubric:

- Layout/spacing and responsive behavior.
- Typography, hierarchy, and visual rhythm.
- Color, borders, shadows, stickers/badges, and brand motifs.
- Content completeness and route-specific copy.
- Interaction behavior, focus, scrolling, close controls, tap targets.
- Accessibility basics: labels, focus-visible, keyboard escape/close, readable sizes.

Fix highest-severity, highest-visibility issues first:

1. Broken interactions or inaccessible controls.
2. Overlaps, clipping, horizontal scroll, unusable drawer/modal scroll.
3. Major design parity gaps in hero/header/nav/product/cart/search.
4. Badge/sticker placement and visual polish.
5. Minor spacing/content refinements.

### 5. Implement scoped fixes

- Keep changes local to the target component/page/style layer.
- Prefer reusable class-level improvements over one-off hacks.
- Preserve dynamic Shopify/metafield data and fallback behavior.
- Avoid committing screenshots, generated caches, design exports, or temporary audit files unless requested.
- If a fix touches shared components, spot-check representative pages.

For mobile interactive QA, explicitly check:

- Drawer/menu opens and closes via close button, outside click when applicable, route click, and Escape if supported.
- Drawer content scrolls independently without body bleed.
- Buttons and links have at least comfortable touch targets, usually 44px+.
- Sticky header, cart badge, search icon, and hamburger do not overlap.
- Seeded cart/search states remain readable.

### 6. Re-run the loop

After each fix:

1. Re-capture the affected state(s).
2. Compare against the same reference and previous screenshot.
3. Update the score and remaining issues.
4. Continue until the target score is met or only explicitly accepted tradeoffs remain.

Do not stop at “looks better.” Use evidence from screenshots, DOM dimensions, interaction tests, and route checks.

### 7. Validate

Run project-appropriate validation before claiming done. Common web validation:

```bash
npm run typecheck
npx eslint --no-error-on-unmatched-pattern app
git diff --check
```

Then smoke routes:

```bash
for r in / /collections /collections/all /cart /search /pages/about /pages/contact /pages/faq /pages/workshop /policies; do
  code=$(curl -s -o /dev/null -w '%{http_code}' "http://localhost:3001$r" || true)
  printf '%s %s\n' "$code" "$r"
  test "$code" = 200 || exit 1
done
```

Adjust routes to the actual project. Restore generated files such as `tsconfig.tsbuildinfo` if they changed only because validation ran.

### 8. Commit, PR, and post-merge

- Commit scoped code changes by default.
- Use a specific message, for example `Improve mobile nav drawer visual QA`.
- Push and create a PR when requested or when the task naturally calls for review.
- If merging, first ensure checks are passing and merge state is clean.
- If local `main` cannot be checked out because another worktree owns it, validate from `origin/main` in detached HEAD or a temporary worktree.
- After merge, run post-merge validation and report merge commit.

## Reporting format

Keep progress updates concise. Final report should include:

- Routes/components covered.
- Key visual/interaction issues found.
- Fixes applied.
- Validation commands and results.
- Commit hash and PR/merge link if applicable.
- Any untouched scratch artifacts or known follow-up.

## Guardrails

- Do not overwrite another agent’s concurrent edits. Inspect and preserve them.
- Do not commit local design exports, `.omx/`, screenshots, Playwright caches, or generated audit folders unless explicitly requested.
- Do not merge, delete branches, resolve review threads, or close PRs without explicit user authorization for that action.
- If browser tooling is flaky, use fallback evidence and continue. Tooling failure is not a reason to abandon the QA loop.
- Never claim 90+ or completion without validation evidence.
