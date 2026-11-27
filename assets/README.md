# Project artwork

`social-preview.svg` and `social-preview-dark.svg` are the editable light and dark versions of the hs banner. They use the existing trivial.si colors and Roboto typeface. Their PNG exports are used by the README; the light PNG also remains the repository's social preview image.

The README uses [GitHub's theme fragments](https://github.blog/changelog/2021-11-24-specify-theme-context-for-images-in-markdown/) so the banner follows the selected GitHub theme. The art and layout stay the same; only the background, text and divider colors change.

With Roboto installed, regenerate the exports with:

```sh
rsvg-convert assets/social-preview.svg -o assets/social-preview.png
rsvg-convert assets/social-preview-dark.svg -o assets/social-preview-dark.png
```

`demo-tui.svg` and its PNG export are a screenshot of the [terminal demo](../docs/demo.md), rendered from a real run in the same palette. `demo-tui.gif` is an animated demo recorded from a real pseudo-terminal run via `scripts/record-demo-gif.py`. Regeneration steps are in that guide.

The original Trivial Superintelligence company artwork is retained in `trivial-superintelligence.png`. The project mascot is original artwork; see [NOTICE](../NOTICE).
