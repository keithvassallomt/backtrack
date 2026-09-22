// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! A small PNG writer, so the demo fixture can contain actual pictures.
//!
//! Every "image" in the fixture used to be prose under a `.jpg` name, which
//! meant the preview pane's picture branch had never run against demo data and
//! the compare view had nothing to put side by side. The pane decides what it
//! is holding by trying to decode the bytes, so nothing short of a real image
//! will do.
//!
//! Written here rather than pulled in, for two reasons. An encoder wants a
//! deflate implementation, and a PNG is allowed to store its pixels
//! *uncompressed* — a deflate stream made of stored blocks is a legal deflate
//! stream — which removes the only hard part. And generating the images means
//! the fixture can vary them per snapshot without carrying a folder of binary
//! blobs in a source repository, which is the thing nobody ever remembers to
//! regenerate.
//!
//! The output is a truecolour, 8-bit, non-interlaced PNG: the most ordinary
//! shape there is, and the one every decoder handles.

/// `width` × `height` truecolour PNG, with each pixel drawn by `paint`.
pub fn encode(width: u32, height: u32, paint: impl Fn(u32, u32) -> [u8; 3]) -> Vec<u8> {
    let mut raw = Vec::with_capacity((height * (1 + width * 3)) as usize);
    for y in 0..height {
        // Filter type 0 (None) at the head of every scanline. Filtering exists
        // to help the compressor, and nothing here is compressing.
        raw.push(0);
        for x in 0..width {
            raw.extend_from_slice(&paint(x, y));
        }
    }

    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend_from_slice(&width.to_be_bytes());
    ihdr.extend_from_slice(&height.to_be_bytes());
    ihdr.extend_from_slice(&[8, 2, 0, 0, 0]); // 8 bits, truecolour, no interlace

    let mut out = Vec::new();
    out.extend_from_slice(&[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]);
    chunk(&mut out, b"IHDR", &ihdr);
    chunk(&mut out, b"IDAT", &zlib_stored(&raw));
    chunk(&mut out, b"IEND", &[]);
    out
}

/// One PNG chunk: length, type, payload, CRC of the type and payload.
fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], payload: &[u8]) {
    out.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    out.extend_from_slice(kind);
    out.extend_from_slice(payload);
    let mut crc = Crc32::new();
    crc.write(kind);
    crc.write(payload);
    out.extend_from_slice(&crc.finish().to_be_bytes());
}

/// `data` as a zlib stream of stored (uncompressed) deflate blocks.
///
/// A stored block carries its length in a 16-bit field, so the data is cut into
/// 65535-byte pieces and the last one carries the final-block flag.
fn zlib_stored(data: &[u8]) -> Vec<u8> {
    const MAX: usize = u16::MAX as usize;

    let mut out = Vec::with_capacity(data.len() + data.len() / MAX * 5 + 6);
    // Deflate, 32K window, no preset dictionary, fastest compression level.
    // The two header bytes read as a big-endian multiple of 31.
    out.extend_from_slice(&[0x78, 0x01]);

    let mut blocks = data.chunks(MAX).peekable();
    // An empty input still needs one (final, empty) block for the stream to be
    // well formed.
    if blocks.peek().is_none() {
        out.extend_from_slice(&[1, 0, 0, 0xff, 0xff]);
    }
    while let Some(block) = blocks.next() {
        out.push(u8::from(blocks.peek().is_none()));
        let len = block.len() as u16;
        out.extend_from_slice(&len.to_le_bytes());
        out.extend_from_slice(&(!len).to_le_bytes());
        out.extend_from_slice(block);
    }

    out.extend_from_slice(&adler32(data).to_be_bytes());
    out
}

/// Adler-32, which is the checksum a zlib stream ends with.
fn adler32(data: &[u8]) -> u32 {
    const MOD: u32 = 65_521;
    let (mut a, mut b) = (1u32, 0u32);
    for byte in data {
        a = (a + *byte as u32) % MOD;
        b = (b + a) % MOD;
    }
    (b << 16) | a
}

/// CRC-32 as PNG specifies it, computed a nibble at a time so the table is
/// sixteen entries rather than two hundred and fifty-six.
struct Crc32(u32);

const CRC_NIBBLES: [u32; 16] = [
    0x0000_0000,
    0x1db7_1064,
    0x3b6e_20c8,
    0x26d9_30ac,
    0x76dc_4190,
    0x6b6b_51f4,
    0x4db2_6158,
    0x5005_713c,
    0xedb8_8320,
    0xf00f_9344,
    0xd6d6_a3e8,
    0xcb61_b38c,
    0x9b64_c2b0,
    0x86d3_d2d4,
    0xa00a_e278,
    0xbdbd_f21c,
];

impl Crc32 {
    fn new() -> Crc32 {
        Crc32(0xffff_ffff)
    }

    fn write(&mut self, data: &[u8]) {
        for byte in data {
            self.0 ^= *byte as u32;
            for _ in 0..2 {
                self.0 = (self.0 >> 4) ^ CRC_NIBBLES[(self.0 & 0x0f) as usize];
            }
        }
    }

    fn finish(self) -> u32 {
        self.0 ^ 0xffff_ffff
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The CRC of "123456789" is the standard check value every CRC-32
    /// implementation is expected to agree on.
    #[test]
    fn the_crc_matches_the_published_check_value() {
        let mut crc = Crc32::new();
        crc.write(b"123456789");
        assert_eq!(crc.finish(), 0xcbf4_3926);
    }

    /// Likewise Adler-32's, from RFC 1950's own example.
    #[test]
    fn the_adler_checksum_matches_the_published_check_value() {
        assert_eq!(adler32(b"123456789"), 0x091e_01de);
    }

    #[test]
    fn an_encoded_image_is_a_well_formed_png() {
        let bytes = encode(3, 2, |x, y| [x as u8, y as u8, 0]);

        assert_eq!(
            &bytes[..8],
            &[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]
        );

        // Walk the chunks, checking each length and CRC, and collect the order
        // they came in.
        let mut kinds = Vec::new();
        let mut at = 8;
        while at < bytes.len() {
            let len = u32::from_be_bytes(bytes[at..at + 4].try_into().unwrap()) as usize;
            let kind = &bytes[at + 4..at + 8];
            let payload = &bytes[at + 8..at + 8 + len];
            let stored = u32::from_be_bytes(bytes[at + 8 + len..at + 12 + len].try_into().unwrap());
            let mut crc = Crc32::new();
            crc.write(kind);
            crc.write(payload);
            assert_eq!(crc.finish(), stored, "chunk CRC");
            kinds.push(String::from_utf8_lossy(kind).to_string());
            at += 12 + len;
        }
        assert_eq!(kinds, ["IHDR", "IDAT", "IEND"]);
        assert_eq!(at, bytes.len(), "the chunks account for the whole file");
    }

    #[test]
    fn a_stored_stream_carries_the_bytes_it_was_given() {
        // Longer than one stored block, so the splitting is exercised.
        let data: Vec<u8> = (0..70_000u32).map(|i| (i % 251) as u8).collect();
        let stream = zlib_stored(&data);

        assert_eq!(&stream[..2], &[0x78, 0x01]);
        assert_eq!(
            u32::from_be_bytes(stream[stream.len() - 4..].try_into().unwrap()),
            adler32(&data),
        );

        // Walk the stored blocks back out and compare against the input.
        let mut out = Vec::new();
        let mut at = 2;
        loop {
            let final_block = stream[at] & 1 == 1;
            let len = u16::from_le_bytes(stream[at + 1..at + 3].try_into().unwrap()) as usize;
            let nlen = u16::from_le_bytes(stream[at + 3..at + 5].try_into().unwrap());
            assert_eq!(nlen, !(len as u16), "NLEN is LEN's complement");
            out.extend_from_slice(&stream[at + 5..at + 5 + len]);
            at += 5 + len;
            if final_block {
                break;
            }
        }
        assert_eq!(out, data);
        assert_eq!(
            at,
            stream.len() - 4,
            "the blocks end where the checksum starts"
        );
    }

    /// Two different paintings have to produce two different files, or "an
    /// older version of this photo" is not a thing the fixture can show.
    #[test]
    fn different_paintings_differ() {
        let one = encode(8, 8, |x, _| [x as u8, 0, 0]);
        let two = encode(8, 8, |_, y| [0, y as u8, 0]);
        assert_ne!(one, two);
        assert_eq!(one.len(), two.len());
    }
}
