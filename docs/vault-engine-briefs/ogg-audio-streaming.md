# Owner brief: Ogg Vorbis audio for mod layers in `vault-formats` and `vault-platform`

Basis: `vault-engine` at revision `4d5183a`. Modules: `vault_formats::acm`
(`decode`, `Decoded`, `MIXER_RATE` = 22050, `MAX_SAMPLES`),
`vault_formats::sfx` (`effect_path`, `speech_path`, `narration_path`,
`lookup`), `vault_formats::lip` (`lip_path`, `LipFile`),
`vault_formats::vfs` (`LayeredVfs`, `Vfs::read_with_layer`),
`vault_platform::audio` (`Mixer`, `Channel`, `Voice`, `Stream`,
`MusicRequest::track`, `MusicState` and its decode worker,
`MAX_DECODED_SAMPLES`, `EFFECT_VOICES`, in
`crates/vault-platform/src/audio.rs`), `vault_platform::map_preview`
(`play_speech`, `play_library`, `update_audio`), `vault_core::host_sound`
(`request_speech`, `speech_started`, `speech_clock`) and
`vault_core::dialogue` (`LipSync`); the `audio` feature of `vault-platform`
(the optional `cpal` dependency); `deny.toml`; the `fuzz/` workspace.
Specifications: SPEC-006 §3.4 "Random Generator and Check Results" and §3.11
"Talking-Head and Background Art Selection"; SPEC-008 §3.10 "Map Numbers, Town Lookup and City Music";
SPEC-010 §3.5 "Output Conversion and Sample Count", §3.6 "Playback Format",
§3.7 "Decode Failure and Degenerate Headers", §3.10 "Speech File Paths",
§3.12 "Lip-Sync Playback", §3.13 "Volume Levels and Master Volume"; SPEC-012
§3.8 "Audio" (movies); SPEC-014 §3.4 "Main-Menu Loop" (menu music); SPEC-018 §4.2 "`data/maps.txt`: the map table"; SPEC-021
§3.1 "What the Hash Covers, and Where Each Value Comes From"; SPEC-023 §3.4 "Mod Layers in Memory", §3.16
"Module Placement and Names" and §3.17 "The `wasm32-unknown-unknown` Compile
Gate"; public format specifications: RFC 3533 (the Ogg encapsulation
format), RFC 5334 (Ogg media types) and the Xiph.Org Vorbis I specification.

Tags: `[Must-match]` must be exactly as stated, and leaves every session
without an Ogg file in a layer exactly as today; `[May-improve]` an
implementation freedom that changes no stated result; `[Safe rule]` what
the engine does with data it cannot honour.

## 1. Purpose

Mods shipping voiced lines and music without an ACM encoder need the engine
to play Ogg Vorbis files from their layers, for music, speech and effects,
with the same volume rules, and without changing anything for the base game.

What the engine does today (`4d5183a`):

1. **One codec.** Only ACM is decoded (`vault_formats::acm::decode`), whole,
   into 16-bit samples.
2. **Mixer.** Effects and speech play as mono 16-bit at 22,050 Hz and music
   as interleaved stereo at 22,050 Hz, whatever the file header says
   (SPEC-010 §3.6). At most four effects play at once, a fifth fails
   without evicting one. Channels are `Effect`, `Speech`, `Library(slot)`,
   `Movie` and `Music`. Every source is resampled to the device rate by
   linear interpolation. Levels follow SPEC-010 §3.13.
3. **Paths.** `sound/sfx/<stem>.ACM`; `sound/speech/<head>/<stem>.ACM` with
   `.LIP`; narration `sound/speech/<name>.ACM`; music
   `sound/music/<name>.acm`, named by SPEC-008 §3.10 or, under Fallout 2,
   `data/maps.txt`; library sound words play the file name a script gives.
4. **Not streamed.** Music is decoded whole on a worker thread and cached up
   to 32 Mi samples (`MAX_DECODED_SAMPLES`); speech is decoded completely
   before playback (SPEC-010 §3.12 step 6). Only movie audio streams, through
   an appendable `Stream`.
5. **Missing `.LIP`.** The speech plays as one phoneme 0, so the mouth stays
   still (the safe rule in `request_speech`).
6. **Speech reaches the world.** The mixer's speech position is reported to
   the dialogue head (`speech_clock`); the speech ends when the audio ends,
   which starts the head's waiting state, whose next fidget makes random
   draws in the shared generator (SPEC-006 §3.4, §3.11; SPEC-010 §3.12). So
   a speech's decoded length is visible to the world today.
7. **Output.** The `audio` feature (default) enables `cpal`; without it the
   desktop decodes and plays silently.
8. **Licences.** `deny.toml` allows only Apache-2.0, Apache-2.0 WITH
   LLVM-exception, BSD-2-Clause, BSD-3-Clause, CC0-1.0, ISC, MIT,
   Unicode-3.0, Unlicense and Zlib (NCSA only for `libfuzzer-sys`), denies
   unknown registries and git sources, and fails on an unmaintained direct
   workspace dependency.
9. **Portable crates.** `vault-formats` depends only on `thiserror` (and
   `cap-std` behind `host-fs`). With default features off, a gated crate's
   normal dependencies must equal the `wasm-graph.tsv` allow-list
   (SPEC-023 §3.17).
10. **Fuzzing.** `fuzz/` has an `acm` target among others, and no Ogg target.

## 2. Proposed API

In a new portable module `vault_formats::vorbis` (no host access, no
threads):

```rust
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum OggError {
    #[error("Ogg page at byte {offset}: {reason}")]
    Page { offset: u64, reason: &'static str },
    #[error("Vorbis header: {0}")]
    Header(&'static str),
    #[error("Vorbis packet {packet}: {reason}")]
    Packet { packet: u64, reason: &'static str },
    #[error("stream exceeds the {0}-sample limit")]
    TooLong(u64),
    #[error("unsupported: {0}")]
    Unsupported(&'static str),
}

pub struct VorbisInfo {
    pub channels: u8,           // 1 or 2 in this revision
    pub rate: u32,              // 8000-192000
    pub total_frames: Option<u64>,
    pub loop_start: Option<u64>, // frames, from the LOOPSTART comment
    pub loop_length: Option<u64>,
}

/// A pull decoder over a byte source; holds at most one page and one
/// packet's output beyond its fixed tables.
pub struct VorbisReader<'a> { /* ... */ }
impl<'a> VorbisReader<'a> {
    pub fn new(bytes: &'a [u8]) -> Result<Self, OggError>;
    pub fn info(&self) -> &VorbisInfo;
    /// Up to `out.len() / channels` frames of interleaved i16; 0 at the end.
    pub fn read(&mut self, out: &mut [i16]) -> Result<usize, OggError>;
    /// Restart at frame `frame` (for loops); exact.
    pub fn seek(&mut self, frame: u64) -> Result<(), OggError>;
}

/// Whole-file decode to the mixer's format: `channels_out` 1 or 2 at
/// `MIXER_RATE`, by the resampler of section 3.3.
pub fn decode_to_mixer(bytes: &[u8], channels_out: u8) -> Result<Vec<i16>, OggError>;
```

In `vault_platform::audio`, a streaming source beside `Voice`:

```rust
enum Source { Pcm(Arc<[i16]>), Ogg(OggStream) }
/// Decoded on a worker into a bounded ring of mixer-rate frames.
struct OggStream { ring: Arc<Ring>, channels: usize, looping: Option<LoopPoints> }
```

## 3. Behaviour

### 3.1 Lookup

1. Each audio request names an `.acm` path (section 1 item 3). For that
   path, the layers are searched from the highest down; in each layer the
   `.acm` file wins, then a file with the same stem and `.ogg`. The first
   layer holding either serves the request. `[Safe rule]`
2. The base data is never searched for `.ogg`: with no layer, or with no
   `.ogg` in any layer, every request reads exactly the file it reads
   today. `[Must-match]`
3. The rule covers effects, the SPEC-010 §3.9 alias chain (each attempted
   stem is looked up by item 1), speech, narration, city and map music, and
   library sound words whose name ends `.acm`. A library name that already
   ends `.ogg` is read as named, from layers only. `[Safe rule]`
4. Case is folded as every `VfsPath` is. `[Must-match]`
5. Movies keep their own audio; this brief adds nothing to `.mve`.
   `[Must-match]`

### 3.2 Formats accepted

1. An Ogg stream (RFC 3533) holding exactly one logical Vorbis I stream,
   with 1 or 2 channels at 8,000 to 192,000 Hz. `[Safe rule]`
2. Chained streams, multiplexed streams, and other codecs in Ogg are
   refused with `OggError::Unsupported`. `[Safe rule]`
3. Decoded samples are converted to 16-bit by `clamp(round(x × 32767),
   -32768, 32767)`, round half away from zero. `[Safe rule]`

### 3.3 Resampling and channels

1. Every Ogg source is converted to the mixer's format before mixing:
   effects, speech and narration to mono, music to stereo, all at 22,050 Hz,
   matching SPEC-010 §3.6. `[Safe rule]`
2. Mono to stereo duplicates the sample; stereo to mono takes
   `trunc((left + right) / 2)`. `[Safe rule]`
3. The rate conversion is deterministic integer linear interpolation: output
   frame n reads source position `p = n × rate / 22050` as a 32.32 fixed
   point value, `i = floor(p)`, `f` its fraction, and gives
   `s[i] + trunc((s[i + 1] - s[i]) × f / 2^32)`, with `s[i + 1]` taken as
   `s[i]` past the end. The output length is
   `ceil(source frames × 22050 / rate)`. `[Safe rule]`
4. The mixer's own device-rate resampling is unchanged. `[Must-match]`

### 3.4 Streaming

1. Music and library sounds with the looping bit stream: a worker decodes
   into a ring of 2 seconds of mixer-rate frames (88,200 stereo samples) and
   the device callback drains it. `[Safe rule]`
2. Effects and speech decode whole, on the decode worker, before they start,
   as ACM speech does (SPEC-010 §3.12 step 6), bounded by `acm::MAX_SAMPLES`.
   `[Safe rule]`
3. An underrun plays silence for the missing frames and is logged once per
   track; it never blocks the device callback. `[Safe rule]`
4. Memory per streamed track is the ring plus the decoder's tables; the
   whole-file cache of decoded music is not used for Ogg music. `[Safe rule]`

### 3.5 Loops

1. Music requested as looping loops at the end of the stream, from frame 0,
   as ACM music does. `[Must-match]` for the request semantics
2. Vorbis comments `LOOPSTART=<frames>` and either `LOOPLENGTH=<frames>` or
   `LOOPEND=<frames>` (decimal, in source frames) set a loop that plays the
   intro once and then repeats `[start, end)`. Converted to mixer frames by
   `floor(x × 22050 / rate)`. `[Safe rule]`
3. A loop point past the end, a negative or non-decimal value, or an end at
   or before the start is ignored with one diagnostic, and the track loops
   from frame 0. `[Safe rule]`
4. A non-looping request ignores loop comments. `[Safe rule]`

### 3.6 Speech and lip-sync

1. An Ogg speech with a `.LIP` beside it (same layer or lower) uses the lip
   file as SPEC-010 §3.12 says; marker positions are bytes of the converted
   mono 22,050 Hz stream. `[Safe rule]`
2. Without a `.LIP`, the speech plays as one phoneme 0 (mouth idle), the
   existing safe rule. `[Must-match]`
3. The stored audio stem of a lip file is looked up by section 3.1, so a
   `.LIP` may name an Ogg-backed stem. `[Safe rule]`

### 3.7 Volume

1. Every Ogg source plays on the channel its request names, at the level
   SPEC-010 §3.13 gives that channel (effects scaled by relative volume,
   speech at `trunc(0.69 × V)`, music at its level), scaled by master
   volume. `[Must-match]`
2. Ogg effects count against the four-effect limit. `[Must-match]`

### 3.8 Crate boundary

1. The decoder and resampler live in `vault-formats`, compile for
   `wasm32-unknown-unknown`, read no clock, spawn no thread, and touch no
   file. `[Safe rule]`
2. Workers, rings and device output live in `vault-platform`. `[Safe rule]`
3. If the decoder is an external crate, it sits behind a non-default
   `vorbis` feature of `vault-formats` that `vault-platform` enables, so
   the SPEC-023 §3.17 gate (`--no-default-features`) and `wasm-graph.tsv`
   stay as they are. `[Safe rule]` (question 1)

## 4. Validation and errors

`VorbisReader::new`, `read`, `seek` and `decode_to_mixer` return
`OggError`, never panic, never read past the input, and always terminate:
`[May-improve]` (the SPEC-010 §3.7 stance for ACM)

1. a page without the `OggS` capture pattern, a version other than 0, a
   segment table that runs past the input, or a CRC that does not match;
2. a first packet that is not a Vorbis identification header, a missing
   comment or setup header, or header fields outside the Vorbis I limits
   (block sizes, channel count 0, rate 0);
3. a packet that decodes outside the setup's codebooks, floors, residues or
   mappings;
4. more than `acm::MAX_SAMPLES` output samples for a whole-file decode, or
   a granule position that goes backwards;
5. a page whose stream serial differs from the first page's.

At run time:

6. A file that fails to decode plays nothing and is reported once per path,
   as an unreadable ACM is today (`MusicState::failed`). A layer's `.ogg`
   that fails does not fall back to a lower layer's `.acm`. `[Safe rule]`
7. A stream that fails partway stops at the last good frame, with one
   diagnostic; a looping track then stops rather than restarting a broken
   stream. `[Safe rule]`

## 5. Determinism property

1. Audio output never changes `state_hash`, `combat_hash`, the checkpoint
   stream or saves; the presented frame and replay digest do not include
   audio. `[Must-match]`
2. Decoding is exact: for one file, `decode_to_mixer` gives the same
   samples on every host, with integer resampling (section 3.3) and no
   floating-point step whose result can differ between targets after the
   Vorbis inverse transform. `[Safe rule]` (question 4)
3. A speech's length reaches the dialogue head (section 1 item 6). Its value
   for an Ogg speech is the exact converted sample count, so two runs of
   one log that play audio give the same head states. `[Safe rule]`
4. A replay runs no device. It keeps today's rule for speech, whatever the
   codec (question 5). `[Must-match]`
5. A session with no Ogg file in its layers decodes and plays exactly what
   it does today. `[Must-match]`

## 6. Edge cases

- A layer that holds both `x.acm` and `x.ogg`: the `.acm` plays.
- A higher layer with `x.ogg` over a lower layer with `x.acm`: the `.ogg`
  plays.
- A 0-frame Vorbis stream: plays nothing and counts as finished at once.
- A mono music file: duplicated to stereo.
- A 48,000 Hz stereo effect: mixed to mono and resampled.
- A comment `LOOPSTART=0` with no length or end: loops the whole stream.
- A stream whose last page's granule position is below its decoded length:
  the extra samples at the end are dropped, per Vorbis I.
- A library sound slot stopped by a script while its worker is decoding:
  the worker stops at the next packet.

## 7. Synthetic test vectors

Described by parameters. Each fixture is a synthetic tone encoded from the
stated parameters with any Vorbis I encoder and committed as hex under
`tests/fixtures/`; no game audio is used. Expected values are tolerances
against the tone, since encoding is lossy.

Vector A, a 0.1-second 440 Hz mono sine at 22,050 Hz, amplitude 0.5,
2,205 frames, encoded at a high quality setting:

- `info()`: 1 channel, rate 22050, `total_frames` 2205.
- `decode_to_mixer(_, 1)` gives 2,205 samples. Sample 0 is within ±64 of 0;
  the sample at frame 12 (a quarter period, 12.53 frames) is within ±1,024
  of 16,384; the decoded RMS is within 5% of 11,585 (0.5 × 32767 / √2).

Vector B, the same tone at 44,100 Hz, 4,410 frames:
`decode_to_mixer(_, 1)` gives `ceil(4410 × 22050 / 44100)` = 2,205 samples,
each within ±1,024 of vector A's sample at the same index.

Vector C, a stereo stream at 22,050 Hz with left a 440 Hz tone and right
silence, 2,205 frames: `decode_to_mixer(_, 1)` gives 2,205 samples within
±1,024 of half of vector A's; `decode_to_mixer(_, 2)` gives 4,410 samples
whose odd samples are within ±64 of 0.

Vector D, loop comments: a 1-second stream at 22,050 Hz with
`LOOPSTART=11025` and `LOOPLENGTH=5512`, played looping for 2 seconds of
mixer frames: frames 0-22049 play the stream, then frames 22050 onward
repeat source frames 11025-16536.

Vector E, lookup. Base `sound/sfx/a.acm`; layer 1 `sound/sfx/a.ogg`; layer 2
none: the effect `a` plays layer 1's Ogg. With no layers: the base ACM. With
layer 2 holding `sound/sfx/a.acm`: layer 2's ACM.

Vector F (refused): the four bytes `4F 67 67 54` and nothing more; a valid
stream with byte 22 (the CRC) changed; a stream whose identification header
declares 0 channels; a three-channel stream (`Unsupported`).

Fuzzing: a new `fuzz/fuzz_targets/ogg.rs` target that runs
`decode_to_mixer` on arbitrary bytes, with seeds from vectors A-D, in the
`fuzz/` workspace beside `acm`. `[Safe rule]`

## 8. Open questions for the owner

1. **Decoder.** A hand-written Vorbis I decoder in `vault-formats` (as the
   engine wrote its own inflate and ACM decoders), or an external pure-Rust
   crate? Recommended: hand-written, keeping `vault-formats` dependency-free
   and the wasm allow-list unchanged; if a crate is preferred, take one
   whose licence is on `deny.toml`'s list (MIT, Apache-2.0, BSD and the
   like; MPL-2.0 and LGPL are not on it), behind the non-default `vorbis`
   feature, and confirm with `cargo deny check` before adding it.
2. **Unmaintained policy.** `deny.toml` fails on an unmaintained direct
   dependency. Accept a crate with little upstream activity? Recommended:
   only with no RustSec advisory, and the owner's explicit waiver recorded.
3. **Base data.** Should a loose `.ogg` in the install's own data folder
   count too? Recommended: no; layers only, so the base game can never
   change by accident.
4. **Bit-exactness.** Vorbis decoding uses floating point. Require
   bit-identical output across hosts, or a tolerance? Recommended:
   bit-identical within this engine (one decoder, `f32` operations in a
   fixed order, no fused multiply-add), with vectors checked by tolerance
   against the format and exactly against the engine's own goldens.
5. **Speech length in replays.** Should a headless replay model each
   speech's length from its decoded sample count, so the dialogue head ends
   speech as the desktop does? Recommended: yes, as a separate change for
   ACM and Ogg alike; this brief keeps today's rule.
6. **Ring size.** Is 2 seconds right? Recommended: yes; it covers a slow
   worker on small devices at 352 KiB per stereo track.
7. **Loop comment names.** `LOOPSTART` / `LOOPLENGTH` / `LOOPEND` only?
   Recommended: yes, case-insensitive names, decimal values.
8. **Other codecs.** Opus or FLAC? Recommended: not in this revision.

## 9. Out of scope

- Encoding audio, and any ACM encoder.
- Movie audio and the `.mve` format.
- Positional audio, effects processing, or more than four effect voices.
- Changing the mixer rate, channel layout or volume formulas.
