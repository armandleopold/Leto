# Leto brand assets

The logo is called "Eclipse": a crescent moon holding a sun. Leto is the mother
of Artemis (the moon) and Apollo (the sun), and the crescent also draws a
rounded L. The palette is called "Braise".

## Colours

| Role   | Hex       | Use                                                    |
|--------|-----------|--------------------------------------------------------|
| Ground | `#151312` | App background; ink and moon on light surfaces         |
| Sun    | `#FF6B35` | The sun disc; the accent colour of the interface       |
| Moon   | `#F2EDE4` | Crescent and wordmark on dark surfaces; light surfaces |

The same values live in code in `crates/leto-core/src/brand.rs`. Change both
together.

## Files

- `svg/`: vector sources. Symbol, app icon, logo (symbol and wordmark) and
  wordmark alone, each for dark and light surfaces, plus one-colour symbols.
- `png/`: the app icon from 16 to 1024 px and the logo, on transparent
  backgrounds.
- `windows/leto.ico`: Windows icon with the 16, 24, 32, 48, 64, 128 and 256 px
  sizes.
- `web/`: favicon, Apple touch icon and maskable icons for the web client and
  the website.
- `android/res/`: adaptive launcher icon (foreground, background colour and
  monochrome layer), ready to copy into an Android app's `res/` folder.

The wordmark is drawn as paths, not set in a typeface, so there is no font to
install or license.

## Usage

- Keep clear space around the logo equal to the radius of the sun.
- Do not stretch, rotate or recolour the symbol beyond the versions provided.
- Minimum size: 16 px for the icon, 80 px wide for the logo with the wordmark.
