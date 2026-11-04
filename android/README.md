# Pocket Agent — Pixel 2 shell (FEAT-DEV3)

Kotlin APK (minSdk 30): a full-screen WebView running PA's own frontend from the
Mac (`http://<Mac>:8650/ui/`) plus a foreground service that streams the phone
microphone to `/audio` and plays PA's replies. A CameraX preview (`Camera.kt`)
sits over the area the page asks for; it can flip, sends the current frame
(long side ~1280 px) when PA asks for a snapshot, and closes when the app goes
to the background or the screen locks. Protocol: `src-tauri/src/api/remote.rs`;
full reference: `docs/API_MANUAL.md` (「手机终端」).

## Build

Needs JDK 17 + Android SDK (platform 34, build-tools) + platform-tools (`adb`).
The Gradle wrapper (8.7) is checked in:

```sh
cd android
./gradlew assembleDebug
adb install -r app/build/outputs/apk/debug/app-debug.apk
```

## Use

1. First launch: enter the Mac's LAN address (port 8650 is implied) and `API_SERVER_KEY`
   from `~/.pocket-agent/.env`. Grant the microphone (the camera is asked for
   the first time Hermes opens it).
2. Back opens the connection form again.
3. Debug: `adb logcat -s PocketAgent`, `adb exec-out screencap -p > shot.png`.
4. Without a phone: `scripts/phone_sim.py hello.wav` plays the phone's part on
   `/audio` (`--camera test.jpg` answers snapshots with a fixed image).

Keep 8650 on the LAN — never port-forward it: `/ui` can approve computer actions on the Mac.
