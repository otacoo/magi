# Multi-disc PlayStation

Put the disc images and a playlist in `Games/PS1/`. The playlist is one file
name per line, relative to its own folder:

```text
Games/PS1/Final Fantasy VII (USA).m3u
    Final Fantasy VII (USA) (Disc 1).cue
    Final Fantasy VII (USA) (Disc 2).cue
    Final Fantasy VII (USA) (Disc 3).cue
```

The `.m3u` appears as one title; the cue/chd files it lists are hidden from the
shelf. The pause menu gains **SWAP DISC**, which ejects the virtual tray, picks
another image and closes it.

- Memory cards and save states belong to the playlist, so the whole set shares
  them; a restored state also re-inserts the disc it was saved on.
- Box art is matched by the first disc's name, and the cover file is named
  after the playlist.
- Multi-disc `.pbp` files work the same way without a playlist, because
  PCSX-ReARMed reports their images directly.
- Playlists accept a BOM, CRLF line endings and comments (`#`).
