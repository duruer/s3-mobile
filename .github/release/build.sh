#!/usr/bin/env bash
# semantic-release "prepare" step (.releaserc.json): build the release APK for version $1 into dist/.
# Needs S3_KEYSTORE + S3_KEYSTORE_PASS (the release key; build-apk.sh refuses to sign with a new one).
# SPDX-License-Identifier: GPL-3.0-or-later
set -euo pipefail
VERSION="$1"
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
: "${S3_KEYSTORE:?release key missing}"
S3_VERSION="$VERSION" bash "$ROOT/port/scripts/build-apk.sh" --abi arm64-v8a
mkdir -p "$ROOT/dist"
cp "$ROOT/port/target/apk/settlers3port.apk" "$ROOT/dist/s3-mobile-$VERSION-android-arm64.apk"
ls -l "$ROOT/dist"
