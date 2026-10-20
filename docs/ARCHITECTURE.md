# Architecture

The appliance is a clean, bounded OMT implementation in Rust, with a
decode-only VMX1 port and OpenSSL as its only third-party library; the
workspace depends on no crate outside `crates/`.

```text
OMT network
  └─ discovery/receive transport
       └─ omt-receiver
            ├─ dependency-free parsing/status core
            ├─ VMX decoder (libvmx)
            ├─ DRM/KMS HDMI presenter
            ├─ ALSA audio worker
            └─ atomic playback status

HTTPS operator
  └─ omt-web (poll loop + OpenSSL TLS)
       ├─ persistent authenticated sessions
       ├─ source/network/diagnostic services
       ├─ About
       └─ System reboot request

unprivileged container
  └─ fixed request file
       └─ root-owned OpenRC/inotify validator
            └─ /sbin/reboot
```

## Receiver

`crates/omt-receiver` composes Linux adapters around the audited, decode-only
decoder in `crates/omt-vmx`. `crates/omt-protocol` owns wire transport and
shared target validation; `crates/omt-receiver-core` owns format policy,
ceilings, and synchronized status projection; `crates/omt-receiver` owns CLI
parsing and HDMI connector
selection over the DRM sysfs tree.
`discover` emits bounded JSON, `probe` checks a direct OMT target, and `play`
owns receive, DRM, ALSA, hotplug, retry, and status publication. Discovered
names are NFC, have no control characters, and are at most 63 UTF-8 bytes.
Direct targets must be exact `omt://host:port` URIs. The validation path uses
no locale-dependent or unbounded parsing and enforces the Web layer's NFC
contract with Unicode normalization. Each announcement's name, withdrawal flag,
address, and port come off a single bounded XML pass rather than one reader per
field, and every rejection -- a duplicate of any requested tag, a document type
declaration, an unknown entity -- still refuses the whole document, so no field
can be read out of one another field's reader would have discarded.

The mDNS path speaks D-Bus itself: `crates/omt-receiver/src/dbus.rs` is a first-party wire
client -- SASL EXTERNAL over the system bus socket, little-endian marshalling
of only the signatures Avahi's interfaces use, and a 64 KiB message cap -- and
`crates/omt-receiver/src/mdns.rs` drives Avahi's browser and resolver over it. Each
signal body is read against its full signature: `ItemNew` and `ItemRemove`
carry six fields and `ServiceResolver.Found` carries eleven, and a body that
does not match is refused whole rather than read partway. Resolvers are freed
with no-reply calls and capped in number, so a busy network cannot grow the
browse without bound. A browse that finds nothing and a browse
that cannot parse anything are otherwise indistinguishable from the outside:
both are an empty list, which is why this survived alongside a working
transport.

`endpoint_from_parts` then refuses an address no connect can use, rather than
carrying it forward as a source that only fails once selected. Direct targets
use the same rule: IPv6 link-local, unspecified, multicast, and IPv4 broadcast
literals are rejected by `parse_direct_target`, so the dashboard cannot save a
URI that only fails at connect. A resolver answers once per address family, so a service on a dual-stack link produces an
unscoped `fe80::` record beside its usable one, and the endpoint map keeps
whichever arrived last. Reaching a link-local IPv6 address needs a zone index
that neither the OMT target grammar nor the operator's direct-target box can
express -- `Endpoint::resolve` already drops one from a host string -- so the
source appeared on the dashboard and then timed out. IPv4 link-local is
deliberately still accepted: a peer in that range is reached over the one link
it is on, with no extra addressing.

Connector selection tolerates a half-populated DRM tree: an unreadable
`status`, a missing `connector_id`, a zero id, or an absent `/dev/dri` node
disqualifies that card and the search continues to the next, because several
cards can expose the same connector name and the attached display may be behind
any of them.

The runtime is capped at 512 MiB and 64 processes on every board. At 1080p it
uses three DRM scanout buffers, one compressed video frame and at most
100 ms of audio in flight, bounded network frames, and a persistent pool of
VMX workers with 128 KiB stacks (created once per decoder, not per frame).
Each worker owns one slice of YUV scratch rather than every slice of the frame,
and bitstream readers grow to the loaded payload instead of memset-ing the
uncompressed worst case on every frame. Workers claim slices from a shared
counter rather than queued jobs, and the receiver reserves
only the additional bytes needed when a network payload grows, so neither jobs
nor a second full-size payload buffer are allocated in the steady-state frame
path. The HTTPS operator is a single-threaded poll loop. The 1 GiB Pi 4
Model B is the memory-design floor and is inside the supported matrix at that
cap.

Memory is not what separates the boards; decode throughput is. VMX is decoded in
software on three of the four cores every supported board has, and a 1.5 GHz
Cortex-A72 is not a 2.4 GHz Cortex-A76. So each board carries a decode ceiling
from `deploy/lib/board-profile.sh`, which the installer resolves once and passes
to the receiver as `--video-ceiling`. A ceiling is a list of shapes and a frame
is admitted when it fits inside any one of them, which is how a Pi 4 takes
either 1080p30 or 720p60 without a pixel-rate budget nobody could explain to an
operator. The ceiling is policy layered above `omt-protocol`'s absolute
1920x1080@60 limit, which still bounds every allocation, so no ceiling and no
operator override can change what the decoder is sized for.

Both ceilings are measured rather than reasoned. The decode benchmark run on
the hardware put the three-worker pool -- the row that decides a tier -- at
6.5 ms per 1080p gradient frame on the Pi 5 against a 16.7 ms budget, and
26.4 ms on the Pi 4 against a 33.3 ms one, with the pre-2.0 decoder. The
current decoder is measured by `crates/omt-vmx/src/bin/bench_vmx.rs`
(`cargo run --release -p omt-vmx --bin bench_vmx`). On a Pi 4 Model B at
1.8 GHz it puts the same row at 3.5 ms per frame on the shipping YCbCr path
(4.8 ms to BGRX), median of five interleaved rounds with no throttling. The
Pi 5 has not been remeasured since, so its margin is still the earlier
figure. Bit-exactness is proven under emulation on every gate run and was
confirmed on the Pi 4 itself; emulation says nothing about speed.

The colour conversion has an AArch64 kernel for the same reason the inverse DCT
does. Once the entropy decode is spread over the pool, packing 1080p into the
8 MiB of BGRX a frame of scanout needs is what is left, and the portable kernel
spends four one-byte stores per pixel doing it. `vst4q_u8` interleaves the four
channels in the store unit instead, sixteen pixels at a time. On the Pi 5 that
takes the conversion-dominated flat vector from 5.7 ms to 4.0 ms per frame; on
the Pi 4, whose memory system rather than its arithmetic is the limit here, it
is worth about five percent on the gradient vector and nothing on the flat one.
Both kernels are checked against each other lane for lane, and the committed
conformance vectors still decode bit-exactly against the reference decoder on
both boards.

The two AArch64 kernels are hand-written assembly
(`crates/omt-vmx/asm/idct_aarch64.s`, `convert_aarch64.s`, assembled by
`global_asm!`) rather than intrinsics, because the instruction choices
that matter are ones the compiler will not make from the portable arithmetic.
The row pass is eight multiply-by-element accumulations per row against
de-interleaved tables rather than a shuffle-heavy pairwise dot product; a
multiply by an even constant is one `sqdmulh` by half of it, which is exact;
the conversion rounds with one `sqrshrun`, which equals the scalar rounding
only because no BT.601 or BT.709 channel can come near the 16-bit limit. Each
of those is a claim about every input, so the suites prove it: the conversion
is compared on every (Y, U, V) under both coefficient sets, and the transform
on random, saturating, and single-coefficient blocks under every quality.
The transform also skips the row passes of empty rows. The entropy decoder
knows the last zig-zag position it wrote, which bounds the rows that can hold a
coefficient, and a zero row's pass is exactly zero, so blocks whose
coefficients end in the first four rows -- most blocks of most pictures -- run
only those rows' passes. That is worth about a fifth of a 1080p gradient
decode on a Pi 4 and costs a dense block one compare. When only the first two rows hold
coefficients -- by far the commonest case -- the column pass shrinks as well:
a saturating add or subtract of zero is the identity, so substituting the six
zero rows through it leaves eight outputs of two inputs, about half the
instructions, and only those two rows are loaded. That took another eighth off
a 1080p gradient decode on the shipping YCbCr path.

What the assembly gives up is the sanitizers, Miri, and the fuzzer's
instrumentation, which see only Rust.
That is acceptable for these two kernels and no others: each reads one fixed
64-coefficient block or one row whose bounds `decode_slice` checks before the
call, and neither ever sees a byte of the stream. The entropy decoder, which
does, stays in Rust under ASan, Miri, and `fuzz_vmx`. Its speed comes from a
twelve-bit lookahead table (`ac_lookahead.rs`, generated) that resolves every
short code in one step. It is consulted only when the whole code is already in
the bit window, so the reader ends exactly where the bit-by-bit path would,
and a differential test holds the two paths to the same pixels, reader
position, and verdict on damaged streams. Each table entry stores its coefficient
unconditionally, a run of zeros storing 0 where 0 already is, so the innermost
loop has no branch on what kind of code it read, which detailed pictures make
unpredictable. And the readers are decoded from a local copy: the build does
not assume strict aliasing, so readers reached through the caller's pointer
were reloaded from memory after every coefficient stored. Together those made
the entropy stage of a dense picture 1.7x faster on a Pi 4.

The kernels emit BTI landing pads and the GNU property note for branch
protection, so they never weaken a binary that has it. The appliance's
binaries do not have it today: Alpine's musl start files carry no property
note, and the linker drops the feature for the whole binary when any input
lacks one. Return-address signing, which needs no note, is in effect.

Playback supports either HDMI connector: both supported boards have two. A
missing, unreadable, or half-populated DRM tree reads as no display, so the
play loop reports `waiting-for-hdmi` and retries instead of exiting. Frames
above the board's ceiling are reported as `unsupported-format`. Interlaced
input is presented progressively without deinterlacing.

The board's ceiling and the display's mode list are separate limits and are
answered separately. The ceiling says what this SoC can decode, and video above
it is refused. The mode list only says what timings the sink advertises, and a
sender's format is often not among them: HDMI sinks advertise what they were
built to show, and a set that stops at 720p is common on small panels and on
TVs whose larger timings the kernel prunes from an otherwise valid EDID.
Selection therefore prefers a mode at the video's own size — the decoder writes
that scanout buffer directly — and otherwise takes the largest usable mode the
frame reduces into, or the smallest one it has to be enlarged into, and
resamples each frame into it with the aspect ratio preserved and black bars
around it. Interlaced modes, modes whose timings give no refresh rate, and
modes outside the fixed 1920x1080 envelope are never selected. Only a display
offering no usable mode at all is now reported as `unsupported-format`, and the
running status names both sizes so a resample is visible rather than silent.

Among modes of the chosen size, a source below 40 fps goes out at twice its
rate when the display offers it: 30 fps at 60 Hz, 25 at 50, 29.97 at 59.94.
Each frame is then shown for exactly two refreshes, so motion is as even as at
the source rate, but a frame waits half as long for the vblank that puts it on
screen and is scanned onto the panel in half the time. Measured on a Pi 4 with
1080p30 over Wi-Fi, the flip wait fell from 18.1 ms to 9.0 ms and the mean
time from send to vblank from 55.3 ms to 46.6 ms; the halved scanout takes
about another 8 ms off the average pixel. Otherwise selection takes the rate
asked for, its nearest whole rate, then 60 Hz, then the fastest on offer.

On the Pi 4 and Pi 5 the colour conversion and that resample are not done on
the CPU at all. Both boards scan out through the HVS, which converts YCbCr and
scales planes in hardware, so the receiver hands it what the decoder already
holds: planar YCbCr 4:2:2 (`DRM_FORMAT_YUV422`, three planes in one dumb
buffer), each plane row copied once from slice scratch with no conversion, at
half the bytes of BGRX. The primary plane is set through atomic mode-setting
with the stream's `COLOR_ENCODING` (BT.601 or BT.709) and limited
`COLOR_RANGE`, its source rectangle the video and its destination the fitted
placement, and each flip is an atomic commit that changes only the plane's
framebuffer. On a Pi 4 at 1.5 GHz that took the receiver from 41.6% to 27.8% of
one core playing 1080p30 -- the Pi 4's multi-worker decode is limited by memory
bandwidth more than arithmetic, and the conversion's 8 MiB of BGRX a frame was
most of the traffic.

At the video's own size the plane uses nearest-neighbour filtering, which
repeats chroma across each pixel pair exactly as the software conversion does;
checked on the hardware through its writeback connector against
`vmx_decode_bgrx`, every conformance vector matched within 2 levels per channel
on smooth content and 4 on noise -- matrix rounding, not a different picture.
The hardware's default filter interpolates chroma instead, which softens sharp
colour edges, so it is used only for a scaled mode, where it is the better
resample. Every step of that path is optional: a kernel without atomic
mode-setting, a plane without `YU16` or the colour properties, or a test-only
commit the hardware refuses sends the configuration down the BGRX path below,
and the receiver log names which path each configuration took and why.

The BGRX fallback's resample is nearest-neighbour with pixel-centre sampling,
the filter a decode-bound board has room for. It runs inside the decode
(the decoder's placed BGRX output) rather than after it: `scale.rs` builds one table of
source columns and one of source rows, and each worker converts only the source
rows its slices contribute, gathers each into a destination row, and stores it
into the scanout buffer once. The resample therefore runs on the whole worker
pool, a downscale never converts rows nobody sees, an enlargement converts each
source row once however many destination rows repeat it, and there is no
intermediate frame. On a Pi 4 that took a 1080p source into a 1280x720 mode
from 12.8 ms to 7.1 ms a frame and into a 3840x2160 mode from 37.4 ms -- over
the 30 fps interval -- to 17.0 ms. Every table index and the destination extent
are checked before any worker runs; pixels outside the placed area, the black
bars, are never written.

HDMI audio is resolved rather than assumed. The Pi 4 and Pi 5 register one ALSA
card per output, `vc4hdmi0` and `vc4hdmi1`. The receiver reads
`/sys/class/sound/card*/id` and takes the indexed card matching the selected
connector, falling back to an unindexed `vc4hdmi` when that is the only card
present. Deriving the ALSA device name from the connector alone rather than
reading it back from sysfs is what let HDMI audio fail silently while video
kept playing.

The PCM's software timing is set explicitly rather than left at ALSA's
defaults. `snd_pcm_hw_params` leaves `start_threshold` at a single frame, so
the device begins playing on the first write with nothing queued behind it and
the next late audio frame is an underrun the operator hears. The receiver sets
a 100 ms start threshold against a 240 ms ring, so playback only begins once
there is a cushion, and because underrun recovery re-prepares the device that
cushion is rebuilt after every underrun instead of restarting empty into the
next one. The ring is capacity, not latency: the appliance's link is Wi-Fi
carrying this session's own video, which delivers audio in bursts, and a ring
only as deep as a couple of bursts runs dry between them. The 100 ms threshold
is what sets the audio latency, chosen inside the ITU-R BT.1359 lip-sync
tolerance and partly offset by the video path's own decode and scanout delay.
Buffer and period sizes are read back from the device after `hw_params` rather
than derived from the times requested, because the device refines both and the
start threshold has to be expressed in the frames it actually chose.

The threshold is where audio latency starts, not where it stays. Sender and
sink run at the same rate, so audio banked by a burst after a Wi-Fi stall is
never drained: the ring is topped up toward 160 ms and keeps whatever it got.
Measured on a Pi 4 over 5 GHz Wi-Fi with HDMI audio playing, audio left the
speaker 106 ms after its fastest delivery in calm periods and 143 ms after a
run of bursts. Shedding that surplus was tried and rejected: capping the ring
at the 100 ms threshold, with the queue in front of it shedding past 40 ms, cut
the mean to 95 ms but turned 0 underruns in nine minutes into 4, and an 80 ms
cap gave 84 ms with 22. On that link the delay audio blocks see is 30 ms at the
99th percentile and 60 to 100 ms at the 99.9th, so the extra depth after a burst
is what was absorbing the next one.

Audio the sender dropped is concealed with silence rather than closed up. vMix
drops audio, not only video, when its send pool fills, and the frames after the
hole carry timestamps that say so. Writing them straight behind the frames
before the hole took that much depth out of the ring for good, because nothing
at an equal send and play rate ever puts it back. Measured on a Pi 5 over
5 GHz Wi-Fi against vMix sending 1080p30 (about 87 Mb/s of video), the sender
dropped about a second of audio a minute, the cushion bled away, and ordinary
50 ms jitter then ran the ring dry: 39 underruns in two minutes. The worker
now writes silence for the missing span, so later audio keeps its place in
time. It inserts silence only for a hole of at least three quarters of a
frame, so a sender whose timestamps jitter is never padded. It inserts nothing
for a jump back or past a second, which is a new timeline, nothing for a ring
that already underran, because the underrun spent that time, and nothing for
the gap the backlog shed below just made. The silence never exceeds what the
ring can take alongside the frame.

A playing stream is never left to run dry. Over HDMI an underrun is not a short
gap: ALSA stops the stream, recovery re-prepares it, and the sink loses audio
lock and has to find it again, which a television plays as a loud crack and
then a mute. On the same Pi 5 the stream restarted 16 to 131 times every two
minutes, under both this receiver and 2.0.4, and the operator heard no program
audio at all, only cracks. Prefill was never the problem; the restart was. So
once the device is playing, the worker wakes once a period even with nothing
queued, and when the ring falls below 60 ms with nothing to play it writes a
period of silence. The audio that then arrives late plays behind that silence,
which rebuilds the cushion the next late burst needs; the 160 ms fill target
and the 100 ms backlog shed bound the latency that adds, as a restart's prefill
did. On the same link that took the stream from dozens of restarts every two
minutes to none, leaving five brief silences. A genuine underrun, a worker that
missed its wake by more than two periods, still recovers and is counted. The
wake has to be kept even while an audio frame is half arrived: a frame that has
begun is otherwise finished on the six-second body budget, and a Wi-Fi stall
mid-frame held the worker in that read while the ring ran dry. Frames are
resumable, so the worker's read ends at its own deadline and leaves the rest of
the frame for the next one.

Recovered underruns and starvations are counted for the life of the audio session and named in
the running status. An underrun is a gap in the sound, so reporting the count
is what separates a starved sink from a link dropping the audio stream
outright — the two are indistinguishable in the room. The document carries one
detail, and while video plays it is the video session's, so the count is
appended there; it used to be published only in the audio worker's own detail,
which the document shows only once audio has failed, so no operator ever saw
it.

The PCM is opened non-blocking, so a momentarily full ring buffer answers a
write with `EAGAIN`. That is back-pressure and it happens in steady playback
whenever the writer runs ahead of the sink; `snd_pcm_recover` handles underrun
and suspend and hands `EAGAIN` straight back, so routing it there reported a
working HDMI sink as lost and dropped playback to `degraded` on both the Pi 4
and the Pi 5. It now waits for room on the same bounded budget a device that
accepts nothing gets, which still ends the audio session for a sink that has
genuinely vanished.

The receive slice bounds how long a loop waits for a frame to *begin*, not how
long a frame may take. Those were once the same bound, and a 1080p VMX payload
is around 200 KiB — tens of milliseconds of wire time on a link already
carrying this session's own video. A frame whose header landed near the end of
a slice therefore ran out of budget mid-payload; because a half-read frame
cannot be resumed, the channel closed, the session ended, and video, audio, and
the DRM output all restarted behind the retry backoff. Nothing had stalled. The
budget was being measured from the wrong instant, and the appliance reported it
as `OMT frame was truncated by a timeout` several times a minute on 2.4 GHz.
The first byte consumed now switches the read onto its own frame budget of six
seconds, which still ends a sender that genuinely stops talking mid-frame and
covers the 3.5 s Wi-Fi stalls measured against vMix. `control-omt.sh` waits
eight seconds after SIGTERM so that budget cannot outlive a shutdown.

A frame read is also resumable: what has arrived of a frame stays in the
channel between calls, so the budget is the only thing that ends a slow frame.
That is what lets the video loop take only frames that are already complete.
After each frame it collects whatever else the socket holds in full, without
waiting, and hands the newest straight to the display; a frame whose header
has landed but whose body is still on the air is left to finish behind the
decode. The loop used to wait a millisecond for the next frame's header and
then commit to reading that frame to the end. On a link carrying frames back
to back that wait always found one, so a complete frame was replaced by one
still arriving, again and again. Measured on a Pi 4 over 5 GHz Wi-Fi, against
a sender that keeps libomtnet's four sends in flight and drops past them, with
both builds offered 1080p30 and 1080p60 modes: alternating 268 KB and 18 KB
1080p30 frames (34 Mb/s), the old loop showed 49% of frames and the picture
on screen averaged 68.3 ms old at its vblank, against 99.5% and 61.2 ms now,
updated every 33 ms instead of every 68 ms. With every frame 268 KB (64 Mb/s,
more than that link carries) the old loop showed 3% of frames, about one a
second, and the picture averaged 850 ms old; it now shows 72%, the rest
dropped by the sender, at 79 ms.

TCP reads stay greedy: OMT requires the receiver never block when accepting
data. There is no playout buffer. An OMT sender keeps none, so frames held on
the Pi would only put the picture further behind it; decode speed, not a
cushion, is what keeps playback live. `crates/omt-receiver/src/handoff.rs` hands each
frame on as soon as its consumer can take it. Video is a single slot: a frame
is decoded and flipped as soon as it arrives, the display's refresh is the only
pacing, and a frame a newer one replaces before the display took it is dropped
and counted in the playing detail. Audio cannot skip ahead, so it queues in
arrival order and is paced by the HDMI audio device: the worker tops the ALSA
ring up to 160 ms whenever it falls below, so the sender's clock, the Pi's
clock, and the sink's clock cannot drift the ring dry, and a backlog past
100 ms -- a sender running ahead of the sink -- sheds its oldest frames. vMix
drops in-flight extras when its send pool fills, so after a stall the next
frames are live rather than a catch-up burst; the last picture stays on screen
meanwhile. Kernel
`SO_RCVBUF` is 8 MiB, matching libomtnet `NETWORK_RECEIVE_BUFFER`.

A closed video socket no longer tears down HDMI and audio with it. The play
loop publishes `retrying`, then reconnects to the endpoint this session already
resolved. Success keeps the last DRM frame scanning and the audio worker
running, counts the reconnect in the playing detail, and resumes; VMX carries no
inter-frame prediction, so the sender's next frame decodes on its own and there
is no keyframe to wait for and nothing to re-sync against the audio that kept
playing. An idle socket whose polling slice expires (`WouldBlock`) stays
connected and is not a reconnect.

That recovery is bounded, because the two failures it sits between look
identical for the first few milliseconds. A sender that restarted its socket is
back on the first attempt; a sender that accepts a connection and then drops it
— one at its subscriber limit, or shutting down — would otherwise fail a
reconnect as fast as the kernel completes a handshake, which is a spin on a core
and a flood of connections at the sender. So a session gets three consecutive
attempts, the first immediate and the rest behind a 250 ms escalating backoff,
each waiting one second rather than the three a session's first connect gets. A
refused connection spends an attempt like any other failure. Any frame that
arrives clears the count, so a link that drops occasionally is carried
indefinitely without audio ever breaking or the display going black. Exhausting
the budget returns to the outer loop, which is the only place that backs off and
asks discovery where the source is now — the case a reconnect to a remembered
address can never fix.

How much time that budget actually buys depends on how the endpoint fails, and
the two ends of the range are far apart. A closed port answers with a reset
instead of timing out, so all three attempts cost nothing but their backoffs:
measured on a Pi 4, a sender killed at t=0 reported the exhausted budget at
t=831 ms. Only a blackholed SYN, where each attempt waits its full second,
approaches the four-second ceiling. **A restarting sender therefore has about
eight tenths of a second to get its listener back up**; one that takes longer
gets a session rebuild rather than a reconnect. That is the intended trade, not
a shortfall: the whole cost of a longer budget is a frozen frame held longer,
and a rebuild is a blink. Widening the window to cover a slower sender restart
would make every unrecoverable case hold a stale picture for seconds first.

That budget covers only a socket this end can see close. A TCP connection can
also stay ESTABLISHED with a peer that is gone — a firewall or NAT that drops
the flow mid-stream, an access point that forgets the association, a sender
whose reset never arrives — and then every read returns `WouldBlock`, the
channel still calls itself connected, and the reconnect budget is never armed.
So a session that goes fifteen seconds without a video frame on a socket that
still claims to be connected fails and rebuilds, which is the only way back to
discovery. Without that bound such a session waits on the dead socket forever
while reporting `retrying`, which is the one outcome worse than a rebuild: the
appliance looks like it is recovering and never does. The audio worker keeps
the same bound, and needs it more — it publishes `running` only when a frame
arrives, so a silent socket would otherwise leave the last healthy document
standing over silence.

TCP packet loss does not flip VMX bits; it stalls or resets the socket. That
cuts both ways: a body the decoder rejects arrived intact, so it is a sender-side
glitch rather than link damage, and a glitch clears within a frame or two. Frame-
local damage — a bad slice bitstream, a bad stream table — is therefore a typed
skip that leaves the on-screen buffer alone and counts itself in the playing
detail, but only for twenty consecutive frames. Past that the stream is not
damaged, it is one this receiver cannot decode, and no further frame from the
same sender changes that; holding indefinitely would freeze the picture while
reporting healthy playback. A presented frame clears the run, so a stream losing
the occasional frame is tolerated for as long as it keeps recovering. Skips are
also refused before the first page flip of a configuration: there is nothing on
screen to hold but the buffer `set_crtc` scanned out.

Three decoder outcomes are deliberately not skippable. An interlaced envelope is
format policy and reports `unsupported-format`, which keeps the session rather
than restarting it in a loop. An empty body, and a frame whose `metadata_length`
overlaps its own video header, mean the sender and this receiver disagree about
frame layout — deterministic, and repeated on every following frame. Internal
decoder and DRM failures are not frame-local at all.

The audio worker reads on the same slice as the video loop. Its frames are a
fraction of a video frame's size, but a read that stalls after its first byte
cannot be resumed and ends the session, and on a link already carrying this
session's own video the smaller frame is no less likely to stall. The fifth of
the budget it used to get was not a smaller need; it was a fivefold better
chance of tearing audio down, which is what a two-board test over one Wi-Fi
radio produced repeatedly while video kept playing. The cost of the longer wait
is a recoverable ALSA underrun.

The direct probe divides its caller's bounded deadline between video and audio
instead of imposing a fixed 100 ms read slice. A compressed 1080p VMX frame is
small enough for that slice on loopback but not necessarily over a busy Pi
Wi-Fi link; consuming part of it and then timing out closes the video channel,
which previously made a healthy A/V sender report as audio-only.

The presenter returns a typed `Present` outcome, so the play loop distinguishes
three things: a stream this display cannot show, which keeps the session and
reports `unsupported-format`; frame-local VMX damage inside the skip budget,
which holds the last picture and keeps reporting `running`; and a failure of the
output itself, which ends the session and retries. None of the three is inferred
from the error text: a genuine
`drmModeSetCrtc` failure also mentions "mode", and treating it as recoverable
reconfigured the display, tearing down and reallocating three 1080p scanout
buffers, once per decoded frame. During playback, connector
hotplug state is sampled at a bounded 500 ms cadence instead of reopening two
sysfs attributes for every decoded frame. Unchanged video/audio events reuse
their status projection rather than allocating per frame; publication still
occurs immediately on change and at the 500 ms heartbeat. Audio failure
degrades playback while video continues.

The presenter decodes the next frame *before* it waits for the outstanding page
flip. DRM allows one flip per CRTC -- a legacy page flip on the BGRX path, an
atomic commit of the plane's framebuffer on the YCbCr one -- so with three surfaces the buffer being
decoded into is neither the one on screen nor the one queued, and the decode
overlaps the previous frame's scanout; waiting first left the decoder idle for
most of every frame interval and made the third buffer pointless. A format the
display can carry no mode for is remembered with the reason it was refused, so
an unsupported stream stops re-reading the connector's mode list at its own
frame rate. Dumb buffers and framebuffers are kernel objects that a dropped handle
does not release, so one path retires any pending flip and destroys both, for
reconfiguration and for shutdown alike. The card is opened non-blocking,
because DRM events arrive by reading it: on a blocking descriptor the wait for
a flip that a vanished display will never complete never returns, and the
500 ms flip timeout that ends such a session could not fire.

## Container and host boundary

The Alpine 3.23.5 runtime is read-only, drops all Linux capabilities, runs as
the `omt` user, retains Docker's default sensitive `/proc` and `/sys` path
confinement, and receives only DRM/ALSA devices, the OMT config volume, a
filtered Avahi D-Bus socket, diagnostics state, and the host-action directory.
The container cannot invoke OpenRC or write the directory containing host
action files. A minimal injected init reaps failed receiver descendants; PID,
memory, swap, shared-memory, file-descriptor, core-dump, and temporary-filesystem
limits bound the remaining process and storage surfaces.

The appliance OpenRC service waits up to 90 seconds for Docker's API
before invoking Compose. Alpine's supervised Docker service can report itself
started while dockerd is still initializing containerd; a 30-second wait lost
that race on a Pi 4 cold start and left the appliance stopped until an
operator restarted it.

The filtered Avahi proxy runs under the container's own UID, not as `nobody`.
`xdg-dbus-proxy` serves its socket through GDBus, whose default authorization
accepts an authenticated peer only when that peer's credentials name the
server's own user, so the group grant that reaches the socket is not what
decides whether a connection survives the D-Bus handshake. Running the proxy as
anyone else drops every connection from the receiver before its first method
call, and because discovery is best-effort by design the failure surfaced as an
empty source list returned instantly rather than as an error. The installer
already resolves the image-owned UID for the state files and now publishes it
to the proxy service as well.

Fresh diagnostics use a separate fixed-inode request channel. The Web process
writes a versioned nonce and `capture_pcap=0|1`, collects container-side data
while the root-owned oneshot runs, and accepts only a stable bounded host
report carrying that nonce. Raw capture is never started unless selected for
that download. Avahi proxy state, diagnostics, and host actions use separate
least-privilege bind mounts. A collection's working files -- each section, the
text captures, and the raw capture -- live in a private directory on `/run`, and
only the report, the capture metadata, and a publishable capture are written
to the SD-card exchange directory (see "Spare RAM and SD-card writes").

The shipped image contains no Python runtime, Rust or C/C++ toolchain, or C++
standard library. The receiver and Web service are stripped release binaries
of 600-750 KB each.

`host-reboot.sh` is installed root-owned. It accepts only a four-line
versioned reboot record from the pre-created mode-0600 request file. It checks
ownership, type, stable inode, age, future skew, replay, and cooldown; writes a
request-correlated result; then calls the fixed reboot command. No request
field can select a command or argument.

## Persistent state

The external `omt-config-v3` volume contains credentials, sessions,
`source_target.json`, OMT `settings.xml`, TLS material, and the receiver log.
The deployer can rotate the Web credential through a fixed privileged action
when the operator enables it. The
new value travels over SSH stdin, is hashed and atomically replaced inside the
unprivileged container, and the service is restarted. Sessions bind to the
password-file digest, so sessions created under the old credential become
invalid. Deploy leaves the existing credential in place unless that option is
selected.
Source state is one atomic schema-versioned record, not a pair of files. The
installer never migrates state from any other installation.

PBKDF2-SHA256 password records must contain a full 32-byte digest. Empty or
truncated digests are rejected at load time rather than accepted as a shorter
password comparison; valid existing PBKDF2 and legacy scrypt records still load.

Per-boot state is kept off that volume. The control lock, PID record, and
published playback status live in `$OMT_RUNTIME_DIR`, a size-capped tmpfs
mounted at `/run/omt`; the entrypoint owns a 0700 directory inside that
world-writable mount point. The receiver republishes status on every change and
then at a 500 ms heartbeat, including while discovery, HDMI, media, or retry
waits are in progress. Status publication uses a private, uniquely named stage
and an atomic replacement. The status directory is created once per process
rather than re-asserted on every publish: the entrypoint owns `/run/omt` and
creates it before the receiver starts, so a `create_dir_all` twice a second
forever bought nothing, and a directory that does vanish now fails the publish
audibly instead of being silently recreated. The Web consumer accepts fresh status only when its
target matches the current atomic source record, so a source change cannot
briefly present the previous receiver session as current. Keeping status on
the volume put a permanent write + fsync + rename load on SD-card-backed flash
for state that is meaningless after a restart.

## Deployment capsule

`deploy/manifest-v3.txt` defines manifest version 3: a bounded, variable-size
capsule with normalized nested paths. `deploy/transaction.sh` stages the files
under a nonce-specific directory, records the transaction's own manifest in a
durable journal, rejects symlinked ancestors, and can roll back nested paths
without trusting a later release's manifest. CLI and terminal deployment hash
every stable local snapshot, verify every remote SHA-256, recover predecessor
journals with their installed helpers, promote the v3 set, and only then invoke
`deploy/host/install.sh`.

## Operator deployment applications

The same Rust deployer builds for the operator's own machine and for Windows
x86-64. A Linux workstation publishes both: `scripts/check-deployer.sh
--publish` stages the static-PIE musl package, and
`scripts/build-windows-deployer.sh` cross-compiles the Windows one against the
pinned OpenSSL. `rpi-omt-deploy` provides human and JSON-lines CLI surfaces,
while `rpi-omt-deploy-gui` presents Connection, SD card, Alpine, Deploy,
Manage, Wi-Fi, Activity, and About views in the workstation's own web browser
-- on Linux and on Windows alike. Both run the jobs in
`crates/omt-deploy/src/jobs.rs`; secrets are wiped when dropped and never accepted
through arguments or environment variables. That covers every buffer a secret
passes through, not only the ones it is stored in: the sudo stdin a deployment
holds for its whole run, the Wi-Fi passphrase handed to the worker thread, the
raw `--secrets-stdin` document, and the parsed JSON values are all wiped
rather than dropped intact.

SSH is first-party too (`crates/omt-deploy/src/ssh`), written on OpenSSL's primitives.
It offers the ML-KEM-768 + X25519 hybrid key exchange first, then Curve25519,
group exchange, and the RFC 8268 MODP groups; Ed25519, ECDSA, and RSA host
keys with SHA-2 signatures only; ChaCha20-Poly1305, AES-GCM, and AES-CTR with
an HMAC-SHA2 tag. There is no CBC, no SHA-1, and no compression. Strict key
exchange -- the Terrapin countermeasure -- is mandatory: a server that does not
offer it is refused. Host keys are checked against OpenSSH `known_hosts`
(plain, wildcard, negated, hashed, and `@revoked` entries) and an unknown or
changed key is fatal; negotiation prefers a key type the file already trusts
for that host. User keys load from OpenSSH's own format, passphrase-protected
ones through a first-party `bcrypt_pbkdf` whose Blowfish tables are generated
from pi, or from PEM through OpenSSL. Every parser is fuzzed, and
`tests/integration/test_ssh_client.sh` holds each algorithm, key format,
upload path, and refusal to a real OpenSSH server.

The Alpine view uploads `deploy/host/setup-sys.sh` over SFTP or a `cat` exec
fallback (factory headless sshd often has no SFTP), sets hostname, IPv4 DHCP,
optional or already-associated Wi-Fi, user `pi`, root/`pi` passwords, US HTTPS
apk mirrors after an NTP clock step, apk OpenSSH, and persistent `sys` mode.
Three orderings in that script are load-bearing. The passwords are set before
apk OpenSSH can replace the sshd carrying the session, because a stock sshd
config refuses the factory image's empty root password. The boot media is
released first with `copy-modloop`, because alpine-conf skips any disk with a
mounted partition and `setup-disk` then *exits 0* having installed nothing.
And the packages the appliance needs to rejoin the network — OpenSSH, and on
a Wi-Fi board `wpa_supplicant` with `linux-firmware-brcm` and
`wireless-regdb` — are installed into the new root with `apk --root` after the
install, because the factory image runs from a read-only modloop where the
firmware belongs to no package and `/lib/firmware` cannot be written at all.
The completion marker the deployer keys on is printed only after the new root
has been mounted and checked, so a `setup-disk` that did nothing cannot report
success.

`linux-firmware-brcm` carries the Pi 5's radio: the CYW43455 is 802.11ac 1x1
dual-band, and the package ships its `brcmfmac43455-sdio.raspberrypi,5-model-b`
firmware, NVRAM, and CLM blob. `wireless-regdb` is the other half and was
easier to miss, because leaving it out produces no error anywhere. The kernel
resolves its regulatory domain by reading `/lib/firmware/regulatory.db`; with
no database the world domain applies, and that domain contains no channels
149-165 at all. A dual-band board then associates happily on 2.4 GHz and
delivers a fraction of the throughput a 1080p OMT stream needs. Both scripts
install the database, and the firmware copy-from-running-image fallback covers
it for the same reason it covers the Broadcom firmware: `apk` has been observed
to report success while placing nothing, and on a Wi-Fi-only board that is a
trip to the SD card reader.

Every path that can write a Wi-Fi configuration defaults the country to `US`,
and every one of them preserves a country that is already declared — that is
the operator saying where the appliance is, and neither a re-deploy nor a
Wi-Fi change may relabel a radio. There are three such paths and they are
separate code because they run in different worlds:
`host_wpa_supplicant_config` for the installer, `install_wpa_config_from` in
`setup-sys.sh` for the hand-written boot-partition file (busybox `ash` on a
factory image, with no access to `deploy/lib`), and the deployer's `wpa_cli`
script for later Wi-Fi management. The last one sets the country *before* it
scans: in the world domain the upper band does not exist, so scanning first
would offer the operator 2.4 GHz networks only and hide the 5 GHz access point
they are standing next to.
Fixed management actions cross the same privilege boundary as deployment and
Wi-Fi: a non-root SSH account uses its bounded sudo-password channel, while a
root session runs the fixed command directly. Neither account needs membership
in the Docker group. Status, logs, service restart, and a deferred OS reboot
are typed actions; the browser frontend requires a second confirmation before the restart and
the reboot. Host-key verification defaults to OpenSSH's
`~/.ssh/known_hosts`; the CLI and browser frontend can select another verified file without
relaxing strict checking.

An untouched Alpine host has neither sudo nor an active doas rule. When the
Alpine root password is supplied (the Alpine view, or `bootstrap_root_password`
on the CLI), the native deployers bootstrap through `su` on a bounded SSH PTY. Terminal echo is disabled before the secret
is sent; only the fixed staged bootstrap is run, and subsequent deployment
returns to the administrator's sudo credential. The root secret is wiped
with the other authentication buffers. Remote commands retain a one-minute
idle timeout but allow the package installer up to thirty minutes while it is
still producing progress; the previous two-minute total ceiling could abort a
healthy first install on a Pi. `install.sh` stops a live appliance, then runs
`apk update` and `apk upgrade --available` with a progress meter and a
twenty-second heartbeat so a kernel or firmware fetch cannot look idle to that
timeout. Host apk fetches use a pinned allowlist of reputable US HTTPS mirrors
(kernel.org, UC Berkeley OCF, and Princeton).
The explicit root credential takes priority over an ambiguous `doas` probe:
stock Alpine can describe its inert rule set as authorization-capable and then
refuse the actual non-PTY command.

The deployer carries the appliance instead of pointing at it.
`crates/omt-capsule/build.rs` reads `deploy/manifest-v3.txt` and emits one
`include_bytes!` per member, the ARM64 image archive included, so the operator's
executable holds every byte the Raspberry Pi receives. That makes the image a
build input of the deployer -- `make build-arm64` before `make build-deployer`,
which both build scripts stop and say -- and it makes a deployer of one release
paired with host scripts of another impossible to assemble. Embedded members are
uploaded straight from the binary's read-only data: there is no file to open, so
the mid-upload re-verification that a working tree needs does not apply to them,
while the Pi's own `sha256sum` is still checked against a digest taken before the
first byte was sent.

A deployment therefore asks nothing of the machine it runs on, and the terminal
application has no workstation setup at all. The exception is the `--project` developer
override, which sources the whole capsule from a checkout and, with
`--rebuild-image`, builds that tree's archive first. Only then does what the
machine provides become part of the contract, and `crates/omt-deploy/src/tools.rs`
owns it: executable discovery that follows `PATHEXT`, the Windows shell
locations Git for Windows installs into, the winget packages that supply a
missing prerequisite, and the plan for invoking the image build. The CLI's
`prerequisites` subcommand renders that probe; with no project root it reports
the embedded capsule instead, which is satisfied by construction.

Windows reaches `scripts/build-arm64.sh` through that shell directly rather
than through GNU Make. The Makefile recipe is a call to the script, so make
without a POSIX shell hands it to `cmd.exe`, and make with one adds nothing --
which leaves Git for Windows and Docker Desktop as the only two prerequisites
an operator has to install. Resolving the build program before spawning it is
what replaced a bare "program not found", whose text named neither the tool
nor the remedy. Nothing in that path is observable from a Linux publisher, so
every Windows rule is a pure function tested with the Windows answer supplied,
and the behaviour of the resulting `.exe` on a real desktop remains a
validation boundary.

The browser frontend is a small HTTP/1.1 server bound to `127.0.0.1` only,
with its page, script, and the appliance Web GUI's own `style.css` compiled
in, so the two operator surfaces share one design system and the deployer
still ships as one file with no third-party code. The page is a view of the
same state model the jobs are started from (`crates/rpi-omt-deploy-gui/src/app.rs`):
its forms are generated from that model's view and row tables, every action
button shows the model's precheck for it, and the server -- not the page --
decides what needs confirming. Loopback is not treated as trust, because any
page the operator's browser opens can send requests to it. The launch URL
carries a random 256-bit session token in its fragment, which a browser never
sends to a server or in a Referer; every API call must present it in a
header, compared in constant time. The `Host` header must name `127.0.0.1` or
`localhost` on this port, which defeats DNS rebinding; a cross-origin request
is refused; actions accept only JSON bodies, which a plain HTML form
cannot send; and responses carry a `'self'`-only Content-Security-Policy and
refuse framing. Secrets are write-only: state reports a password as set or
empty, never its value, and a request body that carried one is wiped. Each
connection carries one request with a bounded head, body, and timeout, and at
most sixteen are served at once.

The SD-card view lists mounted removable media -- FAT and exFAT mounts and
anything under `/media`, `/run/media`, or `/mnt` on Linux, drive letters on
Windows -- and marks each folder that passes the same boot-partition check the
preparation job runs (`omt_deploy::sd_card::boot_partition_problem`), so a
freshly inserted card is one click rather than a typed path. A folder picker
browses the workstation for the boot partition and the optional
`known_hosts` file; it only reads, and lists at most 2000 entries a folder.
Its behaviour on a real Windows desktop -- the browser launch and drive
enumeration -- is a validation boundary: the Linux publisher verifies the
`.exe`'s headers, not its runtime.

## Trust and legal surfaces

`LICENSE` governs project-owned code. `THIRD_PARTY_NOTICES.txt` covers shipped
runtime dependencies. The Web and deployer About views display those texts and
their build version; the deployer compiles them into its executable with the
capsule rather than reading files beside it, so a relocated binary still states
its terms. The container publisher generates a CycloneDX inventory of both
first-party binaries and the appliance's installed Alpine package database; the
deployer publisher inventories OpenSSL, the Rust standard library, and the
static C runtime it carries.

The host is Alpine Linux 3.24 aarch64 in persistent sys mode on a Raspberry Pi
5 or Pi 4 Model B. One `linux-rpi` kernel covers both. A dual-band radio is a
support criterion: the appliance is 5 GHz only, because real-world testing
showed 2.4 GHz packet loss makes OMT playback unusable, so a board that cannot
leave 2.4 GHz cannot be a host. The installer rejects unsupported distributions
and board models, and RAM-backed diskless roots; `deploy/lib/board-profile.sh`
and `crates/omt-deploy/src/ops.rs` hold the same table for the host-side
and workstation-side gates. OpenRC supervises the filtered Avahi proxy and two
inotify watchers; the Docker workload remains detached with its own restart
policy.

Host hardening disables unprivileged BPF and applies BPF JIT constant blinding
for privileged callers as well. IPv4 reverse-path filtering is pinned on, ARP
replies are limited to the incoming interface, IPv6 router advertisements and
SLAAC are refused, and apk repositories are pinned to US HTTPS mirrors.
SSH keeps root key access for recovery but
limits every login to root or the administrative `wheel` group; password root
login and all forwarding paths remain disabled. Onboard Bluetooth is disabled
in firmware, blocked at runtime, and blacklisted as kernel modules. The CPU
frequency governor is pinned to `performance` so decode on the Pi 4 does not
drop below its ceiling under schedutil. Wi-Fi power save is pinned off so
brcmfmac does not drop mDNS or OMT datagrams. Time synchronization is enabled
through the host's existing ntpd, or chrony when nothing else is providing a
clock.

The native deployers reboot the Pi after a successful install, wait for SSH to
return, and wait for the appliance container. They surface the first-start Web
GUI password from the container logs when it is still retained. `make deploy`
does the same.

Pi DRM, ALSA, HDMI hotplug, OpenRC boot ordering, nftables, and live OMT media
remain hardware validation boundaries after local unit and amd64 image checks
pass, once per supported board rather than once. Per-board decode ceilings join
that list: both shipped ceilings are measured on hardware, and a board that
stops holding its tier is caught only by rerunning the decode bench there.
QEMU models none of these SoCs, so there is no full-system VM tier: a guest
cannot validate RP1, either board's device tree, vc4 KMS/HDMI audio, device
groups, or the supported-board preflight.

## The 3.0.0 port

3.0.0 replaced the C17 tree with this Rust workspace, component by component,
with the C build as the behavioural and performance reference until it was
deleted. Every byte contract was kept: routes, headers, cookies, CSRF, the
state files, the support-bundle layout, the CLI's usage text and JSON lines,
and the receiver's CLI and status JSON. The acceptance rule was that the Rust
build be no worse than C, within a 2% noise band, on the measurements below;
where it is not, that is recorded here rather than hidden.

| Measurement | C | Rust |
|---|---|---|
| VMX decode, Pi 4 at 1.8 GHz, 1 and 3 workers | baseline | 0.70-1.06x of C per vector and output |
| VMX decode, Pi 4, 4 workers | baseline | up to 1.11x on dense 1080p and 480p (open) |
| Web, Pi 4: login POST / `GET /` | 839-863 / 23.8-26.2 ms | 839-845 / 24.7-25.3 ms |
| Web resident set after load, Pi 4 | 6.05 MB | 6.33 MB (+4.5%) |
| Deployer SFTP, loopback, chacha20 / aes128-gcm / aes256-ctr+hmac | 490 / 694 / 395 MB/s | 518 / 881 / 554 MB/s |
| Static Linux deployer, same embedded image | 18.46 MB | 19.00 MB (+3%) |

The resident-set and size increases (about 270 KB resident for `omt-web` and
460 KB for `omt-receiver`) are the Rust standard library's text --
about 179 KB of it is the backtrace symbolizer, which stable Rust cannot leave
out -- not heap, which is smaller than C's. `opt-level` `s` and `z` made the
binaries larger rather than smaller and were not adopted. The four-worker
decode row, which no shipped tier uses, remains up to 11% slower on dense
content. The Rust code also carries no AArch64 BTI/PAC branch protection:
stable Rust cannot enable it, so the assembly kernels deliberately carry no
property note either (see `crates/omt-vmx/asm/prelude.s`).

## Spare RAM and SD-card writes

After 3.0.0 the Pi 4 was booted with `mem=1024M` (915 MiB usable, as a 1 GB
board) and measured for what spare RAM could save the SD card. Every soak
wrote its samples to tmpfs and ran without SSH logins, which on their own
added more syslog and journal writes than the appliance does.

| Workload, 3.0.0 under `mem=1024M` | SD writes | Lowest `MemAvailable` |
|---|---|---|
| Idle, 30 min (waiting for HDMI, health check every 30 s) | 168 KiB in 24 operations, no reads | 770 MiB |
| One Web session a minute (login, five pages, settings save, logout), 30 min | 6.2 MiB, about 217 KiB a session | 767 MiB |
| Login, support bundle without packet capture, logout | 1.1 MiB in 130 operations | 726 MiB |
| The same, with working files on `/run` | 0.5 MiB in 46 operations | 714 MiB |
| Login, support bundle with packet capture on a busy network, logout | 64-84 MiB | 703 MiB |
| The same, with working files on `/run` | 0.5-0.7 MiB | 659 MiB |

The appliance was already keeping its steady writes in RAM: playback status,
the control lock, and container scratch are on tmpfs, health checks write
nothing, and the receiver logs only on events. One change shipped:

- **Diagnostics working files on `/run`.** A collection used to create every
  section, both text captures, and the raw capture beside the published report
  on the SD card, then delete them. The raw capture is `tcpdump -C 64 -W 1`,
  so on a busy network it rewrote a 64 MB file for the whole 20 s window --
  and anything over 8 MiB is never published. Working files now go to a
  private 0700 directory under `/run` that is removed when the run ends (and
  at reboot, so an interrupted run no longer leaves files on the card); the
  capture is copied beside its published name and renamed only when
  publishable. Cost: up to 64 MB of RAM for the 20 s of a requested capture,
  nothing otherwise.

Measured and left alone:

- **Session and settings writes.** Each login and logout replaces
  `web_sessions.json` durably (about 100 KiB of SD writes each, mostly ext4
  metadata), and an unchanged settings save writes nothing. Holding sessions in
  RAM and flushing later would trade a re-login after power loss for a few
  hundred KiB per operator visit.
- **Syslog ring buffer, `noatime`, a longer ext4 `commit=`.** Idle writes are
  about 8 MiB a day in total; the host syslog wrote nothing on its own in the
  idle soak, `relatime` already limits atime updates, and the remaining writes
  are `fsync`ed, which a longer commit interval does not defer.
- **zram and the 512 MiB container cap.** No swap was used in any soak; the
  container peaked at 24 MB, and the worst case left 659 MiB available, well
  above a 256 MiB floor.
- **Decode under 1 GiB.** `bench_vmx` on 1080p is unchanged (3.50 ms a frame
  on three workers, 3.46 ms with 8 GB).

Not measured: pre-faulting or locking the scanout and decode buffers, and
larger receive or audio buffers, need live HDMI playback, which the lab Pi
does not have.
