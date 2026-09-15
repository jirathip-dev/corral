# #547 physical-device capture protocol (NOT EXECUTED)

Status: blocked before instrumentation. This protocol is a handoff, not evidence
that instrumentation or the reconnect change shipped. See `.report.md` for the
file-fence conflicts. No physical iPhone was attached (`xcrun devicectl list
devices`: `No devices found.`). No timing samples have been collected.

## Prerequisites / human gates

- Authorize a real row-visibility hook in the board renderer; a model publish,
  row projection, or arbitrary next-main-queue callback is NOT row visibility.
- Complete and verify release-active, privacy-safe instrumentation. Every event
  needs a monotonic timestamp, an ephemeral reconnect-attempt ID, an ephemeral
  per-host discriminator, and a fixed stage name. Never log URLs, profile names,
  tokens, signing keys, agent IDs, or payloads. Confirm the actual category and
  stage names from the delivered implementation before capturing.
- Prepare two owner-authorized physical-device builds: the pinned base
  `ed24e6f57075acf1e5b082c7cd29a931dfbfbd62` plus instrumentation ONLY (serial
  control), and the implemented head with IDENTICAL instrumentation. Record both
  source SHAs and build identifiers. No release/TestFlight promotion is
  authorized by this protocol.
- Pair the same physical iPhone with the owner's Mac host and Bazzite host using
  the ordinary supported pairing flow. Keep key pins, device grants, daemon
  revision, board population, network/VPN route, iOS version, and phone settings
  identical between builds. Do not rotate keys or weaken trust/grant gates.
- The owner supplies and records the reproducible Bazzite workload command and
  its revision/configuration. Start that SAME workload before every Bazzite
  block, hold it throughout the block, and capture its load telemetry. There is
  no approved load-generation command in this lane's brief; do not substitute
  an invented stress command, alter the daemon, or install host software.
  Record the Mac's concurrent workload too, rather than calling it idle without
  measuring. The exact loaded-Bazzite workload remains an owner prerequisite.

## Capture procedure

1. Connect the unlocked iPhone to the Mac. In Console select that device, enable
   Info messages, start streaming, and filter by subsystem
   `com.corral.fleetnotifier` plus the implementation's verified timing category.
   Save the capture privately. Console/system archives can include unrelated
   private logs; publish only the allowlisted timing records.
2. Run the board with at least one visible, expanded row from the host under
   measurement. Confirm initial verified live data. Keep filtering, collapsed
   sections, scroll position, display mode, and row population fixed. Test one
   host's timings per block; keep a separate dual-host block for independence.
3. Switch to another app, verify that Corral actually enters background (a
   transient inactive transition is NOT this test), remain there for five
   seconds, then return. Keep the last board visible during reconnect. Record
   whether the indicator truthfully remains reconnecting/stale until verified
   transport, and whether any preflight obscures the retained rows.
4. Cause one repeatable owner-controlled board update per cycle when the host
   would otherwise emit no data. Record that stimulus, since an idle daemon can
   legitimately produce HTTP 200 plus keep-alives but no first data frame.
   Do not count absence of a frame as a zero-millisecond measurement.
5. Take three warm-up cycles, excluded and retained as such in the raw capture.
   Then take 20 measured cycles for EACH build/host combination: serial Mac,
   parallel Mac, serial loaded-Bazzite, parallel loaded-Bazzite. Counterbalance
   build order across the host blocks. Record all failures/timeouts; do not
   silently replace them with successful samples. A replacement run must be a
   separately identified block with an exclusion reason for the earlier one.
6. Repeat a dual-host check with Bazzite unreachable/slow and Mac reachable.
   Confirm Mac verifies/applies independently while Bazzite stays stale. Retain
   the separate per-host attempt records; do not merge their timelines.
7. Save the source/build IDs, iPhone model and iOS version, workload details,
   capture method, sample/exclusion counts, timing records and observed UI
   behavior. Keep the pin itself and personal row content out of public evidence.

## Stage accounting

Required marks: path-ready, host-key request, host-key response, SSE HTTP 200,
first frame received, first accepted frame applied, updated row visible.

The stream and key-check branches overlap in the proposed implementation; they
are NOT a serial timing chain. Calculate each sample's durations first, then
use `statistics.median` across the valid samples for that metric (never subtract
aggregate timestamp medians). Report sample counts per metric:

- Path-ready to host-key request (dispatch).
- Host-key request to response (key round trip).
- Path-ready to SSE HTTP 200 (stream establishment).
- SSE HTTP 200 to first data frame received.
- First frame received to first accepted frame applied (includes trust wait).
- Host-key response to first accepted frame applied (can include frame wait).
- First accepted frame applied to the corresponding updated row visible.

An NWPathMonitor callback arriving after dispatch must not be relabelled as
an earlier path-ready event. Document missing/not-observed path-ready marks
rather than inventing a zero. A previously retained row already on screen is
not proof that the newly applied revision is visible. Record retained-row
visibility separately from the updated-row mark.

Verification is still a prerequisite to application: the intended gain removes
SERIAL stream setup after verification, not the verify-before-apply dependency.
Do not claim that a slow key response can never remain the limiting stage.

## Results template

All values below are deliberately missing, not zero. Add one table per metric
or retain these four rows with a column for every duration listed above.

| Build | Host condition | Measured cycles attempted | Valid samples per metric | Dispatch median ms | Key RTT median ms | Path to SSE 200 median ms | 200 to frame median ms | Frame to apply median ms | Key response to apply median ms | Apply to visible median ms | Failures / exclusions |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| Serial control SHA: pending | Mac | NOT RUN | missing | missing | missing | missing | missing | missing | missing | missing | missing |
| Implemented SHA: pending | Mac | NOT RUN | missing | missing | missing | missing | missing | missing | missing | missing | missing |
| Serial control SHA: pending | Loaded Bazzite | NOT RUN | missing | missing | missing | missing | missing | missing | missing | missing | missing |
| Implemented SHA: pending | Loaded Bazzite | NOT RUN | missing | missing | missing | missing | missing | missing | missing | missing | missing |

No simulator, network, loaded-host, or physical-device timing result is claimed
by this document. The exact workload and instrumentation must be supplied before
this protocol can become an executable, reproducible capture gate.
