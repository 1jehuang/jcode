# Release artifact signing & SmartScreen / Gatekeeper warnings

Issue [#56](https://github.com/quangdang46/jcode/issues/56) reported that
the release package shows a "risk warning" when launched on Windows
(SmartScreen) or macOS (Gatekeeper). This document explains why and how
to verify the artifacts are legitimate.

## Why the warning?

jcode release binaries are **not yet code-signed** with an Authenticode
(Windows) or Apple Developer ID (macOS) certificate. Both Windows
SmartScreen and macOS Gatekeeper flag any unsigned executable on first
run, regardless of where it came from.

Code signing requires:
- **Windows**: an EV (Extended Validation) Authenticode certificate,
  ~$300/year from a certificate authority. Provides immediate
  reputation; non-EV certs need to accumulate downloads before
  SmartScreen stops flagging.
- **macOS**: an Apple Developer ID Application certificate ($99/year)
  + notarization through Apple's notary service.

Until those are funded and wired into the release workflow, every
release will surface the warning on first run.

## How to verify the binary is legitimate

The release pipeline publishes a `SHA256SUMS` file alongside every
release containing checksums of every artifact:

```bash
VERSION=v0.12.4
ARTIFACT=jcode-linux-x86_64.tar.gz   # adjust for your platform

curl -LO "https://github.com/quangdang46/jcode/releases/download/${VERSION}/${ARTIFACT}"
curl -LO "https://github.com/quangdang46/jcode/releases/download/${VERSION}/SHA256SUMS"

# Linux / macOS / WSL:
sha256sum --check --ignore-missing SHA256SUMS

# Windows PowerShell:
Get-FileHash -Algorithm SHA256 .\jcode-windows-x86_64.tar.gz | Format-List
# Compare against the line in SHA256SUMS for that artifact.
```

`SHA256SUMS` is generated inside the release workflow from the actual
artifacts uploaded — so if both files come from the same Releases page,
matching checksums confirm the binary you downloaded is bit-identical
to what GitHub Actions built.

## Suppressing the warning per-platform (advanced)

> ⚠️ Only do this after verifying the SHA256 checksum.

### Windows SmartScreen

If SmartScreen blocks the binary:
1. Click **More info** in the SmartScreen dialog.
2. Click **Run anyway**.

Alternatively, unblock the file via PowerShell:
```powershell
Unblock-File -Path .\jcode.exe
```

### macOS Gatekeeper

If Gatekeeper blocks the binary:
```bash
xattr -d com.apple.quarantine /path/to/jcode
```

Or right-click the binary in Finder → **Open** → confirm in the dialog.

## Roadmap

Code signing is funded only when there's clear demand or sponsorship
covering the certificate cost + workflow integration time. Track / +1
issue [#56](https://github.com/quangdang46/jcode/issues/56) if this
matters for your team's deployment.

## See also

- [README.md "Verifying release artifacts"](../README.md#verifying-release-artifacts)
  — same SHA256 verification flow.
- [`.github/workflows/release.yml`](../.github/workflows/release.yml)
  — where the SHA256SUMS file is generated.
