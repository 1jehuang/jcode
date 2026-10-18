# AUTH-DNS-FORENSICS-2026-10-17

`auth.kooshapari.com` CIMD DNS forensics — read-only investigation, 2026-10-18T01:26–01:37Z.

## 1. Current resolution status

Zone `kooshapari.com` (id `6c9edab581e9c7b8fdb6a83adc6878ea`), NS `glen`/`grannbo.ns.cloudflare.com`, active, Free plan.
Checked against authoritative NS, 1.1.1.1, 8.8.8.8, and the Cloudflare DNS API.

| Host | Result | Notes |
|---|---|---|
| `kooshapari.com` | 64.29.17.1 / 216.198.79.1 | DNS-only CNAME -> Vercel (`proxied:false`) |
| `www.kooshapari.com` | resolves | Vercel |
| **`auth.kooshapari.com`** | **NXDOMAIN** | **No DNS record exists** (API `dns_records?name=auth...` = `[]`) |
| `api` / `git` / `cloud` / `gh` | NXDOMAIN | never existed; not in zone |
| Present & healthy (context) | `agents`, `agents-relay`, `cpa`, `dash`, `omni`, `paperclip`, `phenokits`, `projects`, `rsend*`, `send*`, `ssh`, `sub.accesstest`, `tasks`, `thegent` | unaffected |

The Worker route `auth.kooshapari.com/oauth/client-metadata.json -> jcode-cimd` **still exists**; the script `jcode-cimd` **still exists** (created 2026-10-15T22:14:13Z, last modified 22:16:48Z). `https://jcode-cimd.kooshapari.workers.dev/oauth/client-metadata.json` returns **200 application/json** right now. Only the DNS record is gone.

## 2. Git history evidence

| Commit | When | Summary |
|---|---|---|
| `27b3707aa` | 2026-10-15T22:14–22:17Z | Created worker `jcode-cimd`; apex route was a silent no-op (grey-cloud); **moved to `auth.kooshapari.com` + created proxied CNAME**. Live-verified `HTTP 200`. |
| `e81d2060f` | ~22:30Z | Re-confirmed via API: script present, route present, **DNS `CNAME auth -> jcode-cimd.kooshapari.workers.dev, proxied true`**; 5x consecutive 200. |
| `3f4746eb5` | 2026-10-17T08:04Z | Re-probe: `HTTP 200`, `application/json`, 356 B. |
| `19e3bcb38` | 2026-10-17T22:46Z | NXDOMAIN from all resolvers. Contradicts §24 condition 1. |

## 3. Cloudflare audit-log evidence (authoritative)

- `2026-10-15T22:17:30Z` `dns.record` **create** in zone `kooshapari.com` (= the `auth` CNAME, 45 s after the route_create).
- `2026-10-17T20:46:50.734Z` `dns.record` **delete** in zone `kooshapari.com`, record id `6241e2fc6850ef45807340add5caa9e3`. **The only `kooshapari.com` DNS delete in the entire window.**
- Actor: `kooshapari@gmail.com`, **interface `UI`** (Cloudflare Dashboard), IP `66.253.120.107`.
- Same session (20:46–21:52Z): a burst of DNS delete/create across `phenotype.space` / `pheno.studio` — a multi-zone DNS cleanup run. The `auth` record was collaterally removed.

## 4. Likely root cause

**(b) Deployed, then deleted.** Live for ~46.5 h (2026-10-15T22:17:30Z -> 2026-10-17T20:46:50Z), then deleted manually via the Cloudflare Dashboard at 20:46:50Z; NXDOMAIN observed 2 h later at 22:46Z.

- Not (a): the Oct-17T08:04Z 200 response plus the audit `create` event both prove it was deployed.
- Not (c): a propagation/TTL fault would still show the record in the API; the API returns `[]` and the authoritative NS answer NXDOMAIN — a deleted record, not an unpropagated one.
- Caveat: audit metadata omits the record name; identity is inferred from being the sole `kooshapari.com` delete + matching the now-missing host + timing.

No local script deletes DNS records. The three repo files referencing the host (`scripts/portal-mcp-bridge.json`, `scripts/stub-cimd-server.py`, `scripts/test-portal-bridge-live.sh`) are additive only.

## 5. Operator action (re-deploy)

The Worker and route already exist; **only the DNS record must be recreated**:

- Dashboard: https://dash.cloudflare.com/49dce512822987e0522f0faeffbcc0c8/kooshapari.com/dns/records
- Add record: **Type `CNAME`**, **Name `auth`**, **Target `jcode-cimd.kooshapari.workers.dev`**, **Proxy ON (orange cloud, non-negotiable)**, TTL Auto.
- (API equivalent, if preferred: `POST /zones/6c9edab581e9c7b8fdb6a83adc6878ea/dns_records` with `{type:"CNAME",name:"auth",content:"jcode-cimd.kooshapari.workers.dev",proxied:true,ttl:1}`.)

Verify: `curl -sS -I https://auth.kooshapari.com/oauth/client-metadata.json` -> `200`, `content-type: application/json`. Then the §24.2 bridge can complete OAuth consent.
