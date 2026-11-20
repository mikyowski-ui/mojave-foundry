# Owner brief: an FRM writer in `vault-formats`

Basis: `vault-engine` at revision `3adeac9`, the `vault-formats` module
`frm` (`FrmArchive::parse`, `FrmFrame::world_rect`) and SPEC-001 §2.2
"`.FRM` and `.FR0` through `.FR5`: shared 62-byte header", §2.3 "Per-frame
records", §3.3 "Sprite sequence loading and direction-specific files",
§3.4 "FPS and action timing" and §3.5 "Anchor, bottom-center alignment, and
frame displacement".

Tags: `[Must-match]` the bytes must be exactly as stated; `[May-improve]`
the parser's existing safe rule, which the writer mirrors by refusing;
`[Safe rule]` what the writer does with input it cannot represent.

## 1. Purpose

A mod-authoring tool needs to write new sprites: scenery, walls, items,
interface art and directional critter art, as `.FRM` files and as the
one-direction `.FR0`-`.FR5` files. `vault-formats` reads FRM but has no
writer. This brief asks for a production writer whose output
`FrmArchive::parse` reads back frame for frame, and which refuses anything
the parser would reject or read differently.

## 2. Proposed API

In the `frm` module, beside the parser, reusing its types:

```rust
/// One frame to write: indexed pixels, row-major, top row first.
pub struct FrmFrameData {
    pub width: u16,
    pub height: u16,
    pub displacement: [i16; 2],
    pub pixels: Vec<u8>, // exactly width * height
}

/// Which frames each of the six direction slots reads.
pub enum FrmDirections {
    /// One sequence for every slot: all six offsets 0.
    Shared(Vec<FrmFrameData>),
    /// One sequence per direction, 0 (north-east) to 5 (north-west).
    PerDirection([Vec<FrmFrameData>; 6]),
}

pub struct FrmWriteSpec {
    pub version: u32,          // stored as is; 4 by convention
    pub fps: i16,
    pub action_frame: i16,
    pub shifts: [[i16; 2]; 6], // per slot, also for Shared
    pub directions: FrmDirections,
}

/// Serialize; errors use `ParseError` with format "FRM", as `MapFile::write` does.
pub fn write_frm(spec: &FrmWriteSpec) -> Result<Vec<u8>, ParseError>;

impl FrmArchive {
    /// Re-serialize a parsed archive, keeping its stored sequences,
    /// sharing, shifts, version, FPS, action frame and declared area length.
    pub fn write(&self) -> Result<Vec<u8>, ParseError>;
}
```

## 3. Layout, as the parser reads it

All multi-byte numbers big-endian. `[Must-match]` for the whole section.

| Offset | Bytes | Field |
| --- | ---: | --- |
| `0x00` | 4 | Version word |
| `0x04` | 2 | FPS, signed |
| `0x06` | 2 | Action frame, signed |
| `0x08` | 2 | Frames per direction sequence, signed, at least 1 |
| `0x0A` | 12 | Six X shifts, direction order |
| `0x16` | 12 | Six Y shifts, direction order |
| `0x22` | 24 | Six direction offsets, signed 32-bit, relative to `0x3E` |
| `0x3A` | 4 | Declared frame-area byte count |
| `0x3E` | ... | Frame records |

A frame record is width (2), height (2), pixel byte count (4),
X displacement (2), Y displacement (2), then the pixel bytes
(SPEC-001 §2.3).

The parser reads sequentially (SPEC-001 §3.3): it reads direction 0's
sequence right after the header; for each later direction it reuses the
previous sequence when the offset equals the previous direction's offset,
and otherwise requires the offset to equal the bytes read so far. So:

- the first offset is 0; `[Must-match]`
- a direction that shares the previous direction's frames repeats its
  offset; a new sequence starts exactly where the previous one ended;
  `[Must-match]`
- sharing is adjacent only. A sequence equal to a non-adjacent one is
  written again; `[Must-match]`
- the declared area count is the number of bytes after the header,
  shared sequences counted once (the parser keeps it as metadata and does
  not check it). `[Must-match]`

## 4. Validation and errors

The writer returns an error, and writes nothing, when:

- the frame count is 0 or over 32767; `[May-improve]` (the parser rejects a
  nonpositive count)
- the sequences of a `PerDirection` spec differ in length (every sequence
  holds the header's count); `[Must-match]`
- a width or height is over 32767 (the parser rejects negative
  dimensions); `[May-improve]`
- `pixels.len() != width * height`; `[Safe rule]` (the parser accepts a
  larger payload and drops the rest, so a writer never emits one)
- the file would exceed the 64 MiB asset limit the parser enforces, or an
  offset would not fit a signed 32-bit word. `[Safe rule]`

Zero-width or zero-height frames are legal (pixel count 0). `[Must-match]`

## 5. Round-trip property

For every spec the writer accepts, with `a = FrmArchive::parse(&write_frm(&s)?)?`:

- `a.version`, `a.fps`, `a.action_frame`, `a.shifts` equal the spec's;
- `a.frame_count()` is the sequence length; for every direction `d` and
  frame `f`, `a.frame(d, f)` has the spec's width, height, displacement and
  pixels;
- `a.stored_frame_area_len()` equals `a.frame_area_len` equals the file
  length minus 62.

For every file `b` that parses, `FrmArchive::parse(&parse(b)?.write()?)`
gives the same frames, shifts and header values; and when `b` is
canonical (packed offsets, payload size equal to width × height),
`parse(b)?.write()? == b` byte for byte. `[Must-match]`

## 6. Edge cases

- One shared sequence with six different shifts (identical offsets share
  pixels, not shifts, SPEC-001 §3.3).
- Six sequences where directions 0 and 1 are equal and 2 to 5 differ.
- Negative shifts and displacements, at the 16-bit limits.
- FPS 0 (the engine's default of 10) and negative FPS, written as given.
- A 32767 × 1 frame and a 1 × 32767 frame.
- Zero-size frames mixed with non-empty ones in one sequence.

## 7. Synthetic test vectors

Vector A, one shared 2 × 1 frame, pixels `01 02`, FPS 10, action frame 0,
all shifts 0, version 4 (76 bytes):

```text
0000: 00 00 00 04 00 0A 00 00 00 01 00 00 00 00 00 00
0010: 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00
0020: 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00
0030: 00 00 00 00 00 00 00 00 00 00 00 00 00 0E 00 02
0040: 00 01 00 00 00 02 00 00 00 00 01 02
```

Vector B: as A, but X shift of every slot `FF FD` (-3), Y shift `00 14`
(20), and the frame's displacement `FF FF 00 01` (-1, 1). Parsing gives
those values; `world_rect` of the frame at anchor (100, 50) with shift
(-3, 20) and no displacement is left 96, top 70, right 97, bottom 70.

Vector C: `PerDirection` with one 2 × 1 frame each, directions 0 and 1
equal, 2 to 5 distinct. Offsets are 0, 0, 14, 28, 42, 56; the declared
area is 70; `stored_sequences()` is 5.

Vector D (refused): a 2 × 2 frame with 3 pixel bytes; a `PerDirection`
spec whose direction 3 has two frames and the others one; a width of
32768.

## 8. Out of scope

- Palette conversion, cropping, dithering or any image processing.
- Compressed or alternative sprite containers.
- Choosing shifts: callers compute them from the §3.5 placement rule.
- Writing the `.FR0`-`.FR5` selection logic; those files use the same
  layout and writer.
