# File badge brand visual inventory

Reviewed: 2026-08-04

Nexa's file badges use the existing `react-icons` 5.7.0 dependency (MIT) and its
Simple Icons components. No upstream SVG file is copied or repackaged by this
feature. Brand color accents identify common file types; they do not imply
endorsement or ownership, and the low-saturation Nexa badge shell remains the
primary UI treatment.

| Families | Treatment | Color source | Asset source |
| --- | --- | --- | --- |
| Python | blue icon plus yellow accent | Python logo usage guidance | `SiPython` through `react-icons`; no custom SVG |
| TypeScript, JavaScript, React | official primary color, with a secondary language accent for React variants | Microsoft TypeScript, OpenJS, React brand pages | matching `Si*` component through `react-icons` |
| Rust, Go, Java, Kotlin, Swift, Docker | official primary color; secondary accent where it improves recognition | each project's official brand or identity page | matching `Si*` component through `react-icons` |

Primary review references and the reasons Nexa does not copy Seti or arbitrary
web SVG assets are recorded in
[`nexa-turn-file-motion-task-cache-primary-sources.md`](./nexa-turn-file-motion-task-cache-primary-sources.md#3-file-badge-brand-and-multicolor-visuals).

High-contrast mode suppresses the accent layer and uses the system `ButtonText`
color. Unknown extensions and every non-allow-listed family retain the mono
fallback.
