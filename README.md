<picture>
  <source media="(prefers-color-scheme: dark)" srcset="assets/brand/svg/lockup-horizontal-dark.svg">
  <img src="assets/brand/svg/lockup-horizontal-light.svg" alt="BareClip" width="420">
</picture>

A fast, offline video editor for Windows, written in Rust.

BareClip focuses on editing and exporting video quickly, with no accounts, cloud sync or background services.

**Status:** early development. BareClip can trim video today, and the full editor is still being built.

## What it does

- Opens MP4, MKV and MOV video, including variable frame rate recordings
- Cuts exactly where you mark, re-encoding only the short parts around each cut
- Keeps several sections from one video, and every audio track
- Uses your GPU where it can, and needs no accounts or internet connection

## Shortcuts

| Key | Action |
|---|---|
| Space | Play or pause |
| I | Start a section here |
| O | End the section here |
| Left and Right | Move one frame |
| Shift + Left and Right | Move one second |
| Home and End | Go to the start or end |
| Ctrl + O | Open a video |
| Ctrl + E | Export |

## License

BareClip is released under the [MIT License](LICENSE).
