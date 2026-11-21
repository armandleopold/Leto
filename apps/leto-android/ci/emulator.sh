#!/usr/bin/env bash
# The emulator check of the pipeline (.github/workflows/android.yml): the
# APK on an Android emulator, against the stand-in PC on the runner, which
# the emulator reaches at 10.0.2.2. It installs the app, adds the PC by its
# address, pairs, starts Desktop, touches the picture, presses a gamepad's
# A and asks the stand-in PC what it received. Screenshots go to shots/, the logs are printed.
set -uo pipefail
cd "$(dirname "$0")/../../.."
screen() { python3 apps/leto-android/ci/screen.py "$@"; }
step() { echo "::group::$*"; }
done_() { echo "::endgroup::"; }
failed=0
fail() { echo "::error::$*"; failed=1; }

mkfifo host-in
./target/release/examples/serve --port 47989 --announce < host-in > host.log 2>&1 &
exec 3> host-in

step "Install and start"
adb install -r dist/android/leto.apk
adb logcat -c
adb shell am start -n re.leto.app/android.app.NativeActivity
sleep 15
screen shot 1-started
adb shell pidof re.leto.app || fail "the app is not running"
done_

step "Add the PC by its address"
# The first start makes this device's key, which takes the emulator, that
# translates the arm64 app, up to half a minute: wait for the first screen.
for try in $(seq 12); do
  screen accent 2-pcs && break
  [ "$try" = 12 ] && fail "no Add a PC button"
  sleep 5
done
sleep 3
adb shell input text 10.0.2.2
sleep 1
screen shot 3-address
adb shell input keyevent 66
sleep 8
done_

step "Pair"
# Adding a PC goes on to pairing with it; otherwise it is in the list.
if ! pin=$(screen pin 4-pin | tail -1); then
  screen tap 4-pc Salon || fail "the stand-in PC is not listed"
  sleep 6
  pin=$(screen pin 5-pin | tail -1) || fail "no PIN on the screen"
fi
echo "PIN read: $pin"
echo "$pin" >&3
sleep 10
screen shot 6-paired
done_

step "Stream"
screen tap 7-apps Desktop || fail "no Desktop app"
sleep 20
screen shot 8-stream
# A stream turns the app to landscape.
shape=$(identify -format "%w %h" shots/8-stream.png)
echo "the stream's screen is $shape"
[ "${shape% *}" -gt "${shape#* }" ] || fail "the stream is not in landscape"
# The screen as the stream holds it: in landscape, not as Android's
# natural size says. Taps off it land on the system's gestures.
width=${shape% *}; height=${shape#* }
adb shell input tap $((width / 2)) $((height / 2))
adb shell input swipe $((width / 3)) $((height / 2)) $((width * 2 / 3)) $((height / 2)) 500
# A gamepad's A, which Android tells of in the window's input events.
adb shell input gamepad keyevent KEYCODE_BUTTON_A
sleep 3
screen shot 9-touched
echo inputs >&3
sleep 2
done_

step "What the stand-in PC saw"
cat host.log
# The count, not the help text, which names the mouse too.
grep -q "Mouse" host.log || fail "the PC heard no touch"
grep -q "GamepadArrived" host.log || fail "the PC heard of no gamepad"
grep -q "buttons: GamepadButtons(1)" host.log || fail "the PC heard no A"
done_

step "The app's log"
adb logcat -d -s RustStdoutStderr:* main:* AndroidRuntime:E DEBUG:* > app.log
tail -400 app.log
# The emulator's decoders stand in for a phone's video hardware.
grep -E "is decoded (by|on the)" app.log || fail "the app never said who decodes"
done_

# The screenshots are kept by the workflow, and printed small too, for
# whoever reads only the log.
step "Screenshots, small, as base64 JPEG"
for shot in shots/*.png; do
  case "$shot" in *.flipped.png|*.digits.png) continue ;; esac
  echo "=== $shot"
  convert "$shot" -resize 480x -quality 60 jpg:- | base64 -w 0
  echo
done
done_

exec 3>&-
exit $failed
