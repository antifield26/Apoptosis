"""Settle P10-07: what metadata (set_entity_data) does a real 26.1.2 server send
for the mob kinds Phase 11 will spawn?

The natural-spawn capture (bodies-lit) answered for three kinds by accident:
slime (index 9 float health, 15 byte, 16 varint size), sheep (9 float only --
defaults are omitted), cow (9 float + 19 serializer 24, a variant serializer
whose width the walk does not know). Phase 11 spawns eight kinds; their tables
must come from the server's own sends, because the indices are per-class code,
not a registry, and a jar grep cannot say which entries a spawn actually emits.

One summon per kind, injected at the player through the console, killed before
the next so every body is attributable to the kind summoned just before it.
add_entity (1) is captured alongside set_entity_data (99): the pair maps
entity id -> type id -> name, and a body with no add_entity behind it is a
stray from an earlier session, not evidence.

It is `entity_experiment.py`, not a change to be_experiment.py: the block
entity trigger question and the metadata table question have different answers,
so they stay separate files that share a shape.
"""

import json
import shutil
import subprocess
import sys
import time
from pathlib import Path

ROOT = Path(r'C:\Users\25371\projects\MinecraftServer')
SCRATCH = ROOT / 'target' / 'entity-capture'
BODIES = SCRATCH / 'bodies'
RIG_EXE = ROOT / 'target' / 'debug' / 'capture-rig.exe'
JAVA = Path(r'C:\Program Files\Microsoft\jdk-25.0.4.7-hotspot\bin\java.exe')
JAR = ROOT / 'target' / 'vanilla-26.1.2' / 'server.jar'

ADD_ENTITY = 1
SET_ENTITY_DATA = 99

SERVER_PORT = 25585
RIG_PORT = 25586

# One block per kind: summon, let it sync, kill everything but players. The
# kill is what makes attribution ordering rather than guesswork.
KINDS = (
    'zombie', 'skeleton', 'creeper', 'spider',
    'pig', 'chicken', 'sheep', 'cow', 'slime',
)

INJECTIONS = []
for kind in KINDS:
    INJECTIONS.append((f'summon {kind}', f'execute at @a run summon minecraft:{kind} ~ ~3 ~'))
    INJECTIONS.append((f'kill before next kind', 'kill @e[type=!minecraft:player]'))


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
    # **A fresh world every run**: a mob left alive by an earlier session would
    # re-sync on join with whatever its state had become, and the walk would
    # attribute it to whichever kind happens to be summoned first.
    shutil.rmtree(SCRATCH / 'world', ignore_errors=True)

    (SCRATCH / 'eula.txt').write_text('eula=true\n', encoding='utf-8', newline='\n')
    (SCRATCH / 'server.properties').write_text(
        f'server-port={SERVER_PORT}\nonline-mode=false\nmotd=entity capture\nlevel-seed=1361882806\n',
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
    for wanted, label in ((ADD_ENTITY, 'add_entity'), (SET_ENTITY_DATA, 'set_entity_data')):
        hits = [e for e in s2c if e['id'] == wanted]
        print(f'\n{label} ({wanted}): {len(hits)} packets')
        files = sorted(BODIES.glob(f'*_s2c_play_{wanted}.bin')) if BODIES.is_dir() else []
        print(f'  bodies saved: {len(files)}')
    print(f'\nplay packets total: {len(s2c)}')
    return 0


if __name__ == '__main__':
    sys.exit(main())
