from __future__ import annotations

import argparse
import json
import socket
import ssl
import threading
import time
from contextlib import suppress
from pathlib import Path


class FtpFixtureServer:
    def __init__(
        self,
        root: Path,
        *,
        tls: bool = False,
        certfile: Path | None = None,
        keyfile: Path | None = None,
        username: str = "radiust",
        password: str = "secret",
        stall_retr: bool = False,
        break_retr: bool = False,
        transient_list_failures: int = 0,
        log_file: Path | None = None,
    ) -> None:
        self.root = root
        self.tls = tls
        self.username = username
        self.password = password
        self.stall_retr = stall_retr
        self.break_retr = break_retr
        self.transient_list_failures = transient_list_failures
        self._transient_lock = threading.Lock()
        self.log_file = log_file
        self._listener = socket.socket()
        self._listener.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        self._listener.bind(("127.0.0.1", 0))
        self._listener.listen()
        self._stop = threading.Event()
        self._thread: threading.Thread | None = None
        self.ssl_context: ssl.SSLContext | None = None
        if tls:
            if certfile is None or keyfile is None:
                raise ValueError("TLS fixture server requires certfile and keyfile")
            context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
            context.load_cert_chain(certfile, keyfile)
            self.ssl_context = context

    @property
    def port(self) -> int:
        return int(self._listener.getsockname()[1])

    def start(self) -> None:
        self._thread = threading.Thread(target=self._serve, daemon=True)
        self._thread.start()

    def close(self) -> None:
        self._stop.set()
        with suppress(OSError):
            self._listener.close()
        if self._thread is not None:
            self._thread.join(timeout=2)

    def _serve(self) -> None:
        self._listener.settimeout(0.2)
        while not self._stop.is_set():
            try:
                conn, _ = self._listener.accept()
            except TimeoutError:
                continue
            except OSError:
                return
            threading.Thread(target=self._handle, args=(conn,), daemon=True).start()

    @staticmethod
    def _send(conn: socket.socket, line: str) -> None:
        conn.sendall((line + "\r\n").encode())

    @staticmethod
    def _readline(conn: socket.socket) -> str | None:
        data = bytearray()
        while True:
            chunk = conn.recv(1)
            if not chunk:
                return None
            data += chunk
            if data.endswith(b"\n"):
                return data.decode(errors="replace").strip()

    @staticmethod
    def _close_data(conn: socket.socket) -> None:
        if isinstance(conn, ssl.SSLSocket):
            with suppress(OSError, ssl.SSLError):
                plain = conn.unwrap()
                plain.close()
                return
        with suppress(OSError):
            conn.close()

    def _handle(self, conn: socket.socket) -> None:
        passive: socket.socket | None = None
        active_data: socket.socket | None = None
        protected_data = False
        logged_in = False
        try:
            self._send(conn, "220 radiust test FTP ready")
            while True:
                line = self._readline(conn)
                if line is None:
                    return
                command, _, argument = line.partition(" ")
                command = command.upper()
                if self.log_file is not None:
                    with self.log_file.open("a", encoding="utf-8") as log:
                        log.write(command + "\n")
                if command == "AUTH" and argument.upper() == "TLS" and self.ssl_context:
                    self._send(conn, "234 AUTH TLS successful")
                    conn = self.ssl_context.wrap_socket(conn, server_side=True)
                elif command == "USER":
                    self._send(conn, "331 password required")
                elif command == "PASS":
                    logged_in = argument == self.password
                    self._send(conn, "230 logged in" if logged_in else "530 login incorrect")
                elif command in {"TYPE", "PBSZ"}:
                    self._send(conn, "200 ok")
                elif command == "PROT":
                    protected_data = argument.upper() == "P"
                    self._send(conn, "200 protection set")
                elif not logged_in and command not in {"QUIT"}:
                    self._send(conn, "530 login required")
                elif command in {"PASV", "EPSV"}:
                    if passive is not None:
                        passive.close()
                    passive = socket.socket()
                    passive.bind(("127.0.0.1", 0))
                    passive.listen(1)
                    port = int(passive.getsockname()[1])
                    if command == "EPSV":
                        self._send(conn, f"229 Entering Extended Passive Mode (|||{port}|)")
                    else:
                        self._send(conn, f"227 Entering Passive Mode (127,0,0,1,{port // 256},{port % 256})")
                elif command in {"LIST", "MLSD", "NLST"}:
                    with self._transient_lock:
                        if self.transient_list_failures > 0:
                            self.transient_list_failures -= 1
                            self._send(conn, "450 temporary listing failure")
                            continue
                    if passive is None:
                        self._send(conn, "425 use PASV first")
                        continue
                    self._send(conn, "150 opening data connection")
                    data_conn, _ = passive.accept()
                    if protected_data and self.ssl_context:
                        data_conn = self.ssl_context.wrap_socket(data_conn, server_side=True)
                    entries = []
                    for path in sorted(self.root.iterdir()):
                        if path.is_file():
                            if command == "NLST":
                                entries.append(f"{path.name}\r\n")
                            elif command == "MLSD":
                                entries.append(f"type=file;size={path.stat().st_size}; {path.name}\r\n")
                            else:
                                entries.append(
                                    f"-rw-r--r-- 1 owner group {path.stat().st_size} Jan 01 00:00 {path.name}\r\n"
                                )
                    data_conn.sendall("".join(entries).encode())
                    self._close_data(data_conn)
                    passive.close()
                    passive = None
                    self._send(conn, "226 transfer complete")
                elif command == "SIZE":
                    path = self.root / Path(argument).name
                    if not path.is_file():
                        self._send(conn, "550 not found")
                    else:
                        self._send(conn, f"213 {path.stat().st_size}")
                elif command == "RETR":
                    path = self.root / Path(argument).name
                    if not path.is_file() or passive is None:
                        self._send(conn, "550 not found")
                        continue
                    self._send(conn, "150 opening data connection")
                    data_conn, _ = passive.accept()
                    if protected_data and self.ssl_context:
                        data_conn = self.ssl_context.wrap_socket(data_conn, server_side=True)
                    if self.stall_retr:
                        active_data = data_conn
                        passive.close()
                        passive = None
                        continue
                    if self.break_retr:
                        self._close_data(data_conn)
                        passive.close()
                        passive = None
                        conn.close()
                        return
                    else:
                        data_conn.sendall(path.read_bytes())
                    self._close_data(data_conn)
                    passive.close()
                    passive = None
                    self._send(conn, "226 transfer complete")
                elif command == "ABOR":
                    if active_data is not None:
                        self._close_data(active_data)
                        active_data = None
                    self._send(conn, "426 transfer aborted")
                    self._send(conn, "226 abort successful")
                elif command == "QUIT":
                    self._send(conn, "221 bye")
                    return
                else:
                    self._send(conn, "502 unsupported")
        except (ConnectionError, OSError, ssl.SSLError):
            return
        finally:
            if passive is not None:
                passive.close()
            if active_data is not None:
                self._close_data(active_data)
            with suppress(OSError):
                conn.close()


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--root", type=Path, required=True)
    parser.add_argument("--tls", action="store_true")
    parser.add_argument("--certfile", type=Path)
    parser.add_argument("--keyfile", type=Path)
    parser.add_argument("--stall-retr", action="store_true")
    parser.add_argument("--break-retr", action="store_true")
    parser.add_argument("--transient-list-failures", type=int, default=0)
    parser.add_argument("--log-file", type=Path)
    args = parser.parse_args()
    server = FtpFixtureServer(
        args.root,
        tls=args.tls,
        certfile=args.certfile,
        keyfile=args.keyfile,
        stall_retr=args.stall_retr,
        break_retr=args.break_retr,
        transient_list_failures=args.transient_list_failures,
        log_file=args.log_file,
    )
    server.start()
    print(json.dumps({"port": server.port}), flush=True)
    try:
        while True:
            time.sleep(1)
    except KeyboardInterrupt:
        pass
    finally:
        server.close()


if __name__ == "__main__":
    main()
