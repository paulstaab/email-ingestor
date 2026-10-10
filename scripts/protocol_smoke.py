#!/usr/bin/env python3
"""Disposable implicit-TLS peers with independent receipts; no production credentials.

Run after cargo build: python3 scripts/protocol_smoke.py
Use --large for an incrementally generated 250 MiB fixture, --image for container smoke.
"""
import argparse
import contextlib
import concurrent.futures
import hashlib
import os
from pathlib import Path
import re
import socket
import ssl
import subprocess
import tempfile
import threading
import time

RAW = b"Return-Path: <sender@example.test>\r\nSubject: private-fixture\r\n\r\n.leading\r\nlast\r\n"
PATTERN = b".generated body\r\n"


class Scenario:
    def __init__(self, mode, size=None):
        self.mode = mode
        self.size = size or len(RAW)
        self.source = {1, 2} if mode == "duplicate_content" else {1}
        self.commands = []
        self.receipts = []
        self.moves = []
        self.attempts = 0
        self.move_attempts = 0
        self.imap_setups = 0
        self.smtp_setups = 0
        self.fetch_attempts = 0
        self.incomplete_commands = 0
        self.data_terminators = 0
        self.aborted_data = []
        self.generation = 42
        self.blocked = threading.Event()
        self.errors = []
        self.lock = threading.Lock()
        self.done = threading.Event()
        self.stop = threading.Event()
        self.connections = []

    def data(self, offset, count):
        if self.mode != "large":
            return RAW[offset:offset + count]
        original_offset = offset
        count = min(count, self.size - offset)
        if count <= 0:
            return b""
        prefix = b"Return-Path: <>\r\n\r\n"
        result = b""
        if offset < len(prefix):
            result = prefix[offset:offset + count]
            offset += len(result)
            count -= len(result)
        if count:
            shift = (offset - len(prefix)) % len(PATTERN)
            result += (PATTERN * ((count + shift) // len(PATTERN) + 1))[shift:shift + count]
        # Force a faithful final CRLF at the exact byte limit, without a full fixture.
        for index, byte in enumerate(b"\r\n", self.size - 2):
            if original_offset <= index < original_offset + len(result):
                position = index - original_offset
                result = result[:position] + bytes([byte]) + result[position + 1:]
        return result

    def expected_digest(self):
        digest = hashlib.sha256()
        for offset in range(0, self.size, 1024 * 1024):
            digest.update(self.data(offset, 1024 * 1024))
        return digest.hexdigest()

    def imap(self, conn):
        with conn.makefile("rwb", buffering=0) as io:
            io.write(b"* OK disposable test peer\r\n")
            while not self.stop.is_set():
                line = io.readline(65536)
                if not line:
                    return
                tag, command = line.decode().rstrip("\r\n").split(" ", 1)
                if command == "AUTHENTICATE PLAIN":
                    self.imap_setups += 1
                    if self.mode == "five_imap_setup":
                        io.write(f"{tag} NO [UNAVAILABLE] fixture\r\n".encode())
                        return
                    io.write(b"+ authenticate\r\n")
                    assert io.readline(4096).strip()  # never record credentials
                    io.write(f"{tag} OK authenticated\r\n".encode())
                    continue
                with self.lock:
                    self.commands.append(command)
                if command == "CAPABILITY":
                    io.write(f"* CAPABILITY IMAP4rev1 MOVE\r\n{tag} OK capabilities\r\n".encode())
                elif command.startswith("SELECT "):
                    io.write(f"* {len(self.source)} EXISTS\r\n* OK [UIDVALIDITY {self.generation}] generation\r\n* OK [UIDNEXT 3] next\r\n{tag} OK [READ-WRITE] selected\r\n".encode())
                elif command.startswith("UID SEARCH UID "):
                    bounds = command.split()[-1].split(":")
                    first, last = int(bounds[0]), int(bounds[-1])
                    with self.lock:
                        found = sorted(u for u in self.source if first <= u <= last)
                    io.write(("* SEARCH" + "".join(f" {u}" for u in found) + f"\r\n{tag} OK found\r\n").encode())
                elif command.startswith("UID FETCH "):
                    uid = int(command.split()[2])
                    if "RFC822.SIZE" in command:
                        self.fetch_attempts += 1
                        if self.mode == "five_fetch":
                            return
                        size = self.size + (1 if self.mode == "inaccurate_size" else 0)
                        io.write(f"* 1 FETCH (UID {uid} RFC822.SIZE {size})\r\n{tag} OK metadata\r\n".encode())
                    else:
                        offset, count = map(int, re.search(r"<(\d+)\.(\d+)>", command).groups())
                        data = self.data(offset, count)
                        if self.mode == "wrong_uid":
                            uid += 10
                        io.write(f"* 1 FETCH (UID {uid} BODY[]<{offset}> {{{len(data)}}}\r\n".encode())
                        io.write(data)
                        if self.mode == "incomplete_command" and offset > 0 and self.incomplete_commands == 0:
                            # Fail the first EOF probe after its literal, before tagged completion.
                            # The retry must retrieve from byte zero and complete normally.
                            assert offset == self.size and not data
                            self.incomplete_commands += 1
                            io.write(b")\r\n")
                            return
                        io.write(f")\r\n{tag} OK body\r\n".encode())
                elif command.startswith("UID MOVE "):
                    uid = int(command.split()[2])
                    with self.lock:
                        assert self.receipts, "move before independent SMTP acceptance"
                        self.move_attempts += 1
                        if self.mode == "generation_change" and self.move_attempts == 1:
                            self.generation += 1
                            return
                        if self.mode == "restart_after_acceptance" and self.move_attempts == 1:
                            self.blocked.set()
                            # Release the lock so the process controller can verify source membership.
                            # The first MOVE intentionally makes no state change before the crash.
                            return
                        if self.mode == "five_move" or (self.mode == "retry_move" and self.move_attempts == 1):
                            io.write(f"{tag} NO [OVERQUOTA] retry fixture\r\n".encode())
                            continue
                        self.source.discard(uid)
                        self.moves.append(uid)
                    if self.mode == "lost_move_reply":
                        return
                    io.write(f"* 1 EXPUNGE\r\n{tag} OK moved\r\n".encode())
                    if not self.source:
                        self.done.set()
                elif command == "NOOP":
                    io.write(f"{tag} OK noop\r\n".encode())
                else:
                    raise AssertionError(f"unexpected ingestor command: {command}")

    def smtp(self, conn):
        with conn.makefile("rb", buffering=65536) as io:
            conn.sendall(b"220 disposable SMTP\r\n")
            while not self.stop.is_set():
                line = io.readline(4096)
                if not line:
                    return
                if line.startswith(b"EHLO "):
                    conn.sendall(b"250-peer\r\n250-8BITMIME\r\n250-SMTPUTF8\r\n250 AUTH PLAIN\r\n")
                elif line.startswith(b"AUTH PLAIN "):
                    self.smtp_setups += 1
                    if self.mode == "five_smtp_setup":
                        conn.sendall(b"454 temporary authentication failure\r\n")
                        return
                    conn.sendall(b"235 authenticated\r\n")
                elif line.startswith(b"MAIL FROM:"):
                    expected = b"MAIL FROM:<>" if self.mode == "large" else b"MAIL FROM:<sender@example.test>"
                    assert line.startswith(expected), "incorrect reverse path"
                    conn.sendall(b"250 sender\r\n")
                elif line == b"RCPT TO:<recipient@example.test>\r\n":
                    if self.mode == "permanent_rejection":
                        conn.sendall(b"550 rejected private address fixture\r\n")
                        return
                    conn.sendall(b"250 recipient\r\n")
                elif line == b"DATA\r\n":
                    conn.sendall(b"354 stream\r\n")
                    digest = hashlib.sha256()
                    count = 0
                    while True:
                        body = io.readline(2048)
                        if not body:
                            with self.lock:
                                self.aborted_data.append((count, digest.hexdigest()))
                            return  # incomplete transaction is never a receipt
                        if body == b".\r\n":
                            with self.lock:
                                self.data_terminators += 1
                            break
                        assert len(body) <= 1001 and body.endswith(b"\r\n")
                        if body.startswith(b".."):
                            body = body[1:]
                        count += len(body)
                        digest.update(body)
                    assert count == self.size, "incomplete/altered accepted bytes"
                    assert digest.hexdigest() == self.expected_digest(), "original-byte digest mismatch"
                    with self.lock:
                        self.attempts += 1
                        if self.mode != "five_submissions":
                            self.receipts.append((count, digest.hexdigest()))
                    if self.mode == "five_submissions":
                        conn.sendall(b"450 final temporary rejection\r\n")
                        return
                    if self.mode == "lost_acceptance" and self.attempts == 1:
                        return
                    conn.sendall(b"250 final accepted\r\n")
                else:
                    raise AssertionError("unexpected SMTP command")

    def listen(self, tls, protocol):
        listener = socket.socket()
        listener.bind(("127.0.0.1", 0))
        listener.listen()
        listener.settimeout(0.2)
        port = listener.getsockname()[1]

        def connection(raw):
            try:
                with tls.wrap_socket(raw, server_side=True) as conn:
                    conn.settimeout(15)
                    self.connections.append(conn)
                    protocol(conn)
            except (ssl.SSLError, ConnectionResetError, BrokenPipeError, TimeoutError):
                pass  # intentional fault/cancellation
            except Exception as error:
                self.errors.append(error)
                self.done.set()

        def accept():
            while not self.stop.is_set():
                try:
                    raw, _ = listener.accept()
                except socket.timeout:
                    continue
                threading.Thread(target=connection, args=(raw,), daemon=True).start()
            listener.close()

        threading.Thread(target=accept, daemon=True).start()
        return port


def exercise(args, directory, tls, mode):
    size = None
    if mode == "large":
        # Exact 250 MiB, with complete final CRLF and a short final line.
        size = 262_144_000
        prefix = len(b"Return-Path: <>\r\n\r\n")
        # Adjust the generated line pattern so the fixture has the exact default limit.
        global PATTERN
        PATTERN = b"." + b"g" * 14 + b"\r\n"
        assert (size - prefix - 2) % len(PATTERN) not in {15, 16}
    scenario = Scenario(mode, size)
    imap_port = scenario.listen(tls, scenario.imap)
    smtp_port = scenario.listen(tls, scenario.smtp)
    env = {k: v for k, v in os.environ.items() if not k.startswith("INGESTOR_")}
    values = {
        "IMAP_HOST": "localhost", "IMAP_PORT": str(imap_port), "IMAP_CA_FILE": str(directory / "ca.pem"),
        "IMAP_USERNAME": "private-user-fixture", "IMAP_PASSWORD": "private-password-fixture",
        "SMTP_HOST": "localhost", "SMTP_PORT": str(smtp_port), "SMTP_CA_FILE": str(directory / "ca.pem"),
        "SMTP_USERNAME": "private-user-fixture", "SMTP_PASSWORD": "private-password-fixture",
        "SMTP_RECIPIENT": "recipient@example.test", "DESTINATION_FOLDER": "Actual Trash",
    }
    if mode == "oversized":
        values["MAX_MESSAGE_BYTES"] = "1"
    if mode == "invalid_tls":
        values["IMAP_HOST"] = "127.0.0.1"
    env.update({"INGESTOR_" + k: v for k, v in values.items()})
    command = [args.binary]
    if args.image:
        command = ["docker", "run", "--rm", "--network", "host", "--read-only", "--cap-drop", "ALL", "--memory", "512m", "-v", f"{directory}:{directory}:ro"]
        for key, value in values.items():
            command += ["-e", f"INGESTOR_{key}={value}"]
        command += [args.image]
    threshold = mode.startswith("five_")
    fatal = threshold or mode in {"permanent_rejection", "oversized", "wrong_uid", "inaccurate_size", "invalid_tls"}
    peak_kib = 0
    with tempfile.TemporaryFile() as output:
        process = subprocess.Popen(command, env=env, stdout=output, stderr=output)
        try:
            deadline = time.monotonic() + (120 if threshold else 180 if mode == "large" else 20)
            restarted = False
            while time.monotonic() < deadline:
                if mode == "restart_after_acceptance" and scenario.blocked.is_set() and not restarted:
                    process.kill()
                    process.wait()
                    assert scenario.source == {1} and len(scenario.receipts) == 1
                    process = subprocess.Popen(command, env=env, stdout=output, stderr=output)
                    restarted = True
                if not args.image:
                    with contextlib.suppress(FileNotFoundError, ProcessLookupError):
                        status = Path(f"/proc/{process.pid}/status").read_text()
                        match = re.search(r"VmHWM:\s+(\d+)", status)
                        if match:
                            peak_kib = max(peak_kib, int(match[1]))
                if fatal and process.poll() is not None:
                    break
                if mode == "lost_move_reply" and scenario.moves:
                    time.sleep(6)  # allow reconnect and absent-source reconciliation
                    break
                if scenario.done.wait(0.02):
                    break
                if process.poll() is not None:
                    break
            if not fatal:
                process.terminate()
            code = process.wait(timeout=35)
            output.seek(0)
            logs = output.read().decode()
            assert not scenario.errors, str(scenario.errors)
            assert code == (1 if fatal else 0), f"{mode}: exit {code}: {logs}"
            for private in ["private-user-fixture", "private-password-fixture", "private-fixture", "sender@example.test", "recipient@example.test"]:
                assert private not in logs, "private data in logs"
            for command in scenario.commands:
                assert not re.search(r"(^| )(STORE|COPY|CLOSE|DELETE|EXPUNGE)( |$)", command), "forbidden purge/emulation"
            if fatal:
                assert scenario.source == {1} and not scenario.moves
                assert len(scenario.receipts) == (1 if mode == "five_move" else 0)
                if threshold:
                    counter = {"five_imap_setup": scenario.imap_setups, "five_smtp_setup": scenario.smtp_setups, "five_submissions": scenario.attempts, "five_fetch": scenario.fetch_attempts, "five_move": scenario.move_attempts}[mode]
                    assert counter == 5, f"{mode}: {counter} attempts"
                    assert "fifth consecutive recoverable failure" in logs
            else:
                assert not scenario.source and scenario.moves, f"{mode}: pending source: {logs}"
                assert len(scenario.receipts) == (2 if mode in {"lost_acceptance", "duplicate_content", "generation_change", "restart_after_acceptance"} else 1)
                if mode == "retry_move":
                    assert scenario.move_attempts == 2
                if mode == "incomplete_command":
                    assert scenario.incomplete_commands == 1, "EOF completion fault was not injected"
                    assert scenario.fetch_attempts == 2, "incomplete retrieval was not retried"
                    assert scenario.aborted_data == [(scenario.size, scenario.expected_digest())], "failed retrieval must close DATA after original bytes without a terminator"
                    assert scenario.data_terminators == 1, "only the complete retry may finalize DATA"
                    offsets = [int(re.search(r"<(\d+)\.\d+>", command).group(1)) for command in scenario.commands if "BODY.PEEK[]" in command]
                    assert offsets == [0, scenario.size, 0, scenario.size], "retry did not refetch from byte zero"
                    assert scenario.move_attempts == 1
            if mode == "large":
                assert peak_kib < 512 * 1024, f"RSS exceeded container target: {peak_kib} KiB"
            print(f"PASS {mode}" + (f" (peak RSS {peak_kib} KiB)" if not args.image else ""), flush=True)
        finally:
            if process.poll() is None:
                process.kill()
                process.wait()
            scenario.stop.set()
            for conn in scenario.connections:
                with contextlib.suppress(OSError):
                    conn.shutdown(socket.SHUT_RDWR)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", default="target/debug/email-ingestor")
    parser.add_argument("--image")
    parser.add_argument("--large", action="store_true")
    parser.add_argument("--thresholds", action="store_true", help="exercise real fixed backoffs through fifth-failure exits (about 80s)")
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix="ingestor-peers-") as temporary:
        directory = Path(temporary)
        directory.chmod(0o755)
        quiet = {"stdout": subprocess.DEVNULL, "stderr": subprocess.DEVNULL}
        subprocess.run(["openssl", "req", "-x509", "-newkey", "rsa:2048", "-noenc", "-keyout", str(directory / "ca.key"), "-out", str(directory / "ca.pem"), "-days", "2", "-subj", "/CN=Disposable Test CA"], check=True, **quiet)
        subprocess.run(["openssl", "req", "-new", "-newkey", "rsa:2048", "-noenc", "-keyout", str(directory / "server.key"), "-out", str(directory / "server.csr"), "-subj", "/CN=localhost"], check=True, **quiet)
        (directory / "extensions").write_text("subjectAltName=DNS:localhost\nbasicConstraints=CA:FALSE\nextendedKeyUsage=serverAuth\n")
        subprocess.run(["openssl", "x509", "-req", "-in", str(directory / "server.csr"), "-CA", str(directory / "ca.pem"), "-CAkey", str(directory / "ca.key"), "-CAcreateserial", "-out", str(directory / "server.pem"), "-days", "2", "-extfile", str(directory / "extensions")], check=True, **quiet)
        tls = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
        tls.load_cert_chain(directory / "server.pem", directory / "server.key")
        modes = ["normal", "duplicate_content", "oversized", "wrong_uid", "inaccurate_size", "permanent_rejection", "invalid_tls", "incomplete_command", "lost_acceptance", "retry_move", "lost_move_reply"]
        modes.append("generation_change")
        if not args.image:
            modes.append("restart_after_acceptance")
        if args.large:
            modes.append("large")
        for mode in modes:
            exercise(args, directory, tls, mode)
        if args.thresholds:
            modes = ["five_imap_setup", "five_smtp_setup", "five_submissions", "five_fetch", "five_move"]
            with concurrent.futures.ThreadPoolExecutor(max_workers=len(modes)) as pool:
                futures = [pool.submit(exercise, args, directory, tls, mode) for mode in modes]
                for future in futures:
                    future.result()


if __name__ == "__main__":
    main()
