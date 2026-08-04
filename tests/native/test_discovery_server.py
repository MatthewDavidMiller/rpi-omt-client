#!/usr/bin/env python3
"""Exercise bounded central OMT discovery without hardware or multicast."""

from __future__ import annotations

import json
import os
import socket
import struct
import subprocess
import sys
import tempfile
import threading
from pathlib import Path


def serve(listener: socket.socket) -> None:
    connection, _ = listener.accept()
    with connection:
        header = connection.recv(16)
        if len(header) != 16:
            return
        data_length = struct.unpack_from("<I", header, 12)[0]
        remaining = data_length
        while remaining:
            received = connection.recv(remaining)
            if not received:
                return
            remaining -= len(received)
        xml = (
            b"<OMTAddress><Name>STUDIO (Camera &amp; One)</Name><Port>6400</Port>"
            b"<Addresses><IPAddress>192.0.2.10</IPAddress></Addresses></OMTAddress>"
        )
        connection.sendall(struct.pack("<BBqHI", 1, 1, 0, 0, len(xml)) + xml)


def main() -> int:
    receiver = Path(sys.argv[1])
    try:
        listener_context = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    except PermissionError:
        print("SKIP: sandbox forbids loopback sockets")
        return 77
    with listener_context as listener:
        listener.bind(("127.0.0.1", 0))
        listener.listen(1)
        port = listener.getsockname()[1]
        worker = threading.Thread(target=serve, args=(listener,), daemon=True)
        worker.start()
        with tempfile.TemporaryDirectory() as temporary:
            settings = Path(temporary) / "settings.xml"
            settings.write_text(
                f"<Settings><DiscoveryServer>omt://127.0.0.1:{port}</DiscoveryServer></Settings>\n",
                encoding="utf-8",
            )
            environment = os.environ.copy()
            environment["OMT_STORAGE_PATH"] = temporary
            result = subprocess.run(
                [str(receiver), "discover", "--wait-ms", "500", "--json"],
                check=True,
                capture_output=True,
                text=True,
                env=environment,
                timeout=5,
            )
        worker.join(timeout=2)
    document = json.loads(result.stdout)
    assert document == [
        {
            "name": "STUDIO (Camera & One)",
            "target": "STUDIO (Camera & One)",
            "kind": "discovered",
        }
    ]
    print("native discovery-server contract passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
