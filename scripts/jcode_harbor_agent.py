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
IN_CONTAINER_CA_BUNDLE = f"{IN_CONTAINER_HOME}/ca-certificates.crt"
DEFAULT_BINARY_PATH = "/tmp/jcode-compat-dist/jcode-linux-x86_64"
DEFAULT_OPENAI_AUTH_PATH = "~/.jcode/openai-auth.json"
CA_BUNDLE_CANDIDATES = (
    os.environ.get("JCODE_HARBOR_CA_BUNDLE"),
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


JCODE_BINARY = _resolve_existing_file(
    env_name="JCODE_HARBOR_BINARY",
    default_path=DEFAULT_BINARY_PATH,
)
OPENAI_AUTH = _resolve_existing_file(
    env_name="JCODE_HARBOR_OPENAI_AUTH",
    default_path=DEFAULT_OPENAI_AUTH_PATH,
)
CA_BUNDLE = _resolve_existing_file(
    env_name="JCODE_HARBOR_CA_BUNDLE",
    candidates=CA_BUNDLE_CANDIDATES,
)


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
                "/usr/local/bin /usr/lib/ssl && "
                f"ln -snf {IN_CONTAINER_HOME} /usr/lib/ssl/certs"
            ),
            timeout_sec=30,
        )
        await environment.upload_file(JCODE_BINARY, IN_CONTAINER_BINARY)
        await environment.exec(f"chmod +x {IN_CONTAINER_BINARY}", timeout_sec=30)
        await environment.upload_file(OPENAI_AUTH, f"{IN_CONTAINER_HOME}/openai-auth.json")
        await environment.upload_file(CA_BUNDLE, IN_CONTAINER_CA_BUNDLE)
        version_result = await environment.exec(
            f"{IN_CONTAINER_BINARY} --quiet --no-update --no-selfdev version --json",
            env={
                "HOME": IN_CONTAINER_HOME,
                "JCODE_HOME": IN_CONTAINER_HOME,
                "JCODE_RUNTIME_DIR": IN_CONTAINER_RUNTIME,
                "JCODE_NO_TELEMETRY": "1",
            },
            timeout_sec=60,
        )
        (self.logs_dir / "setup_version.json").write_text(version_result.stdout or "")
        (self.logs_dir / "setup_version.stderr.txt").write_text(version_result.stderr or "")
        (self.logs_dir / "setup_version.return_code.txt").write_text(str(version_result.return_code))

    async def run(self, instruction: str, environment: BaseEnvironment, context: AgentContext) -> None:
        self.logs_dir.mkdir(parents=True, exist_ok=True)
        local_instruction = self.logs_dir / "instruction.txt"
        local_instruction.write_text(instruction)
        await environment.upload_file(local_instruction, f"{IN_CONTAINER_INPUT}/instruction.txt")

        env = {
            "HOME": IN_CONTAINER_HOME,
            "JCODE_HOME": IN_CONTAINER_HOME,
            "JCODE_RUNTIME_DIR": IN_CONTAINER_RUNTIME,
            "JCODE_NO_TELEMETRY": "1",
            "JCODE_PROVIDER": self._provider_arg,
            "JCODE_MODEL": self._jcode_model,
            "JCODE_OPENAI_REASONING_EFFORT": os.environ.get("JCODE_OPENAI_REASONING_EFFORT", "high"),
            "JCODE_OPENAI_SERVICE_TIER": os.environ.get("JCODE_OPENAI_SERVICE_TIER", "priority"),
            "SSL_CERT_FILE": IN_CONTAINER_CA_BUNDLE,
            "OPENSSL_CERT_FILE": IN_CONTAINER_CA_BUNDLE,
        }

        result = await environment.exec(
            command=(
                f'instruction="$(cat {IN_CONTAINER_INPUT}/instruction.txt)"; '
                f'{IN_CONTAINER_BINARY} --quiet --no-update --no-selfdev '
                '--provider "$JCODE_PROVIDER" --model "$JCODE_MODEL" '
                f'-C /app run --json "$instruction" '
                f'> {IN_CONTAINER_OUTPUT}/result.json 2> {IN_CONTAINER_OUTPUT}/stderr.txt'
            ),
            cwd="/app",
            env=env,
            timeout_sec=600,
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
            "jcode_binary": str(JCODE_BINARY),
        }

        result_json_path = self.logs_dir / "jcode-output" / "result.json"
        if result_json_path.exists():
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
