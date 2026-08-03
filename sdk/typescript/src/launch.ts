/**
 * Launching a private jcode instance.
 *
 * `JcodeClient.connect()` attaches to whatever jcode is already running on the
 * machine, which is right for a tool that automates *your* jcode (an editor
 * plugin, a dashboard) and wrong for everything else. An application embedding
 * jcode as an agent engine wants its own instance: its own sessions, its own
 * state, and no way to disturb the user's live work by accident.
 *
 * `launch()` gives it one. It starts a private daemon and bridge under a
 * dedicated `JCODE_HOME` and runtime directory, and shuts them down on
 * `close()`.
 */

import { spawn, type ChildProcess } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";

/**
 * Files inherited from the user's jcode home when logins are inherited.
 *
 * Deliberately *not* included: `auth-refresh-state.json` and
 * `auth-validation.json`, which are derived records of past auth failures.
 * Copying them imports another jcode's bad day, and a stale
 * `rejected_refresh_fingerprint` makes a fresh instance refuse to even attempt
 * a refresh with credentials that work.
 */
const CREDENTIAL_FILES = ["auth.json", "antigravity_oauth.json", "config.toml"];

/**
 * Credentials that must be *shared* with the user's home, not copied.
 *
 * OAuth refresh tokens rotate: redeeming one issues a new one and invalidates
 * the old. Two homes holding copies of the same token therefore fight, and
 * whichever refreshes second is logged out. A copy also goes stale on its own,
 * since the access token it holds expires in hours while an instance may run
 * for days. Sharing the file keeps one rotation record for one set of
 * credentials, which is what the tokens themselves already assume.
 */
const SHARED_FILES = new Set(["auth.json", "antigravity_oauth.json"]);

/**
 * Where jcode looks for *other* tools' credentials, relative to `$HOME`.
 *
 * jcode can log in by reusing an existing CLI's OAuth store, so a large share
 * of real users have no usable `~/.jcode/auth.json` at all: the working
 * credentials live in `~/.claude/` or `~/.config/github-copilot/`. Under
 * `JCODE_HOME` these lookups are sandboxed to `$JCODE_HOME/external/`, so an
 * instance that inherits only `auth.json` silently has no credentials and
 * fails on the first turn. Linking the directories makes inheritance mean what
 * it says.
 */
/**
 * jcode's own config directory, relative to the platform config root.
 *
 * `app_config_dir()` is where provider env files live (`anthropic.env`,
 * `n.env` for the jcode subscription), and `JCODE_HOME` redirects it to
 * `$JCODE_HOME/config/jcode`. It is easy to miss because it is not under
 * `~/.jcode` at all, and missing it is not a subtle failure: on a machine
 * whose working credential is a jcode subscription, `auth.json` holds only a
 * stale OAuth token, so the instance inherits exactly the credential that does
 * not work and none of the ones that do.
 */
const APP_CONFIG_DIRNAME = "jcode";

const EXTERNAL_CREDENTIAL_DIRS = [
  ".claude",
  ".codex",
  ".gemini",
  ".cursor",
  ".config/cursor",
  ".config/github-copilot",
  ".copilot",
  ".hermes",
  ".pi/agent",
  ".openclaw",
  ".local/share/opencode",
];

export interface LaunchOptions {
  /**
   * Directory holding the instance's state (sessions, logs, credentials).
   *
   * Defaults to a fresh temporary directory that is removed on `close()`.
   * Pass a stable path to keep sessions across runs.
   */
  jcodeHome?: string;
  /** Working directory for sessions created in this instance. */
  workingDir?: string;
  /**
   * Copy the user's provider logins into the instance. Defaults to `true`.
   *
   * Without credentials a fresh instance cannot talk to any model, so the
   * default is the one that works. It does mean the embedding application
   * spends the user's provider quota, so pass `false` to start empty and
   * supply credentials yourself.
   */
  inheritLogins?: boolean;
  /** Path to the jcode binary. Defaults to `jcode` on PATH. */
  binary?: string;
  /** Extra environment variables for the instance. */
  env?: Record<string, string>;
  /** Milliseconds to wait for the socket to appear. Defaults to 30000. */
  startupTimeoutMs?: number;
  /** Forward the instance's stderr to this process. Defaults to false. */
  inheritStderr?: boolean;
}

/** A running private jcode instance. */
export interface LaunchedInstance {
  /** API socket path to connect to. */
  socketPath: string;
  /** The instance's `JCODE_HOME`. */
  jcodeHome: string;
  /** The bridge process. */
  process: ChildProcess;
  /** Stop the instance and clean up anything it created. */
  shutdown(): Promise<void>;
}

/** Resolve the user's real jcode home, ignoring any instance override. */
export function userJcodeHome(): string {
  return process.env.JCODE_HOME ?? path.join(os.homedir(), ".jcode");
}

/** The user's jcode config directory, mirroring `storage::app_config_dir`. */
export function userAppConfigDir(): string {
  if (process.platform === "darwin") {
    return path.join(os.homedir(), "Library", "Application Support", APP_CONFIG_DIRNAME);
  }
  if (process.platform === "win32") {
    const appData = process.env.APPDATA ?? path.join(os.homedir(), "AppData", "Roaming");
    return path.join(appData, APP_CONFIG_DIRNAME);
  }
  const xdg = process.env.XDG_CONFIG_HOME ?? path.join(os.homedir(), ".config");
  return path.join(xdg, APP_CONFIG_DIRNAME);
}

/**
 * Give a launched instance the user's provider logins.
 *
 * Credential files are symlinked so token rotation stays coherent (see
 * {@link SHARED_FILES}); everything else is copied, so the instance can edit
 * its own configuration without touching the user's. Copies are owner-only.
 *
 * Returns the names actually inherited, so a caller can tell "inherited
 * nothing" from "inherited something".
 */
export function inheritCredentials(fromHome: string, toHome: string): string[] {
  fs.mkdirSync(toHome, { recursive: true, mode: 0o700 });
  const inherited: string[] = [];
  for (const name of CREDENTIAL_FILES) {
    const source = path.join(fromHome, name);
    if (!fs.existsSync(source)) continue;
    const destination = path.join(toHome, name);
    if (SHARED_FILES.has(name)) {
      fs.symlinkSync(source, destination);
    } else {
      fs.copyFileSync(source, destination);
      fs.chmodSync(destination, 0o600);
    }
    inherited.push(name);
  }

  // Other CLIs' credential stores, which jcode reads directly and which
  // `JCODE_HOME` redirects to `$JCODE_HOME/external/`. Symlinked for the same
  // rotation reason as `auth.json`, and because these are another tool's files
  // to own.
  // jcode's own config dir, which `JCODE_HOME` moves to $JCODE_HOME/config.
  const userConfig = userAppConfigDir();
  if (fs.existsSync(userConfig)) {
    const instanceConfig = path.join(toHome, "config", APP_CONFIG_DIRNAME);
    fs.mkdirSync(path.dirname(instanceConfig), { recursive: true, mode: 0o700 });
    if (!fs.existsSync(instanceConfig)) {
      fs.symlinkSync(userConfig, instanceConfig);
      inherited.push(`config/${APP_CONFIG_DIRNAME}`);
    }
  }

  const externalRoot = path.join(toHome, "external");
  for (const relative of EXTERNAL_CREDENTIAL_DIRS) {
    const source = path.join(os.homedir(), relative);
    if (!fs.existsSync(source)) continue;
    const destination = path.join(externalRoot, relative);
    fs.mkdirSync(path.dirname(destination), { recursive: true, mode: 0o700 });
    if (fs.existsSync(destination)) continue;
    fs.symlinkSync(source, destination);
    inherited.push(`external/${relative}`);
  }
  return inherited;
}

/**
 * Delete an ephemeral instance home, refusing to follow symlinks out of it.
 *
 * The home contains symlinks that point at the user's real credential stores
 * (`~/.claude`, `~/.jcode/auth.json`). A recursive delete that followed them
 * would destroy the user's logins while "cleaning up a temp directory", which
 * is about the worst thing this SDK could do. So links are unlinked and never
 * descended into, and anything unexpected is left in place rather than
 * force-removed.
 */
export function removeInstanceHomeForTest(home: string): void {
  removeInstanceHome(home);
}

function removeInstanceHome(home: string): void {
  const walk = (target: string): void => {
    let stats;
    try {
      stats = fs.lstatSync(target);
    } catch {
      return;
    }

    // `lstat`, not `stat`: a symlink must be reported as a link so it is
    // unlinked rather than descended into. Swapping in `stat` here deletes the
    // user's real credential store.
    if (stats.isSymbolicLink()) {
      fs.unlinkSync(target);
      return;
    }

    if (stats.isDirectory()) {
      for (const entry of fs.readdirSync(target)) {
        walk(path.join(target, entry));
      }
      fs.rmdirSync(target);
      return;
    }
    fs.unlinkSync(target);
  };
  try {
    walk(home);
  } catch {
    // A stray file is a leaked temp directory; deleting the wrong thing is
    // unrecoverable. Prefer the leak.
  }
}

/**
 * Start a private jcode instance and return once its API socket is accepting
 * connections.
 */
export async function launchInstance(options: LaunchOptions = {}): Promise<LaunchedInstance> {
  const ephemeral = options.jcodeHome === undefined;
  const jcodeHome =
    options.jcodeHome ??
    fs.mkdtempSync(path.join(os.tmpdir(), "jcode-sdk-instance-"));
  fs.mkdirSync(jcodeHome, { recursive: true, mode: 0o700 });

  // The runtime directory holds the sockets. Keeping it inside the instance
  // home is what makes the instance private: the daemon binds its socket
  // there rather than in the shared $XDG_RUNTIME_DIR, so a launched instance
  // and the user's own jcode cannot collide or find each other.
  const runtimeDir = path.join(jcodeHome, "run");
  fs.mkdirSync(runtimeDir, { recursive: true, mode: 0o700 });
  const socketPath = path.join(runtimeDir, "jcode-api.sock");

  if (options.inheritLogins ?? true) {
    inheritCredentials(userJcodeHome(), jcodeHome);
  }

  const child = spawn(
    options.binary ?? "jcode",
    ["api-bridge", "--api-socket", socketPath],
    {
      cwd: options.workingDir ?? process.cwd(),
      env: {
        ...process.env,
        JCODE_HOME: jcodeHome,
        JCODE_RUNTIME_DIR: runtimeDir,
        JCODE_API_SOCKET: socketPath,
        JCODE_SOCKET: path.join(runtimeDir, "jcode.sock"),
        ...options.env,
      },
      stdio: ["ignore", "ignore", options.inheritStderr ? "inherit" : "pipe"],
      detached: false,
    },
  );

  // Keep the last of stderr: when startup fails, the reason is in there, and
  // "timed out" alone sends the caller looking in the wrong place.
  let stderr = "";
  child.stderr?.on("data", (chunk: Buffer) => {
    stderr = (stderr + chunk.toString()).slice(-4000);
  });

  let exited: { code: number | null; signal: string | null } | undefined;
  child.once("exit", (code, signal) => {
    exited = { code, signal };
  });

  const shutdown = async (): Promise<void> => {
    if (exited === undefined) {
      child.kill("SIGTERM");
      // Give it a moment to unwind before insisting.
      await new Promise<void>((resolve) => {
        const timer = setTimeout(() => {
          child.kill("SIGKILL");
          resolve();
        }, 3000);
        timer.unref?.();
        child.once("exit", () => {
          clearTimeout(timer);
          resolve();
        });
      });
    }
    if (ephemeral) {
      // The daemon flushes sessions, logs, and caches as it exits, and it does
      // so a moment after the bridge process is gone. Deleting immediately
      // races that flush: the walk empties a directory, the daemon recreates
      // it, and `rmdir` fails with ENOTEMPTY. Retry rather than leak, and
      // leave the directory behind rather than fight it forever.
      for (let attempt = 0; attempt < 10; attempt += 1) {
        removeInstanceHome(jcodeHome);
        if (!fs.existsSync(jcodeHome)) return;
        await new Promise((resolve) => setTimeout(resolve, 100));
      }
    }
  };

  const deadline = Date.now() + (options.startupTimeoutMs ?? 30_000);
  while (Date.now() < deadline) {
    if (exited) {
      await shutdown();
      throw new Error(
        `jcode exited during startup (code ${exited.code}, signal ${exited.signal})` +
          (stderr ? `:\n${stderr.trim()}` : ""),
      );
    }
    if (fs.existsSync(socketPath)) {
      return { socketPath, jcodeHome, process: child, shutdown };
    }
    await new Promise((resolve) => setTimeout(resolve, 50));
  }

  await shutdown();
  throw new Error(
    `jcode did not create its API socket at ${socketPath} within the startup timeout` +
      (stderr ? `:\n${stderr.trim()}` : ""),
  );
}
