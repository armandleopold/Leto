Leto
====

Remote desktop and game streaming, unified: one app to reach a PC that runs
Apollo or Sunshine.
https://leto.re

What is in this folder
----------------------

- leto (leto.exe on Windows): the app.
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

Good to know
------------

- Leto is in development. What has been tried on a real PC is listed on
  https://leto.re, under "Where Leto stands".
- The files are not signed. Windows SmartScreen may warn on the first start:
  "More info", then "Run anyway". On macOS: right-click leto, then "Open".
- The macOS and Linux builds have never been tried by hand.
- On Linux, sound goes through ALSA and gamepads through udev (the libasound2
  and libudev1 packages, present on most systems).
