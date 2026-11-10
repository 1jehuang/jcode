#!/usr/bin/env python3
"""Run with python3 test_jcode_socket_watcher.py. Uses isolated fake sockets, no cloud calls."""
import glob
import importlib.machinery
import importlib.util
import json
import os
from pathlib import Path
import queue
import socket
import struct
import subprocess
import sys
import tempfile
import threading
import time
import unittest
from unittest import mock

MODULE = Path(__file__).with_name('jcode-sessions-status')


def load_module():
    loader = importlib.machinery.SourceFileLoader('sessions_status', str(MODULE))
    spec = importlib.util.spec_from_loader(loader.name, loader)
    module = importlib.util.module_from_spec(spec)
    loader.exec_module(module)
    return module


m = load_module()


class FakeServer:
    def __init__(self, directory, reject=False, ack_delay=0):
        self.path = str(Path(directory) / 'jcode-test-debug.sock')
        self.listener = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        self.listener.bind(self.path)
        self.listener.listen()
        self.listener.settimeout(.1)
        self.stopped = threading.Event()
        self.subscribed = threading.Event()
        self.clients = []
        self.streams = []
        self.threads = []
        self.attempts = []
        self.commands = []
        self.premature_queries = []
        self.reject = reject
        self.ack_delay = ack_delay
        self.processing = False
        self.change_during_query = False
        self.thread = threading.Thread(target=self.accept, daemon=True)
        self.thread.start()

    def accept(self):
        while not self.stopped.is_set():
            try:
                conn, _ = self.listener.accept()
            except socket.timeout:
                continue
            except OSError:
                return
            self.clients.append(conn)
            thread = threading.Thread(target=self.handle, args=(conn,), daemon=True)
            self.threads.append(thread)
            thread.start()

    def handle(self, conn):
        try:
            with conn.makefile('rb') as reader:
                request = json.loads(reader.readline())
                command = request['command']
                self.commands.append(command)
                if command.startswith('events:subscribe:'):
                    self.attempts.append(time.monotonic())
                    if self.reject:
                        return
                    time.sleep(self.ack_delay)
                    self.streams.append(conn)
                    self.subscribed.set()
                    conn.sendall(b'{"type":"debug_response","ok":true,"output":"{}"}\n')
                    while not self.stopped.is_set():
                        if not conn.recv(1):
                            break
                else:
                    if not self.subscribed.is_set():
                        self.premature_queries.append(command)
                    output = ([{'session_id': 'fake-root', 'is_processing': self.processing}]
                              if command == 'sessions' else
                              {'id': 'fake-server', 'spawned_swarm_agent_count': 2})
                    if command == 'sessions' and self.change_during_query:
                        self.change_during_query = False
                        self.processing = True
                        self.event()
                    conn.sendall((json.dumps({'ok': True, 'output': json.dumps(output)}) + '\n').encode())
        except (OSError, ValueError):
            pass
        finally:
            conn.close()

    def event(self, kind='event'):
        for conn in list(self.streams):
            try:
                conn.sendall((json.dumps({'type': kind}) + '\n').encode())
            except OSError:
                pass

    def close(self):
        self.stopped.set()
        self.listener.close()
        for conn in self.clients:
            try:
                conn.shutdown(socket.SHUT_RDWR)
            except OSError:
                pass
        self.thread.join(1)
        for thread in self.threads:
            thread.join(1)
        try:
            os.unlink(self.path)
        except FileNotFoundError:
            pass


class ModuleProcess:
    def __init__(self, directory, fallback=False):
        self.outputs = queue.Queue()
        self.process = subprocess.Popen(
            [sys.executable, __file__, '--runner', directory, str(int(fallback))],
            stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        self.reader = threading.Thread(target=self.read, daemon=True)
        self.reader.start()

    def read(self):
        for line in self.process.stdout:
            self.outputs.put((time.monotonic(), json.loads(line)))

    def output(self, fragment, timeout=2):
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            try:
                timestamp, output = self.outputs.get(timeout=max(.01, deadline - time.monotonic()))
            except queue.Empty:
                break
            if fragment in output['tooltip']:
                return timestamp, output
        raise AssertionError(f'no output containing {fragment!r}, process={self.process.poll()}')

    def close(self):
        self.process.terminate()
        self.process.wait(timeout=3)
        self.reader.join(1)
        stderr = self.process.stderr.read()
        self.process.stdout.close()
        self.process.stderr.close()
        if stderr:
            raise AssertionError(stderr)


class WatcherTests(unittest.TestCase):
    def test_event_masks_and_unrelated_names(self):
        def event(mask, name=b''):
            return struct.pack('iIII', 1, mask, 0, len(name)) + name
        for mask in (0x100, 0x200, 0x40, 0x80):
            self.assertTrue(m.SocketDirectoryWatcher.relevant(event(mask, b'jcode-x-debug.sock\0')))
            self.assertFalse(m.SocketDirectoryWatcher.relevant(event(mask, b'unrelated\0')))
        for mask in (0x4000, 0x8000, 0x400, 0x800, 0x2000):
            self.assertTrue(m.SocketDirectoryWatcher.relevant(event(mask)))
        self.assertTrue(m.SocketDirectoryWatcher.relevant(event(0x100, b'other') + event(0x4000)))

    def test_create_rename_delete_and_stop(self):
        with tempfile.TemporaryDirectory() as directory:
            updates = queue.Queue()
            watcher = m.SocketDirectoryWatcher(lambda: updates.put(True), [directory])
            watcher.start()
            try:
                path = Path(directory) / 'jcode-one-debug.sock'
                path.touch()
                updates.get(timeout=1)
                path.rename(Path(directory) / 'unrelated')
                updates.get(timeout=1)
                (Path(directory) / 'unrelated').rename(path)
                updates.get(timeout=1)
                path.unlink()
                updates.get(timeout=1)
            finally:
                watcher.stop()
                watcher.join(1)
            self.assertFalse(watcher.is_alive())
            self.assertFalse(watcher.failed)

    def test_watch_invalidated_by_directory_move_and_delete(self):
        for action in ('move', 'delete'):
            with self.subTest(action=action), tempfile.TemporaryDirectory() as root:
                path = Path(root) / 'watched'
                path.mkdir()
                updated = threading.Event()
                watcher = m.SocketDirectoryWatcher(updated.set, [str(path)])
                watcher.start()
                if action == 'move':
                    path.rename(Path(root) / 'moved')
                else:
                    path.rmdir()
                watcher.join(1)
                self.assertFalse(watcher.is_alive())
                self.assertTrue(watcher.failed)
                self.assertTrue(updated.is_set())

    def test_overflow_wakes_rescan_without_losing_watch(self):
        with tempfile.TemporaryDirectory() as directory:
            updated = threading.Event()
            watcher = m.SocketDirectoryWatcher(updated.set, [directory])
            overflow = struct.pack('iIII', -1, 0x4000, 0, 0)
            # Inject a kernel overflow record into the actual run-loop parser.
            def notify():
                updated.set()
                watcher.stop()
            watcher.on_update = notify
            with mock.patch.object(m.select, 'select', return_value=([watcher.fd], [], [])), \
                 mock.patch.object(m.os, 'read', side_effect=[overflow, BlockingIOError()]):
                watcher.run()
            self.assertTrue(updated.is_set())
            self.assertFalse(watcher.failed)

    def test_subscriber_stop_before_and_during_receive(self):
        sub = m.SocketSubscriber('/not/a/socket', lambda *_: None)
        sub.stop()
        sub.start()
        sub.join(1)
        self.assertFalse(sub.is_alive())
        with tempfile.TemporaryDirectory() as directory:
            server = FakeServer(directory)
            try:
                sub = m.SocketSubscriber(server.path, lambda *_: None)
                sub.start()
                self.assertTrue(sub.ready.wait(1))
                sub.stop()
                sub.join(1)
                self.assertFalse(sub.is_alive())
                self.assertIsNone(sub.connection)
            finally:
                server.close()


class IntegrationTests(unittest.TestCase):
    def test_event_during_snapshot_is_not_lost(self):
        with tempfile.TemporaryDirectory() as directory:
            server = FakeServer(directory)
            process = ModuleProcess(directory)
            try:
                process.output('0 streaming, 1 root sessions')
                time.sleep(.15)  # Let the initial subscription wake settle.
                server.change_during_query = True
                server.event()
                process.output('1 streaming, 1 root sessions', timeout=1)
            finally:
                process.close()
                server.close()

    def test_healthy_watcher_has_no_periodic_local_scan(self):
        with tempfile.TemporaryDirectory() as directory:
            process = ModuleProcess(directory)
            try:
                process.output('0 root sessions')
                log = Path(directory) / 'discovery.log'
                before = log.read_text()
                time.sleep(.65)  # More than three shortened fallback intervals.
                self.assertEqual(before, log.read_text())
            finally:
                process.close()

    def test_socket_replaced_at_same_path_replaces_subscription(self):
        with tempfile.TemporaryDirectory() as directory:
            old = FakeServer(directory)
            process = ModuleProcess(directory)
            new = None
            try:
                process.output('1 root sessions')
                os.unlink(old.path)
                new = FakeServer(directory)
                new.processing = True
                process.output('1 streaming, 1 root sessions')
                self.assertTrue(new.subscribed.is_set())
                new.processing = False
                new.event()
                process.output('0 streaming, 1 root sessions')
            finally:
                process.close()
                if new:
                    new.close()
                old.close()

    def test_create_delete_immediate_and_subscribe_before_query(self):
        with tempfile.TemporaryDirectory() as directory:
            process = ModuleProcess(directory)
            server = None
            try:
                process.output('0 root sessions')
                start = time.monotonic()
                server = FakeServer(directory, ack_delay=.15)
                created, _ = process.output('1 root sessions')
                create_latency = created - start
                self.assertLess(created - start, 1)
                self.assertFalse(server.premature_queries)
                self.assertTrue(server.commands[0].startswith('events:subscribe:'))
                server.processing = True
                start = time.monotonic()
                server.event('lag')
                changed, _ = process.output('1 streaming, 1 root sessions')
                self.assertLess(changed - start, 1)
                start = time.monotonic()
                server.close()
                deleted, _ = process.output('0 root sessions')
                self.assertLess(deleted - start, 1)
                print(f'acceptance: create={create_latency:.3f}s (150ms ack delay), delete={deleted-start:.3f}s', file=sys.stderr)
            finally:
                if server:
                    server.close()
                process.close()

    def test_retry_backoff_despite_event_storm(self):
        with tempfile.TemporaryDirectory() as directory:
            server = FakeServer(directory, reject=True)
            process = ModuleProcess(directory)
            try:
                process.output('0 root sessions')
                end = time.monotonic() + 3.5
                path = Path(directory) / 'jcode-noise-debug.sock'
                while time.monotonic() < end:
                    path.touch()
                    path.unlink()
                    time.sleep(.02)
                self.assertEqual(len(server.attempts), 3, server.attempts)
                gaps = [b-a for a, b in zip(server.attempts, server.attempts[1:])]
                self.assertGreaterEqual(gaps[0], .95)
                self.assertGreaterEqual(gaps[1], 1.95)
            finally:
                process.close()
                server.close()

    def test_fixed_debounce_under_continuous_events(self):
        with tempfile.TemporaryDirectory() as directory:
            server = FakeServer(directory)
            process = ModuleProcess(directory)
            try:
                process.output('1 root sessions')
                server.processing = True
                start = time.monotonic()
                stop = threading.Event()
                def flood():
                    while not stop.wait(.005):
                        server.event()
                thread = threading.Thread(target=flood)
                thread.start()
                try:
                    changed, _ = process.output('1 streaming, 1 root sessions', timeout=1)
                    self.assertLess(changed-start, .5)
                finally:
                    stop.set()
                    thread.join()
            finally:
                process.close()
                server.close()

    def test_fallback_discovery_when_inotify_unavailable(self):
        with tempfile.TemporaryDirectory() as directory:
            process = ModuleProcess(directory, fallback=True)
            server = None
            try:
                process.output('0 root sessions')
                server = FakeServer(directory)
                process.output('1 root sessions', timeout=1)
            finally:
                if server:
                    server.close()
                process.close()

    def test_directory_invalidation_fallback_and_recovery(self):
        with tempfile.TemporaryDirectory() as root:
            directory = Path(root) / 'runtime'
            directory.mkdir()
            process = ModuleProcess(str(directory))
            server = None
            try:
                process.output('0 root sessions')
                (directory / 'discovery.log').unlink()
                directory.rmdir()
                time.sleep(.1)
                directory.mkdir()
                server = FakeServer(str(directory))
                process.output('1 root sessions', timeout=1)
                server.close()
                process.output('0 root sessions', timeout=1)
            finally:
                if server:
                    server.close()
                process.close()


if __name__ == '__main__':
    if '--runner' in sys.argv:
        directory = sys.argv[2]
        fallback = sys.argv[3] == '1'
        original = m.SocketDirectoryWatcher
        def watcher(callback):
            if fallback:
                raise OSError('inotify unavailable')
            return original(callback, [directory])
        m.SocketDirectoryWatcher = watcher
        def discover():
            # Parent may temporarily remove the directory in recovery tests.
            try:
                with (Path(directory) / 'discovery.log').open('a') as log:
                    log.write('scan\n')
            except FileNotFoundError:
                pass
            return glob.glob(str(Path(directory) / 'jcode*-debug.sock'))
        m._find_debug_sockets = discover
        m.CloudPoller.start = lambda self: None
        m.REFRESH_INTERVAL = .2
        m.main()
    else:
        unittest.main(verbosity=2)
