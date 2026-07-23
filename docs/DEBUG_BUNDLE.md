# Debug Bundle

The authenticated Diagnostics page creates a bounded ZIP named
`omt-debug-<UTC>.zip`. It contains:

- `version.txt`
- `runtime.txt`
- `discovery.json`
- `controller-status.txt`
- `playback-status.json`
- `omt-settings.xml`
- `runtime-sha256.manifest`
- `host-debug.txt`

Missing, unsafe, or oversized inputs are represented by an `unavailable`
record. Commands have fixed argument shapes and timeouts. The bundle may reveal
source names, network addresses, device details, and configuration, so inspect
it before sharing.

Host diagnostics are collected by the root-owned `host-debug.sh` through a
pre-created trigger file. The Web container cannot replace the trigger’s
directory or select arbitrary host commands.
