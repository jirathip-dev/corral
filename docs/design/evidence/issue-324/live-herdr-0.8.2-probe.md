# #324 live upstream probe — herdr 0.8.2 (read-only, 2026-09-02)

Probe of the LIVE herdr 0.8.2 socket (~/.config/herdr/herdr.sock) at Corral
base 58a41f0. Read-only JSON-RPC `agent.read` (no mutation; the request id
is a string per herdr's protocol — an integer id is rejected with
`invalid_request`).

Target: `orch-hermes-brain` (pane `w2H:p4`, agent_status `done`),
`source: recent_unwrapped`, `lines: 10`.

| Variant | read.revision | body bytes | body lines |
|---|---|---|---|
| no `rev` | 0 | 929 | 8 |
| `rev = 1` | 0 | 929 | 8 |
| `rev = 999999` | 0 | 929 | 8 |

The three bodies are byte-identical. The response `read` object exposes
`{pane_id, workspace_id, tab_id, source, format, text, revision, truncated}`;
`revision` is always `0` and `rev` has no effect.

Conclusion: the live Herdr 0.8.2 provider does NOT support revisions. The
adapter's legacy fallback (bounded full page, `source_rev` echoing the
client's cached revision) is the honest live behavior; the incremental
contract is exercised against the simulated contract-honoring provider in
`probe.log` (see README.md).
