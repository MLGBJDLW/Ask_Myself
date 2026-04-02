# Internationalization Guidelines

## Goal

Ask Myself ships to users across all supported locales. Internationalization is not a cleanup step; it is a product requirement.

## Supported Locales

Current shipped locales:

- `en`
- `zh-CN`
- `zh-TW`
- `ja`
- `ko`
- `fr`
- `de`
- `es`
- `pt`
- `ru`

## Non-negotiable Rules

### 1. No user-facing hardcoded strings in components

All user-visible UI copy must go through i18n keys.

This includes:

- labels
- helper text
- headings
- button text
- badges
- status text
- empty states
- hints

### 2. New UI copy requires all locale entries

When adding a new translation key:

- update `apps/desktop/src/i18n/types.ts`
- update `en.ts`
- update every shipped locale file in the same change

Do not leave partial locale coverage for later.

### 3. Prefer reusable semantic keys

Good:

- `chat.investigationStatusReady`
- `chat.investigationEvidenceHigh`
- `search.scopeHint`

Bad:

- `chat.newHeaderThing`
- `misc.label7`
- page-specific keys for concepts already used elsewhere

### 4. Avoid engineer-only wording

Translations should describe the product in ordinary language. Favor concepts users understand over internal architecture terms.

### 5. Keep interpolation simple

Prefer:

- `{count} cited sources`
- `{selected} of {total} sources selected`

Avoid long sentences with many placeholders unless necessary.

## Review Checklist

Before merging UI work:

- search for new hardcoded strings in changed components
- verify every new translation key exists in `types.ts`
- verify every locale file includes the new key
- check that fallback English has not leaked into non-English locales for the changed area
- verify layouts still work with longer translated text

## Engineering Note

The cost of i18n discipline is paid once during development.
The cost of missing i18n is paid forever in UX inconsistency, regressions, and user confusion.
