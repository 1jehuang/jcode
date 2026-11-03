# [POLISH-001] README still points at upstream `1jehuang/jcode` (rebrand incomplete)

**ID:** POLISH-001
**Category:** polish
**Severity:** medium
**File:** README.md
**Status:** open

## Description
The README's badges and links reference the upstream `1jehuang/jcode` repository (22 occurrences),
even though the product/rebrand is `kcode` on `KooshaPari/KCode`. Users land on the wrong repo for
releases, issues, and CI status, and the branding is internally inconsistent with the binary name.

## Evidence
```
grep -c "1jehuang/jcode" README.md   # -> 22
head -8 README.md
# [![Latest Release](https://badgen.net/github/release/1jehuang/jcode?icon=github)]...
```

## Suggested fix
Update README badges/links to `KooshaPari/KCode` (or the canonical fork URL), verify the release/
license/platform badges resolve, and add a short "upstream" attribution note if the fork lineage
should be credited.

## Effort
- S (< 30m)
