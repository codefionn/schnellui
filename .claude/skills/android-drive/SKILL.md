---
name: android-drive
description: Drive a schnellui Android app on a phone or emulator over adb. Use when asked to test, click through, screenshot or reproduce a bug in the Android demo on a device.
---

You are a **tester** driving native Android views. The **dump** says what the UI is, a screenshot says how it looks. Every claim in your report rests on a dump line read after the action.

All input goes through `drive.py` beside this file. It runs a **guard** before every input: the app must be the foreground app on an awake, unlocked device. Exit code 3 with `STOP:` means the guard failed. Then you send nothing more, and you report what is in the foreground.

A phone is the user's own device. Input reaches only the app under test, and the phone ends the run with the settings it started with.

## Steps

1. **Pick the device.** `adb devices -l`, then `export ANDROID_SERIAL=<serial>`. With a phone and an emulator both attached, use the one the user named. `DRIVE_PKG` names another app than the demo. Done when `adb shell getprop ro.product.model` prints the device you mean.
2. **Install and launch.** Build and install as [android/README.md](../../../android/README.md) says, then `am start` the activity. Done when `drive.py guard` prints `ok` and the screen size.
3. **Keep it awake.** `drive.py awake on` prints `was <n>`. Keep `<n>` for step 6.
4. **Drive.** One action, then one `dump`, then compare the dump line with what you expected. Write down each result as you get it. Done when every widget or flow you were asked about has a dump line or a screenshot behind it, or is listed as not checked with the reason.
5. **Look for a crash.** `adb logcat -d -b crash` and `adb shell pidof <package>`. A trace that names `uiautomator` is the tool, not the app.
6. **Restore.** `drive.py awake restore <n>`. Done when `adb shell settings get global stay_on_while_plugged_in` prints `<n>`.

## Commands

```
drive.py guard                              ok and the screen size, or STOP
drive.py dump                               class, text, description, flags, bounds per view
drive.py show [--class C] <label>           scroll until the view is on screen
drive.py tap  [--class C] <label>           show, then tap its centre
drive.py drag [--class C] <label> <a> <b>   swipe inside the view, 0.0 to 1.0 of its width
drive.py type <text>                        into the focused field: letters, digits, _.,@-
drive.py hide-keyboard                      does nothing when no keyboard is up
drive.py scroll down|up                     one page step without a dump
drive.py tap-xy <x> <y>                     for views the dump does not list
drive.py shot <file.png>                    screenshot, in the session scratch directory
drive.py awake on | awake restore <n>
```

`<label>` is the exact text or content description. A SeekBar, a Checkbox and an EditText carry their label as the description. Flags are `c` checked, `s` selected, `d` disabled. `--class` takes the short class name from the dump and separates a Button from a tab of the same text.

## What the dump does not show

- **An animating view on screen.** An indeterminate `Loading` never goes idle, so `dump` exits 4 with `could not get idle state`. `show` scrolls past it by itself. To read a view beside it, `scroll` until the animation has left the screen.
- **Popup windows.** The option list of a Dropdown is its own window. A stock emulator may list the options, One UI does not. Take a `shot`, read the option's position from the picture and use `tap-xy`. The picture's pixels are the tap coordinates.
- **Changes that need a frame.** A background-thread update shows in the next dump, a second later.

## Device behaviour that looks like an app bug

- **BACK with no keyboard up closes the activity.** Use `hide-keyboard`, never a bare BACK, to dismiss a keyboard.
- **A swipe across the keyboard types words.** `show`, `tap` and `drag` hide the keyboard first, so type right after the tap that focused the field.
- **The edges are gesture zones.** A swipe that starts at the bottom or side edge navigates the system. `drive.py` swipes mid-screen. Before a `drag` on a view that sits at an edge, `show` moves it inward.
- **One dump at a time.** Two `drive.py` calls in parallel make one fail and leave a crash-log entry from `uiautomator`. Run commands one after another.
- **Rotation changes every coordinate.** Dump again after the device rotated. Leave the rotation and the night mode of a phone as they are, and ask the user to change them.
- **A link opens another app.** The guard then stops the run. On a phone, check that the link renders and leave the tap to the user.
