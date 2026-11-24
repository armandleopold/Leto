# Changelog

## [0.0.9](https://github.com/armandleopold/Leto/compare/v0.0.8...v0.0.9) (2026-11-24)


### Bug fixes

* a stream on a shaky network shows what arrives and warns only after ten seconds ([#87](https://github.com/armandleopold/Leto/issues/87)) ([ad59866](https://github.com/armandleopold/Leto/commit/ad59866ec08187f7eda1eb31adbdc0c1b91a902b))

## [0.0.8](https://github.com/armandleopold/Leto/compare/v0.0.7...v0.0.8) (2026-11-24)


### Features

* **android:** the app draws as fast as its screen shows, 120 Hz on a Pixel ([#83](https://github.com/armandleopold/Leto/issues/83)) ([f7f6f45](https://github.com/armandleopold/Leto/commit/f7f6f452c00b608ed802afb76c5b3f4accad0fc1))
* starting Leto again shows the one that runs instead of a second ([#85](https://github.com/armandleopold/Leto/issues/85)) ([04280e3](https://github.com/armandleopold/Leto/commit/04280e3fb14a0bcc667dafc2d04e971f285cef19))


### Bug fixes

* **app:** draw the logo from its SVG so it stays sharp ([#84](https://github.com/armandleopold/Leto/issues/84)) ([c70be47](https://github.com/armandleopold/Leto/commit/c70be4728cfe76b427e1f6de569b230f4bb18fbe))

## [0.0.7](https://github.com/armandleopold/Leto/compare/v0.0.6...v0.0.7) (2026-11-23)


### Features

* **host:** a device's microphone reaches the PC uncompressed, beside Apollo ([#81](https://github.com/armandleopold/Leto/issues/81)) ([5f68fac](https://github.com/armandleopold/Leto/commit/5f68faccf211c52f4396c87e9595db99239b968c))

## [0.0.6](https://github.com/armandleopold/Leto/compare/v0.0.5...v0.0.6) (2026-11-23)


### Features

* **host:** a card for each device of Apollo on the first screen, with its own page ([#76](https://github.com/armandleopold/Leto/issues/76)) ([e8ccd3d](https://github.com/armandleopold/Leto/commit/e8ccd3d83da46c3cb77b07be9a95834a7e6798e1))

## [0.0.5](https://github.com/armandleopold/Leto/compare/v0.0.4...v0.0.5) (2026-11-22)


### Bug fixes

* **android:** draw a picture from its part of the decoder's buffer, without the green lines under it ([#77](https://github.com/armandleopold/Leto/issues/77)) ([01b3894](https://github.com/armandleopold/Leto/commit/01b389486197fbf8a91d39d86740267d7d5f37ce))

## [0.0.4](https://github.com/armandleopold/Leto/compare/v0.0.3...v0.0.4) (2026-11-22)


### Features

* **android:** a stream has the whole screen, without Android's bars, and its keyboard shortcuts ([#75](https://github.com/armandleopold/Leto/issues/75)) ([04a1632](https://github.com/armandleopold/Leto/commit/04a1632914e5b932f8637f283802791d0c09eecf))
* **stream:** show whether the picture travels encrypted, and note it in the log ([#71](https://github.com/armandleopold/Leto/issues/71)) ([6adc85e](https://github.com/armandleopold/Leto/commit/6adc85ea466492dec9acbdae7957d41ada0a3fee))


### Bug fixes

* **android:** full screen on a monitor without the freeze, edges of the PC's screen within reach, and the Windows key sent to the PC ([#73](https://github.com/armandleopold/Leto/issues/73)) ([94dd7fd](https://github.com/armandleopold/Leto/commit/94dd7fd0dac4d719d11b9158ed8c992d056a0459))
* **android:** smooth sound and pictures on a Fire TV ([#72](https://github.com/armandleopold/Leto/issues/72)) ([c927d1c](https://github.com/armandleopold/Leto/commit/c927d1cc12969b52481ea0bcf792380658863a77))

## [0.0.3](https://github.com/armandleopold/Leto/compare/v0.0.2...v0.0.3) (2026-11-22)


### Features

* **android:** run on Fire TV and Android TV ([#59](https://github.com/armandleopold/Leto/issues/59)) ([415778c](https://github.com/armandleopold/Leto/commit/415778c0d19e3247e86b71743af4671463d276a4))
* **android:** stream to the monitor a phone is plugged into, in full screen, with its mouse ([#60](https://github.com/armandleopold/Leto/issues/60)) ([306e757](https://github.com/armandleopold/Leto/commit/306e757f4127704db02dad8c602b38e048c13fdd))
* **host:** a device's camera and microphone for the apps of the shared PC ([#65](https://github.com/armandleopold/Leto/issues/65)) ([ced22fe](https://github.com/armandleopold/Leto/commit/ced22fe1c238d6b437a3d561dcce0a5463aa015a))
* **host:** share this PC from the Leto app, beside Apollo, for a client's camera ([#58](https://github.com/armandleopold/Leto/issues/58)) ([25c9ea2](https://github.com/armandleopold/Leto/commit/25c9ea2d8bbfeed25113cccd01930cb0f423a172))
* navigate the app with a gamepad, the mouse's back button and Android's back gesture ([#57](https://github.com/armandleopold/Leto/issues/57)) ([adff700](https://github.com/armandleopold/Leto/commit/adff700ad6229faf87d56cb00c2ef98ba8779fd1))
* show WireGuard or LAN beside a PC in the list ([#68](https://github.com/armandleopold/Leto/issues/68)) ([228f8d7](https://github.com/armandleopold/Leto/commit/228f8d729a18a714ce0374aa3297f6fe328788cb))
* **tunnel:** say why the WireGuard server does not answer ([#67](https://github.com/armandleopold/Leto/issues/67)) ([1d927d6](https://github.com/armandleopold/Leto/commit/1d927d66ee50da13d680d9e8bb4e90c169d118ae))


### Bug fixes

* always draw decoded pictures from the graphics card on Windows ([#61](https://github.com/armandleopold/Leto/issues/61)) ([9a32be8](https://github.com/armandleopold/Leto/commit/9a32be8d62bbf991f8a7a74e0d4158dbff84a794))
* **android:** navigate the settings with a remote or gamepad, and no AV1 by default on a TV ([#66](https://github.com/armandleopold/Leto/issues/66)) ([d969b9a](https://github.com/armandleopold/Leto/commit/d969b9abf6f8f3efd40ab4734db9da09bfa294bc))
* **android:** send the PC where a key sits, whatever layout Android was told ([#69](https://github.com/armandleopold/Leto/issues/69)) ([d7f4260](https://github.com/armandleopold/Leto/commit/d7f4260a00611af5962be141941d1fab7e4ea066))
* stop showing HDR on apps, as Leto never streams in HDR ([#64](https://github.com/armandleopold/Leto/issues/64)) ([2a2d5da](https://github.com/armandleopold/Leto/commit/2a2d5da54c109641a69f90eda7501e18e7d7f5b8))

## [0.0.2](https://github.com/armandleopold/Leto/compare/v0.0.1...v0.0.2) (2026-11-21)


### Features

* **android:** decode on the phone's video hardware, and scale the PC's screen ([#53](https://github.com/armandleopold/Leto/issues/53)) ([3502f99](https://github.com/armandleopold/Leto/commit/3502f997052dffe46027c17f405026019838d63e))
* **android:** gamepads, and an automatic resolution a phone decodes ([#51](https://github.com/armandleopold/Leto/issues/51)) ([8f9f79c](https://github.com/armandleopold/Leto/commit/8f9f79c6d89b5a78d439bf64a8e4333f6c9f73e9))


### Bug fixes

* ask again for a stream that connects and brings no picture ([#54](https://github.com/armandleopold/Leto/issues/54)) ([bd9b288](https://github.com/armandleopold/Leto/commit/bd9b288c2522dae38244a34b129d09356bfa3d6c))
* remember what was asked of the PC's pointer when Leto starts again ([#55](https://github.com/armandleopold/Leto/issues/55)) ([388a66f](https://github.com/armandleopold/Leto/commit/388a66fa84ac6459e33e4328ef9e33daeb08030d))
