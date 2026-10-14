# ChatGPT web models (`[web]` routes)

jcode can use the ChatGPT models available to your own ChatGPT account (for
example `gpt-6[web]`) through chatgpt.com. Prompts run in a **private, invisible
browser** that jcode controls. Your own browser windows and tabs are never used.

## How it works

- jcode starts Google Chrome or Chromium **headless** with its own profile at
  `~/.jcode/chatgpt-web/profile` (inside `JCODE_HOME` when set). The profile
  holds your ChatGPT session and is owner-only (`0700`).
- jcode talks to it over Chrome's DevTools **pipe**. No network port is opened,
  so other local programs cannot reach the session, and the browser exits with
  jcode. An idle browser stops after 10 minutes.
- Every turn opens an owned Temporary Chat tab, verifies the exact model,
  Power (thinking effort) and Temporary Chat state before sending, then
  restores your original ChatGPT model and Power choice and closes the tab.
- Only one jcode process uses the profile at a time. Others wait their turn.

## Signing in

jcode reuses the ChatGPT session from **your main browser**. When the private
profile is signed out (first use, or after the session expires), jcode asks
the jcode browser bridge (`jcode browser setup`) to open chatgpt.com in a
background tab of your browser, reads the session there, closes that tab, and
copies the session into its private browser. No window opens and no password
is entered. The session never leaves your machine and is not logged.

Only if no bridge is connected, or your browser is not signed in to ChatGPT,
jcode falls back to a one-time visible sign-in window for its private profile,
for a `[web]` message or `jcode browser chatgpt-login`. Background model-list
refreshes never show a window.

Before any `[web]` route appears in `/model`, run once:

```bash
jcode browser chatgpt-login    # copies your browser's ChatGPT session
jcode browser chatgpt-status   # browser, profile, display and session check
```

## Requirements

- **Linux or macOS:** Google Chrome or Chromium. jcode looks for
  `google-chrome`, `google-chrome-stable`, `chromium`, `chromium-browser` and
  `chrome` on `PATH`, the standard macOS app bundles, then the newest
  Playwright Chromium in `~/.cache/ms-playwright`. Override with
  `JCODE_CHATGPT_WEB_CHROME=/path/to/chrome`.
- **WSL2:** install the **Linux** Chrome or Chromium inside WSL (Windows
  browsers under `/mnt/<drive>` are never used). Any of these works:
  - `npx playwright install chromium` (no root needed), or
  - the Google Chrome `.deb`: `sudo apt install ./google-chrome-stable_current_amd64.deb`.

  With the browser bridge connected to your Windows browser, the session is
  copied from there and no display is needed. Without the bridge, the
  fallback sign-in window needs a display: WSLg on Windows 11 provides one
  (`DISPLAY`/`WAYLAND_DISPLAY` are set). Prompts never need a display.
- **Native Windows** is not supported yet. Use WSL2.

## Troubleshooting

| Symptom | Fix |
| --- | --- |
| `[web]` models missing from `/model` | Sign in to chatgpt.com in your browser, run `jcode browser chatgpt-login`, then `/model refresh`. |
| "ChatGPT is not signed in in your main browser" | Sign in to chatgpt.com in the browser the bridge is connected to (`jcode browser status`). |
| "no display is available for the sign-in window" | Run `jcode browser chatgpt-login` from a desktop session (WSLg, X11, Wayland). |
| "needs Google Chrome or Chromium" | Install one as above, or set `JCODE_CHATGPT_WEB_CHROME`. |
| Session keeps expiring | Run `jcode browser chatgpt-status`. Sign in again with `jcode browser chatgpt-login`. |
| Start fresh | Quit jcode and delete `~/.jcode/chatgpt-web/`. |
