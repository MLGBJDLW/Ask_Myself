# Nexa Office.js live adapter

This add-in is a separate, user-authorized deployment surface for the currently
open Word, Excel, or PowerPoint document. It never receives local file-engine
receipts and never runs without the task pane being paired to Nexa.

Security and lifecycle:

- Nexa binds the bridge only to `127.0.0.1` on an ephemeral port.
- `office_artifact live_pairing` reveals a one-time six-digit code; successful
  pairing rotates it and returns a 256-bit bridge token only to the add-in.
- The add-in registers its host, document identity, supported requirement sets,
  and exact operation capabilities.
- `live_execute` rejects host/capability mismatches before queueing work and
  waits for an authenticated result envelope.
- The add-in executes a closed operation union. There is no script/eval/raw
  Office.js escape hatch.

For sideload development, serve this directory over trusted HTTPS at
`https://localhost:3000`, sideload `manifest.xml`, and keep the task pane open.
Production packaging must use an organization-trusted HTTPS origin and the same
checked-in taskpane assets.
