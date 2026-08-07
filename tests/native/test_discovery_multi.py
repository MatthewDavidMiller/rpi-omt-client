#!/usr/bin/env python3
"""Exercise the central discovery server's multi-source announcement stream.

A discovery server announces each source as its own metadata frame and can
withdraw one with ``<Removed>True</Removed>``. The receiver has to collect the
whole stream for its wait budget, drop withdrawn sources, keep the last
announcement per name, sort the result, and reject anything that fails the
shared source-name and target grammars.
"""

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

ANNOUNCEMENTS = [
    # Announced, then withdrawn: must not appear.
    b"<OMTAddress><Name>Gone</Name><Port>6400</Port>"
    b"<Addresses><IPAddress>192.0.2.9</IPAddress></Addresses></OMTAddress>",
    # Out of alphabetical order: the result must be sorted.
    b"<OMTAddress><Name>Zulu</Name><Port>6402</Port>"
    b"<Addresses><IPAddress>192.0.2.12</IPAddress></Addresses></OMTAddress>",
    b"<OMTAddress><Name>Alpha</Name><Port>6400</Port>"
    b"<Addresses><IPAddress>192.0.2.10</IPAddress></Addresses></OMTAddress>",
    # A control character in the name fails source-name validation.
    b"<OMTAddress><Name>Bad\x01Name</Name><Port>6400</Port>"
    b"<Addresses><IPAddress>192.0.2.13</IPAddress></Addresses></OMTAddress>",
    # Port zero is not a valid direct target.
    b"<OMTAddress><Name>ZeroPort</Name><Port>0</Port>"
    b"<Addresses><IPAddress>192.0.2.14</IPAddress></Addresses></OMTAddress>",
    # Re-announced with a new port: the last announcement wins.
    b"<OMTAddress><Name>Alpha</Name><Port>6401</Port>"
    b"<Addresses><IPAddress>192.0.2.11</IPAddress></Addresses></OMTAddress>",
    b"<OMTAddress><Name>Gone</Name><Removed>True</Removed></OMTAddress>",
]


def serve(listener: socket.socket) -> None:
    connection, _ = listener.accept()
    with connection:
        header = connection.recv(16)
        if len(header) != 16:
            return
        remaining = struct.unpack_from("<I", header, 12)[0]
        while remaining:
            received = connection.recv(remaining)
            if not received:
                return
            remaining -= len(received)
        for xml in ANNOUNCEMENTS:
            connection.sendall(struct.pack("<BBqHI", 1, 1, 0, 0, len(xml)) + xml)
        # Hold the connection open so the receiver reads its whole budget.
        connection.recv(1)


def main() -> int:
    receiver = Path(sys.argv[1])
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as listener:
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
                [str(receiver), "discover", "--wait-ms", "800", "--json"],
                check=True,
                capture_output=True,
                text=True,
                env=environment,
                timeout=15,
            )
        worker.join(timeout=2)

    document = json.loads(result.stdout)
    expected = [
        {"name": "Alpha", "target": "Alpha", "kind": "discovered"},
        {"name": "Zulu", "target": "Zulu", "kind": "discovered"},
    ]
    if document != expected:
        print(f"FAIL: discovery returned {document}, expected {expected}")
        return 1
    print("multi-source discovery contracts passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
