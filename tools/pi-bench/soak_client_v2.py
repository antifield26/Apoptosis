"""P09 Pi soak client v2: 10 scripted survival clients, reader+writer threads.

v1 findings (first soak, 2026-09-12): the server's per-IP admission cap
(MAX_CONNECTIONS_PER_IP = 4, limits.rs) correctly refused 10 clients from one
source address, and a 20 ms socket timeout could interrupt sendall mid-packet.
v2 therefore (a) runs one client per loopback source address 127.0.0.2.., so
the per-IP gate sees one connection per address, (b) splits each client into a
blocking reader thread (keepalive/ping/teleport answers, chunk drain) and a
writer thread (20 Hz movement), so a busy receive path can never interrupt a
send.

Protocol conversation mirrors mc-test-support TestClient exactly (see v1
header in the repo history): handshake -> HELLO -> [compression] ->
LOGIN_FINISHED -> LOGIN_ACKNOWLEDGED -> CLIENT_INFORMATION -> known packs ->
FINISH_CONFIGURATION ack -> play (LOGIN, ACCEPT_TELEPORTATION, KEEP_ALIVE,
PONG, drain chunks).

Usage: pi_soak_client.py HOST PORT DURATION_SECONDS [PLAYERS]
Runs on the Pi itself (nice -n 19) for the P09 acceptance soak.
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

CB = {
    "login_disconnect": 0, "login_finished": 2, "login_compression": 3,
    "cfg_custom_payload": 1, "cfg_disconnect": 2, "cfg_finish": 3,
    "cfg_keepalive": 4, "cfg_ping": 5, "cfg_registry": 7, "cfg_features": 12,
    "cfg_tags": 13, "cfg_known_packs": 14,
    "play_disconnect": 32, "play_game_event": 38, "play_keepalive": 44,
    "play_chunk": 45, "play_login": 49, "play_ping": 61,
    "play_player_position": 72, "play_center": 94, "play_radius": 95,
}
SB = {
    "handshake": 0, "login_hello": 0, "login_ack": 3,
    "cfg_client_info": 0, "cfg_finish": 3, "cfg_keepalive": 4, "cfg_pong": 5,
    "cfg_known_packs": 7, "play_teleport_ack": 0, "play_chat_command": 7,
    "play_keepalive": 28, "play_move_pos": 30, "play_move_rot": 32,
    "play_pong": 45,
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

def read_varint_buf(buf, pos):
    num, shift = 0, 0
    while True:
        b = buf[pos]
        pos += 1
        num |= (b & 0x7F) << shift
        if not b & 0x80:
            return num, pos
        shift += 7
        if shift >= 35:
            raise IOError("varint too long")

class Client:
    """One scripted player. The reader owns the socket receive path; the
    writer owns sends. Both share `lock` because the reader answers packets
    (keepalive) on the same socket."""

    def __init__(self, host, port, name, source_ip, duration, index, results):
        self.host, self.port, self.name = host, port, name
        self.source_ip = source_ip
        self.duration = duration
        self.index = index
        self.results = results
        self.compression = None
        self.lock = threading.Lock()
        self.sock = None
        self.reached_play = False
        self.keepalives = 0
        self.commands = 0
        self.chunks = 0
        self.teleports = 0
        self.error = None
        self.spawn = None
        self.tick = 0
        self.thread = threading.Thread(target=self.run, daemon=True)
        self.joined = threading.Event()

    # ---- socket helpers (thread-safe via self.lock) ----
    def send(self, pid, payload):
        body = varint(pid) + payload
        with self.lock:
            if self.compression is None:
                self.sock.sendall(varint(len(body)) + body)
            else:
                if len(body) < self.compression:
                    self.sock.sendall(varint(len(body) + 1) + varint(0) + body)
                else:
                    comp = zlib.compress(body)
                    self.sock.sendall(varint(len(comp) + len(varint(len(body))))
                                      + varint(len(body)) + comp)

    def recv_packet(self):
        """Blocking read of one frame; returns (pid, payload)."""
        buf = bytearray()
        while True:
            b = self.sock.recv(1)
            if not b:
                raise IOError("connection closed")
            buf += b
            if not b[0] & 0x80:
                break
        packet_len, pos = read_varint_buf(bytes(buf), 0)
        payload = bytearray()
        while len(payload) < packet_len:
            chunk = self.sock.recv(packet_len - len(payload))
            if not chunk:
                raise IOError("connection closed mid-frame")
            payload += chunk
        if self.compression is None:
            pid, p = read_varint_buf(payload, 0)
            return pid, payload[p:]
        dlen, p = read_varint_buf(payload, 0)
        body = payload[p:]
        if dlen != 0:
            body = zlib.decompress(bytes(body))
        pid, p = read_varint_buf(body, 0)
        return pid, body[p:]

    # ---- stages ----
    def run(self):
        try:
            self.sock = socket.socket()
            self.sock.bind((self.source_ip, 0))
            self.sock.connect((self.host, self.port))
            self.sock.settimeout(30)
            hs = varint(PROTOCOL) + pack_str(self.host) + struct.pack(">H", self.port) + varint(2)
            self.send(SB["handshake"], hs)
            self.send(SB["login_hello"], pack_str(self.name) + b"\x00" * 16)
            self.complete_login()
            self.complete_configuration()
            self.enter_play()
            self.reached_play = True
            self.joined.set()
            self.reader_loop()  # the main thread reads until the soak ends
        except Exception as exc:  # noqa: BLE001
            self.error = "%s: %s" % (type(exc).__name__, exc)
            self.results["failures"].append((self.name, self.error))
            self.joined.set()

    def complete_login(self):
        for _ in range(4):
            pid, payload = self.recv_packet()
            if pid == CB["login_compression"]:
                threshold, _ = read_varint_buf(payload, 0)
                with self.lock:
                    self.compression = threshold
            elif pid == CB["login_finished"]:
                self.send(SB["login_ack"], b"")
                return
            elif pid == CB["login_disconnect"]:
                raise IOError("login disconnect: %r" % payload[:80])
            else:
                raise IOError("unexpected login packet id %d" % pid)
        raise IOError("no login success")

    def complete_configuration(self):
        # Matches `ClientInformation` (config.rs): locale, view_distance,
        # chat_mode, chat_colors (bool byte), skin_parts, main_hand,
        # text_filtering (bool byte), server_listing (bool byte),
        # particle_status (VarInt). The v2 body missed particle_status and
        # encoded the bools as VarInts — the server answered with
        # "truncated VarInt" after login accepted (P18-04 soak finding).
        info = (
            pack_str("en_us")
            + struct.pack("b", VIEW_DISTANCE)
            + varint(0)
            + b"\x01"
            + bytes([0x7F])
            + varint(1)
            + b"\x00"
            + b"\x01"
            + varint(0)
        )
        self.send(SB["cfg_client_info"], info)
        deadline = time.time() + 60
        while time.time() < deadline:
            pid, payload = self.recv_packet()
            if pid == CB["cfg_known_packs"]:
                answer = varint(1) + pack_str("minecraft") + pack_str("core") + pack_str("26.1.2")
                self.send(SB["cfg_known_packs"], answer)
            elif pid == CB["cfg_finish"]:
                self.send(SB["cfg_finish"], b"")
                return
            elif pid in (CB["cfg_registry"], CB["cfg_tags"], CB["cfg_custom_payload"],
                         CB["cfg_features"]):
                pass
            elif pid == CB["cfg_keepalive"]:
                self.send(SB["cfg_keepalive"], payload[:8])
            elif pid == CB["cfg_ping"]:
                self.send(SB["cfg_pong"], payload[:4])
            elif pid == CB["cfg_disconnect"]:
                raise IOError("config disconnect: %r" % payload[:80])
            else:
                raise IOError("unexpected config packet id %d" % pid)
        raise IOError("no finish configuration")

    def enter_play(self):
        deadline = time.time() + 60
        while time.time() < deadline:
            pid, payload = self.recv_packet()
            if pid == CB["play_login"]:
                return
            if pid == CB["play_player_position"]:
                self.ack_teleport(payload)
            elif pid == CB["play_keepalive"]:
                self.send(SB["play_keepalive"], payload[:8])
            elif pid in (CB["play_center"], CB["play_radius"], CB["play_game_event"],
                         CB["play_chunk"]):
                if pid == CB["play_chunk"]:
                    self.chunks += 1
            elif pid == CB["play_disconnect"]:
                raise IOError("play disconnect during join: %r" % payload[:80])
            else:
                raise IOError("unexpected play packet id %d during join" % pid)
        raise IOError("no join game")

    def reader_loop(self):
        """Answers everything the server sends until the soak window ends."""
        end = time.time() + self.duration
        while time.time() < end:
            try:
                pid, payload = self.recv_packet()
            except socket.timeout:
                continue
            if pid == CB["play_keepalive"]:
                self.send(SB["play_keepalive"], payload[:8])
                self.keepalives += 1
            elif pid == CB["play_ping"]:
                self.send(SB["play_pong"], payload[:4])
            elif pid == CB["play_player_position"]:
                self.ack_teleport(payload)
            elif pid == CB["play_chunk"]:
                self.chunks += 1
            elif pid == CB["play_disconnect"]:
                raise IOError("disconnected during soak: %r" % payload[:80])

    def ack_teleport(self, payload):
        teleport_id, _ = read_varint_buf(payload, 48)
        self.send(SB["play_teleport_ack"], varint(teleport_id))
        self.teleports += 1
        if self.spawn is None:
            x, y, z = struct.unpack(">ddd", payload[:24])
            self.spawn = (x, y, z)

    # ---- writer: 20 Hz workload (P08-10 shape) ----
    def writer_loop(self):
        self.joined.wait(60)
        if not self.reached_play:
            return
        yaw = float(self.index) * 3.0
        end = time.time() + self.duration
        next_tick = time.time()
        while time.time() < end:
            self.tick += 1
            yaw = (yaw + 1.87) % 360.0
            try:
                self.send(SB["play_move_rot"], struct.pack(">ff", yaw, 0.0) + varint(1))
                if self.tick % 20 == 0 and self.spawn is not None:
                    sx, sy, sz = self.spawn
                    step = 0.2 * ((self.tick // 20) % 2)
                    self.send(SB["play_move_pos"],
                              struct.pack(">ddd", sx + 0.5 + step, float(sy), sz + 0.5) + varint(1))
                if self.tick % 100 == (self.index * 5) % 100:
                    self.send(SB["play_chat_command"], pack_str("list"))
                    self.commands += 1
            except OSError as exc:
                self.results["failures"].append((self.name, "writer: %s" % exc))
                return
            next_tick += 0.05
            delay = next_tick - time.time()
            if delay > 0:
                time.sleep(delay)
            else:
                next_tick = time.time()

def main():
    host, port, duration = sys.argv[1], int(sys.argv[2]), int(sys.argv[3])
    players = int(sys.argv[4]) if len(sys.argv) > 4 else 10
    results = {"failures": []}
    clients = []
    for i in range(players):
        source = "127.0.0.%d" % (2 + i)  # 127.0.0.2..11: one per-IP gate slot each
        c = Client(host, port, "Soak%d" % i, source, duration, i, results)
        clients.append(c)
    started = time.time()
    for c in clients:
        c.writer = threading.Thread(target=c.writer_loop, daemon=True)
        c.thread.start()
        time.sleep(0.25)  # stagger joins; the burst token bucket refills per 250 ms
        c.writer.start()
    for c in clients:
        c.thread.join(duration + 30)
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
