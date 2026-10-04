#!/usr/bin/env python3
# Copyright (c) 2026 Matthew David Miller
# SPDX-License-Identifier: MIT
"""Interop test for the receiver's first-party D-Bus client.

A private dbus-daemon stands in for the host's system bus, and a fake
org.freedesktop.Avahi service answers ServiceBrowserNew/ServiceResolverNew the
way avahi-daemon does: the browser and resolver signals are addressed to the
client that created them rather than broadcast, which is why the receiver needs
no match rule. The receiver's `discover` output is then checked against what the
fake announced, including a withdrawn service, an undiallable link-local
address, a name the shared grammar refuses, and that every browser and resolver
object is freed afterwards.

Usage: test_mdns_dbus.py /path/to/omt-receiver
Requires dbus-daemon, dbus-python, and PyGObject.
"""

import json
import os
import subprocess
import sys
import tempfile
import time

import dbus
import dbus.lowlevel
import dbus.mainloop.glib
import dbus.service
from gi.repository import GLib

SERVICE = "org.freedesktop.Avahi"
SERVER = "org.freedesktop.Avahi.Server"
BROWSER = "org.freedesktop.Avahi.ServiceBrowser"
RESOLVER = "org.freedesktop.Avahi.ServiceResolver"

# name -> (address, port); None marks a service that is announced and then
# withdrawn before it resolves.
SERVICES = {
    "Camera One": ("192.0.2.10", 6400),
    "Studio (B)": ("2001:db8::7", 6401),
    "Link Local": ("fe80::1", 6402),
    "bad‮name": ("192.0.2.11", 6403),
    "Withdrawn": None,
}
EXPECTED = [
    {"name": "Camera One", "target": "Camera One", "kind": "discovered"},
    {"name": "Studio (B)", "target": "Studio (B)", "kind": "discovered"},
]


class Bus:
    """A private bus daemon, torn down on exit."""

    def __init__(self):
        self.dir = tempfile.mkdtemp(prefix="omt-dbus-")
        socket = os.path.join(self.dir, "bus")
        config = os.path.join(self.dir, "bus.conf")
        with open(config, "w", encoding="utf-8") as handle:
            handle.write(
                f"""<!DOCTYPE busconfig PUBLIC "-//freedesktop//DTD D-BUS Bus Configuration 1.0//EN"
 "http://www.freedesktop.org/standards/dbus/1.0/busconfig.dtd">
<busconfig>
  <type>custom</type>
  <listen>unix:path={socket}</listen>
  <auth>EXTERNAL</auth>
  <policy context="default">
    <allow send_destination="*" eavesdrop="true"/>
    <allow eavesdrop="true"/>
    <allow own="*"/>
  </policy>
</busconfig>
"""
            )
        self.address = f"unix:path={socket}"
        self.process = subprocess.Popen(
            ["dbus-daemon", "--nofork", f"--config-file={config}", "--nopidfile"],
            stdout=subprocess.DEVNULL,
        )
        for _ in range(100):
            if os.path.exists(socket):
                return
            time.sleep(0.05)
        raise SystemExit("dbus-daemon did not start")

    def close(self):
        self.process.terminate()
        self.process.wait(timeout=5)


class Avahi(dbus.service.Object):  # type: ignore[misc]
    def __init__(self, bus):
        super().__init__(bus, "/")
        self.bus = bus
        self.next_id = 0
        self.freed = set()
        self.created = set()

    def unicast(self, path, interface, member, signature, args, destination):
        message = dbus.lowlevel.SignalMessage(path, interface, member)
        message.set_destination(destination)
        message.append(*args, signature=signature)
        self.bus.send_message(message)

    def new_path(self, kind):
        self.next_id += 1
        path = f"/Client1/{kind}{self.next_id}"
        self.created.add(path)
        FreeTarget(self.bus, path, kind, self.freed)
        return dbus.ObjectPath(path)

    @dbus.service.method(SERVER, in_signature="iissu", out_signature="o", sender_keyword="sender")
    def ServiceBrowserNew(self, interface, protocol, kind, domain, flags, sender=None):
        assert (interface, protocol, kind, domain, flags) == (-1, -1, "_omt._tcp", "", 0)
        path = self.new_path("ServiceBrowser")

        def announce():
            for name, value in SERVICES.items():
                self.unicast(
                    path,
                    BROWSER,
                    "ItemNew",
                    "iisssu",
                    (2, 0, name, "_omt._tcp", "local", 0),
                    sender,
                )
                if value is None:
                    self.unicast(
                        path,
                        BROWSER,
                        "ItemRemove",
                        "iisssu",
                        (2, 0, name, "_omt._tcp", "local", 0),
                        sender,
                    )
            # Something the receiver must ignore: another service type.
            self.unicast(
                path,
                BROWSER,
                "ItemNew",
                "iisssu",
                (2, 0, "Printer", "_ipp._tcp", "local", 0),
                sender,
            )
            return False

        # Avahi announces as soon as the browser exists, often before the
        # client has read the reply; idle_add reproduces that ordering race.
        GLib.idle_add(announce)
        return path

    @dbus.service.method(SERVER, in_signature="iisssiu", out_signature="o", sender_keyword="sender")
    def ServiceResolverNew(
        self, interface, protocol, name, kind, domain, aprotocol, flags, sender=None
    ):
        path = self.new_path("ServiceResolver")
        value = SERVICES.get(str(name))

        def found():
            if value is not None:
                address, port = value
                self.unicast(
                    path,
                    RESOLVER,
                    "Found",
                    "iissssisqaayu",
                    (
                        interface,
                        protocol,
                        name,
                        kind,
                        domain,
                        "host.local",
                        1 if ":" in address else 0,
                        address,
                        dbus.UInt16(port),
                        dbus.Array([dbus.ByteArray(b"txtvers=1")], signature="ay"),
                        0,
                    ),
                    sender,
                )
            return False

        GLib.idle_add(found)
        return path


class FreeTarget(dbus.service.Object):  # type: ignore[misc]
    """A browser or resolver object. Browser frees arrive as calls here;
    resolver frees are counted by the connection filter in main(), because
    dbus-python binds a method name to one interface per class."""

    def __init__(self, bus, path, _kind, freed):
        super().__init__(bus, path)
        self.path = path
        self.freed = freed

    @dbus.service.method(BROWSER)
    def Free(self):  # noqa: N802 - D-Bus method name
        self.freed.add(self.path)


def run_on_loop(loop, command, env, timeout):
    """Runs `command` while the GLib loop serves the fake daemon.

    dbus-python dispatches on the default main context, so the loop has to
    own the main thread; the receiver runs beside it and the loop stops when
    the receiver exits.
    """
    process = subprocess.Popen(
        command, env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True
    )
    deadline = time.time() + timeout

    def poll():
        if process.poll() is not None or time.time() > deadline:
            loop.quit()
            return False
        return True

    GLib.timeout_add(20, poll)
    loop.run()
    if process.poll() is None:
        process.kill()
        raise SystemExit(f"{command[1]} timed out")
    stdout, stderr = process.communicate()
    return process.returncode, stdout, stderr


def settle(loop, seconds):
    """Lets the loop deliver whatever the receiver sent before it exited."""
    GLib.timeout_add(int(seconds * 1000), loop.quit)
    loop.run()


def main():
    receiver = sys.argv[1]
    bus = Bus()
    try:
        dbus.mainloop.glib.DBusGMainLoop(set_as_default=True)
        loop = GLib.MainLoop()
        connection = dbus.bus.BusConnection(bus.address)
        connection.request_name(SERVICE)
        avahi = Avahi(connection)

        def watch(_bus, message):
            if message.get_member() == "Free" and message.get_interface() == RESOLVER:
                avahi.freed.add(message.get_path())
            # Anything else falls through to the exported objects.
            return dbus.lowlevel.HANDLER_RESULT_NOT_YET_HANDLED

        connection.add_message_filter(watch)
        env = dict(os.environ, DBUS_SYSTEM_BUS_ADDRESS=bus.address, OMT_STORAGE_PATH=bus.dir)
        code, stdout, stderr = run_on_loop(
            loop, [receiver, "discover", "--wait-ms", "1500", "--json"], env, 30
        )
        if code != 0:
            raise SystemExit(f"discover failed ({code}): {stderr}")
        found = json.loads(stdout)
        if found != EXPECTED:
            raise SystemExit(f"unexpected sources: {found}")
        # The frees carry no reply, so the loop needs a moment to see them.
        settle(loop, 0.5)
        if avahi.freed != avahi.created:
            raise SystemExit(f"objects not freed: {sorted(avahi.created - avahi.freed)}")

        # Without the daemon on the bus, discovery is empty rather than an error.
        connection.release_name(SERVICE)
        code, stdout, stderr = run_on_loop(
            loop, [receiver, "discover", "--wait-ms", "300", "--json"], env, 30
        )
        if code != 0 or json.loads(stdout) != []:
            raise SystemExit(f"discover without Avahi: {code} {stdout} {stderr}")
    finally:
        bus.close()
    print("mDNS D-Bus interop contracts passed")


if __name__ == "__main__":
    main()
