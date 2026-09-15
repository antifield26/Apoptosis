"""Fill the one hole the entity-capture run left: the chicken table.

The main run's summoned creeper walked to the player and killed RealClient
(RigClient was forward-proxying a live HMCL client), so the chicken, sheep and
slime injections after the death produced no add_entity packets. Sheep and
slime tables are already covered by the earlier bodies-lit natural-spawn
capture; the chicken is not. This run is day-lit and passive-only: no hostile
mob can kill the viewer mid-run, and the kill-before-next pattern keeps every
body attributable. Slime sizes 1/2/4 are summoned explicitly so all three size
rows land in one session.
"""

import json
import shutil
import subprocess
import sys
import time
from pathlib import Path

ROOT = Path(r'C:\Users\25371\projects\MinecraftServer')
SCRATCH = ROOT / 'target' / 'chicken-capture'
BODIES = SCRATCH / 'bodies'
RIG_EXE = ROOT / 'target' / 'debug' / 'capture-rig.exe'
JAVA = Path(r'C:\Program Files\Microsoft\jdk-25.0.4.7-hotspot\bin\java.exe')
JAR = ROOT / 'target' / 'vanilla-26.1.2' / 'server.jar'

SERVER_PORT = 25587
RIG_PORT = 25588

INJECTIONS = (
    ('daylight', 'time set day'),
    ('summon chicken', 'execute at @a run summon minecraft:chicken ~ ~3 ~'),
    ('kill', 'kill @e[type=!minecraft:player]'),
    ('summon sheep', 'execute at @a run summon minecraft:sheep ~ ~3 ~'),
    ('kill', 'kill @e[type=!minecraft:player]'),
    ('summon slime 1', 'execute at @a run summon minecraft:slime ~ ~3 ~ {Size:1b}'),
    ('summon slime 2', 'execute at @a run summon minecraft:slime ~ ~3 ~ {Size:2b}'),
    ('summon slime 4', 'execute at @a run summon minecraft:slime ~ ~3 ~ {Size:4b}'),
    ('kill', 'kill @e[type=!minecraft:player]'),
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
    shutil.rmtree(SCRATCH / 'world', ignore_errors=True)

    (SCRATCH / 'eula.txt').write_text('eula=true\n', encoding='utf-8', newline='\n')
    (SCRATCH / 'server.properties').write_text(
        f'server-port={SERVER_PORT}\nonline-mode=false\nmotd=chicken capture\nlevel-seed=1361882806\n',
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
            time.sleep(6)
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
    for wanted, label in ((1, 'add_entity'), (99, 'set_entity_data')):
        hits = [e for e in s2c if e['id'] == wanted]
        print(f'\n{label} ({wanted}): {len(hits)} packets')
    deaths = [e for e in s2c if e['id'] == 63]
    print(f'death packets: {len(deaths)}')
    print(f'play packets total: {len(s2c)}')
    return 0


if __name__ == '__main__':
    sys.exit(main())
