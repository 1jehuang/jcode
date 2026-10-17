from __future__ import annotations

import json
import os
from pathlib import Path
from typing import Any

from harbor.agents.base import BaseAgent
from harbor.environments.base import BaseEnvironment
from harbor.models.agent.context import AgentContext

IN_CONTAINER_HOME = "/tmp/jcode-home"
IN_CONTAINER_RUNTIME = "/tmp/jcode-runtime"
IN_CONTAINER_INPUT = "/tmp/jcode-input"
IN_CONTAINER_OUTPUT = "/tmp/jcode-output"
IN_CONTAINER_BINARY = "/usr/local/bin/jcode"
IN_CONTAINER_LIB_DIR = f"{IN_CONTAINER_RUNTIME}/lib"
IN_CONTAINER_CA_BUNDLE = f"{IN_CONTAINER_HOME}/ca-certificates.crt"
DEFAULT_BINARY_PATH = "/tmp/jcode-compat-dist/jcode-linux-x86_64.bin"
DEFAULT_CLAUDE_AUTH_PATH = "~/.kcode/auth.json"
DEFAULT_OPENROUTER_ENV_PATH = "~/.config/kcode/openrouter.env"
DEFAULT_ANTHROPIC_ENV_PATH = "~/.config/kcode/anthropic.env"
CA_BUNDLE_CANDIDATES = (
    os.environ.get("KCODE_HARBOR_CA_BUNDLE"),
    "/etc/ca-certificates/extracted/tls-ca-bundle.pem",
    "/etc/ssl/certs/ca-certificates.crt",
)


def _resolve_existing_file(*, env_name: str, default_path: str | None = None, candidates: tuple[str | None, ...] = ()) -> Path:
    raw_value = os.environ.get(env_name) or default_path
    values = [raw_value, *candidates] if raw_value is not None else list(candidates)
    checked: list[str] = []
    for value in values:
        if not value:
            continue
        candidate = Path(value).expanduser()
        checked.append(str(candidate))
        if candidate.exists() and candidate.is_file():
            return candidate.resolve()
    raise FileNotFoundError(f"Could not find a readable file for {env_name}. Checked: {checked}")


def _resolve_optional_existing_file(*, candidates: tuple[str | None, ...]) -> Path | None:
    for value in candidates:
        if not value:
            continue
        candidate = Path(value).expanduser()
        if candidate.exists() and candidate.is_file():
            return candidate.resolve()
    return None


def _sibling_runtime_lib_candidates(binary: Path, stem: str) -> tuple[str, ...]:
    return tuple(str(path) for path in sorted(binary.parent.glob(f"{stem}.so*")) if path.is_file())


def _load_key_from_env_file(env_path: str, env_var: str, *direct_env: str) -> str | None:
    for name in direct_env:
        value = os.environ.get(name)
        if value and value.strip():
            return value.strip()
    path = Path(env_path).expanduser()
    if path.exists() and path.is_file():
        for line in path.read_text().splitlines():
            line = line.strip()
            if not line or line.startswith("#"):
                continue
            if "=" in line:
                key, _, value = line.partition("=")
                if key.strip() != env_var:
                    continue
                value = value.strip().strip('"').strip("'")
            else:
                value = line
            if value:
                return value
    return None


def _load_anthropic_key() -> str | None:
    return _load_key_from_env_file(
        os.environ.get("KCODE_HARBOR_ANTHROPIC_ENV", DEFAULT_ANTHROPIC_ENV_PATH),
        "ANTHROPIC_API_KEY",
        "ANTHROPIC_API_KEY",
        "KCODE_HARBOR_ANTHROPIC_KEY",
    )


def _load_openrouter_key() -> str | None:
    # Priority: explicit env, then the jcode openrouter.env file (raw key per line).
    direct = os.environ.get("OPENROUTER_API_KEY") or os.environ.get("KCODE_HARBOR_OPENROUTER_KEY")
    if direct and direct.strip():
        return direct.strip()
    path = Path(os.environ.get("KCODE_HARBOR_OPENROUTER_ENV", DEFAULT_OPENROUTER_ENV_PATH)).expanduser()
    if path.exists() and path.is_file():
        for line in path.read_text().splitlines():
            line = line.strip()
            if not line or line.startswith("#"):
                continue
            # Support both "KEY=value" and bare "value" formats.
            if "=" in line:
                _, _, value = line.partition("=")
                value = value.strip().strip('"').strip("'")
            else:
                value = line
            if value:
                return value
    return None


KCODE_BINARY = _resolve_existing_file(
    env_name="KCODE_HARBOR_BINARY",
    default_path=DEFAULT_BINARY_PATH,
)
CA_BUNDLE = _resolve_existing_file(
    env_name="KCODE_HARBOR_CA_BUNDLE",
    candidates=CA_BUNDLE_CANDIDATES,
)
OPENSSL_RUNTIME_LIBS = tuple(
    lib
    for lib in (
        _resolve_optional_existing_file(
            candidates=(
                os.environ.get("KCODE_HARBOR_LIBSSL"),
                *_sibling_runtime_lib_candidates(KCODE_BINARY, "libssl"),
                "/usr/lib/libssl.so.3",
                "/usr/lib/x86_64-linux-gnu/libssl.so.3",
                "/lib/x86_64-linux-gnu/libssl.so.3",
            )
        ),
        _resolve_optional_existing_file(
            candidates=(
                os.environ.get("KCODE_HARBOR_LIBCRYPTO"),
                *_sibling_runtime_lib_candidates(KCODE_BINARY, "libcrypto"),
                "/usr/lib/libcrypto.so.3",
                "/usr/lib/x86_64-linux-gnu/libcrypto.so.3",
                "/lib/x86_64-linux-gnu/libcrypto.so.3",
            )
        ),
    )
    if lib is not None
)


def _benchmark_instruction_preamble() -> str:
    return os.environ.get("KCODE_HARBOR_EXTRA_PREAMBLE", "")


def _load_final_payload(output_dir: Path) -> dict[str, Any] | None:
    result_json_path = output_dir / "result.json"
    if result_json_path.exists():
        raw = result_json_path.read_text()
        if raw.strip():
            return json.loads(raw)

    events_path = output_dir / "events.ndjson"
    if not events_path.exists():
        return None

    final_done: dict[str, Any] | None = None
    for line in events_path.read_text().splitlines():
        line = line.strip()
        if not line:
            continue
        try:
            event = json.loads(line)
        except json.JSONDecodeError:
            continue
        if event.get("type") == "done":
            final_done = event

    if final_done is None:
        return None

    payload = {
        "session_id": final_done.get("session_id"),
        "provider": final_done.get("provider"),
        "model": final_done.get("model"),
        "text": final_done.get("text", ""),
        "usage": final_done.get("usage") or {},
    }
    result_json_path.write_text(json.dumps(payload, indent=2) + "\n")
    return payload


class JcodeClaudeHarborAgent(BaseAgent):
    """Harbor adapter that runs jcode with Opus 4.8.

    Default route is the native Anthropic API (provider=anthropic-api,
    model=claude-opus-4-8) using ANTHROPIC_API_KEY. OpenRouter
    (provider=openrouter, model=anthropic/claude-opus-4.8) and native Claude
    OAuth (provider=claude via ~/.kcode/auth.json) are also supported.
    """

    def __init__(self, logs_dir: Path, model_name: str | None = None, *args, **kwargs):
        super().__init__(logs_dir, model_name, *args, **kwargs)
        self._model_arg = model_name or "anthropic-api/claude-opus-4-8"
        if "/" in self._model_arg:
            self._provider_arg, self._jcode_model = self._model_arg.split("/", 1)
        else:
            self._provider_arg, self._jcode_model = "anthropic-api", self._model_arg
        self._openrouter_key = _load_openrouter_key() if self._provider_arg == "openrouter" else None
        self._anthropic_key = _load_anthropic_key() if self._provider_arg == "anthropic-api" else None
        self._claude_auth: Path | None = None
        if self._provider_arg == "claude":
            self._claude_auth = _resolve_existing_file(
                env_name="KCODE_HARBOR_CLAUDE_AUTH",
                default_path=DEFAULT_CLAUDE_AUTH_PATH,
            )

    @staticmethod
    def name() -> str:
        return "jcode-harbor-claude"

    def version(self) -> str | None:
        return "compat-opus-4-8"

    async def setup(self, environment: BaseEnvironment) -> None:
        # NOTE: Do NOT symlink/override the container's system OpenSSL cert dir
        # (e.g. ln -s ... /usr/lib/ssl/certs). The TB base images are often bare
        # ubuntu:24.04 with no /etc/ssl/certs, and the per-task verifier runs
        # `apt-get install ca-certificates curl` then bootstraps uv over https.
        # Hijacking the system cert dir breaks the verifier's curl (error 77,
        # "error setting certificate file") so tests never run. jcode itself
        # gets its CA bundle via SSL_CERT_FILE/OPENSSL_CERT_FILE below, so no
        # global cert override is needed.
        await environment.exec(
            (
                "mkdir -p "
                f"{IN_CONTAINER_HOME} {IN_CONTAINER_RUNTIME} {IN_CONTAINER_INPUT} {IN_CONTAINER_OUTPUT} "
                f"{IN_CONTAINER_LIB_DIR} /usr/local/bin"
            ),
            timeout_sec=30,
        )
        await environment.upload_file(KCODE_BINARY, IN_CONTAINER_BINARY)
        await environment.exec(f"chmod +x {IN_CONTAINER_BINARY}", timeout_sec=30)
        for lib in OPENSSL_RUNTIME_LIBS:
            await environment.upload_file(lib, f"{IN_CONTAINER_LIB_DIR}/{lib.name}")
        if self._claude_auth is not None:
            await environment.upload_file(self._claude_auth, f"{IN_CONTAINER_HOME}/auth.json")
            await environment.exec(f"chmod 600 {IN_CONTAINER_HOME}/auth.json", timeout_sec=30)
        await environment.upload_file(CA_BUNDLE, IN_CONTAINER_CA_BUNDLE)
        version_result = await environment.exec(
            f"{IN_CONTAINER_BINARY} --quiet --no-update --no-selfdev version --json",
            env={
                "HOME": IN_CONTAINER_HOME,
                "KCODE_HOME": IN_CONTAINER_HOME,
                "KCODE_RUNTIME_DIR": IN_CONTAINER_RUNTIME,
                "KCODE_NO_TELEMETRY": "1",
                "LD_LIBRARY_PATH": IN_CONTAINER_LIB_DIR,
            },
            timeout_sec=60,
        )
        (self.logs_dir / "setup_version.json").write_text(version_result.stdout or "")
        (self.logs_dir / "setup_version.stderr.txt").write_text(version_result.stderr or "")
        (self.logs_dir / "setup_version.return_code.txt").write_text(str(version_result.return_code))

    async def run(self, instruction: str, environment: BaseEnvironment, context: AgentContext) -> None:
        self.logs_dir.mkdir(parents=True, exist_ok=True)
        benchmark_instruction = f"{_benchmark_instruction_preamble()}{instruction}"
        local_instruction = self.logs_dir / "instruction.txt"
        local_instruction.write_text(benchmark_instruction)
        await environment.upload_file(local_instruction, f"{IN_CONTAINER_INPUT}/instruction.txt")

        env = {
            "HOME": IN_CONTAINER_HOME,
            "KCODE_HOME": IN_CONTAINER_HOME,
            "KCODE_RUNTIME_DIR": IN_CONTAINER_RUNTIME,
            "KCODE_NO_TELEMETRY": "1",
            "KCODE_PROVIDER": self._provider_arg,
            "KCODE_MODEL": self._jcode_model,
            "KCODE_ANTHROPIC_REASONING_EFFORT": os.environ.get("KCODE_ANTHROPIC_REASONING_EFFORT", "high"),
            "SSL_CERT_FILE": IN_CONTAINER_CA_BUNDLE,
            "OPENSSL_CERT_FILE": IN_CONTAINER_CA_BUNDLE,
            "LD_LIBRARY_PATH": IN_CONTAINER_LIB_DIR,
        }
        if self._openrouter_key:
            env["OPENROUTER_API_KEY"] = self._openrouter_key
        if self._anthropic_key:
            env["ANTHROPIC_API_KEY"] = self._anthropic_key

        result = await environment.exec(
            command=(
                'set -e; '
                'workdir="${KCODE_TASK_WORKDIR:-}"; '
                'if [ -z "$workdir" ]; then '
                '  if [ -d /app ]; then workdir=/app; else workdir="$(pwd)"; fi; '
                'fi; '
                f'instruction="$(cat {IN_CONTAINER_INPUT}/instruction.txt)"; '
                f'{IN_CONTAINER_BINARY} --quiet --no-update --no-selfdev '
                '--provider "$KCODE_PROVIDER" --model "$KCODE_MODEL" '
                '-C "$workdir" run --ndjson "$instruction" '
                f'> {IN_CONTAINER_OUTPUT}/events.ndjson 2> {IN_CONTAINER_OUTPUT}/stderr.txt'
            ),
            env=env
        )

        (self.logs_dir / "exec_stdout.txt").write_text(result.stdout or "")
        (self.logs_dir / "exec_stderr.txt").write_text(result.stderr or "")
        (self.logs_dir / "exec_return_code.txt").write_text(str(result.return_code))

        try:
            await environment.download_dir(IN_CONTAINER_OUTPUT, self.logs_dir / "jcode-output")
        except Exception as e:  # noqa: BLE001
            (self.logs_dir / "download_error.txt").write_text(str(e))

        metadata: dict[str, Any] = {
            "return_code": result.return_code,
            "provider": self._provider_arg,
            "model": self._jcode_model,
            "jcode_binary": str(KCODE_BINARY),
        }

        output_dir = self.logs_dir / "jcode-output"
        payload = _load_final_payload(output_dir)
        if payload is not None:
            usage = payload.get("usage") or {}
            context.n_input_tokens = usage.get("input_tokens")
            context.n_output_tokens = usage.get("output_tokens")
            cache_read = usage.get("cache_read_input_tokens")
            cache_create = usage.get("cache_creation_input_tokens")
            if isinstance(cache_read, int) and isinstance(cache_create, int):
                context.n_cache_tokens = cache_read + cache_create
            elif isinstance(cache_read, int):
                context.n_cache_tokens = cache_read
            metadata["jcode_result"] = payload

        context.metadata = metadata
