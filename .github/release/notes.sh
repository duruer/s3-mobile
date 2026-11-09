#!/usr/bin/env bash
# semantic-release "generateNotes" step: the end-user part of every release text, printed after the list of
# changes (version in $1).
# SPDX-License-Identifier: GPL-3.0-or-later
set -euo pipefail
V="$1"
cat <<EOF

### Download

- **Android 8.0+ (64-bit phones):** \`s3-mobile-$V-android-arm64.apk\` below. Open it on your phone and allow the install.
- **iPhone / iPad (iOS 17+):** \`s3-mobile-$V-ios-unsigned.ipa\` below (added a few minutes after the release). Install it from a computer with a sideloading tool such as Sideloadly or AltStore and your own Apple ID: [how](https://github.com/duruer/s3-mobile#iphone).

**You need your own copy of The Settlers III** (GOG "Ultimate Collection", version 1.60). The app contains no game files. After installing, open **Game files** in the app and copy your game folder to the phone: [step by step](https://github.com/duruer/s3-mobile#get-started-on-android).

**Updating:** install the new APK over the old one. Do not uninstall first: that deletes your game files and save games.

S3 Mobile is an unofficial fan project, not affiliated with, endorsed or sponsored by Ubisoft, Blue Byte or GOG. "The Settlers" is a trademark of Ubisoft Entertainment. Free software under GPL-3.0-or-later, no warranty.
EOF
