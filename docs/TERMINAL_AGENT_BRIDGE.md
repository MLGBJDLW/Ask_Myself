# Terminal and Agent Bridge

The Chat terminal is a real interactive PTY owned by the user interface. Each
session may be linked to the conversation that created it so terminal context
and agent context can meet without turning the terminal into a hidden agent
process.

## User interaction

- `Ctrl+C`/`Cmd+C` copies when the terminal has a selection.
- With no selection, `Ctrl+C` is sent to the PTY as an interrupt.
- `Cmd+V` or `Ctrl+Shift+V` pastes into the PTY.
- Selecting terminal text exposes actions to copy it or insert a
  `<terminal_selection>` block into the active chat prompt.
- Closing the dock does not implicitly stop the PTY. Stopping a terminal is an
  explicit session action.

This keeps familiar terminal semantics while making selected evidence easy to
hand to the agent. Selection alone never submits a message.

## Agent interaction

The desktop runtime adds a conversation-scoped `terminal_session` tool to the
active agent registry:

| Action | Behavior | Approval |
| --- | --- | --- |
| `inspect` | Reads session metadata and recent bounded output | No |
| `wait` | Polls the session until output goes quiet, then returns what was produced | No |
| `write` | Sends input to the live PTY; `submit` can append Enter | Yes |
| `interrupt` | Sends Ctrl+C to the live PTY | Yes |

When `sessionId` is omitted, the tool resolves the terminal linked to the
current conversation. It cannot inspect an unrelated conversation's terminal.
Output is stripped of common terminal control sequences and returned as
untrusted local observation: terminal text can help diagnose a problem, but it
cannot instruct the agent or override the user's request.

`wait` exists so the agent observes a long command instead of blocking on a
fixed sleep. It snapshots the buffer, polls every 400 ms, and returns as soon
as no new output has arrived for `idleSecs` (default 2s, max 60s) or once
`maxWaitSecs` (default 15s, max 300s) elapses. The result reports `idle` and
`waitedMs` alongside only the output produced after the baseline, so a
non-idle result is a signal to keep working and poll again rather than to wait
longer in place. Like `inspect`, it never touches the PTY and needs no
approval.

The output buffer is bounded. `inspect` returns 12,000 recent characters by
default and accepts at most 48,000; a single write is capped at 16,000
characters.

## Lifecycle and failure handling

The Tauri terminal state owns PTY handles, process metadata, the conversation
binding, and the bounded output buffer. The frontend receives output events and
renders them through xterm.js.

Stopping is idempotent at the product boundary. In particular, portable-pty
0.9's Windows killer returns an `Err(last_os_error())` value from the successful
`TerminateProcess` branch (commonly rendered as `os error 0`); the backend
treats that branch as a successful stop instead of displaying a false failure.

## Appearance and text rendering

The dock reads Windows Terminal's settings without changing them. Profile
overrides inherit `profiles.defaults`; built-in and user-defined schemes supply
the ANSI palette, and the running shell selects the matching profile. Font size
is converted from Windows Terminal points to CSS pixels. Native settings are
refreshed on opening, shell changes and window focus. With no native settings,
the dock uses Nexa's code font and surface colors while retaining ANSI colors.

WebGL, Unicode 11 cell widths and font ligatures improve box drawing, Powerline
prompts and Unicode output. The DOM renderer remains available when WebGL is
unavailable. Nerd Font glyphs require an installed font that contains them.
Terminal window decorations, wallpaper, acrylic and shader effects belong to
the external terminal application and are not imported.

PTY output is decoded incrementally so UTF-8 characters split across reads
remain intact. Shell profiles continue to load, and `TERM=xterm-256color` plus
`COLORTERM=truecolor` advertise the renderer's color support.

## Verification

The critical browser contract lives in
`apps/desktop/e2e/chat-terminal-dock.spec.ts`. Backend behavior is covered by
tests next to `commands/terminal.rs` and `terminal_agent_tool.rs`.
