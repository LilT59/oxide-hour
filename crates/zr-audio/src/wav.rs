//! Minimal bounds-checked RIFF/WAVE header reader and IMA ADPCM decoder.
//!
//! [`parse_wav`] reads what a player needs to hand to a decoder: the format tag, channel count,
//! sample rate and the byte range of the sample data, plus a frame count. [`decode_ima_adpcm`]
//! decodes IMA ADPCM blocks to interleaved `i16`, and [`to_pcm_wav`] wraps them in a PCM WAV. PCM
//! (`WAVE_FORMAT_PCM`), IMA ADPCM (`WAVE_FORMAT_IMA_ADPCM`) and the extensible
//! wrapper (`WAVE_FORMAT_EXTENSIBLE`) are understood; other tags are reported as
//! [`WavFormat::Other`] with their raw tag.
//!
//! All reads are checked; malformed input returns [`WavError`], never a panic.

const RIFF: &[u8; 4] = b"RIFF";
const WAVE: &[u8; 4] = b"WAVE";
const FMT: &[u8; 4] = b"fmt ";
const DATA: &[u8; 4] = b"data";
const FACT: &[u8; 4] = b"fact";

/// `WAVE_FORMAT_*` values we name.
pub const WAVE_FORMAT_PCM: u16 = 0x0001;
pub const WAVE_FORMAT_MS_ADPCM: u16 = 0x0002;
pub const WAVE_FORMAT_IMA_ADPCM: u16 = 0x0011;
pub const WAVE_FORMAT_EXTENSIBLE: u16 = 0xFFFE;

/// Effective format of a WAV file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WavFormat {
    Pcm,
    ImaAdpcm,
    MsAdpcm,
    Other(u16),
}

impl WavFormat {
    pub fn from_tag(tag: u16) -> WavFormat {
        match tag {
            WAVE_FORMAT_PCM => WavFormat::Pcm,
            WAVE_FORMAT_IMA_ADPCM => WavFormat::ImaAdpcm,
            WAVE_FORMAT_MS_ADPCM => WavFormat::MsAdpcm,
            other => WavFormat::Other(other),
        }
    }

    pub fn tag(self) -> u16 {
        match self {
            WavFormat::Pcm => WAVE_FORMAT_PCM,
            WavFormat::ImaAdpcm => WAVE_FORMAT_IMA_ADPCM,
            WavFormat::MsAdpcm => WAVE_FORMAT_MS_ADPCM,
            WavFormat::Other(t) => t,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            WavFormat::Pcm => "PCM",
            WavFormat::ImaAdpcm => "IMA ADPCM",
            WavFormat::MsAdpcm => "MS ADPCM",
            WavFormat::Other(_) => "other",
        }
    }
}

/// A parse failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WavError {
    /// Fewer bytes than the fixed headers require.
    Truncated,
    /// Missing `RIFF` / `WAVE` tags.
    NotRiff,
    /// No `fmt ` chunk.
    MissingFmt,
    /// No `data` chunk.
    MissingData,
    /// A chunk size runs past the end of the buffer.
    BadChunk,
    /// The `fmt ` chunk is shorter than its fixed part.
    ShortFmt,
}

impl std::fmt::Display for WavError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            WavError::Truncated => "truncated header",
            WavError::NotRiff => "not a RIFF/WAVE file",
            WavError::MissingFmt => "missing fmt chunk",
            WavError::MissingData => "missing data chunk",
            WavError::BadChunk => "chunk size past end of file",
            WavError::ShortFmt => "fmt chunk too short",
        };
        f.write_str(s)
    }
}

impl std::error::Error for WavError {}

/// What a player needs from the header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WavInfo {
    pub format: WavFormat,
    pub channels: u16,
    pub sample_rate: u32,
    pub byte_rate: u32,
    /// Bytes per encoded block (`nBlockAlign`).
    pub block_align: u16,
    pub bits_per_sample: u16,
    /// Offset of the sample data within the file.
    pub data_offset: usize,
    pub data_len: usize,
    /// Frames (per-channel samples); for ADPCM the `fact` chunk when present.
    pub frames: u64,
}

fn u16le(d: &[u8], o: usize) -> Option<u16> {
    Some(u16::from_le_bytes([*d.get(o)?, *d.get(o + 1)?]))
}

fn u32le(d: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_le_bytes([*d.get(o)?, *d.get(o + 1)?, *d.get(o + 2)?, *d.get(o + 3)?]))
}

/// Raw `fmt ` fields.
struct Fmt {
    tag: u16,
    channels: u16,
    sample_rate: u32,
    byte_rate: u32,
    block_align: u16,
    bits_per_sample: u16,
}

fn parse_fmt(d: &[u8]) -> Result<Fmt, WavError> {
    if d.len() < 16 {
        return Err(WavError::ShortFmt);
    }
    let mut f = Fmt {
        tag: u16le(d, 0).ok_or(WavError::ShortFmt)?,
        channels: u16le(d, 2).ok_or(WavError::ShortFmt)?,
        sample_rate: u32le(d, 4).ok_or(WavError::ShortFmt)?,
        byte_rate: u32le(d, 8).ok_or(WavError::ShortFmt)?,
        block_align: u16le(d, 12).ok_or(WavError::ShortFmt)?,
        bits_per_sample: u16le(d, 14).ok_or(WavError::ShortFmt)?,
    };
    // WAVE_FORMAT_EXTENSIBLE: the first two bytes of the SubFormat GUID are the
    // real format tag (offset 24 in the `fmt ` body).
    if f.tag == WAVE_FORMAT_EXTENSIBLE && d.len() >= 26 {
        f.tag = u16le(d, 24).ok_or(WavError::ShortFmt)?;
    }
    Ok(f)
}

fn adpcm_samples_per_block(block_align: u16, channels: u16, bits: u16) -> u64 {
    if channels == 0 || bits == 0 || block_align as u32 <= 4 * channels as u32 {
        return 0;
    }
    let data_per_block = block_align as u32 - 4 * channels as u32;
    (data_per_block as u64 * 8 / (bits as u64 * channels as u64)) + 1
}

/// Reads a RIFF/WAVE header. Never panics; malformed input is [`WavError`].
pub fn parse_wav(d: &[u8]) -> Result<WavInfo, WavError> {
    if d.len() < 12 {
        return Err(WavError::Truncated);
    }
    if &d[0..4] != RIFF || &d[8..12] != WAVE {
        return Err(WavError::NotRiff);
    }
    let mut pos = 12usize;
    let mut fmt: Option<Fmt> = None;
    let mut data: Option<(usize, usize)> = None;
    let mut fact: Option<u32> = None;
    while pos + 8 <= d.len() {
        let id = &d[pos..pos + 4];
        let size = u32le(d, pos + 4).ok_or(WavError::BadChunk)? as usize;
        let body = pos + 8;
        let end = body.checked_add(size).ok_or(WavError::BadChunk)?;
        if end > d.len() {
            return Err(WavError::BadChunk);
        }
        if id == FMT {
            fmt = Some(parse_fmt(&d[body..end])?);
        } else if id == DATA {
            data = Some((body, size));
        } else if id == FACT {
            fact = u32le(d, body);
        }
        // Chunks are word-aligned: skip the pad byte after an odd size.
        pos = end + (size & 1);
    }
    let fmt = fmt.ok_or(WavError::MissingFmt)?;
    let (data_offset, data_len) = data.ok_or(WavError::MissingData)?;

    let bytes_per_frame = (fmt.channels as u64) * (fmt.bits_per_sample as u64) / 8;
    let frames = match WavFormat::from_tag(fmt.tag) {
        WavFormat::Pcm => {
            if fmt.block_align > 0 {
                (data_len as u64).checked_div(fmt.block_align as u64).unwrap_or(0)
            } else {
                (data_len as u64).checked_div(bytes_per_frame).unwrap_or(0)
            }
        }
        WavFormat::ImaAdpcm | WavFormat::MsAdpcm => {
            if let Some(n) = fact {
                n as u64
            } else if fmt.block_align > 0 {
                let spb = adpcm_samples_per_block(fmt.block_align, fmt.channels, fmt.bits_per_sample);
                (data_len as u64).checked_div(fmt.block_align as u64).unwrap_or(0) * spb
            } else {
                0
            }
        }
        WavFormat::Other(_) => 0,
    };

    Ok(WavInfo {
        format: WavFormat::from_tag(fmt.tag),
        channels: fmt.channels,
        sample_rate: fmt.sample_rate,
        byte_rate: fmt.byte_rate,
        block_align: fmt.block_align,
        bits_per_sample: fmt.bits_per_sample,
        data_offset,
        data_len,
        frames,
    })
}

// IMA ADPCM (DVI4) step/index tables (standard IMA algorithm; the format the engine's Miles
// `ADPCM` codec reads). 89 step entries, 16 index deltas.
const IMA_INDEX_TABLE: [i32; 16] = [-1, -1, -1, -1, 2, 4, 6, 8, -1, -1, -1, -1, 2, 4, 6, 8];
const IMA_STEP_TABLE: [i32; 89] = [
    7, 8, 9, 10, 11, 12, 13, 14, 16, 17, 19, 21, 23, 25, 28, 31, 34, 37, 41, 45, 50, 55, 60, 66, 73, 80, 88, 97,
    107, 118, 130, 143, 157, 173, 190, 209, 230, 253, 279, 307, 337, 371, 408, 449, 494, 544, 598, 658, 724, 796,
    876, 963, 1060, 1166, 1282, 1411, 1552, 1707, 1878, 2066, 2272, 2499, 2749, 3024, 3327, 3660, 4026, 4428,
    4871, 5358, 5894, 6484, 7132, 7845, 8630, 9493, 10442, 11487, 12635, 13899, 15289, 16818, 18500, 20350, 22385,
    24623, 27086, 29794, 32767,
];

/// One IMA ADPCM nibble: `(predictor, step_index)`.
fn ima_step(predictor: i32, index: i32, nibble: u8) -> (i32, i32) {
    let step = IMA_STEP_TABLE[index.clamp(0, 88) as usize];
    let mut diff = step >> 3;
    if nibble & 1 != 0 {
        diff += step >> 2;
    }
    if nibble & 2 != 0 {
        diff += step >> 1;
    }
    if nibble & 4 != 0 {
        diff += step;
    }
    let predictor = if nibble & 8 != 0 { predictor - diff } else { predictor + diff };
    let predictor = predictor.clamp(-32768, 32767);
    let index = (index + IMA_INDEX_TABLE[(nibble & 0x0F) as usize]).clamp(0, 88);
    (predictor, index)
}

/// Decodes `WAVE_FORMAT_IMA_ADPCM` sample data to interleaved `i16` (standard IMA/DVI ADPCM,
/// matching the engine's Miles ADPCM reader). Every block is 4 bytes of per-channel header
/// (predictor `i16`, step index `u8`, reserved) followed by interleaved nibbles, low nibble first.
/// Bounds-checked; malformed input is a [`WavError`].
pub fn decode_ima_adpcm(info: &WavInfo, data: &[u8]) -> Result<Vec<i16>, WavError> {
    let ch = info.channels as usize;
    let block = info.block_align as usize;
    if ch == 0 || block < 4 * ch {
        return Err(WavError::ShortFmt);
    }
    let start = info.data_offset;
    let end = start.checked_add(info.data_len).ok_or(WavError::BadChunk)?;
    if end > data.len() {
        return Err(WavError::BadChunk);
    }
    let spb = (block - 4 * ch) * 2 / ch + 1; // samples per channel per block
    let nibbles = (block - 4 * ch) * 2;
    let mut out: Vec<i16> = Vec::new();
    let mut pos = start;
    while pos + block <= end {
        let mut predictors = [0i32; 2];
        let mut indices = [0i32; 2];
        for c in 0..ch.min(2) {
            let h = pos + c * 4;
            predictors[c] = i16::from_le_bytes([data[h], data[h + 1]]) as i32;
            indices[c] = (data[h + 2] as i32).clamp(0, 88);
            out.push(predictors[c] as i16);
        }
        let mut s = 0usize;
        while s + 1 < spb {
            for c in 0..ch.min(2) {
                let ni = s * ch + c;
                if ni >= nibbles {
                    break;
                }
                let byte = data[pos + 4 * ch + ni / 2];
                let nibble = if ni % 2 == 0 { byte & 0x0F } else { byte >> 4 };
                let (p, i) = ima_step(predictors[c], indices[c], nibble);
                predictors[c] = p;
                indices[c] = i;
                out.push(p as i16);
            }
            s += 1;
        }
        pos += block;
    }
    if info.frames > 0 {
        let want = info.frames as usize * ch;
        if out.len() > want {
            out.truncate(want);
        }
    }
    Ok(out)
}

/// Wraps interleaved `i16` samples in a minimal 16-bit PCM RIFF/WAVE of the same rate/channels.
pub fn to_pcm_wav(info: &WavInfo, samples: &[i16]) -> Vec<u8> {
    let ch = info.channels.max(1);
    let block = ch * 2;
    let data_len = samples.len() * 2;
    let mut v = Vec::with_capacity(44 + data_len);
    v.extend_from_slice(RIFF);
    v.extend_from_slice(&(36 + data_len as u32).to_le_bytes());
    v.extend_from_slice(WAVE);
    v.extend_from_slice(FMT);
    v.extend_from_slice(&16u32.to_le_bytes());
    v.extend_from_slice(&WAVE_FORMAT_PCM.to_le_bytes());
    v.extend_from_slice(&ch.to_le_bytes());
    v.extend_from_slice(&info.sample_rate.to_le_bytes());
    v.extend_from_slice(&(info.sample_rate * block as u32).to_le_bytes());
    v.extend_from_slice(&block.to_le_bytes());
    v.extend_from_slice(&16u16.to_le_bytes());
    v.extend_from_slice(DATA);
    v.extend_from_slice(&(data_len as u32).to_le_bytes());
    for s in samples {
        v.extend_from_slice(&s.to_le_bytes());
    }
    v
}

/// Builds a minimal PCM WAV of `frames` silence, for tests.
#[cfg(test)]
pub(crate) fn synth_pcm(channels: u16, rate: u32, bits: u16, frames: u32) -> Vec<u8> {
    let block = (channels as u32) * (bits as u32) / 8;
    let data_len = (block * frames) as usize;
    let mut v = Vec::with_capacity(44 + data_len);
    v.extend_from_slice(RIFF);
    v.extend_from_slice(&(36 + data_len as u32).to_le_bytes());
    v.extend_from_slice(WAVE);
    v.extend_from_slice(FMT);
    v.extend_from_slice(&16u32.to_le_bytes());
    v.extend_from_slice(&WAVE_FORMAT_PCM.to_le_bytes());
    v.extend_from_slice(&channels.to_le_bytes());
    v.extend_from_slice(&rate.to_le_bytes());
    v.extend_from_slice(&(rate * block).to_le_bytes());
    v.extend_from_slice(&(block as u16).to_le_bytes());
    v.extend_from_slice(&bits.to_le_bytes());
    v.extend_from_slice(DATA);
    v.extend_from_slice(&(data_len as u32).to_le_bytes());
    v.resize(v.len() + data_len, 0);
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_pcm_header() {
        let wav = synth_pcm(2, 44100, 16, 1000);
        let info = parse_wav(&wav).unwrap();
        assert_eq!(info.format, WavFormat::Pcm);
        assert_eq!((info.channels, info.sample_rate, info.bits_per_sample), (2, 44100, 16));
        assert_eq!((info.data_offset, info.data_len), (44, 4000));
        assert_eq!(info.frames, 1000);
    }

    #[test]
    fn reads_mono_8bit() {
        let wav = synth_pcm(1, 22050, 8, 250);
        let info = parse_wav(&wav).unwrap();
        assert_eq!((info.channels, info.bits_per_sample, info.frames), (1, 8, 250));
    }

    #[test]
    fn synthetic_ima_adpcm_uses_fact() {
        // RIFF/WAVE + fmt (tag 0x11, 1ch, 22050, block 512) + fact(2000) + data.
        let mut v = Vec::new();
        v.extend_from_slice(RIFF);
        v.extend_from_slice(&0u32.to_le_bytes());
        v.extend_from_slice(WAVE);
        v.extend_from_slice(FMT);
        v.extend_from_slice(&20u32.to_le_bytes());
        v.extend_from_slice(&WAVE_FORMAT_IMA_ADPCM.to_le_bytes());
        v.extend_from_slice(&1u16.to_le_bytes());
        v.extend_from_slice(&22050u32.to_le_bytes());
        v.extend_from_slice(&11025u32.to_le_bytes());
        v.extend_from_slice(&512u16.to_le_bytes());
        v.extend_from_slice(&4u16.to_le_bytes());
        v.extend_from_slice(&2u16.to_le_bytes()); // cbSize
        v.extend_from_slice(&0u16.to_le_bytes()); // samplesPerBlock (unused here)
        v.extend_from_slice(FACT);
        v.extend_from_slice(&4u32.to_le_bytes());
        v.extend_from_slice(&2000u32.to_le_bytes());
        v.extend_from_slice(DATA);
        v.extend_from_slice(&1024u32.to_le_bytes());
        v.resize(v.len() + 1024, 0);
        let info = parse_wav(&v).unwrap();
        assert_eq!(info.format, WavFormat::ImaAdpcm);
        assert_eq!(info.frames, 2000);
        assert_eq!((info.data_offset, info.data_len), (v.len() - 1024, 1024));
    }

    #[test]
    fn ima_adpcm_decodes_header_and_nibbles() {
        // 1 channel, block_align 8: 4-byte header + 4 nibble bytes -> 9 samples per block.
        let (pred, index): (i16, u8) = (1000, 0);
        let nibble_bytes: [u8; 4] = [0x04, 0x00, 0x00, 0x00]; // low nibble 4, then zeros
        let mut v = Vec::new();
        v.extend_from_slice(RIFF);
        v.extend_from_slice(&0u32.to_le_bytes());
        v.extend_from_slice(WAVE);
        v.extend_from_slice(FMT);
        v.extend_from_slice(&16u32.to_le_bytes());
        v.extend_from_slice(&WAVE_FORMAT_IMA_ADPCM.to_le_bytes());
        v.extend_from_slice(&1u16.to_le_bytes());
        v.extend_from_slice(&22050u32.to_le_bytes());
        v.extend_from_slice(&11025u32.to_le_bytes());
        v.extend_from_slice(&8u16.to_le_bytes()); // block_align
        v.extend_from_slice(&4u16.to_le_bytes());
        v.extend_from_slice(DATA);
        v.extend_from_slice(&8u32.to_le_bytes());
        v.extend_from_slice(&pred.to_le_bytes());
        v.push(index);
        v.push(0);
        v.extend_from_slice(&nibble_bytes);
        let info = parse_wav(&v).unwrap();
        assert_eq!(info.frames, 9);
        let samples = decode_ima_adpcm(&info, &v).unwrap();
        assert_eq!(samples.len(), 9);
        assert_eq!(samples[0], 1000, "header predictor");
        // nibble 4 at step 7 -> +7, then index += 2 (step 9).
        assert_eq!(samples[1], 1007);
        assert_eq!(samples[2], 1008);
    }

    #[test]
    fn to_pcm_wav_round_trips() {
        let src = synth_pcm(1, 8000, 16, 10);
        let info = parse_wav(&src).unwrap();
        let samples: Vec<i16> = (0..10).map(|i| i * 100).collect();
        let pcm = to_pcm_wav(&info, &samples);
        let out = parse_wav(&pcm).unwrap();
        assert_eq!(out.format, WavFormat::Pcm);
        assert_eq!(out.frames, 10);
        assert_eq!(&pcm[44..46], &0i16.to_le_bytes());
        assert_eq!(&pcm[44 + 18..44 + 20], &900i16.to_le_bytes());
    }

    #[test]
    fn rejects_malformed_without_panicking() {
        assert_eq!(parse_wav(&[]), Err(WavError::Truncated));
        assert_eq!(parse_wav(b"RIFFxxxxNOPE"), Err(WavError::NotRiff));
        // `fmt ` chunk size past the end of the file.
        let mut v = synth_pcm(1, 8000, 8, 4);
        v[16..20].copy_from_slice(&u32::MAX.to_le_bytes());
        assert_eq!(parse_wav(&v), Err(WavError::BadChunk));
        // No data chunk.
        let mut v = synth_pcm(1, 8000, 8, 4);
        v[36..40].copy_from_slice(b"junk");
        assert_eq!(parse_wav(&v), Err(WavError::MissingData));
    }
}
