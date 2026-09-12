"""P09 Pi soak client: 10 scripted survival clients over a real LAN socket.

Mirrors the mc-test-support TestClient conversation exactly (crates/test-support/
src/client.rs), which is the protocol the e2e suite proves against the server:

  handshake(775, intent 2) -> HELLO(name, nil uuid) -> [LOGIN_COMPRESSION]
  -> LOGIN_FINISHED -> LOGIN_ACKNOWLEDGED -> CLIENT_INFORMATION
  -> [SELECT_KNOWN_PACKS answer] -> [REGISTRY_DATA/tags/payload drained]
  -> FINISH_CONFIGURATION ack -> play: LOGIN(49), ACCEPT_TELEPORTATION for
  PLAYER_POSITION(72), KEEP_ALIVE(44)->(28) i64, PING(61)->PONG(45) i32,
  everything else (chunk flood) drained.

Workload per client while soaking (the P08-10 driver shape, minus in-process
drains): MOVE_PLAYER_ROT every tick-ish (20 Hz), one MOVE_PLAYER_POS 0.2-block
step every 20th tick, and a rotating /list chat_command every ~5 s.

Usage: python pi_soak_client.py HOST PORT DURATION_SECONDS [PLAYERS]
Exits non-zero if any client fails to reach play or is disconnected.
"""
import json
import socket
import struct
import sys
import threading
import time
import zlib

PROTOCOL = 775
VIEW_DISTANCE = 8

# clientbound ids (from crates/protocol/src/ids.rs, jar-extracted)
CB = {
    "login_disconnect": 0, "login_finished": 2, "login_compression": 3,
    "cfg_cookie_request": 0, "cfg_custom_payload": 1, "cfg_disconnect": 2,
    "cfg_finish": 3, "cfg_keepalive": 4, "cfg_ping": 5, "cfg_registry": 7,
    "cfg_features": 12, "cfg_tags": 13, "cfg_known_packs": 14,
    "play_block_update": 8, "play_disconnect": 32, "play_game_event": 38,
    "play_keepalive": 44, "play_chunk": 45, "play_login": 49,
    "play_ping": 61, "play_pong": 62, "play_player_position": 72,
    "play_center": 94, "play_radius": 95,
}
# serverbound ids
SB = {
    "handshake": 0, "login_hello": 0, "login_ack": 3,
    "cfg_client_info": 0, "cfg_finish": 3, "cfg_keepalive": 4, "cfg_pong": 5,
    "cfg_known_packs": 7,
    "play_teleport_ack": 0, "play_chat_command": 7, "play_keepalive": 28,
    "play_move_pos": 30, "play_move_rot": 32, "play_pong": 45,
}

def varint(v):
    out = bytearray()
    while True:
        b = v & 0x7F
        v >>= 7
        if v:
            out.append(b | 0x80)
        else:
            out.append(b)
            return bytes(out)

def pack_str(s):
    raw = s.encode("utf-8")
    return varint(len(raw)) + raw

def frame_uncompressed(payload):
    return varint(len(payload)) + payload

def frame_compressed(payload, threshold):
    if len(payload) < threshold:
        return varint(len(payload) + 1) + varint(0) + payload
    comp = zlib.compress(payload)
    return varint(len(comp) + len(varint(len(payload)))) + varint(len(payload)) + comp

def read_varint(sock, state):
    num, shift = 0, 0
    while True:
        b = state["buf"][state["pos"]]
        state["pos"] += 1
        num |= (b & 0x7F) << shift
        if not b & 0x80:
            return num
        shift += 7
        if shift >= 35:
            raise IOError("varint too long")

def fill(sock, state, n):
    while state["pos"] + n > len(state["buf"]):
        chunk = sock.recv(262144)
        if not chunk:
            raise IOError("connection closed")
        state["buf"] += chunk

def next_frame(sock, state):
    """Return one raw packet payload (already decompressed) or None on timeout."""
    fill(sock, state, 1)
    start = state["pos"]
    packet_len = read_varint(sock, state)
    fill(sock, state, packet_len)
    body = state["buf"][state["pos"]:state["pos"] + packet_len]
    state["pos"] += packet_len
    if state["compression"] is None:
        return body[1:], body[0]  # id byte, rest -- ids all < 128 here
    dlen, p = 0, 0
    while True:
        b = body[p]
        p += 1
        dlen |= (b & 0x7F) << (7 * (p - 1))
        if not b & 0x80:
            break
    payload = body[p:]
    if dlen == 0:
        payload = payload  # below threshold, plain
    else:
        payload = zlib.decompress(payload)
    return payload[1:], payload[0]

def send_packet(sock, state, pid, payload):
    body = varint(pid) + payload
    if state["compression"] is None:
        sock.sendall(varint(len(body)) + body)
    else:
        sock.sendall(frame_compressed(body, state["compression"]))

def uuid_nil():
    return b"\x00" * 16

class SoakClient(threading.Thread):
    def __init__(self, host, port, name, duration, index, results):
        super().__init__(daemon=True)
        self.host, self.port, self.name = host, port, name
        self.duration = duration
        self.index = index
        self.results = results
        self.state = {"buf": bytearray(), "pos": 0, "compression": None}
        self.reached_play = False
        self.keepalives = 0
        self.commands = 0
        self.chunks = 0
        self.teleports = 0
        self.error = None
        self.spawn = None
        self.sock = None
        self.stop_flag = threading.Event()
        self.tick = 0

    def run(self):
        try:
            self.sock = socket.create_connection((self.host, self.port), timeout=15)
            self.sock.settimeout(0.02)
            # handshake (state 2 = login)
            hs = varint(PROTOCOL) + pack_str(self.host) + struct.pack(">H", self.port) + varint(2)
            send_packet(self.sock, self.state, SB["handshake"], hs)
            send_packet(self.sock, self.state, SB["login_hello"], pack_str(self.name) + uuid_nil())
            self.complete_login()
            self.complete_configuration()
            self.enter_play()
            self.reached_play = True
            self.soak_loop()
        except Exception as exc:  # noqa: BLE001 - record and report, never crash the harness
            self.error = "%s: %s" % (type(exc).__name__, exc)
            if not self.stop_flag.is_set():
                self.results["failures"].append((self.name, self.error))

    # ---- login/config/play stages (mirror of TestClient) ----
    def complete_login(self):
        for _ in range(4):
            payload, pid = next_frame(self.sock, self.state)
            if pid == CB["login_compression"]:
                threshold, _ = self._read_varint(payload, 0)
                self.state["compression"] = threshold
            elif pid == CB["login_finished"]:
                send_packet(self.sock, self.state, SB["login_ack"], b"")
                return
            elif pid == CB["login_disconnect"]:
                raise IOError("login disconnect: %r" % payload[:80])
            else:
                raise IOError("unexpected login packet id %d" % pid)
        raise IOError("no login success")

    def complete_configuration(self):
        info = (pack_str("en_us") + varint(VIEW_DISTANCE) + varint(0)
                + varint(1) + varint(0x7F) + varint(1) + varint(0) + varint(0))
        send_packet(self.sock, self.state, SB["cfg_client_info"], info)
        deadline = time.time() + 60
        while time.time() < deadline:
            payload, pid = self._recv_or_timeout()
            if payload is None:
                continue
            if pid == CB["cfg_known_packs"]:
                answer = varint(1) + pack_str("minecraft") + pack_str("core") + pack_str("26.1.2")
                send_packet(self.sock, self.state, SB["cfg_known_packs"], answer)
            elif pid == CB["cfg_finish"]:
                send_packet(self.sock, self.state, SB["cfg_finish"], b"")
                return
            elif pid in (CB["cfg_registry"], CB["cfg_tags"], CB["cfg_custom_payload"],
                         CB["cfg_features"]):
                pass
            elif pid == CB["cfg_keepalive"]:
                self._reply_cfg_keepalive(payload)
            elif pid == CB["cfg_ping"]:
                self._reply_cfg_ping(payload)
            elif pid == CB["cfg_disconnect"]:
                raise IOError("config disconnect: %r" % payload[:80])
            else:
                raise IOError("unexpected config packet id %d" % pid)
        raise IOError("no finish configuration")

    def enter_play(self):
        deadline = time.time() + 60
        while time.time() < deadline:
            payload, pid = self._recv_or_timeout()
            if payload is None:
                continue
            if pid == CB["play_login"]:
                return
            if pid == CB["play_player_position"]:
                self._ack_teleport(payload)
            elif pid == CB["play_keepalive"]:
                self._reply_keepalive(payload)
            elif pid in (CB["play_center"], CB["play_radius"], CB["play_game_event"]):
                pass
            elif pid == CB["play_disconnect"]:
                raise IOError("play disconnect during join: %r" % payload[:80])
            elif pid == CB["play_chunk"]:
                self.chunks += 1
            else:
                raise IOError("unexpected play packet id %d" % pid)
        raise IOError("no join game")

    # ---- soak loop ----
    def soak_loop(self):
        deadline = time.time() + self.duration
        yaw = float(self.index) * 3.0
        while time.time() < deadline and not self.stop_flag.is_set():
            self.tick += 1
            yaw = (yaw + 1.87) % 360.0
            rot = struct.pack(">ff", yaw, 0.0) + varint(1)
            send_packet(self.sock, self.state, SB["play_move_rot"], rot)
            if self.tick % 20 == 0 and self.spawn is not None:
                sx, sy, sz = self.spawn
                step = 0.2 * ((self.tick // 20) % 2)
                pos = struct.pack(">ddd", sx + 0.5 + step, float(sy), sz + 0.5) + varint(1)
                send_packet(self.sock, self.state, SB["play_move_pos"], pos)
            if self.tick % 100 == (self.index * 5) % 100:
                send_packet(self.sock, self.state, SB["play_chat_command"], pack_str("list"))
                self.commands += 1
            self.drain_inbound(0.04)
        self.stop_flag.set()

    def drain_inbound(self, window):
        end = time.time() + window
        while time.time() < end:
            payload, pid = self._recv_or_timeout()
            if payload is None:
                return
            if pid == CB["play_keepalive"]:
                self._reply_keepalive(payload)
            elif pid == CB["play_ping"]:
                send_packet(self.sock, self.state, SB["play_pong"], payload[:4])
            elif pid == CB["play_player_position"]:
                self._ack_teleport(payload)
            elif pid == CB["play_chunk"]:
                self.chunks += 1
            elif pid == CB["play_disconnect"]:
                raise IOError("disconnected during soak: %r" % payload[:80])
            # everything else drained

    # ---- helpers ----
    def _recv_or_timeout(self):
        try:
            return next_frame(self.sock, self.state)
        except socket.timeout:
            return None, None

    @staticmethod
    def _read_varint(buf, pos):
        num, shift = 0, 0
        while True:
            b = buf[pos]
            pos += 1
            num |= (b & 0x7F) << shift
            if not b & 0x80:
                return num, pos
            shift += 7

    def _reply_keepalive(self, payload):
        # serverbound keepalive echoes the i64
        send_packet(self.sock, self.state, SB["play_keepalive"], payload[:8])
        self.keepalives += 1

    def _reply_cfg_keepalive(self, payload):
        send_packet(self.sock, self.state, SB["cfg_keepalive"], payload[:8])

    def _reply_cfg_ping(self, payload):
        send_packet(self.sock, self.state, SB["cfg_pong"], payload[:4])

    def _ack_teleport(self, payload):
        # PLAYER_POSITION: 6xf64 + 2xf32 + varint flags + varint teleport_id
        pos = 48
        flags, pos = self._read_varint(payload, pos)
        teleport_id, pos = self._read_varint(payload, pos)
        send_packet(self.sock, self.state, SB["play_teleport_ack"], varint(teleport_id))
        self.teleports += 1
        if self.spawn is None:
            x, y, z = struct.unpack(">ddd", payload[:24])
            self.spawn = (x, y, z)

def main():
    host, port, duration = sys.argv[1], int(sys.argv[2]), int(sys.argv[3])
    players = int(sys.argv[4]) if len(sys.argv) > 4 else 10
    results = {"failures": []}
    clients = [SoakClient(host, port, "Soak%d" % i, duration, i, results)
               for i in range(players)]
    started = time.time()
    for c in clients:
        c.start()
        time.sleep(0.25)  # stagger joins like real players
    for c in clients:
        c.join(duration + 30)
    elapsed = time.time() - started
    summary = {
        "players": players,
        "duration_requested_s": duration,
        "elapsed_s": round(elapsed, 1),
        "reached_play": sum(1 for c in clients if c.reached_play),
        "keepalives_answered": sum(c.keepalives for c in clients),
        "teleports_acked": sum(c.teleports for c in clients),
        "commands_sent": sum(c.commands for c in clients),
        "chunks_received": sum(c.chunks for c in clients),
        "failures": results["failures"],
    }
    print(json.dumps(summary, indent=1))
    ok = (summary["reached_play"] == players) and not summary["failures"]
    sys.exit(0 if ok else 1)

if __name__ == "__main__":
    main()
