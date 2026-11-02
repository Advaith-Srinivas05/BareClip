<picture>
  <source media="(prefers-color-scheme: dark)" srcset="assets/brand/svg/lockup-horizontal-dark.svg">
  <img src="assets/brand/svg/lockup-horizontal-light.svg" alt="BareClip" width="420">
</picture>

A fast, offline video editor for Windows, written in Rust.

BareClip focuses on editing and exporting video quickly, with no accounts, cloud sync or background services.

**Status:** early development. BareClip can trim video today, and the full editor is still being built.

## What it does

- Adds several videos to a project, including MP4, MKV and MOV with variable frame rates
- Splits, trims, moves and deletes clips anywhere on a timeline, with undo
- Leaves dead space when you trim or delete, for you to keep or remove
- Cuts exactly where you choose, re-encoding only the short parts around each cut
- Keeps every audio track, uses your GPU where it can, and needs no accounts or internet

## Shortcuts

| Key | Action |
|---|---|
| Space | Play or pause |
| S | Split the clip at the playhead |
| Delete | Delete the selected clip or dead space |
| Ctrl + Z and Ctrl + Y | Undo and redo |
| Left and Right | Move one frame |
| Shift + Left and Right | Move one second |
| Home and End | Go to the start or end |
| Ctrl + mouse wheel | Zoom the timeline |
| Ctrl + O | Add videos |
| Ctrl + E | Export |

## License

BareClip is released under the [MIT License](LICENSE).
