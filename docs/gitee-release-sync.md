# GitHub to Gitee release sync

Nexa publishes releases from GitHub Actions and mirrors the resulting release to Gitee.

## Repositories

- GitHub: `MLGBJDLW/Nexa`
- Gitee: `ButlerW/Nexa`
- Main branch: `master`

## Required GitHub configuration

Create a Gitee personal access token that can write to `ButlerW/Nexa`, then add it as a GitHub Actions repository secret named `GITEE_TOKEN`.

If the token owner is not `ButlerW`, add an Actions repository variable named `GITEE_USERNAME` with the token owner's Gitee username. Tokens created by `ButlerW` do not need this variable.

## Workflows

- `Sync ref to Gitee` pushes GitHub `master` and release tags to `https://gitee.com/ButlerW/Nexa`.
- `Sync GitHub release to Gitee` creates or updates the matching Gitee release and uploads the same release assets produced by GitHub Actions.
- `Release` generates `latest.json`, `latest-ghproxy.json`, and `latest-gitee.json`, publishes the verified GitHub release, then starts the Gitee release sync workflow.

## Gitee settings

After verifying that `Sync ref to Gitee` succeeds, disable Gitee's original GitHub pull mirror. Do not keep both the Gitee pull mirror and the GitHub Actions push mirror enabled, because the pull mirror can create a release tag before GitHub Actions pushes it and prevent Gitee Go from receiving a new tag push event.

Configure the Gitee Go release pipeline to listen only for tag pushes matching:

```yaml
triggers:
  push:
    tags:
      include:
        - '^v[0-9]+\.[0-9]+\.[0-9]+([-.].*)?$'
```

Do not add branch or commit-message filters to this pipeline; those filters can prevent a tag push from triggering the release pipeline.

## Manual test

Run `Sync GitHub release to Gitee` from GitHub Actions and provide an existing tag such as `v0.9.11`. The matching Gitee release should contain the Windows, Linux, and macOS installers, signature files, `latest.json`, and `latest-gitee.json`.
