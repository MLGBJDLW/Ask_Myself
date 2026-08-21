# Nexa image-size compatibility subset

This package replaces the transitive `image-size` dependency used by
PptxGenJS 4.0.1. It preserves the small CommonJS API surface that PptxGenJS
expects, but deliberately supports only PNG, JPEG, GIF, and SVG—the same
reviewed formats accepted by Nexa's bounded PptxGenJS adapter.

The upstream 1.2.1 package is MIT licensed. This clean-room compatibility
implementation does not include its complex ICNS, JXL, or HEIF parsers, which
were affected by GHSA-w3rx-r6r6-pgpr and GHSA-5p2g-fcmc-qvqq. Every input is
bounded to 4 MiB and every scan advances monotonically with an iteration cap.

`2.0.3-nexa.1` is a Nexa-local package version, not an upstream release.
