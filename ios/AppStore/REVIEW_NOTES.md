# App Review Notes (paste into App Store Connect > App Review Information)

## What the app is

jcode is a remote control for the `jcode` coding agent that the user runs on
their own computer. The phone is a thin client: it pairs with a server the user
already controls, then streams that session's transcript and sends messages
back. The app has no backend of its own and no accounts.

## How to review it without any setup (important)

The app ships an **offline demo mode** so it is fully functional with no server,
no account, and no network access.

1. Launch the app.
2. On the pairing screen, tap **"Try the offline demo"** (bottom of the screen,
   below the pairing form).
3. The full interface opens with a scripted session. Tap any starter prompt, or
   type your own message, and the app streams reasoning, a tool call, and an
   answer exactly as it does against a real server.
4. A persistent banner reads "Demo mode: scripted replies, no server connected"
   so the demo is never mistaken for a live session. Tapping **Pair** in that
   banner returns to pairing.

Demo mode runs entirely on-device and makes zero network requests.

## No demo account needed

There is no login, no account system, and no server operated by us. Sign-in is
not applicable; the "Demo account" fields can be left blank.

## Camera usage

The camera is used only to scan a QR code printed by the `jcode pair` command on
the user's own machine. It is never used in demo mode and is optional even when
pairing (host, port, and code can be typed in manually).

## Local network usage

The app connects to a server the user explicitly paired with, on their own LAN
or their private Tailscale tailnet. It performs no scanning or discovery, and
contacts no host the user did not enter.

## App Transport Security exception (NSAllowsArbitraryLoads)

The app connects to the user's own development machines as `ws://host:7643`.
These are personal machines on a private network; they have no public DNS name
and therefore cannot obtain a certificate from a public CA, so TLS is not
available. Transport confidentiality is provided by the network itself:
Tailscale is WireGuard-encrypted end to end, and the alternative is a trusted
LAN. The app never connects to any host the user did not explicitly pair with,
and no traffic goes to the public internet. This is why the exception is
required and why it cannot be narrowed to a fixed domain list: the hostnames are
different for every user.

## Privacy

The app collects nothing. It stores one pairing token per server in the
Keychain, and one randomly generated device identifier in UserDefaults
(declared in `PrivacyInfo.xcprivacy` under required-reason code CA92.1) so a
server can recognize a returning device. Nothing is transmitted anywhere except
to the user's own paired server.

## Content

All content shown in the app comes from the user's own machine (their code,
their commands) or, in demo mode, from a fixed script compiled into the app. The
app does not host, generate, or serve user-generated content between users.
