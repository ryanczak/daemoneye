# NEXT

**Active phase:** none yet — **M23 (Client decoupling & shell UX) is scoped
and PE-signed-off (2026-09-30)**; decisions recorded in its README § Notes.
Run `/rexymcp:architect next` to draft row **01a** (pty-spawn-transient),
the first of M21's four carry-forwards.

Decisions that shape the drafting: `daemon.tmux_session` keeps the
`"daemoneye"` default and only `""` means off (row 02); the live close (row
10) runs in a private tmux server used only as the terminal emulator, with
the no-server check enforced; `/panes` stays until M26.
