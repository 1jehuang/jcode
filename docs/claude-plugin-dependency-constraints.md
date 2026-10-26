# Claude plugin dependency constraint evidence

Investigated 2026-10-26 against installed Claude Code **2.1.310**. This is an evidence note, not a claim that jcode implements version constraints. **Keep jcode's explicit unsupported errors for version/sha objects and `@^` suffixes.** No parser subset, Cargo dependency, or plugin behavior was changed in this investigation.

## Artifact identity and source locations

See `docs/claude-plugin-rea.md`, especially E5 (`ev_e99846209afa13abb853276e53b97623993355a7d54e68064dab8d39bd4b67d6`), for retained extraction provenance.

- Target: `/Users/siavash/.local/share/claude/versions/2.1.310`.
- Independently rechecked SHA-256: `ffb64331bc0ee1b69efb7c2c428bd3ab516bc31b75bf60886b356c97c1c25ffd`.
- Retained source directory: `/Users/siavash/.jcode/scratch/claude-plugin-rea-20261026-full/`.
- The imported dependency helper `chunk-2nbdxfp7.js` was **omitted by the original keyword-selected extraction**, not absent from Claude. It was read directly from the verified binary, in memory only, as an exact NUL-terminated source block at binary byte range **[220818422, 220838790)**. Block SHA-256: `7e4923580bf358911ba0a46573f09ba0f52fa9f785db7d1b8782c227ff25ae89`.

The supplemental helper inspection has no newly issued REA Evidence ID. The following offsets are **JavaScript string character offsets within each source block**, not binary addresses:

| Source block | Offset | Evidence |
| --- | ---: | --- |
| `embedded-216124080.js` | 110021 / 110096 | `Oo` dependency string regex / `Ko` normalization schema |
| Supplemental helper above | 3986 | `U9s`, raw object constraint extraction |
| Same helper | 4565 | `xU`, declaring-marketplace qualification |
| Same helper | 5138 | `EWt`, constraint intersection |
| Same helper | 7056 | `d7n`, installed-version comparison |
| Same helper | 7331 / 8082 | `j9s`, acquisition closure / `W9s`, runtime demotion |
| `embedded-217740010.js` | [2063, 28407) | Pure bundled semver module definitions, ending after `oy` export factory |
| `embedded-225156883.js` | 4483727 | `JOr`, git-tag range selection |
| Same source | 4620646 / 4628293 | `KAo`, existing pinner ranges / `qs`, materialization constraints |
| Same source | 4697513 / 4706384 | Raw constraints returned by manifest loader / `Kle` propagation |
| `embedded-236639648.js` | 111501 | Update path gathers enabled and disabled pinners using `B9s` |

## Parser and enforcement are separate

**Observed static:** the string schema accepts a plugin name, optional `@marketplace`, and optional `@^` followed by any characters except `@`. It then **strips the entire `@^...` suffix**. It does not turn that suffix into an enforced range.

The object schema validates `name` and optional `marketplace`, permits extra keys through `loose()`, then normalizes the object to an identifier. Separately, `U9s` reads the **raw candidate manifest** and extracts object-only string `version` and string `sha` fields into a map. Entries with neither string field are skipped. Duplicate object entries for the same identifier overwrite earlier constraint entries. Numeric `version` is ignored by this extractor, rather than rejected there.

`d7n` uses the actual bundled node-style semver implementation, explicitly applying:

```js
const normalized = P.valid(installed) ?? P.coerce(installed)?.version;
return normalized !== undefined && P.satisfies(normalized, required);
```

No loose-mode or include-prerelease options are supplied. `EWt` validates ranges, expands `||` combinations into conjunctions, rejects invalid or empty intersections, and caps total input at 4096 characters and expansion at 1024 conjunctions.

**Observed static lifecycle:** installation combines existing and newly discovered object version requirements. For git-capable sources, `JOr` reads remote tags named `<plugin>--v<version>`, cleans versions, prefers peeled annotated-tag SHAs, and selects `maxSatisfying`. Some plain marketplace-path cases fall back to HEAD when no tag matches; other git cases return `no-matching-tag`. Existing incompatible versions can return `range-conflict` / `installed-unsatisfied` with a policy-aware update suggestion. The update path also intersects requirements from enabled and disabled pinners.

At runtime, `W9s` iteratively demotes dependent plugins for missing/disabled dependencies or incompatible versions, cascading until stable. It prefers dependency `resolvedVersion` over `manifest.version`. There are host/synced-source fallback exceptions, so this is not evidence that every possible source receives identical checks. Root marketplace trust remains root-only in `j9s`; transitive marketplace allowlists do not extend it.

### Unresolved SHA semantics

`U9s` **retains** a string `sha`. The traced installation aggregation/materialization, update-pinner, and runtime-version paths use `version`, not `sha`. This does **not** establish whether another path enforces SHA pins. No successful SHA pin check, rejection, checkout selection, or precedence rule was observed. Do not invent enforcement or silently discard `sha`; keep an explicit unsupported error until separately traced and tested.

## Isolated executable probes

These are synthetic, isolated probes of extracted pure functions, **not live plugin installation or application runtime acceptance**. The bundled semver slice was evaluated in a VM with a minimal CommonJS factory and empty `process.env`; no app imports/bootstrap were run. Extracted `U9s`, `d7n`, and `EWt` function bodies were exercised without filesystem/network capabilities. The oversized-range probe substituted only the logging/error helper while retaining the actual threshold logic.

| Probe | Observed result |
| --- | --- |
| String `foo@^1.2.3`, `foo@^garbage`, or `foo@^` | Accepted, normalized to `foo`; no extracted constraint |
| String `foo@market@^>=1 <2` | Accepted, normalized to `foo@market` |
| String `foo@>=1` or `foo@market@^x@y` | Rejected by string grammar |
| Object `{name:"foo",version:"^1.2"}` | Extracted version `^1.2` |
| Object `{name:"foo",version:123}` | No extracted constraint |
| Duplicate objects for `foo`, versions `^1` then `^2` | Extractor keeps `^2` |
| Object `{name:"foo",sha:"deadbeef"}` | SHA retained, enforcement not established |
| Installed `1.2.3`, requirement `1.2.3` | True |
| Installed `1.2.4`, requirement `1.2.3` | False: bare npm version is exact |
| Installed `v1.2.3`, requirement `^1.2` | True |
| Installed `1.2`, requirement `>=1.2.0 <2` | True through coercion |
| Installed `release-1.2.3-abc`, requirement `^1` | True through coercion |
| Installed `1.3.0-beta.1`, requirement `^1.2` | False |
| Installed `1.2.3`, requirement `1.x || >=3` | True |
| Missing or noncoercible installed version, requirement `*` | False in `d7n` |
| Intersect no ranges, `*`, or empty-string range | `*` |
| Intersect `^1` and `^2` | `disjoint` |
| Intersect `bogus` | `invalid` |
| Range `1.2.3 - 2.0.0` | Normalized to `>=1.2.3 <=2.0.0` |
| Total input 4097 characters | `too-complex` |

## jcode boundary and future regression requirements

`crates/jcode-base/src/plugin/dependencies.rs` explicitly rejects suffix ranges and object keys beyond `name` / `marketplace`. This is honest unsupported behavior, not range enforcement.

`PluginManifest`, `InstalledPlugin`, `PluginRoot`, and `AvailablePlugin` in `plugin.rs` carry optional string versions, without Claude's separate `resolvedVersion`. `effective_manifest` prefers the source manifest version with catalog fallback. Installation writes the effective manifest into the cached payload and records its version. Installed dependency reuse and runtime dependency checks reread the **cached manifest and its dependency edges**, not newer catalog entries. Future constraints must check those actual bytes, not substitute a catalog version.

`Cargo.lock` contains Rust `semver` **1.0.27 transitively**; the inspected Cargo manifests have no direct semver dependency. Its cached source documents comma-separated comparators (`src/lib.rs:172`, missing-comma example at 505), and `src/parse.rs:153` defaults an omitted operator to **caret**. Direct `VersionReq::parse` is not npm parity: bare exact versions, whitespace conjunctions, `||`, hyphen ranges, and installed-version coercion differ.

Any future approved implementation needs fixtures for npm exact/caret/coercion/prerelease behavior; malformed and absent versions; multiple incoming diamond constraints checked before visited-node shortcuts; stale catalog versus cached manifest; staged rollback; effective-scope disabled masking; root-only cross-marketplace trust; and explicit unsupported SHA behavior. Preserve cache containment and atomic registry publication. An ad hoc semver subset or tag-acquisition change is not approved by this note.
