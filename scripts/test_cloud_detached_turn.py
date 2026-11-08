#!/usr/bin/env python3
"""Run actual detached cloud turns against an isolated loopback fake provider."""
import argparse
import json
import os
import signal
from pathlib import Path
import subprocess
import sys
import tempfile
import time


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', required=True, type=Path)
    args = parser.parse_args()
    binary = args.binary.resolve()
    scratch = Path(os.environ['JCODE_SCRATCH_DIR'])
    with tempfile.TemporaryDirectory(prefix='cloud-detached-', dir=scratch) as tmp:
        root = Path(tmp)
        home = root / 'home'
        jcode = home / '.jcode'
        runtime = root / 'runtime'
        gates = root / 'gates'
        for directory in (jcode, runtime, gates):
            directory.mkdir(parents=True, exist_ok=True)
            directory.chmod(0o700)
        env = {
            'PATH': '/usr/local/bin:/usr/bin:/bin', 'LANG': 'C.UTF-8',
            'HOME': str(home), 'JCODE_HOME': str(jcode),
            'XDG_RUNTIME_DIR': str(runtime), 'JCODE_RUNTIME_DIR': str(runtime),
            'TMPDIR': str(root), 'JCODE_NO_TELEMETRY': '1',
            'JCODE_TEST_WORKSPACE': str(root),
            'JCODE_NO_AUTO_UPDATE': '1', 'JCODE_NON_INTERACTIVE': '1',
            'FAKE_LOG': str(root / 'requests.jsonl'), 'FAKE_LABEL': 'detached',
            'FAKE_PORT_FILE': str(root / 'port'), 'FAKE_CONTROL_DIR': str(gates),
        }
        fake = Path(__file__).with_name('fake_openai_server.py').resolve()
        with (root / 'provider.log').open('w') as output:
            provider = subprocess.Popen([sys.executable, str(fake), '0'], env=env, stdout=output, stderr=output)
            supervisor_pid = None
            try:
                deadline = time.monotonic() + 10
                while not (root / 'port').exists():
                    if provider.poll() is not None or time.monotonic() >= deadline:
                        raise AssertionError('fake provider did not start')
                    time.sleep(0.05)
                port = (root / 'port').read_text().strip()
                env.update({
                    'JCODE_OPENAI_COMPAT_API_BASE': f'http://127.0.0.1:{port}/v1',
                    'OPENAI_COMPAT_API_KEY': 'isolated-fixture',
                    'JCODE_OPENAI_COMPAT_DEFAULT_MODEL': 'fake-model',
                })
                (jcode / 'config.toml').write_text('[provider]\ndefault_provider = "openai-compatible"\n[server]\nmobile_push = false\n')

                def run(*arguments, expected=0):
                    result = subprocess.run([str(binary), '--no-update', '--no-selfdev', *arguments], env=env, cwd=root, capture_output=True, text=True, timeout=60)
                    if result.returncode != expected:
                        raise AssertionError(f'{arguments}: exit {result.returncode}\n{result.stdout}\n{result.stderr}')
                    return result

                seed = json.loads(run('--provider', 'openai-compatible', '--model', 'fake-model', 'run', '--json', 'seed detached fixture').stdout)
                session = seed['session_id']
                receipt = json.loads(run('cloud', 'run-detached', '--session', session, '--', '[e2e:hold:detached] continue this task').stdout)
                supervisor_pid = receipt.get('pid')
                assert receipt['session_id'] == session and receipt['state'] == 'running'
                deadline = time.monotonic() + 30
                while not (gates / 'detached.held').exists():
                    report = json.loads(run('cloud', 'inspect', '--session', session).stdout)
                    if report['state'] != 'running' or time.monotonic() >= deadline:
                        raise AssertionError(f'detached turn did not reach provider gate: {report}')
                    time.sleep(0.1)
                report = json.loads(run('cloud', 'inspect', '--session', session).stdout)
                assert report['state'] == 'running'
                duplicate = run('cloud', 'run-detached', '--session', session, '--', 'must not run concurrently', expected=1)
                assert 'migration' in duplicate.stderr or 'active' in duplicate.stderr or 'owns' in duplicate.stderr
                (gates / 'detached.release').write_text('release\n')
                deadline = time.monotonic() + 30
                while True:
                    report = json.loads(run('cloud', 'inspect', '--session', session, '--messages', '4', '--log-lines', '20').stdout)
                    if report['state'] != 'running':
                        break
                    if time.monotonic() >= deadline:
                        raise AssertionError('detached supervisor did not finish')
                    time.sleep(0.1)
                assert report['state'] == 'idle' and report['exit_code'] == 0, report
                assert '[detached] ack' in report['log'], report
                assert 'must not run concurrently' not in (root / 'requests.jsonl').read_text()
                if os.name == 'posix':
                    log = jcode / 'cloud_runs' / session / 'run.log'
                    assert log.stat().st_mode & 0o077 == 0
                print('PASS: real detached turn, busy rejection, supervisor completion, private logs, session inspection')
            finally:
                (gates / 'detached.release').write_text('release\n')
                if supervisor_pid is not None:
                    try:
                        marker = ('JCODE_TEST_WORKSPACE=' + str(root)).encode()
                        owned = marker in Path(f'/proc/{supervisor_pid}/environ').read_bytes().split(b'\0')
                        if owned:
                            os.killpg(supervisor_pid, signal.SIGTERM)
                    except (FileNotFoundError, ProcessLookupError):
                        pass
                provider.terminate()
                try:
                    provider.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    provider.kill()
                    provider.wait()


if __name__ == '__main__':
    main()
