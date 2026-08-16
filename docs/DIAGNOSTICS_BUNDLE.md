# Diagnostics Bundle

The authenticated Diagnostics page creates a bounded ZIP named
`omt-diagnostics-<UTC>.zip`. It contains:

- `version.txt`
- `runtime-settings.txt`
- `runtime.txt`
- `discovery.json`
- `controller-status.txt`
- `current-target-receive-probe.json` — always a JSON document. A successful
  probe is the receiver's JSON object; skipped or failed probes are
  `{"ok":false,"error":"..."}` so the member never contains plain text.
- `playback-status.json`
- `omt-settings.xml`
- `runtime-sha256.manifest`
- `host-report.txt`
- `host-network-pcap.txt`
- optionally, `host-network.pcap`

Missing, unsafe, or oversized inputs are represented by an `unavailable`
record. Commands have fixed argument shapes and timeouts. The controller is
asked for its status once per bundle, so `runtime.txt` and
`controller-status.txt` always report the same observation rather than two that
may disagree.

The bundle may reveal source names, network addresses, device details,
configuration, and—when explicitly selected—raw network packets, so inspect it
before sharing.

`host-report.txt` carries both what the HDMI sink advertises and what the
streams are doing: the connector's EDID and mode list, and the ALSA playback
substream's `status`, `hw_params`, and `sw_params`. The last three are what
make an underrun report checkable — the ELD alone says what the sink accepts,
not whether the stream ran dry.

Host diagnostics are collected by the root-owned
`deploy/host/host-diagnostics.sh`. The Web process writes a bounded versioned
request with a random request ID and `capture_pcap=0|1` to a pre-created
fixed-inode channel. It starts container-side checks immediately and accepts
only a stable bounded host report carrying the same request ID.

That report's header is composed after collection, so its `status` describes the
run that actually happened: `complete` when every section ran, and `partial`
when the host wall budget (`OMT_DIAGNOSTICS_HOST_BUDGET_SECONDS`) ran out and
later sections were skipped. Both are accepted; anything else is rejected.

Raw PCAP is unchecked by default. An unchecked request removes stale capture
output and never starts the unfiltered capture. A checked request retains the
8 MiB cap; before adding the PCAP to the in-memory archive, the Web service
validates metadata version, request ID, status, declared size, magic, stable
inode, and SHA-256. Capture metadata is included for both choices.
Text members use ZIP deflate compression. The validated PCAP member is stored
without recompression because packet data compresses poorly and deflating up to
8 MiB would spend the Pi's CPU inside the fixed Web request budget.
