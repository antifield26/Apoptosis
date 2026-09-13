"""Make a real client receive a `light_update`, and check whether it survives it.

Run from the repository root:

```text
python tools/light-update-trigger/run.py
```

## The question

Our `light_update` has been accepted only by our own `TestClient`, and test-client acceptance is exactly the
evidence that failed in KD-44 — two implementations of one format agreeing with each other. What is missing is
a **real client**. It only receives one if the server changes a block while it is connected, and the server
changes blocks only when a player breaks or places one.

## How this closes it without a person

Two clients, two jobs:

```text
real 26.1.2 client  ->  capture-rig  ->  our server
                                          ^
cargo test light_update_trigger ---------+  (a TestClient that breaks one block)
```

The block change is applied for the second client, and `light_update` goes to **every** session holding that
chunk — so the real client receives one without anybody touching a keyboard.

## What counts as the answer

The client writes a **protocol-error report** to `debug/` when it fails to handle a packet, and it does that
before disconnecting. So the check is not "did it stay connected" — a client can be wedged silently — but
**did it write a new report**. The directory is snapshotted first so an old report cannot be mistaken for a new
one; two were already sitting there from the owner's own sessions when this was written.

If no report appears and the client is still running, a real client accepted our `light_update`.
"""

import shutil
import subprocess
import sys
import time
from pathlib import Path

ROOT = Path(r'C:\Users\25371\projects\MinecraftServer')
SCRATCH = ROOT / 'target' / 'trigger-light-update'
CLIENT_DEBUG = Path(r'D:\HMCL\.minecraft\debug')
CLIENT_LOG = Path(r'D:\HMCL\.minecraft\logs\latest.log')

SERVER_PORT = 25577
RIG_PORT = 25566
USERNAME = 'RealClient'


def port_open(port) -> bool:
    import socket

    try:
        with socket.create_connection(('127.0.0.1', port), timeout=1):
            return True
    except OSError:
        return False


def wait_for_port(port, timeout):
    deadline = time.time() + timeout
    while time.time() < deadline:
        if port_open(port):
            return True
        time.sleep(1)
    return False


def main() -> int:
    for exe in (ROOT / 'target/debug/mc-server.exe', ROOT / 'target/debug/capture-rig.exe'):
        if not exe.is_file():
            print(f'missing {exe}; run `cargo build -p mc-server-app -p mc-capture-rig` first')
            return 2
    for port in (SERVER_PORT, RIG_PORT):
        if port_open(port):
            print(f'port {port} is already in use; stop whatever holds it first')
            return 2

    SCRATCH.mkdir(parents=True, exist_ok=True)
    config = SCRATCH / 'server.toml'
    config.write_text(
        '[network]\n'
        f'bind = "127.0.0.1:{SERVER_PORT}"\n'
        'max_players = 10\n'
        'online_mode = false\n'
        'compression_threshold = 256\n'
        'motd = "light_update trigger"\n'
        '\n[simulation]\n'
        'view_distance = 6\n'
        '\n[storage]\n'
        f'world_dir = "{(SCRATCH / "world").as_posix()}"\n'
        'autosave_ticks = 6000\n',
        encoding='utf-8', newline='\n',
    )

    before = {p.name for p in CLIENT_DEBUG.glob('*.txt')} if CLIENT_DEBUG.is_dir() else set()
    print(f'client protocol-error reports before: {len(before)}')

    processes = []
    try:
        server_handle = (SCRATCH / 'server.log').open('w', encoding='utf-8')
        server = subprocess.Popen([str(ROOT / 'target/debug/mc-server.exe'), str(config)],
                                  cwd=str(ROOT), stdout=server_handle, stderr=subprocess.STDOUT)
        processes.append((server, server_handle))
        if not wait_for_port(SERVER_PORT, 60):
            print('the server did not start')
            return 1
        print(f'our server listening on {SERVER_PORT}')

        rig_handle = (SCRATCH / 'rig.log').open('w', encoding='utf-8')
        rig = subprocess.Popen(
            [str(ROOT / 'target/debug/capture-rig.exe'),
             '--listen', f'127.0.0.1:{RIG_PORT}', '--upstream', f'127.0.0.1:{SERVER_PORT}',
             '--out', str(SCRATCH / 'trace.jsonl'), '--once'],
            cwd=str(ROOT), stdout=rig_handle, stderr=subprocess.STDOUT,
        )
        processes.append((rig, rig_handle))
        time.sleep(3)
        print(f'rig listening on {RIG_PORT}')

        import importlib.util
        spec = importlib.util.spec_from_file_location('launcher', ROOT / 'target' / 'p10_launcher.py')
        launcher = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(launcher)
        command, _uuid = launcher.build(f'127.0.0.1:{RIG_PORT}', USERNAME, [])
        client_handle = (SCRATCH / 'client.log').open('w', encoding='utf-8')
        client = subprocess.Popen(command, cwd=r'D:\HMCL\.minecraft',
                                  stdout=client_handle, stderr=subprocess.STDOUT)
        processes.append((client, client_handle))
        print('real client launched')

        trace = SCRATCH / 'trace.jsonl'
        deadline = time.time() + 200
        joined = False
        while time.time() < deadline:
            if client.poll() is not None:
                break
            if trace.is_file() and '"name":"join_game"' in trace.read_text(
                    encoding='utf-8', errors='ignore'):
                joined = True
                break
            time.sleep(2)
        if not joined:
            print('the real client never reached play')
            return 1
        print('the real client is in the world; letting it finish its initial chunks')
        time.sleep(20)

        print('\nbreaking one block through a second connection')
        environment = dict(**__import__('os').environ, MC_TRIGGER_ADDR=f'127.0.0.1:{SERVER_PORT}')
        trigger = subprocess.run(
            ['cargo', 'test', '-p', 'mc-server', '--test', 'light_update_trigger',
             '--', '--ignored', '--nocapture'],
            cwd=str(ROOT), env=environment, capture_output=True, text=True, timeout=600,
        )
        for line in (trigger.stdout or '').splitlines():
            if any(word in line for word in ('logged in', 'breaking', 'done', 'test result', 'panicked')):
                print(f'  {line.strip()}')

        # The client needs a moment to receive and handle the packet, and to write a report if it does not.
        time.sleep(10)
        alive = client.poll() is None
    finally:
        for process, log_handle in processes:
            if process.poll() is None:
                process.terminate()
                try:
                    process.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    process.kill()
            log_handle.close()

    after = {p.name for p in CLIENT_DEBUG.glob('*.txt')} if CLIENT_DEBUG.is_dir() else set()
    new = sorted(after - before)
    print(f'\n=== the answer ===')
    print(f'client still running after the light update: {alive}')
    print(f'new protocol-error reports: {new if new else "none"}')
    for name in new:
        print(f'  --- {name} ---')
        for line in (CLIENT_DEBUG / name).read_text(encoding='utf-8', errors='ignore').splitlines()[:12]:
            print(f'    {line[:150]}')

    if CLIENT_LOG.is_file():
        errors = [line for line in CLIENT_LOG.read_text(encoding='utf-8', errors='ignore').splitlines()
                  if 'ERROR' in line and '401' not in line]
        print(f'client ERROR lines other than the offline-auth 401s: {len(errors)}')
        for line in errors[:5]:
            print(f'  {line.strip()[:150]}')

    print()
    if alive and not new:
        print('VERDICT: a real client accepted our light_update — no report, and it is still running')
    elif new:
        print('VERDICT: the real client REJECTED it — see the report above')
    else:
        print('VERDICT: inconclusive — the client exited without writing a report')
    return 0


if __name__ == '__main__':
    sys.exit(main())
