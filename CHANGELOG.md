# Changelog

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

* generate all Tauri icon sizes and upgrade Linux runner ([1dc210f](https://github.com/MLGBJDLW/Ask_Myself/commit/1dc210f4394e421021ccc9c8706be549d7412801))

## [0.1.2](https://github.com/MLGBJDLW/Ask_Myself/compare/v0.1.1...v0.1.2) (2026-04-08)


### Bug Fixes

* convert icon.png to RGBA and fix compiler warnings ([50acc0f](https://github.com/MLGBJDLW/Ask_Myself/commit/50acc0f6f5c954683d5cb77c2119c1999c42ae8e))

## [0.1.1](https://github.com/MLGBJDLW/Ask_Myself/compare/v0.1.0...v0.1.1) (2026-04-08)


### Bug Fixes

* citation links broken by rehype-sanitize stripping custom protocols ([5ca391c](https://github.com/MLGBJDLW/Ask_Myself/commit/5ca391c415f9faf037f454114f538ddfc35d9623))
* move release-please config to repo root for manifest mode ([191e371](https://github.com/MLGBJDLW/Ask_Myself/commit/191e3714245dc53009ad383760a19a892bc9a53d))
* use jsonpath for TOML extra-file in release-please config ([dfaca88](https://github.com/MLGBJDLW/Ask_Myself/commit/dfaca8889b426b7a32e200bb9c299207934de8ae))
