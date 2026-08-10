#!/usr/bin/env python3
"""A synthetic OMT sender, enough to drive a receiver's ingest path.

This older fixture intentionally sends filler rather than decodable VMX media;
it remains useful for narrow ingest and error-path tests. For a legitimate A/V
source backed by reference-encoded VMX frames, use the first-party Rust sender
documented in ``docs/OMT_TEST_SENDER.md``.

This speaks only the framing in `crates/omt-protocol`: a 16-byte frame header
followed by `data_length` bytes, which for video begin with the 32-byte video
header and for audio with the 24-byte audio header. The media itself is filler.
That is sufficient for `omt-receiver probe`, which parses headers and never
decodes, and it is enough to get `omt-receiver play` as far as selecting an
HDMI connector.

Run it on the appliance itself and point the receiver at loopback, which needs
no firewall change on either machine:

    docker exec -d <container> python3 omt_fake_sender.py --port 5960
    docker exec <container> omt-receiver probe \
        --target omt://127.0.0.1:5960 --timeout-ms 8000 --json

Note the `omt://` scheme: `parse_direct_target` requires it and reports a bare
`host:port` as "OMT target was not discovered", which reads like a network
fault rather than a malformed argument.
"""

from __future__ import annotations

import argparse
import socket
import struct
import threading
import time

FRAME_VIDEO = 2
FRAME_AUDIO = 4
VIDEO_HEADER_SIZE = 32
AUDIO_HEADER_SIZE = 24
CODEC_VMX1 = 0x31584D56
CODEC_FPA1 = 0x31415046


def frame_header(frame_type: int, timestamp: int, metadata_length: int, data_length: int) -> bytes:
    """The 16-byte fixed header. `data_length` counts the extended header."""
    return struct.pack("<BBqHI", 1, frame_type, timestamp, metadata_length, data_length)


def video_frame(width: int, height: int, fps_n: int, fps_d: int, timestamp: int) -> bytes:
    payload = b"\x00" * 1024
    header = struct.pack(
        "<iiiiifIi",
        CODEC_VMX1,
        width,
        height,
        fps_n,
        fps_d,
        16.0 / 9.0,
        0,
        709,
    )
    assert len(header) == VIDEO_HEADER_SIZE
    return frame_header(FRAME_VIDEO, timestamp, 0, len(header) + len(payload)) + header + payload


def audio_frame(sample_rate: int, channels: int, samples: int, timestamp: int) -> bytes:
    active = (1 << channels) - 1
    # parse_audio_header requires the payload to be exactly one 32-bit sample
    # per active channel per sample, so this length is not arbitrary.
    payload = b"\x00" * (bin(active).count("1") * samples * 4)
    # Four trailing reserved bytes: the parser demands a 24-byte header but
    # reads only the first 20.
    header = struct.pack("<iiiiI4x", CODEC_FPA1, sample_rate, samples, channels, active)
    assert len(header) == AUDIO_HEADER_SIZE
    return frame_header(FRAME_AUDIO, timestamp, 0, len(header) + len(payload)) + header + payload


def _drain(conn: socket.socket) -> None:
    """Swallow the receiver's OMTSubscribe frames; both media types are sent."""
    try:
        while conn.recv(4096):
            pass
    except OSError:
        pass


def serve(conn: socket.socket, args: argparse.Namespace) -> None:
    threading.Thread(target=_drain, args=(conn,), daemon=True).start()
    started = time.monotonic()
    try:
        while time.monotonic() - started < args.seconds:
            timestamp = int((time.monotonic() - started) * 10_000_000)
            conn.sendall(video_frame(args.width, args.height, args.fps, 1, timestamp))
            conn.sendall(audio_frame(48_000, 2, 1024, timestamp))
            time.sleep(1.0 / max(args.fps, 1))
    except OSError:
        pass
    finally:
        conn.close()


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--port", type=int, default=5960)
    parser.add_argument("--width", type=int, default=1920)
    parser.add_argument("--height", type=int, default=1080)
    parser.add_argument("--fps", type=int, default=60)
    parser.add_argument("--seconds", type=float, default=60.0)
    args = parser.parse_args()

    listener = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    listener.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    listener.bind(("0.0.0.0", args.port))  # noqa: S104
    listener.listen(8)
    listener.settimeout(1.0)
    print(
        f"synthetic OMT sender on :{args.port} offering {args.width}x{args.height}@{args.fps}",
        flush=True,
    )
    deadline = time.monotonic() + args.seconds
    while time.monotonic() < deadline:
        try:
            conn, peer = listener.accept()
        except TimeoutError:
            continue
        except OSError:
            break
        print(f"connection from {peer[0]}:{peer[1]}", flush=True)
        threading.Thread(target=serve, args=(conn, args), daemon=True).start()
    listener.close()


if __name__ == "__main__":
    main()
