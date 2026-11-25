# Plugin catalogs

The `jcode plugin` commands manage Claude-style marketplace catalogs and explicit user, project or local installations. They run without provider credentials or a server. Listing reads local registry metadata; adding or updating a marketplace and installing or updating a plugin can acquire Git sources.

```sh
jcode plugin marketplace add /path/to/marketplace
jcode plugin marketplace list --json
jcode plugin install sample@market --scope project
jcode plugin list --json
jcode plugin disable sample@market --scope project
jcode plugin enable sample@market --scope project
jcode plugin marketplace update market
jcode plugin update sample@market --scope project
jcode plugin uninstall sample@market --scope project
jcode plugin validate /path/to/plugin
```

`--cwd` chooses the project for project/local records. Mutations use `--scope user` by default; listing shows all records applicable to the current project. A local record takes precedence over project and user records, including when disabled. Ambiguous names require `name@marketplace`. Removing a record preserves its immutable cached versions. Catalog records and payloads live under `JCODE_HOME/plugin-catalogs`, separate from external-provider bundle storage. On Unix, published cache directories are private to the current user.

A marketplace contains `.claude-plugin/marketplace.json` with a `name` and a `plugins` array. Each entry names a plugin and its source. A plugin can provide `.claude-plugin/plugin.json`; marketplace declarations can provide the manifest metadata when that file is absent. Component paths must remain inside the plugin. Symlinks and special files are refused during publication. Git objects support branch/tag refs, exact full commit SHAs and a contained `git-subdir` path. A declared SHA is authoritative, and a failed pin never substitutes HEAD.

Dependencies use an array of names, `name@marketplace` identifiers or `{ "name": "name", "marketplace": "market" }` objects. Names without a marketplace resolve in the declaring catalog. Dependencies in other registered marketplaces require the root catalog's `allowCrossMarketplaceDependenciesOn` array; a transitive catalog cannot expand that allowlist. Cycles, unsupported ranges, missing dependencies and graph-limit violations fail before registry publication. A failed update preserves the existing installed registry and enabled state. An unavailable or disabled effective dependency prevents the dependent root from being advertised.

Installing or updating validates the dependency closure in the requested scope. Higher project/local masks still control runtime activation in that project, without blocking a user-wide installation elsewhere. A `defaultEnabled: false` root keeps newly acquired dependencies inactive. Enabling that root also enables its automatically disabled dependencies; explicitly disabled dependencies require a separate enable command. Registries without disable provenance conservatively preserve disabled records.

Already enabled dependencies in lower visible scopes remain usable when re-enabling a root. Automatic enablement changes only records in the requested scope; disabled lower-scope dependencies need a separate explicit enable command.

These commands manage catalog records and files. Plugin skill, command, hook, MCP and LSP activation is provided by separate integrations; this catalog layer does not run component code. It does not register external provider extensions.
