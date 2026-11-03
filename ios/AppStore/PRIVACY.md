# jcode for iOS: Privacy Policy

_Last updated: 2026-08-08_

## Summary

The jcode iOS app collects no personal data. There is no account, no analytics,
no advertising, no tracking, and no server operated by us that your data passes
through.

## What the app connects to

The app is a remote control for a `jcode` server that **you** run on **your own
computer**. It connects only to servers you explicitly pair with, by entering
their address or scanning a pairing QR code that your own machine displays.
Traffic goes directly from your phone to your machine over your local network or
your private Tailscale tailnet. It does not pass through any service we operate.

## What the app stores on your device

- **Pairing tokens.** One authentication token per paired server, stored in the
  iOS Keychain. Used only to authenticate to that server.
- **A device identifier.** A random UUID generated on first launch and stored in
  UserDefaults, so a server you pair with can recognize the same device on
  reconnect. It is not tied to your identity, is not shared with anyone, and is
  destroyed when you delete the app.
- **Server addresses.** The host and port of servers you paired with, so the app
  can reconnect.

Nothing else is stored, and none of it leaves your device except the token,
which is sent to the server it belongs to.

## Camera

The camera is used for one purpose: scanning a pairing QR code shown by the
`jcode pair` command on your own machine. Camera frames are processed on device
to read the code and are never stored or transmitted. Pairing can also be
completed by typing the details manually, without granting camera access.

## Conversation content

Messages you send and the responses you see are exchanged directly with your own
paired server and are stored on that machine, under your control. The app keeps
them only in memory for display. We never receive them.

## Demo mode

Offline demo mode uses a fixed script compiled into the app. It makes no network
requests and produces no data.

## Third parties

The app contains no third-party SDKs, no analytics, and no advertising
libraries.

## Children

The app is not directed at children and collects no data from anyone.

## Changes

Any change to this policy will be published at this URL along with a new "last
updated" date.

## Contact

Questions: open an issue at <https://github.com/1jehuang/jcode/issues>.
