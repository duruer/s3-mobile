# Releases (automatic, semantic-release)

Since 2026-11-09 every push to `main` runs [`.github/workflows/release.yml`](../../.github/workflows/release.yml):

1. **test** (also on pull requests): `cargo test --workspace --release` on ubuntu-24.04.
2. **release** (push to `main` only, after test): [semantic-release](https://semantic-release.gitbook.io/) with
   [`.releaserc.json`](../../.releaserc.json) reads the commits since the last `v*` tag.

## Commit messages decide the version

[Conventional Commits](https://www.conventionalcommits.org/). The version is `MAJOR.MINOR.PATCH`:

| Commit | Release |
| --- | --- |
| `feat: ...`, `feat(ios): ...` | minor (0.1.0 -> 0.2.0) |
| `fix: ...`, `perf: ...` | patch (0.1.0 -> 0.1.1) |
| `feat!: ...` or a `BREAKING CHANGE:` footer | major |
| `docs:`, `chore:`, `refactor:`, `test:`, `ci:`, `build:` | no release |

Release notes = the `feat`/`fix`/`perf` subjects (sections New / Fixed / Faster) plus the fixed end-user text from
[`.github/release/notes.sh`](../../.github/release/notes.sh) (download, own game copy, update warning, legal
notice). Write `feat`/`fix` subjects so a player understands them.

## What the release job builds

[`.github/release/build.sh`](../../.github/release/build.sh) -> `port/scripts/build-apk.sh --abi arm64-v8a` with
`S3_VERSION=<version>`:

- launcher shows `v<version>` (`s3_frontend::VERSION`, `option_env!("S3_VERSION")`; local builds show the workspace
  version from `port/Cargo.toml`),
- APK `versionName` = version, `versionCode` = `major*10000 + minor*100 + patch` (aapt2 `--version-name/--version-code`;
  needs `--replace-version`, without it aapt2 keeps the manifest's `versionCode="1"`/`0.1.0`; v0.1.1 shipped like that),
- asset `dist/s3-mobile-<version>-android-arm64.apk`, attached to the GitHub release `v<version>`.

Nothing is committed back to `main` (no version bump commits); the tag is the source of truth.

No iOS asset: an xtool build is signed with a personal Apple ID profile (Team ID + device UDID inside).

## Signing key

The APK must always be signed with the **same key** as earlier releases, or Android refuses the update and the user
has to uninstall (= loses game files and saves). The key is the original `port/target/debug.keystore`
(gitignored; backup outside the repo). In CI it comes from repository secrets:

| Secret | Value |
| --- | --- |
| `ANDROID_KEYSTORE_B64` | `base64 -w0 port/target/debug.keystore` |
| `ANDROID_KEYSTORE_PASS` | the store/key password |

`build-apk.sh` with `S3_KEYSTORE` set fails instead of generating a new key; the workflow fails if the secret is
missing.

Set them with `gh secret set ANDROID_KEYSTORE_B64 --repo duruer/s3-mobile < <(base64 -w0 port/target/debug.keystore)`
and `gh secret set ANDROID_KEYSTORE_PASS --repo duruer/s3-mobile`.

## Manual release

`v0.1.0` was made by hand (`gh release create`). To re-run a release: Actions -> release -> Run workflow.

## Local builds after a release

A local `build-apk.sh` without `S3_VERSION` has `versionCode` 1, lower than any release, so `adb install -r` over an
installed release fails (downgrade). Build with `S3_VERSION=<release version or higher>` or install with
`adb install -r -d`. Never uninstall instead (deletes the game files).

The key password secret is the word `android`, so GitHub masks that word as `***` in the job logs (cosmetic).
