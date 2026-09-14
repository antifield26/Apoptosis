"""Settle P10-09: WHEN does a real 26.1.2 server send block_entity_data?

Three sessions placed and filled a chest at the player and captured none, so the
trigger is not placement and not container fill. What has not been tried is the
class of block entity whose *own* data is client-visible and changes on its own:
a sign whose text is edited, a campfire whose cooking items change, a spawner
whose spawn delay ticks. None of these need a player interaction — the console
can place and merge all of them — which keeps the experiment automatable.

Control group (expected silent, matching the three falsified hypotheses):
a chest placed and filled at the player. Experimental group: a sign whose text
is merged, a campfire that is given an item, a spawner that is placed. Both
answer packets are captured — block_entity_data (6) AND block_event (7),
because a chest lid runs through the event channel and confusing the two is
exactly the wrong-shape trap this tool exists to avoid.

It is `be_experiment.py`, not a change to run.py: the chat capture answers its
own question and this one answers a different one, so they stay separate files
that share a shape.
"""

import json
import shutil
import subprocess
import sys
import time
from pathlib import Path

ROOT = Path(r'C:\Users\25371\projects\MinecraftServer')
SCRATCH = ROOT / 'target' / 'be-capture'
BODIES = SCRATCH / 'bodies'
RIG_EXE = ROOT / 'target' / 'debug' / 'capture-rig.exe'
JAVA = Path(r'C:\Program Files\Microsoft\jdk-25.0.4.7-hotspot\bin\java.exe')
JAR = ROOT / 'target' / 'vanilla-26.1.2' / 'server.jar'

BLOCK_ENTITY_DATA = 6
BLOCK_EVENT = 7

SERVER_PORT = 25583
RIG_PORT = 25584

INJECTIONS = (
    ('control: chest placed at the player', 'execute at @a run setblock ~ ~2 ~ minecraft:chest'),
    ('control: chest filled', 'execute at @a run item replace block ~ ~2 ~ container.0 with minecraft:diamond 5'),
    ('sign placed', 'execute at @a run setblock ~ ~2 ~ minecraft:oak_sign'),
    ('sign text merged', "execute at @a run data merge block ~ ~2 ~ {front_text:{messages:['\"hello from the review\"','\"\"','\"\"','\"\"']}}"),
    ('campfire placed', 'execute at @a run setblock ~ ~2 ~ minecraft:campfire'),
    ('campfire given an item', 'execute at @a run item replace block ~ ~2 ~ container.0 with minecraft:porkchop 2'),
    ('spawner placed with a pig', 'execute at @a run setblock ~ ~2 ~ minecraft:spawner{SpawnData:{entity:{id:"minecraft:pig"}}}'),
)


def port_open(port) -> bool:
    import socket

    try:
        with socket.create_connection(('127.0.0.1', port), timeout=1):
            return True
    except OSError:
        return False


def wait_for(port, timeout):
    deadline = time.time() + timeout
    while time.time() < deadline:
        if port_open(port):
            return True
        time.sleep(2)
    return False


def main() -> int:
    if port_open(SERVER_PORT):
        print(f'port {SERVER_PORT} is in use')
        return 2
    SCRATCH.mkdir(parents=True, exist_ok=True)
    shutil.rmtree(BODIES, ignore_errors=True)
    (SCRATCH / 'trace.jsonl').unlink(missing_ok=True)
    # **A fresh world every run**: a spawner or sign left at spawn by an earlier
    # session would tick from the first second and contaminate the control group.
    shutil.rmtree(SCRATCH / 'world', ignore_errors=True)

    (SCRATCH / 'eula.txt').write_text('eula=true\n', encoding='utf-8', newline='\n')
    (SCRATCH / 'server.properties').write_text(
        f'server-port={SERVER_PORT}\nonline-mode=false\nmotd=be capture\nlevel-seed=1361882806\n',
        encoding='utf-8', newline='\n',
    )
    handle = (SCRATCH / 'server.log').open('w', encoding='utf-8')
    server = subprocess.Popen([str(JAVA), '-Xmx2G', '-jar', str(JAR), 'nogui'],
                              cwd=str(SCRATCH), stdout=handle, stderr=subprocess.STDOUT,
                              stdin=subprocess.PIPE, text=True)
    processes = [(server, handle)]
    try:
        print('starting the vanilla server...')
        if not wait_for(SERVER_PORT, 300):
            print('  it never accepted a connection')
            return 1
        time.sleep(5)

        rig_handle = (SCRATCH / 'rig.log').open('w', encoding='utf-8')
        rig = subprocess.Popen(
            [str(RIG_EXE), '--listen', f'127.0.0.1:{RIG_PORT}', '--upstream', f'127.0.0.1:{SERVER_PORT}',
             '--out', str(SCRATCH / 'trace.jsonl'), '--bodies', str(BODIES), '--once'],
            cwd=str(ROOT), stdout=rig_handle, stderr=subprocess.STDOUT,
        )
        processes.append((rig, rig_handle))
        time.sleep(4)

        import importlib.util
        spec = importlib.util.spec_from_file_location('launcher', ROOT / 'target' / 'p10_launcher.py')
        launcher = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(launcher)
        command, _uuid = launcher.build(f'127.0.0.1:{RIG_PORT}', 'RealClient', [])
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
            print('the client never reached play')
            return 1
        print('the client is in the world; letting it take its chunks')
        time.sleep(20)

        for label, command in INJECTIONS:
            print(f'  injection: {label}')
            server.stdin.write(command + chr(10))
            server.stdin.flush()
            time.sleep(4)
        time.sleep(8)
    finally:
        for process, log_handle in processes:
            if process.poll() is None:
                process.terminate()
                try:
                    process.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    process.kill()
            log_handle.close()

    events = [json.loads(line) for line
              in (SCRATCH / 'trace.jsonl').read_text(encoding='utf-8', errors='ignore').splitlines()
              if line.strip()]
    s2c = [e for e in events if e.get('kind') == 'packet' and e['state'] == 'play' and e['dir'] == 's2c']
    for wanted, label in ((BLOCK_ENTITY_DATA, 'block_entity_data'), (BLOCK_EVENT, 'block_event')):
        hits = [e for e in s2c if e['id'] == wanted]
        print(f'\n{label} ({wanted}): {len(hits)} packets')
        for e in hits[:8]:
            print(f'  seq={e["seq"]} body={e["body_bytes"]} head={e["head"][:96]}')
        files = sorted(BODIES.glob(f'*_s2c_play_{wanted}.bin')) if BODIES.is_dir() else []
        print(f'  bodies saved: {len(files)}')
        for path in files[:8]:
            print(f'  {path.name}  {path.stat().st_size} bytes  {path.read_bytes()[:48].hex(" ")}')
    print(f'\nplay packets total: {len(s2c)}')
    return 0


if __name__ == '__main__':
    sys.exit(main())
