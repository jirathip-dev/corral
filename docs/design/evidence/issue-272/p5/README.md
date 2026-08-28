# Issue #272 P5 evidence

The live local tmux probe used a lane-owned session running lazygit in the
P5 worktree. `protocol-trace.json` records the tmux cursor metadata and
`lazygit-tmux-capture.txt` preserves the ANSI screen frame. `scrollback.txt`
contains the bounded scrollback capture. `live-proof.txt` records that the
session was closed and was absent afterward.

This probe validates the transport's real tmux capture/cursor path. It does
not claim a phone-to-corrald authenticated WS run: no live device identity or
registered attach grant was available in this lane.
