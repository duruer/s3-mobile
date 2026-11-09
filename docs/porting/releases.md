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

**iOS asset** (since 2026-11-09): after a new release the `ios` job calls [`ios.yml`](../../.github/workflows/ios.yml)
(macOS runner, `macos-15`), which runs [`.github/release/build-ipa.sh`](../../.github/release/build-ipa.sh) on the
release tag and uploads `s3-mobile-<version>-ios-unsigned.ipa`:

- no xtool and no Apple account: `cargo rustc -p s3-ios --crate-type staticlib -- --print native-static-libs`
  (`CARGO_TERM_COLOR=never`, the line is parsed), `swiftc -parse-as-library` over `port/ios/Sources/S3Mobile/*.swift`
  with a generated `module.modulemap` for `CS3iOS`, bundle = our `Info.plist` + the keys `xtool dev build` adds
  (checked against an xtool bundle) + `AppIcon.png`, ad-hoc `codesign -s -`, zip `Payload/`.
- No provisioning profile, Team ID or UDID inside (an xtool build has them, so never publish that one). Users
  re-sign with Sideloadly / AltStore and their own Apple ID.
- Version: semantic-release `successCmd` writes `version=` to `$GITHUB_OUTPUT`; job output of `release`.
- Manual test build: Actions -> ios -> Run workflow (`version` e.g. `0.0.0-test`, `upload` off) -> artifact `ipa`.
- macOS arm64 runners can be short: one run waited 15 min and was cancelled ("not acquired by Runner"); rerun it.
- First CI build 2026-11-09: 4 MB `.ipa`; NOT yet installed on a phone through Sideloadly/AltStore.

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
