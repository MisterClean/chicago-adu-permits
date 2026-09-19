# Profile artwork

The bot's avatar is a blue coach house outline around a red six-point Chicago
star. The garage door suggests a home above an alley garage. Rounded framing,
flat color, and a white background follow the visual style of the supplied
Chicago civic icon reference. This is original artwork for the unofficial feed.

- [`avatar.svg`](avatar.svg): scalable vector artwork with an accessible title
  and description; no fonts, linked images, or external resources.
- [`avatar.png`](avatar.png): opaque 1024 × 1024 upload copy for Bluesky, with
  room for a circular crop.
- [`../../examples/generate_avatar.rs`](../../examples/generate_avatar.rs):
  generator for both formats, using the same paths and the existing Rust graphics
  dependencies. It does not require credentials or make network requests.

Regenerate from the repository:

```sh
cargo run --locked --example generate_avatar
```

Edit the geometry in `artwork()` and regenerate both files together. The PNG is
rendered at twice its output size, then downsampled for smooth edges. Its white
background is intentional so the mark retains its colors in dark and light UI.

The palette uses [Chicago Design System colors](https://design.chicago.gov/basics/):
Flag Blue `#41B6E6`, Star Red `#E4002B`, and White `#FFFFFF`. The star uses six
outer points with an inner-to-outer radius ratio of 3:7, following the city's
published six-inch inner circle and fourteen-inch overall height.
