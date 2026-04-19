#!/usr/bin/env python3
from __future__ import annotations

import argparse
import concurrent.futures
import datetime as dt
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
from typing import Any


def repo_root() -> Path:
    return Path(__file__).resolve().parent.parent


def run(cmd: list[str], *, env: dict[str, str] | None = None, cwd: Path | None = None) -> subprocess.CompletedProcess[str]:
    return subprocess.run(cmd, check=True, text=True, cwd=cwd, env=env)


def capture(cmd: list[str], *, cwd: Path | None = None) -> str:
    return subprocess.check_output(cmd, text=True, cwd=cwd).strip()


def sha256_file(path: Path) -> str:
    h = hashlib.sha256()
    with path.open("rb") as f:
        for chunk in iter(lambda: f.read(1024 * 1024), b""):
            h.update(chunk)
    return h.hexdigest()


def resolve_existing_file(candidates: list[str | None]) -> Path | None:
    for raw in candidates:
        if not raw:
            continue
        p = Path(raw).expanduser()
        if p.exists() and p.is_file():
            return p.resolve()
    return None


def load_tasks(args: argparse.Namespace) -> list[str]:
    tasks: list[str] = list(args.task)
    if args.tasks_file:
        for line in Path(args.tasks_file).read_text().splitlines():
            line = line.strip()
            if not line or line.startswith("#"):
                continue
            tasks.append(line)
    deduped: list[str] = []
    seen: set[str] = set()
    for task in tasks:
        if task not in seen:
            seen.add(task)
            deduped.append(task)
    if not deduped:
        raise SystemExit("No tasks provided. Use --task and/or --tasks-file.")
    return deduped


def ensure_binary(root: Path, env: dict[str, str]) -> Path:
    binary_dir = Path(env.get("JCODE_HARBOR_BINARY_DIR", "/tmp/jcode-compat-dist")).expanduser()
    binary_path = Path(env.get("JCODE_HARBOR_BINARY", str(binary_dir / "jcode-linux-x86_64"))).expanduser()
    if not (binary_path.exists() and os.access(binary_path, os.X_OK)):
        run([str(root / "scripts" / "build_linux_compat.sh"), str(binary_dir)], env=env, cwd=root)
    return binary_path.resolve()


def current_settings(root: Path, args: argparse.Namespace) -> dict[str, Any]:
    env = os.environ.copy()
    binary_path = ensure_binary(root, env)
    openai_auth = resolve_existing_file([
        env.get("JCODE_HARBOR_OPENAI_AUTH"),
        "~/.jcode/openai-auth.json",
    ])
    if openai_auth is None:
        raise SystemExit("OpenAI OAuth file not found. Set JCODE_HARBOR_OPENAI_AUTH or log in first.")
    settings: dict[str, Any] = {
        "schema_version": 1,
        "created_at": dt.datetime.now(dt.UTC).isoformat(),
        "repo_root": str(root),
        "git_head": capture(["git", "rev-parse", "HEAD"], cwd=root),
        "runner_script": str((root / "scripts" / "run_terminal_bench_harbor.sh").resolve()),
        "model": args.model,
        "reasoning_effort": os.environ.get("JCODE_OPENAI_REASONING_EFFORT", "high"),
        "service_tier": os.environ.get("JCODE_OPENAI_SERVICE_TIER", "priority"),
        "binary_path": str(binary_path),
        "binary_sha256": sha256_file(binary_path),
        "openai_auth_path": str(openai_auth),
        "dataset": args.dataset,
        "path": str(Path(args.path).resolve()) if args.path else None,
        "attempts_per_task": args.n_attempts,
        "n_concurrent": 1,
        "timeout_multiplier": args.timeout_multiplier,
    }
    return settings


PINNED_KEYS = [
    "runner_script",
    "model",
    "reasoning_effort",
    "service_tier",
    "binary_path",
    "binary_sha256",
    "openai_auth_path",
    "dataset",
    "path",
    "attempts_per_task",
    "n_concurrent",
    "timeout_multiplier",
]


def ensure_manifest(campaign_dir: Path, settings: dict[str, Any]) -> dict[str, Any]:
    manifest_path = campaign_dir / "campaign.json"
    if manifest_path.exists():
        manifest = json.loads(manifest_path.read_text())
        mismatches: list[str] = []
        for key in PINNED_KEYS:
            if manifest.get(key) != settings.get(key):
                mismatches.append(f"{key}: existing={manifest.get(key)!r} current={settings.get(key)!r}")
        if mismatches:
            raise SystemExit(
                "Campaign settings drift detected. Refusing to mix incompatible runs in one campaign:\n- "
                + "\n- ".join(mismatches)
            )
        return manifest

    manifest = dict(settings)
    manifest["tasks_run"] = []
    manifest["notes"] = [
        "This campaign is intended to preserve coherent sequential Harbor runs for later leaderboard assembly."
    ]
    manifest_path.write_text(json.dumps(manifest, indent=2) + "\n")
    return manifest


def load_manifest(campaign_dir: Path) -> dict[str, Any]:
    return json.loads((campaign_dir / "campaign.json").read_text())


def write_results_jsonl(campaign_dir: Path, records: list[dict[str, Any]]) -> None:
    results_jsonl = campaign_dir / "results.jsonl"
    with results_jsonl.open("w", encoding="utf-8") as f:
        for record in records:
            f.write(json.dumps(record) + "\n")


def append_result(campaign_dir: Path, record: dict[str, Any]) -> None:
    manifest_path = campaign_dir / "campaign.json"
    manifest = json.loads(manifest_path.read_text())
    existing = manifest.setdefault("tasks_run", [])
    replaced = False
    for idx, item in enumerate(existing):
        if item.get("task_name") == record.get("task_name") and item.get("job_name") == record.get("job_name"):
            if item == record:
                return
            existing[idx] = record
            replaced = True
            break

    if not replaced:
        existing.append(record)

    manifest_path.write_text(json.dumps(manifest, indent=2) + "\n")
    write_results_jsonl(campaign_dir, existing)


def collect_trial_results(job_dir: Path) -> list[dict[str, Any]]:
    trial_results: list[dict[str, Any]] = []
    for result_path in sorted(job_dir.glob("*__*/result.json")):
        payload = json.loads(result_path.read_text())
        verifier_result = payload.get("verifier_result") or {}
        rewards = verifier_result.get("rewards") or {}
        exception_info = payload.get("exception_info") or {}
        agent_result = payload.get("agent_result") or {}
        metadata = agent_result.get("metadata") or {}
        trial_results.append(
            {
                "task_name": payload["task_name"],
                "trial_name": payload["trial_name"],
                "reward": rewards.get("reward"),
                "exception_type": exception_info.get("exception_type"),
                "exception_message": exception_info.get("exception_message"),
                "agent_return_code": metadata.get("return_code"),
                "started_at": payload.get("started_at"),
                "finished_at": payload.get("finished_at"),
                "result_path": str(result_path),
            }
        )
    return trial_results


def summarize_job(job_result_path: Path, trial_results: list[dict[str, Any]]) -> dict[str, Any]:
    payload = json.loads(job_result_path.read_text())
    rewards = [trial.get("reward") for trial in trial_results]
    numeric_rewards = [r for r in rewards if isinstance(r, (int, float))]
    return {
        "job_result_path": str(job_result_path),
        "n_total_trials": payload.get("n_total_trials"),
        "job_started_at": payload.get("started_at"),
        "job_finished_at": payload.get("finished_at"),
        "trial_names": [trial["trial_name"] for trial in trial_results],
        "rewards": rewards,
        "mean_reward": (sum(numeric_rewards) / len(numeric_rewards)) if numeric_rewards else None,
        "trial_results": trial_results,
    }


def completed_recorded_jobs(campaign_dir: Path) -> dict[str, dict[str, Any]]:
    manifest = load_manifest(campaign_dir)
    out: dict[str, dict[str, Any]] = {}
    for item in manifest.get("tasks_run", []):
        mean_reward = item.get("mean_reward")
        if item.get("status") == "completed" and item.get("task_name") and isinstance(mean_reward, (int, float)):
            out[item["task_name"]] = item
    return out


def adopt_existing_job(campaign_dir: Path, task: str, task_jobs_dir: Path) -> dict[str, Any] | None:
    for job_dir in sorted([p for p in task_jobs_dir.iterdir() if p.is_dir()], reverse=True):
        job_result_path = job_dir / "result.json"
        if not job_result_path.exists():
            continue
        trial_results = collect_trial_results(job_dir)
        if not trial_results:
            continue
        numeric_rewards = [t.get("reward") for t in trial_results if isinstance(t.get("reward"), (int, float))]
        if not numeric_rewards:
            continue
        record = {
            "task_name": task,
            "job_name": job_dir.name,
            "jobs_dir": str(task_jobs_dir),
            "status": "completed",
            **summarize_job(job_result_path, trial_results),
        }
        append_result(campaign_dir, record)
        return record
    return None


def build_task_command(
    *,
    runner: Path,
    task: str,
    task_jobs_dir: Path,
    job_name: str,
    args: argparse.Namespace,
    pass_through_args: list[str],
) -> list[str]:
    cmd = [
        str(runner),
        "--include-task-name", task,
        "--n-tasks", "1",
        "--n-concurrent", "1",
        "--jobs-dir", str(task_jobs_dir),
        "--job-name", job_name,
        "--yes",
        "--timeout-multiplier", str(args.timeout_multiplier),
        "-k", str(args.n_attempts),
    ]
    if args.path:
        cmd.extend(["--path", str(Path(args.path).resolve())])
    else:
        cmd.extend(["--dataset", args.dataset])
    if args.model:
        cmd.extend(["--model", args.model])
    cmd.extend(pass_through_args)
    return cmd


def execute_task_process(
    *,
    runner: Path,
    task: str,
    task_jobs_dir: Path,
    job_name: str,
    args: argparse.Namespace,
    pass_through_args: list[str],
) -> tuple[str, str, Path, int]:
    cmd = build_task_command(
        runner=runner,
        task=task,
        task_jobs_dir=task_jobs_dir,
        job_name=job_name,
        args=args,
        pass_through_args=pass_through_args,
    )
    print(f"\n=== Running task {task} as {job_name} ===", flush=True)
    proc = subprocess.run(cmd, text=True)
    return task, job_name, task_jobs_dir, proc.returncode


def finalize_task_result(
    *,
    campaign_dir: Path,
    task: str,
    job_name: str,
    task_jobs_dir: Path,
    process_return_code: int,
    continue_on_failure: bool,
) -> tuple[bool, dict[str, Any]]:
    job_result_path = task_jobs_dir / job_name / "result.json"
    trial_results = collect_trial_results(task_jobs_dir / job_name)
    if job_result_path.exists() and trial_results:
        task_result = {
            "task_name": task,
            "job_name": job_name,
            "jobs_dir": str(task_jobs_dir),
            "status": "completed",
            "process_return_code": process_return_code,
            **summarize_job(job_result_path, trial_results),
        }
        if isinstance(task_result.get("mean_reward"), (int, float)):
            append_result(campaign_dir, task_result)
            print(
                f"Completed {task}: mean_reward={task_result['mean_reward']} trials={len(trial_results)}",
                flush=True,
            )
            return True, task_result

    if process_return_code != 0 or not job_result_path.exists():
        record = {
            "task_name": task,
            "job_name": job_name,
            "status": "failed_to_produce_result",
            "return_code": process_return_code,
            "jobs_dir": str(task_jobs_dir),
        }
        append_result(campaign_dir, record)
        if continue_on_failure:
            print(f"Task {task} failed, continuing because --continue-on-failure is set.", file=sys.stderr)
        return False, record

    if not trial_results:
        record = {
            "task_name": task,
            "job_name": job_name,
            "status": "missing_trial_results",
            "return_code": process_return_code,
            "job_result_path": str(job_result_path),
            "jobs_dir": str(task_jobs_dir),
        }
        append_result(campaign_dir, record)
        if continue_on_failure:
            print(f"Task {task} produced no per-trial results, continuing.", file=sys.stderr)
        return False, record

    raise AssertionError("unreachable")


def prepare_task(campaign_dir: Path, jobs_root: Path, task: str) -> tuple[str, Path] | None:
    recorded = completed_recorded_jobs(campaign_dir)
    if task in recorded:
        print(f"\n=== Skipping task {task}; already recorded as {recorded[task]['job_name']} ===", flush=True)
        return None

    task_jobs_dir = jobs_root / task
    task_jobs_dir.mkdir(parents=True, exist_ok=True)

    adopted = adopt_existing_job(campaign_dir, task, task_jobs_dir)
    if adopted is not None:
        print(
            f"\n=== Adopted existing job for {task}: {adopted['job_name']} mean_reward={adopted['mean_reward']} ===",
            flush=True,
        )
        return None

    return task, task_jobs_dir


def main() -> int:
    parser = argparse.ArgumentParser(description="Run a sequential Terminal-Bench campaign for jcode and preserve stitchable artifacts.")
    parser.add_argument("--campaign-dir", required=True, help="Persistent output directory for the campaign")
    parser.add_argument("--task", action="append", default=[], help="Task name to run. Can be passed multiple times.")
    parser.add_argument("--tasks-file", help="File with one task name per line")
    parser.add_argument("--dataset", default="terminal-bench@2.0", help="Harbor dataset name to use")
    parser.add_argument("--path", help="Local task/dataset path to use instead of --dataset")
    parser.add_argument("--model", default="openai/gpt-5.4", help="Harbor model string")
    parser.add_argument("-k", "--n-attempts", type=int, default=1, help="Attempts per task")
    parser.add_argument("--timeout-multiplier", type=float, default=1.0)
    parser.add_argument("--continue-on-failure", action="store_true", help="Continue to the next task if one task fails")
    parser.add_argument("--max-parallel-tasks", type=int, default=1, help="Maximum number of separate task jobs to run at once")
    parser.add_argument("harbor_args", nargs=argparse.REMAINDER, help="Extra args passed through after '--'")
    args = parser.parse_args()

    root = repo_root()
    campaign_dir = Path(args.campaign_dir).expanduser().resolve()
    campaign_dir.mkdir(parents=True, exist_ok=True)
    jobs_root = campaign_dir / "harbor-jobs"
    jobs_root.mkdir(parents=True, exist_ok=True)

    tasks = load_tasks(args)
    settings = current_settings(root, args)
    ensure_manifest(campaign_dir, settings)

    pass_through_args = list(args.harbor_args)
    if pass_through_args and pass_through_args[0] == "--":
        pass_through_args = pass_through_args[1:]

    runner = root / "scripts" / "run_terminal_bench_harbor.sh"

    pending: list[tuple[str, Path, str]] = []
    for task in tasks:
        prepared = prepare_task(campaign_dir, jobs_root, task)
        if prepared is None:
            continue
        task_name, task_jobs_dir = prepared
        existing_runs = [p for p in task_jobs_dir.iterdir() if p.is_dir()]
        run_index = len(existing_runs) + 1
        job_name = f"run-{run_index:03d}"
        pending.append((task_name, task_jobs_dir, job_name))

    if not pending:
        return 0

    max_workers = max(1, args.max_parallel_tasks)
    if max_workers == 1:
        for task, task_jobs_dir, job_name in pending:
            _, _, _, return_code = execute_task_process(
                runner=runner,
                task=task,
                task_jobs_dir=task_jobs_dir,
                job_name=job_name,
                args=args,
                pass_through_args=pass_through_args,
            )
            ok, _record = finalize_task_result(
                campaign_dir=campaign_dir,
                task=task,
                job_name=job_name,
                task_jobs_dir=task_jobs_dir,
                process_return_code=return_code,
                continue_on_failure=args.continue_on_failure,
            )
            if not ok and not args.continue_on_failure:
                return return_code or 1
        return 0

    had_failure = False
    with concurrent.futures.ThreadPoolExecutor(max_workers=max_workers) as executor:
        future_map = {
            executor.submit(
                execute_task_process,
                runner=runner,
                task=task,
                task_jobs_dir=task_jobs_dir,
                job_name=job_name,
                args=args,
                pass_through_args=pass_through_args,
            ): (task, task_jobs_dir, job_name)
            for task, task_jobs_dir, job_name in pending
        }
        for future in concurrent.futures.as_completed(future_map):
            task, task_jobs_dir, job_name = future_map[future]
            _task, _job_name, _task_jobs_dir, return_code = future.result()
            ok, _record = finalize_task_result(
                campaign_dir=campaign_dir,
                task=task,
                job_name=job_name,
                task_jobs_dir=task_jobs_dir,
                process_return_code=return_code,
                continue_on_failure=args.continue_on_failure,
            )
            if not ok:
                had_failure = True

    return 1 if had_failure and not args.continue_on_failure else 0


if __name__ == "__main__":
    raise SystemExit(main())
