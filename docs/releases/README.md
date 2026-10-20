# Releases

OpenAlgo Desktop keeps its release notes in one file, not one page per
version. This folder only points at it and at the documents that describe how
a release is made.

In the desktop, a release is a set of installers on the
[GitHub Releases page](https://github.com/marketcalls/openalgo-desktop/releases),
not a `git pull` on a server: OpenAlgo web's `docs/releases/version-*.md`
pages and its upgrade scripts have no desktop counterpart.

## What changed in each version

[CHANGELOG.md](../../CHANGELOG.md) at the root of the repository. Each version
has a `## <version>` section, newest first, written for a trader; an
`## Unreleased` section at the top collects what has landed since the last
version. The in-app update notice shows the section of the version it offers,
and the release page carries the same text.

## Getting a new version

| You want to | Read |
| --- | --- |
| Know how the app tells you about a new version, and what Update does | [Updating OpenAlgo Desktop](../user/updating.md) |
| Install a version by hand, or check a download | README, [Download and install](../../README.md#download-and-install) |
| Go back to an earlier version | [Going back to an earlier version](../user/updating.md#going-back-to-an-earlier-version) |
| Know what happens to your data the first time a new version opens it | [Your data is backed up before an update](../user/updating.md#your-data-is-backed-up-before-an-update) and, for developers, [Data-folder upgrades](../port/data-upgrades.md) |

## How a release is made (maintainers)

| Step | Read |
| --- | --- |
| Bump the version in every file that carries it and write the `CHANGELOG.md` section | the `version-bump` skill, [.claude/skills/version-bump/SKILL.md](../../.claude/skills/version-bump/SKILL.md) |
| What the release workflow checks before it builds (a green `ci-ok` on the tagged commit, the installed-app jobs, a fresh advisory scan, matching versions) and the repository rules that back it | [Branch and tag protection](../release/branch-protection.md) |
| What the workflow produces (installers, update bundles, signatures, `latest.json`, `SHA256SUMS.txt`), why publishing the draft is what makes an update visible, and the signing key | [In-app updates](../release/updates.md) |
| The hands-on checklist for each release and each supported system | [Platform qualification](../release/platform-qualification.md) |
| Certifying a broker with a real account before Live trading is offered for it | [Broker certification](../release/broker-certification.md) |

The workflow itself is `.github/workflows/release.yml`. Pushing the `v*` tag
that starts it is the maintainer's decision, never an agent's.
