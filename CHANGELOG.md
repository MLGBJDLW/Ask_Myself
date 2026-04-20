# Changelog

## [0.1.9](https://github.com/MLGBJDLW/Ask_Myself/compare/v0.1.8...v0.1.9) (2026-04-17)


### Features

* apply current workspace updates ([4179176](https://github.com/MLGBJDLW/Ask_Myself/commit/41791767c56deeeab00661300a6c4d00e42cddb1))
* **chat:** auto-collapse/follow thinking + persist image attachments ([b0df791](https://github.com/MLGBJDLW/Ask_Myself/commit/b0df791e0c2359f1b1596970cbd2c0365843f8b1))
* **desktop:** draggable sidebar tabs and persistent UI/window state ([d09a622](https://github.com/MLGBJDLW/Ask_Myself/commit/d09a6228c42051f6850b8aec5f57eb8eacda83da))
* **tools:** add run_shell tool with program whitelist and confirmation ([2364a66](https://github.com/MLGBJDLW/Ask_Myself/commit/2364a662a103d3a658ecc9ac5681f94b24db253e))


### Bug Fixes

* align Rust formatting with cargo fmt ([ddf3db4](https://github.com/MLGBJDLW/Ask_Myself/commit/ddf3db4dd199f2fb6890648a579dbe5e147a0776))
* **chat:** show unassigned conversations when no project is selected ([e94224a](https://github.com/MLGBJDLW/Ask_Myself/commit/e94224a917041bb912c38dc30196b1c5127d2360))
* **core:** handle UTF-8 char boundaries in edit_document str_replace ([69a7a40](https://github.com/MLGBJDLW/Ask_Myself/commit/69a7a4058877dca18937258195598c7b97d48577))

## [0.1.8](https://github.com/MLGBJDLW/Ask_Myself/compare/v0.1.7...v0.1.8) (2026-04-13)


### Features

* resolve 8 user-reported issues + add edit_document tool ([396e6fe](https://github.com/MLGBJDLW/Ask_Myself/commit/396e6fe6b1cf8f71015ce6c459310ac8bd247855))
* **search:** overhaul search pipeline for quality and reliability ([d5b0f18](https://github.com/MLGBJDLW/Ask_Myself/commit/d5b0f18a532ffd4837f75e155a68d4060accf767))


### Bug Fixes

* tab state persistence, compile timeout, CI linting ([b61c16d](https://github.com/MLGBJDLW/Ask_Myself/commit/b61c16d89607ec615ddf91485a1b16888b39d0b5))

## [0.1.7](https://github.com/MLGBJDLW/Ask_Myself/compare/v0.1.6...v0.1.7) (2026-04-11)


### Features

* **core:** split generate_document into specialized DOCX/XLSX/PPTX tools ([3cdd79c](https://github.com/MLGBJDLW/Ask_Myself/commit/3cdd79cb97f60a6e14cee1465d4c49082ac8e520))
* **core:** upgrade built-in skills with structured imperative instructions ([8be04be](https://github.com/MLGBJDLW/Ask_Myself/commit/8be04becc7dfa1bb9b5d7458d2d7c4cb55b87d16))
* **desktop:** add microphone device selector in settings ([55e42e1](https://github.com/MLGBJDLW/Ask_Myself/commit/55e42e1f5268bd5a05a47c027ad209ce3690a80d))
* **desktop:** add periodic knowledge health checks and insights ([8e982d7](https://github.com/MLGBJDLW/Ask_Myself/commit/8e982d71c10fd17b1433d01ec46622f2780a7727))
* **desktop:** auto-compile knowledge graph after file changes ([49767fe](https://github.com/MLGBJDLW/Ask_Myself/commit/49767fe529ca15ba3609e84790b464c12f5563d7))


### Bug Fixes

* **core:** resolve all i64/String document ID mismatches across compile subsystem ([343e70c](https://github.com/MLGBJDLW/Ask_Myself/commit/343e70cf8805c12595a812cd325e70b3ea3df98d))

## [0.1.6](https://github.com/MLGBJDLW/Ask_Myself/compare/v0.1.5...v0.1.6) (2026-04-11)


### Features

* **core:** add persistent scan error tracking with retry backoff ([c588130](https://github.com/MLGBJDLW/Ask_Myself/commit/c5881309ef06f3dbbe30976192543ed75ab95b31))


### Bug Fixes

* **core:** correct column names and types in lint and knowledge_loop ([8cc1b59](https://github.com/MLGBJDLW/Ask_Myself/commit/8cc1b59b1fd24c5ba323c9b1cba9da3e3ad6c4b4))
* **updater:** add createUpdaterArtifacts config and CI validation ([2413bed](https://github.com/MLGBJDLW/Ask_Myself/commit/2413bed4bc2944fc29a2419c6f1686cee2564669))

## [0.1.5](https://github.com/MLGBJDLW/Ask_Myself/compare/v0.1.4...v0.1.5) (2026-04-10)


### Features

* add auto-update detection and in-app update ([14f8a8a](https://github.com/MLGBJDLW/Ask_Myself/commit/14f8a8a860e84f0e74df5314af7638185b09e8da))
* add ErrorBoundary, onboarding wizard, embedding warning, API key encryption ([05e0d75](https://github.com/MLGBJDLW/Ask_Myself/commit/05e0d759e7d93be0054e4f9ce32877711a5e9ce2))
* add support for .doc, .ppt, .epub, .odt/.ods/.odp and HTML tag stripping ([c2dcac7](https://github.com/MLGBJDLW/Ask_Myself/commit/c2dcac7ff6ddb709df7bc312b2bdd435446fd037))


### Bug Fixes

* document attachments, vision detection, agent search behavior, version sync ([6502ade](https://github.com/MLGBJDLW/Ask_Myself/commit/6502ade48b3c8a824ac3d228a8b621761002da09))
* resolve image paste not reaching LLM ([2a0c0ff](https://github.com/MLGBJDLW/Ask_Myself/commit/2a0c0ff01da2d88fc3364a57f3ba4b17ac474c4f))

## [0.1.4](https://github.com/MLGBJDLW/Ask_Myself/compare/v0.1.3...v0.1.4) (2026-04-10)


### Bug Fixes

* handle models without token_type_ids input & add model management ([da84a3a](https://github.com/MLGBJDLW/Ask_Myself/commit/da84a3a24ab378db7768dcb2f39341a9092e366c))
* handle models without token_type_ids input & add model management ([a0b4781](https://github.com/MLGBJDLW/Ask_Myself/commit/a0b478154b19ae9eba9b290a7b5e8e2e2d12a1b4))

## [0.1.3](https://github.com/MLGBJDLW/Ask_Myself/compare/v0.1.2...v0.1.3) (2026-04-08)


### Bug Fixes

* generate all Tauri icon sizes and upgrade Linux runner ([1dc210f](https://github.com/MLGBJDLW/Nexa/commit/1dc210f4394e421021ccc9c8706be549d7412801))

## [0.1.2](https://github.com/MLGBJDLW/Nexa/compare/v0.1.1...v0.1.2) (2026-04-08)


### Bug Fixes

* convert icon.png to RGBA and fix compiler warnings ([50acc0f](https://github.com/MLGBJDLW/Nexa/commit/50acc0f6f5c954683d5cb77c2119c1999c42ae8e))

## [0.1.1](https://github.com/MLGBJDLW/Nexa/compare/v0.1.0...v0.1.1) (2026-04-08)


### Bug Fixes

* citation links broken by rehype-sanitize stripping custom protocols ([5ca391c](https://github.com/MLGBJDLW/Nexa/commit/5ca391c415f9faf037f454114f538ddfc35d9623))
* move release-please config to repo root for manifest mode ([191e371](https://github.com/MLGBJDLW/Nexa/commit/191e3714245dc53009ad383760a19a892bc9a53d))
* use jsonpath for TOML extra-file in release-please config ([dfaca88](https://github.com/MLGBJDLW/Nexa/commit/dfaca8889b426b7a32e200bb9c299207934de8ae))
