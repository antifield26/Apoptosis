"""Start the server and the client and **leave them running**, so a person can look at the world.

Run from the repository root:

```text
python tools/visual-check/run.py
```

## Why this exists

Everything about the light is verified except the one thing that cannot be: whether the world *looks* right. A
client does not validate light levels — it renders whatever it is told — so a wrong light level produces no
error, no warning and no log line. That is not a gap that more tests close; it needs eyes.

Every other script here runs the client, collects evidence and shuts everything down. This one does the
opposite: it starts the server, starts the rig, launches the client at it, and then **stops and waits**, so the
window stays open.

## What to look for

* **Is the sky bright and the ground lit?** Before P10-05 every chunk shipped four empty masks, which the
  client renders as an unlit world. If it still looks black, something regressed.
* **Are there shadows under overhangs and in holes?** Light that is uniformly 15 everywhere would look flat
  and washed out even though it is "lit".
* **Does it change when you place a torch?** It will not, until a chunk is re-sent: light is recomputed when a
  chunk is sent, and a placed torch does not trigger a re-send. That is a **known and recorded** limitation
  (KD-45), not a surprise.

Press Ctrl+C here to stop the server and rig; the client window closes on its own or can be closed by hand.
"""

import subprocess
import sys
import time
from pathlib import Path

ROOT = Path(r'C:\Users\25371\projects\MinecraftServer')
SCRATCH = ROOT / 'target' / 'visual-check'
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
        time.sleep(2)
    return False


def client_evidence() -> list:
    """The lines in the client's log that say it actually rendered something.

    `Chunk Sections UBO` growth is the client uploading chunk geometry to the GPU, which it only does for
    chunks it is drawing; the chat line shows our message reached its screen. Neither proves the *lighting* is
    right — nothing but looking does — but they do prove the world is being drawn rather than sat behind a
    loading screen.
    """
    if not CLIENT_LOG.is_file():
        return []
    text = CLIENT_LOG.read_text(encoding='utf-8', errors='ignore')
    interesting = []
    if 'Chunk Sections UBO' in text:
        sizes = [line for line in text.splitlines() if 'Chunk Sections UBO' in line]
        interesting.append(f"chunk sections uploaded to the GPU: {len(sizes)} resize events")
    if 'Welcome to the Rust Minecraft server' in text:
        interesting.append('our chat message reached the client screen')
    errors = [line for line in text.splitlines() if 'ERROR' in line]
    interesting.append(f"client ERROR lines: {len(errors)}")
    return interesting


def main() -> int:
    for exe in (ROOT / 'target/debug/mc-server.exe', ROOT / 'target/debug/capture-rig.exe'):
        if not exe.is_file():
            print(f'missing {exe}; run `cargo build -p mc-server-app -p mc-capture-rig` first')
            return 2
    if port_open(SERVER_PORT) or port_open(RIG_PORT):
        print(f'port {SERVER_PORT} or {RIG_PORT} is already in use; stop whatever holds it first')
        return 2

    SCRATCH.mkdir(parents=True, exist_ok=True)
    config = SCRATCH / 'server.toml'
    config.write_text(
        '[network]\n'
        f'bind = "127.0.0.1:{SERVER_PORT}"\n'
        'max_players = 10\n'
        'online_mode = false\n'
        'compression_threshold = 256\n'
        'motd = "Visual check"\n'
        '\n[simulation]\n'
        'view_distance = 8\n'
        '\n[storage]\n'
        f'world_dir = "{(SCRATCH / "world").as_posix()}"\n'
        'autosave_ticks = 6000\n',
        encoding='utf-8', newline='\n',
    )

    handle = (SCRATCH / 'server.log').open('w', encoding='utf-8')
    server = subprocess.Popen([str(ROOT / 'target/debug/mc-server.exe'), str(config)],
                              cwd=str(ROOT), stdout=handle, stderr=subprocess.STDOUT)
    processes = [(server, handle)]
    try:
        if not wait_for_port(SERVER_PORT, 60):
            print('the server did not start')
            return 1
        print(f'server listening on {SERVER_PORT}')

        rig_handle = (SCRATCH / 'rig.log').open('w', encoding='utf-8')
        rig = subprocess.Popen(
            [str(ROOT / 'target/debug/capture-rig.exe'),
             '--listen', f'127.0.0.1:{RIG_PORT}', '--upstream', f'127.0.0.1:{SERVER_PORT}',
             '--out', str(SCRATCH / 'trace.jsonl')],
            cwd=str(ROOT), stdout=rig_handle, stderr=subprocess.STDOUT,
        )
        processes.append((rig, rig_handle))
        if not wait_for_port(RIG_PORT, 30):
            print('the rig did not start')
            return 1
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
        print('client launched at the rig\n')

        print('--- look at the game window ---')
        print('the world should be lit: bright sky, shaded ground, shadows under overhangs')
        print('a torch placed by hand will NOT relight the chunk until it is re-sent (KD-45)\n')

        # Wait for the client to be in the world, then report what its own log says about rendering.
        for _ in range(60):
            if client.poll() is not None:
                print('the client exited early')
                return 1
            if client_evidence():
                break
            time.sleep(2)
        print('--- what the client log says ---')
        for line in client_evidence():
            print(f'  {line}')
        print(f'\nfull client log: {CLIENT_LOG}')
        print(f'server log     : {SCRATCH / "server.log"}')
        print(f'trace          : {SCRATCH / "trace.jsonl"}')

        print('\nleaving everything running. Press Ctrl+C here to stop the server and rig.')
        while True:
            time.sleep(5)
            if client.poll() is not None:
                print('the client window closed; stopping the server and rig')
                return 0
    except KeyboardInterrupt:
        print('\nstopping')
        return 0
    finally:
        for process, log_handle in processes:
            if process.poll() is None:
                process.terminate()
                try:
                    process.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    process.kill()
            log_handle.close()


if __name__ == '__main__':
    sys.exit(main())
