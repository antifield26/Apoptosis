"""Capture our own server's chunks in full, with no client and no GUI.

Run from the repository root:

`	ext
python tools/surface-capture/run.py
`

## Why not the live session's trace

The rig's trace stores only the first 64 bytes of each packet, and the light arrays sit at the **end** of
`level_chunk_with_light`, so they are not in it. The full bodies need `--bodies`.

## Why no real client is needed

The owner is in the world right now and should not be interrupted. A `TestClient` logs in just as well for the
purpose: the rig relays it and dumps the bodies, and the chunk data the server sends does not depend on which
client is asking. The `light_update_trigger` test already knows how to log in against a running server, so it is
reused as the driver rather than writing another one.
"""

import os
import shutil
import subprocess
import sys
import time
from pathlib import Path

ROOT = Path(r'C:\Users\25371\projects\MinecraftServer')
SCRATCH = ROOT / 'target' / 'surface-capture'
BODIES = SCRATCH / 'bodies'

SERVER_PORT = 25581
RIG_PORT = 25582


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
    for port in (SERVER_PORT, RIG_PORT):
        if port_open(port):
            print(f'port {port} is in use')
            return 2

    SCRATCH.mkdir(parents=True, exist_ok=True)
    # **The world goes too.** A world that already exists is never regenerated — that is a property the
    # server enforces on purpose — so a capture that reuses one examines terrain from whichever seed created
    # it, which is what made the wire and the engine disagree about the same chunk.
    shutil.rmtree(SCRATCH / "world", ignore_errors=True)
    shutil.rmtree(BODIES, ignore_errors=True)
    # The trace too: its `seq` continues across runs while the body file names restart at zero, so leaving
    # it makes any comparison between the two a comparison between different runs.
    (SCRATCH / "trace.jsonl").unlink(missing_ok=True)
    config = SCRATCH / 'server.toml'
    config.write_text(
        '[network]\n'
        f'bind = "127.0.0.1:{SERVER_PORT}"\n'
        'max_players = 10\n'
        'online_mode = false\n'
        'compression_threshold = 256\n'
        '\n[simulation]\n'
        'view_distance = 4\n'
        '\n[storage]\n'
        f'world_dir = "{(SCRATCH / "world").as_posix()}"\n'
        'autosave_ticks = 6000\n',
        encoding='utf-8', newline='\n',
    )

    processes = []
    try:
        server_handle = (SCRATCH / 'server.log').open('w', encoding='utf-8')
        server = subprocess.Popen([str(ROOT / 'target/debug/mc-server.exe'), str(config)],
                                  cwd=str(ROOT), stdout=server_handle, stderr=subprocess.STDOUT)
        processes.append((server, server_handle))
        if not wait_for_port(SERVER_PORT, 60):
            print('the server did not start')
            return 1
        print(f'server on {SERVER_PORT}')

        rig_handle = (SCRATCH / 'rig.log').open('w', encoding='utf-8')
        rig = subprocess.Popen(
            [str(ROOT / 'target/debug/capture-rig.exe'),
             '--listen', f'127.0.0.1:{RIG_PORT}', '--upstream', f'127.0.0.1:{SERVER_PORT}',
             '--out', str(SCRATCH / 'trace.jsonl'), '--bodies', str(BODIES), '--once'],
            cwd=str(ROOT), stdout=rig_handle, stderr=subprocess.STDOUT,
        )
        processes.append((rig, rig_handle))
        time.sleep(3)
        print(f'rig on {RIG_PORT}')

        environment = dict(**os.environ, MC_TRIGGER_ADDR=f'127.0.0.1:{RIG_PORT}')
        driver = subprocess.run(
            ['cargo', 'test', '-p', 'mc-server', '--test', 'light_update_trigger',
             '--', '--ignored', '--nocapture'],
            cwd=str(ROOT), env=environment, capture_output=True, text=True, timeout=900,
        )
        for line in (driver.stdout or '').splitlines():
            if 'logged in' in line or 'test result' in line or 'panicked' in line:
                print(f'  {line.strip()}')

        # Give the rig a moment to flush every body it has already relayed.
        time.sleep(5)
    finally:
        for process, log_handle in processes:
            if process.poll() is None:
                process.terminate()
                try:
                    process.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    process.kill()
            log_handle.close()

    chunks = sorted(BODIES.glob('*_s2c_play_45.bin')) if BODIES.is_dir() else []
    print(f'\nchunk bodies captured: {len(chunks)}')
    if chunks:
        print(f'  {chunks[0].name}  {chunks[0].stat().st_size} bytes')
        print(f'\nnow run:  cargo test -p mc-server --test sky_light_surface -- --ignored')
    else:
        print('  NOTHING CAPTURED — check the rig log')
    return 0


if __name__ == '__main__':
    sys.exit(main())
