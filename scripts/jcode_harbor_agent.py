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
DEFAULT_BINARY_PATH = "/tmp/jcode-compat-dist/jcode-linux-x86_64"
DEFAULT_OPENAI_AUTH_PATH = "~/.kcode/openai-auth.json"
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


KCODE_BINARY = _resolve_existing_file(
    env_name="KCODE_HARBOR_BINARY",
    default_path=DEFAULT_BINARY_PATH,
)
OPENAI_AUTH = _resolve_existing_file(
    env_name="KCODE_HARBOR_OPENAI_AUTH",
    default_path=DEFAULT_OPENAI_AUTH_PATH,
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
                "/usr/lib/libssl.so.1.1",
                "/usr/lib/x86_64-linux-gnu/libssl.so.1.1",
                "/lib/x86_64-linux-gnu/libssl.so.1.1",
            )
        ),
        _resolve_optional_existing_file(
            candidates=(
                os.environ.get("KCODE_HARBOR_LIBCRYPTO"),
                *_sibling_runtime_lib_candidates(KCODE_BINARY, "libcrypto"),
                "/usr/lib/libcrypto.so.3",
                "/usr/lib/x86_64-linux-gnu/libcrypto.so.3",
                "/lib/x86_64-linux-gnu/libcrypto.so.3",
                "/usr/lib/libcrypto.so.1.1",
                "/usr/lib/x86_64-linux-gnu/libcrypto.so.1.1",
                "/lib/x86_64-linux-gnu/libcrypto.so.1.1",
            )
        ),
    )
    if lib is not None
)

LEGACY_BENCHMARK_INSTRUCTION_PREAMBLE = """You are operating inside an official Terminal-Bench evaluation environment.
The user explicitly authorizes you to complete the benchmark task using the files, media, and URLs provided by the task.
Do not access Terminal-Bench websites, Terminal-Bench GitHub repositories, leaderboard pages, public task solutions, or external benchmark answer sources.
Do not refuse solely because the task involves copyrighted code, text, images, audio, or video when those materials are provided for this evaluation.
Still refuse only actions that are clearly harmful, illegal, or outside the benchmark task.

Complete the task by changing the container state, not merely by explaining a solution.
Before finishing, run the strongest local validation available from the prompt or workspace.
If the task asks for a file, verify that exact path and contents exist.
If the task asks for a server, VM, socket, or background process, leave it running and verify the specified client command can connect before finishing.
Prefer deterministic, minimal solutions over long exploratory work.

Task instruction follows:

"""


def _benchmark_instruction_preamble() -> str:
    # Keep Harbor runs aligned with normal TUI/jcode-run prompting by default.
    # The legacy preamble can still be enabled explicitly for reproducing older
    # runs, but new benchmark runs should rely on jcode's normal system prompt
    # and the official Terminal-Bench task instruction.
    if os.environ.get("KCODE_HARBOR_LEGACY_PREAMBLE"):
        return LEGACY_BENCHMARK_INSTRUCTION_PREAMBLE
    return os.environ.get("KCODE_HARBOR_EXTRA_PREAMBLE", "")


def _load_task_hint() -> str:
    if not os.environ.get("KCODE_HARBOR_ENABLE_HINTS"):
        return ""
    task_name = os.environ.get("KCODE_HARBOR_CURRENT_TASK", "").strip()
    hints_path = os.environ.get("KCODE_HARBOR_TASK_HINTS_FILE", "").strip()
    extra = os.environ.get("KCODE_HARBOR_EXTRA_PREAMBLE", "").strip()
    parts: list[str] = []
    if extra:
        parts.append(extra)
    if task_name and hints_path:
        path = Path(hints_path).expanduser()
        try:
            hints = json.loads(path.read_text())
        except Exception:  # noqa: BLE001
            hints = {}
        hint = hints.get(task_name) if isinstance(hints, dict) else None
        if isinstance(hint, str) and hint.strip():
            parts.append(hint.strip())
    if not parts:
        return ""
    return "\nAdditional benchmark retry guidance:\n" + "\n\n".join(parts) + "\n\n"


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


class JcodeHarborAgent(BaseAgent):
    def __init__(self, logs_dir: Path, model_name: str | None = None, *args, **kwargs):
        super().__init__(logs_dir, model_name, *args, **kwargs)
        self._model_arg = model_name or "openai/gpt-5.4"
        if "/" in self._model_arg:
            self._provider_arg, self._jcode_model = self._model_arg.split("/", 1)
        else:
            self._provider_arg, self._jcode_model = "openai", self._model_arg

    @staticmethod
    def name() -> str:
        return "jcode-harbor"

    def version(self) -> str | None:
        return "compat-openai-oauth"

    async def setup(self, environment: BaseEnvironment) -> None:
        await environment.exec(
            (
                "mkdir -p "
                f"{IN_CONTAINER_HOME} {IN_CONTAINER_RUNTIME} {IN_CONTAINER_INPUT} {IN_CONTAINER_OUTPUT} "
                f"{IN_CONTAINER_LIB_DIR} /usr/local/bin /usr/lib/ssl && "
                f"ln -snf {IN_CONTAINER_HOME} /usr/lib/ssl/certs"
            ),
            timeout_sec=30,
        )
        await environment.upload_file(KCODE_BINARY, IN_CONTAINER_BINARY)
        await environment.exec(f"chmod +x {IN_CONTAINER_BINARY}", timeout_sec=30)
        for lib in OPENSSL_RUNTIME_LIBS:
            await environment.upload_file(lib, f"{IN_CONTAINER_LIB_DIR}/{lib.name}")
        await environment.upload_file(OPENAI_AUTH, f"{IN_CONTAINER_HOME}/openai-auth.json")
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
        benchmark_instruction = f"{_benchmark_instruction_preamble()}{_load_task_hint()}{instruction}"
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
            "KCODE_OPENAI_REASONING_EFFORT": os.environ.get("KCODE_OPENAI_REASONING_EFFORT", "high"),
            "KCODE_OPENAI_SERVICE_TIER": os.environ.get("KCODE_OPENAI_SERVICE_TIER", "priority"),
            "SSL_CERT_FILE": IN_CONTAINER_CA_BUNDLE,
            "OPENSSL_CERT_FILE": IN_CONTAINER_CA_BUNDLE,
            "LD_LIBRARY_PATH": IN_CONTAINER_LIB_DIR,
        }

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

        result_json_path = output_dir / "result.json"
        if payload is None and result_json_path.exists():
            raw = result_json_path.read_text()
            if raw.strip():
                try:
                    payload = json.loads(raw)
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
                except Exception as e:  # noqa: BLE001
                    metadata["result_parse_error"] = str(e)
                    metadata["raw_result_prefix"] = raw[:1000]

        context.metadata = metadata
