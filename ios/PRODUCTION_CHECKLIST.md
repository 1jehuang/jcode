# iOS Production Readiness Checklist

Each item has an executable pass condition. Run `./TestHarness/check_production.sh`
to evaluate everything that can be checked locally.

## Code and behavior

| # | Item | Pass condition | Status |
|---|------|----------------|--------|
| 1 | JCodeKit unit tests | `cd ios && swift test` exit 0 | PASS |
| 2 | E2E harness (mock gateway, simulator) | `./TestHarness/run_e2e.sh` exit 0 | PASS |
| 3 | Protocol smoke vs real gateway | `protocol_smoke_test.py --port 7643` vs `jcode serve` | PASS |
| 4 | Interaction-graph engine deterministic | `python3 -m reward.interaction.test_engine` exit 0 | PASS |
| 5 | Reward scorers deterministic | `python3 -m reward.test_determinism` exit 0 | PASS |
| 6 | UX reward at or above baseline (88.7), worst cell >= 83 | `reward.aggregate --baseline --candidate` non-negative delta | PASS |
| 7 | Foreground reconnect | scenePhase handler in JCodeMobileApp.swift | PASS |
| 8 | Unauthorized (revoked token) stops reconnect loop, prompts re-pair | `unauthorizedStopsReconnectingAndAsksForRePair` test | PASS |

## App Store submission requirements

| # | Item | Pass condition | Status |
|---|------|----------------|--------|
| 9 | Privacy manifest | `Sources/JCodeMobile/PrivacyInfo.xcprivacy` present, UserDefaults reason CA92.1 | PASS |
| 10 | Camera permission string | `NSCameraUsageDescription` in Info.plist | PASS |
| 11 | Local network permission string | `NSLocalNetworkUsageDescription` in Info.plist | PASS |
| 12 | Export compliance | `ITSAppUsesNonExemptEncryption=false` in Info.plist | PASS |
| 13 | App icon (no alpha) | `AppIcon.appiconset` with 1024pt marketing icon | PASS |
| 14 | Launch screen | `UILaunchScreen` dict + LaunchBackground colorset | PASS |
| 15 | ATS exception justified | `NSAllowsArbitraryLoads=true` documented below | PASS (documented) |
| 16 | Version/build numbers | MARKETING_VERSION set; build number injected by CI from run number | PASS |

### ATS justification (App Review note)

The app is a remote control for the user's own `jcode` servers, reached over
their private tailnet (WireGuard-encrypted) or LAN as `ws://host:7643`.
Servers are user-owned dev machines without public CAs, so TLS is not
available; transport privacy comes from Tailscale itself. The app never
connects to any host the user did not explicitly pair with. This is why
`NSAllowsArbitraryLoads` is set.

## CI / delivery

| # | Item | Pass condition | Status |
|---|------|----------------|--------|
| 17 | swift test job | `.github/workflows/ios-testflight.yml` test job green | PASS |
| 18 | Simulator compile check | compile-check job green | PASS |
| 19 | TestFlight upload | build-and-upload job green | **BLOCKED: cloud signing permission error (see below)** |

## Account-holder-only items (exact instructions)

The workflow now archives unsigned and signs at export. The remaining
failure is `exportArchive Cloud signing permission error` + `No profiles
for 'com.jcode.mobile' were found`, which means the App Store Connect
API key cannot create the distribution certificate/profile. Fix once:

1. Sign in at <https://developer.apple.com/account> as the Account Holder
   (Jeremy Huang). If a Program License Agreement banner is pending,
   accept it first; nothing below works until it is accepted.
2. In App Store Connect > Users and Access > Integrations > App Store
   Connect API: make sure the key used by CI (`APPSTORE_API_KEY_ID`)
   has the **Admin** role. Cloud-managed signing does not work with
   Developer/App Manager keys.
3. In Certificates, Identifiers & Profiles, register the App ID
   `com.jcode.mobile` (Identifiers > + > App IDs) if it does not exist.
4. In App Store Connect > Apps, create the app record for
   `com.jcode.mobile` (name: jcode, platform: iOS) if it does not exist.
5. Re-run the workflow: `gh workflow run "iOS TestFlight" --ref master`.
6. After the first successful upload, complete the app privacy
   questionnaire ("Data Not Collected") and add TestFlight testers.
