# App Store Connect metadata (paste-ready)

Everything below is the exact text to enter in App Store Connect. Nothing here
requires a decision at submission time.

## Basics

| Field | Value |
|---|---|
| App name | `jcode` |
| Subtitle (30 char max) | `Your coding agent, remotely` |
| Bundle ID | `com.jcode.mobile` |
| SKU | `jcode-mobile` |
| Primary category | Developer Tools |
| Secondary category | Productivity |
| Price | Free |
| Age rating | 4+ (answer "None" to every content question) |
| Content rights | Does not contain, show, or access third-party content |

## Promotional text (170 char max)

```
Drive the jcode coding agent on your own machine from your phone. Pair over your
tailnet, watch tool calls stream live, and steer runs from anywhere.
```

## Description

```
jcode puts the coding agent running on your own computer in your pocket.

Pair once with a jcode server on your machine, and your phone becomes a full
remote control for that session: read the streaming response, watch each tool
call as it runs, interrupt a run that is going the wrong way, queue a follow-up
mid-turn, switch sessions, and change models without touching your desk.

WHAT YOU CAN DO
- Follow live output: streaming text, reasoning, and tool calls with results
- Interrupt an in-flight run, or queue a message that lands at the next safe point
- Switch between sessions on your server, and rename them
- Change model and reasoning effort from the header
- Compact a long conversation when context gets tight
- Reconnect automatically when you come back to the app

HOW IT CONNECTS
Run jcode on your computer and it prints a pairing QR code. Scan it, and the
phone talks directly to your machine over your LAN or your private Tailscale
tailnet. There is no cloud service in between, no account to create, and no
data sent to anyone. Your pairing token is stored in the Keychain.

TRY IT WITHOUT A SERVER
Not set up yet? Tap "Try the offline demo" on the pairing screen for a scripted
session that runs entirely on your device, so you can see the whole interface
before you pair anything.

REQUIREMENTS
A computer running jcode, reachable from your phone over your local network or
tailnet. jcode is free and open source: https://github.com/1jehuang/jcode
```

## Keywords (100 char max, comma separated, no spaces)

```
coding,agent,terminal,developer,remote,ssh,tailscale,ai,cli,devtools,programming
```

## URLs

| Field | Value |
|---|---|
| Support URL | `https://github.com/1jehuang/jcode/issues` |
| Marketing URL | `https://github.com/1jehuang/jcode` |
| Privacy Policy URL | `https://github.com/1jehuang/jcode/blob/master/ios/AppStore/PRIVACY.md` |

## App Privacy questionnaire

Answer **"No, we do not collect data from this app."** That is the complete
answer; no data types follow. It matches `PrivacyInfo.xcprivacy`, which declares
no collected data types, no tracking, and one required-reason API (UserDefaults,
CA92.1).

## Export compliance

`ITSAppUsesNonExemptEncryption` is already `false` in Info.plist, so App Store
Connect will not ask. (The app uses no encryption of its own; TLS/WireGuard is
provided by the network layer.)

## Screenshots

Required: 6.9" iPhone (1320 x 2868). Optional but recommended: 13" iPad
(2064 x 2752) since the app is universal (`TARGETED_DEVICE_FAMILY: "1,2"`).

Capture them from demo mode so no private code appears:

```bash
cd ios
./TestHarness/capture_screenshots.sh
```

Suggested five shots, in order:
1. Live conversation with a streamed answer
2. A tool call card, expanded with output
3. Sessions sheet
4. Model picker
5. Pairing screen (shows the QR affordance and the demo entry)

## Version information

| Field | Value |
|---|---|
| Version | `2.0.0` (from `MARKETING_VERSION` in `ios/project.yml`) |
| Build | injected by CI from the workflow run number |
| Copyright | `2026 Jeremy Huang` |

## What's New in This Version

```
First public release.
- Pair with your own jcode server by QR code or manually
- Live streaming transcript with tool calls, interrupts, and queued messages
- Session switching, model and reasoning-effort control, context compaction
- Offline demo mode so you can explore the app before pairing
```

## App Review Information

- Sign-in required: **No**
- Contact: account holder's name, phone, and email
- Notes: paste the entire contents of `REVIEW_NOTES.md`
