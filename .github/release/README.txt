Leto
====

Remote desktop and game streaming, unified: one app to reach a PC that runs
Apollo or Sunshine.
https://leto.re

What is in this folder
----------------------

- leto (leto.exe on Windows): the app, to reach a PC and to share this
  one. See "Your camera on the PC" below.
- leto-cli (leto-cli.exe on Windows): the command line, and the bridge that
  serves the web client to browsers.
- dist/web: the web client, which the bridge sends to browsers.
- LICENSE.txt: the licence, GPL-3.0. The source code of this version is
  published next to this file, as leto-<version>-source.tar.gz.

Getting started
---------------

1. Start leto. It looks for your PCs on the network.
2. Choose your PC, then type the PIN that Leto shows into Apollo or
   Sunshine on the PC.
3. Choose an app to stream.

Ctrl+Alt+Shift+Q gives the keyboard and mouse back during a stream.

From a browser: open a terminal in this folder and run "leto-cli bridge".
It prints the address to open and the code the browser has to be given. The
browser warns once that the certificate is the bridge's own: go on.

Your camera on the PC
---------------------

Apollo and Sunshine take no camera. On the PC they run on, start leto,
open "This PC" (the screen button at the top of the list of PCs) and turn
"Share this PC" on. Leto reads their settings without changing them, and
takes the camera of the devices they are paired with: on Apollo, the ones
allowed to use the keyboard and the mouse. In Leto on the device, the
camera switch over a stream then works, and "This PC" shows what the camera
sees. On Windows, closing the window keeps Leto running while the PC is
shared: its icon beside the clock brings the window back, or quits. The
pictures are not a camera for the PC's apps yet. Never tried beside a real
Apollo so far: if Windows refuses to let the settings be read, start leto
as administrator, and allow it through the firewall when Windows asks.
Without a window, "leto-cli companion" does the same.

Good to know
------------

- Leto is in development. What has been tried on a real PC is listed on
  https://leto.re, under "Where Leto stands".
- The files are not signed. Windows SmartScreen may warn on the first start:
  "More info", then "Run anyway". On macOS: right-click leto, then "Open".
- The macOS and Linux builds have never been tried by hand.
- On Linux, sound goes through ALSA and gamepads through udev (the libasound2
  and libudev1 packages, present on most systems).
