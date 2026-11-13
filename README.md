<picture>
  <source media="(prefers-color-scheme: dark)" srcset="assets/brand/svg/lockup-horizontal-dark.svg">
  <img src="assets/brand/svg/lockup-horizontal-light.svg" alt="BareClip" width="420">
</picture>

A fast, offline video editor for Windows, written in Rust.

BareClip is made for screen recordings and gameplay clips. It starts quickly, and when an edit doesn't need the picture redrawn, it copies the video instead of re-encoding it, so trims and joins export in seconds. When a picture does have to be redrawn, it uses your graphics card.

There are no accounts, no cloud, no AI features and no telemetry. BareClip never connects to the internet and leaves nothing running after you close it.

## Download

Get the latest installer: [BareClip.msi](https://github.com/Advaith-Srinivas05/BareClip/releases/latest/download/BareClip.msi). Older versions are on the [Releases](https://github.com/Advaith-Srinivas05/BareClip/releases) page.

The installer asks whether to install for just you or for everyone on the PC, where to install, and which shortcuts to add. Windows may show "Windows protected your PC" the first time you run it. Choose **More info**, then **Run anyway**.

BareClip runs on 64-bit Windows with an Intel or AMD processor, and has been tested on Windows 11. PCs with an ARM processor, such as Snapdragon laptops, aren't supported yet.

## What it does

**Editing**

- Several video and sound rows, with clips you can split, trim, move, copy and duplicate
- Select several clips with Ctrl or Shift and move, copy, duplicate or delete them together
- Snapping, timeline zoom, and undo and redo for every change
- Gaps left by deleted clips stay until you remove them, and export as black and silence
- Project files that save themselves as you work and recover after a crash
- Rename videos within a project
- Right-click a video or song in File Explorer and choose **Edit with BareClip** to add it to the timeline. On Windows 11 it's under **Show more options**.
- The window reopens where and at the size you left it, and still works snapped to half or a quarter of the screen

**Picture**

- Resize, move, crop, rotate and flip clips, with picture-in-picture and see-through layers
- A background colour, and project sizes for wide, tall and square videos
- Speed changes from slow motion to fast-forward, with the sound kept at its pitch
- Images, and phone videos shown the right way up
- A content library of shapes, arrows, callouts, frames, borders, overlays and backgrounds

**Titles, looks and effects**

- Over 70 title styles, many of them moving, and captions you can type, import from an SRT file or save as one
- 44 transitions, 49 colour filters with adjustable strength, colour adjustments and fades
- 27 effects such as blur, glow, glitch, VHS, spin and slow zoom, several at once on one clip

**Sound**

- Every sound track a recording has, kept separate or mixed as you choose
- Volume up to 200%, fades, mute, and detaching a video's sound onto its own row
- Noise suppression for voices, to quieten fans, hum and hiss
- Sound waveforms on clips

**Recording**

- Record your screen or one window, your webcam, both at once, or just your microphone, straight onto the timeline
- The PC's sound and the microphone on separate tracks, or only the recorded window's app sound
- 30 or 60 fps, with a smaller size and a quality choice
- A small bar that stays on top while you play, and keys that stop, pause and mute the microphone from any program, which you can change
- BareClip's own windows stay out of the recording, and a crash keeps what was recorded

**Export**

- Copies the video wherever it can, and re-encodes only the few frames around each exact cut
- Redraws on the graphics card when it has to, and says why before you start
- Resolution, quality and frame rate choices, each starting on Match source
- H.264 or AV1 in MP4, or an animated GIF
- Encodes with your NVIDIA, AMD or Intel graphics card, or Windows' own encoder or the processor when there isn't one, and you can choose which graphics card BareClip uses
- Sound-only export to M4A

## Files it opens

| Kind | Formats |
|---|---|
| Video | MP4, MKV, MOV, WebM, AVI and TS, including H.264, HEVC and AV1, and recordings with uneven frame timing |
| Sound | MP3, WAV, FLAC, M4A, AAC, OGG and Opus |
| Images | JPG, PNG, WebP, BMP and GIF |
| Captions | SRT |

## Shortcuts

| Key | Action |
|---|---|
| Space | Play or pause |
| S | Split the clip at the playhead |
| Delete or Backspace | Delete the selected clips or gap |
| Left and Right | Move 5 seconds back or forward |
| Shift + Left and Right | Move one frame back or forward |
| Home and End | Go to the start or end |
| Ctrl + Z | Undo |
| Ctrl + Y or Ctrl + Shift + Z | Redo |
| Ctrl + click or Shift + click | Select several clips |
| Ctrl + A | Select every clip on the selected clip's row |
| Ctrl + C and Ctrl + V | Copy and paste the selected clips |
| Ctrl + D | Duplicate the selected clips |
| Ctrl + mouse wheel | Zoom the timeline |
| Ctrl + Alt + 3 | Show or hide the timeline |
| Ctrl + O | Add files |
| Ctrl + N | New project |
| Ctrl + Shift + O | Open a project |
| Ctrl + S and Ctrl + Shift + S | Save and Save as |
| Ctrl + E | Export |

While recording the screen, these work from any program, and can be changed in Settings:

| Key | Action |
|---|---|
| Ctrl + Alt + Shift + R | Stop recording |
| Ctrl + Alt + Shift + P | Pause or resume |
| Ctrl + Alt + Shift + M | Turn the microphone on or off |

## Limitations

- Windows only, on Intel or AMD processors. There's no ARM version yet.
- Redrawn videos are saved as H.264 or AV1 in MP4, or as a GIF. HEVC export isn't offered.
- Recording only one app's sound needs Windows 10 version 2004 or later. Before that, all PC sound is recorded.

## Building from source

You need Windows, [Rust](https://rustup.rs) stable and PowerShell. The scripts download a pinned, checksummed LGPL build of FFmpeg into `vendor/` the first time they run.

```powershell
./scripts/cargo.ps1 run --release -p bareclip-app   # build and run BareClip
./scripts/cargo.ps1 test --workspace                # run the tests
./scripts/build-msi.ps1                             # build the installer into dist/
```

`scripts/cargo.ps1` runs Cargo with the FFmpeg DLLs on the path. Use it in place of `cargo` for anything that runs BareClip or its tests.

## License

BareClip is released under the [MIT License](LICENSE).

It ships with an LGPL build of [FFmpeg](https://ffmpeg.org) as separate DLLs, which you can replace with your own build of the same version. The licences of FFmpeg, the libraries built into it, the Rust libraries, fonts and icons BareClip uses are in [assets/licenses](assets/licenses) and are installed alongside the app.
