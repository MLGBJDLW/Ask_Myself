# Nexa Office.js live adapter

This add-in is the separate, user-authorized surface for editing the currently
open Word, Excel, or PowerPoint document. It never receives local-file engine
receipts and never runs unless its task pane is paired to Nexa.

## Closed live-operation surface

- Word: replace/insert text, add/reply/resolve comments, set change tracking,
  and wrap one exact text match in a tagged content control (`WordApi 1.4`).
- Excel: set values/formulas, allowlisted range formatting, create a table, add
  an allowlisted chart, and request native calculation (`ExcelApi 1.13`).
- PowerPoint: add slides, set text by stable slide/shape identity, and add text
  boxes or allowlisted geometric shapes (`PowerPointApi 1.3/1.4`).

The Rust boundary and task pane share the same typed operation union. Unknown
fields, host/capability mismatches, unsupported enum values, expired leases, and
late results fail closed. There is no script, `eval`, or raw Office.js escape
hatch.

## Pairing and transport

- Nexa binds the bridge only to `127.0.0.1` on an ephemeral port.
- `office_artifact live_pairing` reveals a one-time six-digit code. Successful
  pairing rotates the code and returns a 256-bit bearer token only to the pane.
- The pane registers its exact Office host, requirement sets, document identity,
  and capability list. `live_execute` validates that declaration before queueing.
- Development mode uses Nexa's loopback HTTP bridge. Trusted deployment mode
  can use a TLS loopback bridge and pins CORS to one exact HTTPS add-in origin.

## Trusted local development

Provision a localhost certificate that Windows, Office, and WebView2 already
trust. Certificate installation changes the machine trust store, so these tools
deliberately never install or trust a certificate for you.

Start the closed asset server with the certificate and key outside this folder:

```powershell
python .\serve_https.py --cert C:\secure\localhost-cert.pem --key C:\secure\localhost-key.pem --host localhost --port 3000
```

Sideload `manifest.xml`, keep the task pane open, request `live_pairing` from
Nexa, and enter the exact endpoint and one-time code shown by Nexa. The server
exposes only `taskpane.html`, `taskpane.js`, `support.html`, and `icon.png`.

## Organization deployment

Render a new manifest for the exact organization-trusted HTTPS origin. The
renderer validates and canonicalizes the origin and refuses implicit overwrite:

```powershell
python .\render_manifest.py --origin https://office.example.com --output .\manifest.production.xml
```

Configure the Nexa process before its bridge starts:

```powershell
$env:NEXA_OFFICE_LIVE_ORIGIN = 'https://office.example.com'
$env:NEXA_OFFICE_LIVE_TLS_CERT = 'C:\secure\loopback-cert.pem'
$env:NEXA_OFFICE_LIVE_TLS_KEY = 'C:\secure\loopback-key.pem'
```

`NEXA_OFFICE_LIVE_TLS_CERT` and `NEXA_OFFICE_LIVE_TLS_KEY` must be supplied
together. Setting `NEXA_OFFICE_LIVE_ORIGIN` without the TLS identity is rejected.
The loopback certificate must be valid for `127.0.0.1` and already trusted by
the Office client environment. Distribute the rendered manifest and unchanged,
reviewed runtime assets through the organization's normal Office add-in policy.

## Acceptance boundary

Automated tests verify the typed union, capability negotiation, bridge auth,
origin pinning, manifest rendering, asset allowlist, and TLS configuration path.
Actual sideload and mutations in desktop Word, Excel, and PowerPoint remain a
trust-gated native acceptance step because Office must trust the deployment and
loopback certificates.
