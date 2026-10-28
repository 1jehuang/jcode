# Native signing-helper validation

This dedicated, dispatch-only workflow validates the standalone
`1jehuang/justrust` helper at `17c344f2e83f836598b82715950497c0879aa713`.
The existing Jcode signing/notarization run `40628778548` passed, but is not
proof that this standalone helper executed on macOS. This lane supplies that
separate evidence only after its protected run succeeds.

## Review and dispatch

A coordinator must review the four new files and the committed master revision.
Keep the existing `native-handoff-signing` environment's required reviewer and
master-only deployment restriction. Do not change its credentials or protection
rules. After review, the coordinator can dispatch exactly:

```bash
gh workflow run signing-helper-validation.yml --repo 1jehuang/jcode --ref master
```

There are no workflow inputs. No dispatch is part of implementation validation.
The workflow runs only for canonical `1jehuang/jcode` master, with `contents:
read`, no publishing, releases, arbitrary artifact downloads, or notarization.
It reuses only `APPLE_CERTIFICATE_P12_BASE64`, `APPLE_CERTIFICATE_PASSWORD`, and
`APPLE_SIGNING_IDENTITY`, scoped to the signing step. Environment approval gates
the job. Earlier steps in that job have no signing credential environment.

## Evidence and boundaries

Before credentials, the ARM64 macOS runner checks out exact Jcode source and the
sparse, pinned helper source, runs the helper's 17 Python tests and the local
credential-free fixtures, and compiles fixed C source using `/usr/bin/clang`.
The unsigned executable prints `jcode-signing-helper-native-ok`. Its trusted
SHA256 is a same-job step output, not caller-controlled input.

The signing step rechecks the source pin and unsigned hash. It uses the proven
temporary keychain import, search-list, and ACL setup from
`scripts/sign_native_handoff_macos.sh` without changing that script. The original
search list is snapshotted before keychain creation and restored on exit before
the temporary keychain and P12 file are removed. Forced runner termination is
ultimately subject to ephemeral runner destruction, not an EXIT-trap guarantee.
The exact full Developer ID identity must resolve uniquely to a SHA1 fingerprint
inside the isolated keychain.

The real pinned helper first plans without creating output, then executes with
`--input`, a new `--output`, the trusted `--sha256`,
`--target aarch64-apple-darwin`, `--identity` fingerprint, and `--execute`.
It invokes absolute `/usr/bin/codesign`, not a test replacement. An additional
strict verification uses inline requirements with leading `=` for the Apple
Developer ID chain, `com.jcode.cli` identifier, and exact Team ID. The signed
binary must execute the expected string. The unsigned hash must remain unchanged,
and retrying into the existing destination must fail without changing it.

Only `signing-helper-validation/receipt.json` is uploaded. This public,
allowlisted receipt binds repository, source SHA/ref, run ID/attempt, workflow,
pinned helper revision, original and signed hashes, Team ID, target, identifier,
and completed checks. No P12, password, raw identity discovery files, keychain,
helper plan, binary, or broad directory is uploaded. It is a workflow receipt,
not a cryptographic artifact attestation or a notarization/Gatekeeper claim.

## Credential-free checks

```bash
bash -n scripts/check_signing_helper_macos.sh
shellcheck scripts/check_signing_helper_macos.sh
actionlint .github/workflows/signing-helper-validation.yml
python3 -m unittest discover -s scripts -p test_signing_helper_validation.py
```

The fixture tests exercise exact identity selection, ambiguity and malformed
identity refusal, absolute native-tool plan validation, inline requirement
selection, public receipt fields, dispatch guards, and workflow scope. They do
not substitute for the coordinator-reviewed protected macOS execution.
