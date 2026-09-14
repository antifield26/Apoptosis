"""Capture a real chat packet, so the chat NBT fixture can be checked against something that is not ours.

**It is `disguised_chat` (clientbound play 33), not `system_chat` (121).** The script was written expecting the
latter and the capture proved otherwise: a console `say` produced two `disguised_chat` packets and zero
`system_chat`, which is also why filtering for the wrong id reported "no chat was sent" while the packets sat in
the trace.

`crates/test-support/fixtures/protocol/nbt_literal_text.hex` claims to be "network NBT for the text component
{\"text\":\"bye\"}" and had **never been compared with network NBT from a real server** — none of the existing
vanilla captures contained a chat packet at all, so nothing in the repository could contradict it. Comparing it
with our own chat output would be the round-trip trap this review exists to find.

So: a vanilla server, a real client, and the console `say` command **after** the client is in the world — the
same shape as the lit-glowstone capture, which had to place its blocks after the join for the server to send the
packet that was being looked for.
"""

import shutil
import subprocess
import sys
import time
from pathlib import Path

ROOT = Path(r'C:\Users\25371\projects\MinecraftServer')
SCRATCH = ROOT / 'target' / 'chat-capture'
BODIES = SCRATCH / 'bodies'
RIG_EXE = ROOT / 'target' / 'debug' / 'capture-rig.exe'
JAVA = Path(r'C:\Program Files\Microsoft\jdk-25.0.4.7-hotspot\bin\java.exe')
JAR = ROOT / 'target' / 'vanilla-26.1.2' / 'server.jar'

# game clientbound 33, from docs/protocol/packet-ids-775.tsv.
DISGUISED_CHAT = 33

SERVER_PORT = 25583
RIG_PORT = 25584


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

    # The vanilla server refuses to boot without this, and a scratch directory starts without it.
    (SCRATCH / 'eula.txt').write_text('eula=true\n', encoding='utf-8', newline='\n')
    # **The port and offline mode must be set before boot.** server.properties defaults to 25565, which the
    # script does not wait on and which belongs to the owner's own server, so leaving it unset both failed the
    # wait and briefly bound a port this project must not touch.
    (SCRATCH / 'server.properties').write_text(
        f'server-port={SERVER_PORT}\nonline-mode=false\nmotd=chat capture\nlevel-seed=1361882806\n',
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

        # **After the join**, which is what makes the server send it to a player rather than nobody.
        for text in ('hello from the review', 'bye'):
            print(f'saying {text!r} on the console')
            server.stdin.write(f'say {text}\n')
        server.stdin.flush()
        # **P10-07 and P10-09 need a session in which a block entity existed and an item was dropped**, and
        # neither has one: the capture has zero bodies for `block_entity_data` and no `minecraft:item` metadata.
        # The server reads console commands from stdin, so the whole of what is missing is two lines -- no
        # gameplay, no player, nothing to play through.
        #
        # Explicit coordinates rather than `~`: the console has no position of its own to be relative to, and a
        # command that fails is captured as a log line rather than as a packet.
        for command in (
            'setblock 0 80 0 minecraft:chest',
            # **Contents, because an empty chest has nothing to describe.** The previous session placed one and
            # no `block_entity_data` came out; if that packet is sent only when a block entity has contents or
            # state worth syncing, this line is what makes one appear.
            'item replace block 0 80 0 container.0 with minecraft:diamond 5',
            # Two drops at two heights, so a despawn before the client ever saw one is not the only possibility,
            # and a shorter-lived one is not the only thing under test.
            'summon minecraft:item 0 81 0 {Item:{id:"minecraft:stone",count:3}}',
            'summon minecraft:item 0 82 0 {Item:{id:"minecraft:diamond",count:7}}',
        ):
            print(f'  injection: {command}')
            server.stdin.write(command + chr(10))
            server.stdin.flush()
            time.sleep(3)

            server.stdin.flush()
            time.sleep(2)
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

    import json

    events = [json.loads(line) for line in
              (SCRATCH / 'trace.jsonl').read_text(encoding='utf-8', errors='ignore').splitlines()
              if line.strip()]
    s2c = [e for e in events if e.get('kind') == 'packet' and e['state'] == 'play' and e['dir'] == 's2c']
    # **33, looked up in the table rather than remembered.** A console say is disguised_chat; filtering for
    # system_chat or a guessed id reported "no chat was sent" while two of them sat in the trace.
    chat = [e for e in s2c if e['id'] == DISGUISED_CHAT]
    print(f'\nplay packets: {len(s2c)}   system_chat: {len(chat)}')
    for e in chat[:4]:
        print(f'  seq={e["seq"]} id={e["id"]} body={e["body_bytes"]} head={e["head"][:80]}')
    files = sorted(BODIES.glob(f'*_s2c_play_{DISGUISED_CHAT}.bin')) if BODIES.is_dir() else []
    print(f'disguised_chat bodies: {len(files)}')
    for path in files[:4]:
        print(f'  {path.name}  {path.stat().st_size} bytes  {path.read_bytes()[:40].hex(" ")}')
    return 0


if __name__ == '__main__':
    sys.exit(main())
