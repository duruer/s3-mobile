<p align="center">
  <img src="docs/assets/logo.png" width="128" height="128" alt="S3 Mobile logo">
</p>

<h1 align="center">S3 Mobile</h1>

<p align="center">
  <b>Play your own copy of The Settlers III on an Android phone or an iPhone.</b><br>
  Unofficial fan project. Free and open source. No game files included.
</p>

<p align="center">
  <a href="https://github.com/duruer/s3-mobile/releases/latest"><img alt="Download" src="https://img.shields.io/github/v/release/duruer/s3-mobile?label=download&color=2b8a4a"></a>
  <img alt="Android 8.0+" src="https://img.shields.io/badge/Android-8.0%2B-3ddc84">
  <img alt="iOS 17+" src="https://img.shields.io/badge/iOS-17%2B-lightgrey">
  <a href="LICENSE"><img alt="License: GPL-3.0" src="https://img.shields.io/badge/license-GPL--3.0-blue"></a>
</p>

---

## What is this?

S3 Mobile is an app that lets you play the classic PC game **The Settlers III** on your phone, using the game
files from a copy **you already own**.

It does not remake or change the game. It runs the original game program from your copy, so the campaigns, maps,
music and save games are exactly the ones you know. The app only adds what a phone needs: touch controls, a
layout that fits a wide phone screen, and an easy way to copy your game onto the phone.

## What you need

- **Your own copy of The Settlers III.** The supported version is the **GOG release ("Ultimate Collection",
  version 1.60)**, which you can buy at [gog.com](https://www.gog.com/). Other versions may not work yet.
  The app never downloads or contains any part of the game.
- A computer (Windows, macOS or Linux) with the game installed, to copy the game folder from.
- **Android:** a phone with Android 8.0 or newer (64-bit). A recent phone gives the smoothest play.
- **iPhone / iPad:** iOS 17 or newer, plus a computer to install the app from. See [iPhone](#iphone) below.

## Get started on Android

1. Download the latest **`.apk`** file from the [Releases page](https://github.com/duruer/s3-mobile/releases/latest)
   on your phone.
2. Open it. Android asks if your browser may install apps: allow it, then tap **Install**.
3. Open **S3 Mobile** and tap **Game files**.
4. Copy your game onto the phone. The easiest way:
   - Tap **Start upload server**. The app shows a web address and a QR code.
   - On your computer (in the same Wi-Fi), open that address in a web browser.
   - Choose your Settlers III folder (the folder that contains `S3.EXE`) and press **Upload**.
     The game is about 1.7 GB, so this takes a few minutes. Keep the page open until it says it is done.

   You can also copy the folder with a USB cable into `Android/data/com.settlers3port.app/files/game/`.
5. When the app says **Ready**, go back and tap **Play**.

> **Updating:** install a newer `.apk` over the old one. **Do not uninstall the app first:** uninstalling
> deletes the game files and save games stored inside it.

## iPhone

There is no App Store version. An iPhone only runs apps from the App Store or apps signed with your own Apple ID,
so the iPhone version is installed from a computer:

1. Download the latest **`.ipa`** file from the [Releases page](https://github.com/duruer/s3-mobile/releases/latest)
   to your computer.
2. Install it with a sideloading tool such as [Sideloadly](https://sideloadly.io/) or
   [AltStore](https://altstore.io/): connect the iPhone, drop the `.ipa` in, sign in with your own Apple ID.
3. On the iPhone, allow the app under *Settings > General > VPN & Device Management*, and turn on
   *Settings > Privacy & Security > Developer Mode* if iOS asks for it.

With a free Apple ID the app stops opening after 7 days and must be installed again the same way. Your game
files and save games stay on the phone. Developers can also build it themselves: [docs/porting/ios.md](docs/porting/ios.md).

Copying the game works like on Android: **Game files > Start upload server**, or put the folder into the Files
app under *On My iPhone > S3 Mobile > Settlers3*, or pick a folder you already have in the Files app.

## How to play

- **Tap** to select a settler or a building, tap the ground to send units there.
- **Drag with one finger** to move the map. **Pinch** to zoom in and out.
- **Long press and drag** to select several units at once. **Tap with two fingers** to cancel.
- Two control styles, switchable in the game menu:
  - **Classic Touch**: the original side panel, made bigger and easier to hit with a finger.
  - **Mobile HUD**: the map fills the screen, with a bottom bar and build menus made for phones.
- Music, sound effects, saving and loading work as in the original game. On Android a running game keeps going
  (muted) when you switch to another app.
- **Settings** (in the app's start screen) has the display mode, frame rate limit and more.

## Status

This is an early, experimental release. Single player games, saving and loading, music and
sound work. Multiplayer over the local network is being tested. Expect some rough edges; reports are welcome in
[Issues](https://github.com/duruer/s3-mobile/issues).

Things to know:

- The game runs inside an emulator, which needs a fairly fast phone. On a recent phone the game reaches 60 frames
  per second; older phones are slower.
- Please do not attach game files, screenshots of copyrighted game content or parts of the game program to
  issues.

## Legal

- **S3 Mobile is not affiliated with, endorsed by or sponsored by Ubisoft, Blue Byte or GOG.**
  "The Settlers" is a trademark of Ubisoft Entertainment. All other trademarks belong to their owners. The names
  are used only to say which game this app works with.
- **This project contains no part of the game:** no game program, graphics, sounds, music, maps or text, and no
  code taken from the game. You need your own legally obtained copy and you copy it to your phone yourself.
- The app icon and all code in this repository are our own work.
- S3 Mobile is free software under the [GNU General Public License v3.0 or later](LICENSE). It comes with no
  warranty.

## For developers

S3 Mobile is written in Rust. It loads the original Windows game program, runs it on a built-in x86 processor
emulator and provides its own versions of the Windows and DirectX functions the game calls. Start here:

- [AGENTS.md](AGENTS.md): project index and rules
- [docs/porting/architecture.md](docs/porting/architecture.md): how it works, how to build and run
- [docs/porting/android.md](docs/porting/android.md) and [docs/porting/ios.md](docs/porting/ios.md): building the apps
- [docs/porting/status.md](docs/porting/status.md): what works, with measurements

Contributions are welcome. The same rule applies to code as to issues: never commit game files or code
derived from the game.
