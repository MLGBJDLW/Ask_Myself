# Live inline file diff

## Product contract

File-mutation tools use the diff itself as the primary live surface. While a model is assembling `create_file`, `edit_file`, `multi_edit`, or `write_note` arguments, the chat timeline renders the available hunk immediately instead of presenting a generic tool-running card.

The interaction follows these rules:

- added content appears as visible `+` rows;
- removed content appears as visible `-` rows;
- replacements show the old `-` rows followed by the new `+` rows;
- the panel opens automatically while the mutation is live;
- the last growing addition row carries a subtle live caret;
- the viewport follows new rows only while the user remains near the bottom;
- manually scrolling upward disables automatic following;
- the final authoritative tool result replaces the preview without changing the visual surface;
- tool status remains secondary metadata rather than the principal progress UI.

## Data flow

The backend emits bounded `fileChangePreview` artifacts from partial tool-call JSON. Each artifact contains the current diff stats and line-level hunks. Frontend tool projection patches the existing call by `callId`, preserving the newest preview and the cumulative `inputProgress.receivedBytes` value even when raw argument text is truncated for transport.

`ToolCallCard` detects a file-change render with an available diff and switches to `FileDiffPreview` directly. It remembers that the mounted call was shown as a live diff so the same component remains stable across `preparing`, `running`, and terminal states.

## Safety boundary

Partial diff parsing is display-only. It never authorizes or performs a file mutation. Actual execution still requires complete schema-valid arguments, normal approval checks, path policy enforcement, and the authoritative completed tool event.
