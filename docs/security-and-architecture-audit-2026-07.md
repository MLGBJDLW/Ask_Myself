# Security and architecture audit — July 2026

## Outcome

This pass closed every known dependency vulnerability reported by the current
RustSec and npm advisory databases. It also removed a duplicate Office parsing
path, hardened rich-content rendering, and made browser/computer-use evidence
cross explicit trust boundaries.

| Gate | Before | After | Enforcement |
| --- | ---: | ---: | --- |
| `cargo audit` vulnerabilities | 13 | 0 | Pull-request CI |
| RustSec allowed warnings | 31 | 20 | Recorded upstream watch list |
| Root npm vulnerabilities | 17 | 0 | Local lockfile audit |
| Desktop npm vulnerabilities | 17 | 0 | Pull-request CI |

The Rust workspace MSRV is now 1.88. This is required by the secure current
releases of the document and image-processing dependencies.

## Resolved findings

### One Office parsing path

`dotext` duplicated the repository's existing ZIP/XML Office extractors and
held obsolete `quick-xml 0.9` and `time 0.1` dependencies. The fallback has
been removed. DOCX, PPTX, and ODF text extraction now use the same bounded,
local ZIP/XML path as the rest of the document system. `quick-xml`, `calamine`,
and `lopdf` were upgraded to fixed releases.

### Dependency vulnerabilities

- Updated `crossbeam-epoch`, `quinn-proto`, `rustls-webpki`, and their parents
  to patched versions.
- Updated `headless_chrome`, `imageproc`, `notify`, `tokenizers`, and `scraper`
  to maintained compatible lines.
- Removed the unused `dependency-cruiser` installation from both JavaScript
  workspaces.
- Added pinned `cargo-audit 0.22.2` and npm audit gates to CI.

### Rich-content execution boundaries

- Mermaid SVG output is sanitized with DOMPurify before it reaches
  `dangerouslySetInnerHTML`.
- Markdown links only reach native openers for `http`, `https`, or `mailto`;
  other protocols are inert.
- Browser screenshots are accepted only as bounded image attachments. They are
  injected into the current vision-capable model turn, removed from UI/trace
  output, and never persisted in conversation history.
- Browser redirects and subresource requests pass the same public-network
  policy as the original URL, preserving the SSRF boundary.

### Computer-use boundary

Computer use is represented as an isolated MCP connector plugin, not as an
in-process event-injection API. The plugin advertises observe → decide → act,
uses the existing MCP approval settings, and requires visible-window evidence.
The integration contract and threat boundary are documented in
`docs/computer-use-integration.md`.

### Dead and misleading code

- Removed unused Tauri knowledge-command aliases and an unused visual-document
  error helper.
- Removed stale embedding fallback comments after catalog-backed dimensions
  became authoritative.
- Kept provider metadata in shared JSON catalogs so the Rust and frontend
  surfaces cannot drift into parallel model lists.

## Remaining upstream warnings

RustSec reports no vulnerabilities, but still reports 20 allowed warnings:

- Linux Tauri/Wry currently brings the unmaintained GTK3 bindings and
  `glib 0.18`. Nexa does not use the affected `VariantStrIter` API directly.
- Tauri's build-time HTML selector chain brings `rand 0.7`; the advisory needs
  a custom logger plus `rand::rng()`, and this path executes only during build.
- Tauri/readability transitive chains include several unmaintained macro,
  Unicode, and hashing crates. They have no RustSec vulnerability classification
  today.

Replacing these transitive crates locally would require forking Tauri or its
build stack and would create a second framework path. The repository therefore
tracks them as upstream migration items while CI blocks every RustSec
vulnerability. Project-owned warnings were reduced from 31 to 20 during this
pass.

## Verification commands

```text
cargo fmt --all -- --check
cargo clippy -p nexa-core -- -D warnings
cargo test -p nexa-core
cargo audit
npm audit --audit-level=moderate
npm test
npm run build
```

## Primary references

- [RustSec cargo-audit](https://github.com/rustsec/rustsec/tree/main/cargo-audit)
- [npm audit](https://docs.npmjs.com/cli/v11/commands/npm-audit)
- [Tauri security documentation](https://v2.tauri.app/security/)
