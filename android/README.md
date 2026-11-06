# Pocket Agent — Pixel 2 shell (FEAT-DEV3)

Kotlin APK (minSdk 30): a device page listing the computers the phone is
paired with, a full-screen WebView running the chosen PA's own frontend
(`http://<computer>:8650/ui/`), and a foreground service that keeps a `/phone`
link to every paired PA and, for the one using the phone, streams the
microphone to `/audio` and plays PA's replies. A CameraX preview (`Camera.kt`)
sits over the area the page asks for; it can flip, sends the current frame
(long side ~1280 px) when PA asks for a snapshot, and closes when the app goes
to the background or the screen locks. Protocol: `src-tauri/src/api/remote.rs`;
full reference: `docs/API_MANUAL.md` (「手机终端」).

## Build

**Experimental.** There is no store release: you build the debug APK and
install it over `adb`. On the phone, turn on **Developer options** (tap
*Settings → About phone → Build number* seven times), then **USB debugging**
(or **Wireless debugging**), and accept the computer's debugging prompt.

Needs JDK 17 + Android SDK (platform 34, build-tools) + platform-tools (`adb`).
The Gradle wrapper (8.7) is checked in:

```sh
cd android
./gradlew assembleDebug
adb install -r app/build/outputs/apk/debug/app-debug.apk
```

## Use

The app's own screens (device list, scanner, notification) follow the phone's
system language: English, 中文, 日本語, 한국어, Français, Deutsch, Español; other
languages show English. Labels below are the English ones.

1. **Pair a computer** (once per computer). On the computer open Pocket Agent
   Settings → Devices → Add phone: a QR code shows for 2 minutes. On the phone
   tap Scan to add device and point the camera at it (the camera permission is asked
   the first time). The phone checks that the computer really holds the code's
   secret, then stores a device token for that computer only. Pair as many
   computers as you like (e.g. a Mac and a Windows PC). Grant the microphone too.
2. **Problems.** A failed pairing stays on screen with the reason and Scan again.
   "QR code expired": the code expired, was used, or a newer one
   replaced it — click Add phone again. "Can't reach the computer": phone and computer
   must be on the same LAN (on Windows allow TCP 8650, see docs/windows-install.md).
   A computer that got a new LAN address: show its QR code again and scan; the
   phone updates the existing entry instead of adding a second one.
3. **Switch.** The phone serves one computer at a time. Each row shows the
   computer's name and Online / Offline / In use. Connect hands the phone to that
   computer and opens its page. While one computer uses the phone, every other
   one is greyed out with "Ask "X" to return to the computer first" and its requests are refused —
   from the computer ("切到手机") and from the phone alike. The phone is free again when:
   - the computer using it moves back to its desk: 「移到电脑」 on the page, or
     "切回电脑" said to that computer (the page closes, the device page returns);
   - you tap Disconnect on the notification or swipe the app away;
   - that computer has been offline for 90 seconds (a short Wi-Fi drop does not free it).
   Saying "切到手机" to a free computer works with the app on screen: the
   phone answers on its own and opens that computer's page.
   Long-press a row to remove the computer from the phone; Settings → Devices on
   the computer removes the phone there (its connections close at once).
4. **Each computer has its own conversation.** Every PA talks to its own
   Hermes, so switching computers switches history and context too.
5. Back on a PA's page returns to the device page. Old setups (one address +
   key) become a hand-entered entry after updating and keep working; nothing to
   re-enter. New computers can only be added by scanning.
6. Home and screen lock keep the voice on the phone (the service keeps running).
   Swiping the app away from recents, or Disconnect on its notification, stops the
   service (and with Disconnect also the camera and the page): PA goes back to the
   computer's mic and speaker. Opening the app again reconnects. PA only uses the
   phone's mic and speaker while the phone page is driving. The notification
   shows "Using <computer>".
   With the screen off, the wake word (owner's voice) turns the screen on and
   shows PA over the lock screen, without unlocking — the page is frozen while
   hidden, so it has to come forward. The wake word and every reply keep the
   screen on for 3 more minutes; after that the lock screen comes back and the
   screen goes off as usual. Plugged in, the screen stays on.
7. Debug: `adb logcat -s PocketAgent`, `adb exec-out screencap -p > shot.png`.
8. Without a phone: `scripts/phone_sim.py hello.wav` plays the phone's part on
   `/audio` (`--camera test.jpg` answers snapshots with a fixed image);
   `scripts/phone_sim.py --pa <PA1> --pa <PA2> --scenario` plays a phone paired
   with two PAs on `/phone` and checks the who-uses-the-phone rules.

Keep 8650 on the LAN — never port-forward it: `/ui` can approve computer actions on the Mac.
