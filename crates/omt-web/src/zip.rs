//! An in-memory ZIP writer for the support bundle: Stored and Deflate
//! members, no ZIP64, no encryption. The Deflate encoder is a greedy LZ77
//! over a 32 KiB window with fixed Huffman codes -- encoding only, so the
//! appliance never parses an attacker's compressed stream.

// Every narrowing cast here is of a value the format bounds: Huffman
// symbols and codes, bits below a byte, names under 128 bytes, and at most
// 64 members.
#![allow(clippy::cast_possible_truncation)]

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Method {
    Stored = 0,
    Deflated = 8,
}

pub const MAX_MEMBERS: usize = 64;

#[derive(Clone, Debug)]
pub struct Member {
    pub name: String,
    pub crc: u32,
    pub compressed: u32,
    pub size: u32,
    pub offset: u32,
    pub method: Method,
}

const fn crc_table() -> [u32; 256] {
    let mut table = [0_u32; 256];
    let mut i = 0;
    while i < 256 {
        let mut c = i as u32;
        let mut k = 0;
        while k < 8 {
            c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
            k += 1;
        }
        table[i] = c;
        i += 1;
    }
    table
}

static CRC_TABLE: [u32; 256] = crc_table();

pub fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFF_u32;
    for &b in data {
        crc = CRC_TABLE[((crc ^ u32::from(b)) & 0xFF) as usize] ^ (crc >> 8);
    }
    crc ^ 0xFFFF_FFFF
}

struct BitWriter<'a> {
    out: &'a mut Vec<u8>,
    bits: u32,
    count: u32,
}

impl BitWriter<'_> {
    fn put_bits(&mut self, value: u32, n: u32) {
        self.bits |= value << self.count;
        self.count += n;
        while self.count >= 8 {
            self.out.push(self.bits as u8);
            self.bits >>= 8;
            self.count -= 8;
        }
    }

    /// Huffman codes are sent most-significant bit first.
    fn put_code(&mut self, code: u32, n: u32) {
        self.put_bits(code.reverse_bits() >> (32 - n), n);
    }

    /// The fixed literal/length code of RFC 1951 section 3.2.6.
    fn put_literal(&mut self, symbol: u32) {
        match symbol {
            0..=143 => self.put_code(0x30 + symbol, 8),
            144..=255 => self.put_code(0x190 + (symbol - 144), 9),
            256..=279 => self.put_code(symbol - 256, 7),
            _ => self.put_code(0xC0 + (symbol - 280), 8),
        }
    }

    fn put_match(&mut self, length: u32, distance: u32) {
        let l = LENGTH_BASE.iter().rposition(|&b| u32::from(b) <= length).unwrap_or(0);
        self.put_literal(257 + l as u32);
        if LENGTH_EXTRA[l] != 0 {
            self.put_bits(length - u32::from(LENGTH_BASE[l]), u32::from(LENGTH_EXTRA[l]));
        }
        let d = DIST_BASE.iter().rposition(|&b| u32::from(b) <= distance).unwrap_or(0);
        self.put_code(d as u32, 5);
        if DIST_EXTRA[d] != 0 {
            self.put_bits(distance - u32::from(DIST_BASE[d]), u32::from(DIST_EXTRA[d]));
        }
    }
}

const LENGTH_BASE: [u16; 29] = [
    3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115, 131,
    163, 195, 227, 258,
];
const LENGTH_EXTRA: [u8; 29] =
    [0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0];
const DIST_BASE: [u16; 30] = [
    1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537,
    2049, 3073, 4097, 6145, 8193, 12289, 16385, 24577,
];
const DIST_EXTRA: [u8; 30] = [
    0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13,
    13,
];

const WINDOW: usize = 32768;
const HASH_BITS: u32 = 15;
const HASH_SIZE: usize = 1 << HASH_BITS;
const MAX_CHAIN: usize = 64;
const MIN_MATCH: usize = 3;
const MAX_MATCH: usize = 258;

fn hash3(p: &[u8]) -> usize {
    let v = (u32::from(p[0]) << 16) ^ (u32::from(p[1]) << 8) ^ u32::from(p[2]);
    (v.wrapping_mul(2_654_435_761) >> (32 - HASH_BITS)) as usize
}

/// Raw Deflate (RFC 1951) of `data`, appended to `out`.
pub fn deflate(data: &[u8], out: &mut Vec<u8>) {
    let mut w = BitWriter { out, bits: 0, count: 0 };
    // One fixed-Huffman block for the whole member: BFINAL=1, BTYPE=01.
    w.put_bits(1, 1);
    w.put_bits(1, 2);
    let mut head = vec![usize::MAX; HASH_SIZE];
    let mut prev = vec![usize::MAX; WINDOW];
    let len = data.len();
    let mut i = 0;
    while i < len {
        let (mut best_len, mut best_dist) = (0, 0);
        if len - i >= MIN_MATCH {
            let mut candidate = head[hash3(&data[i..])];
            let limit = MAX_MATCH.min(len - i);
            let mut chain = MAX_CHAIN;
            while candidate != usize::MAX && chain > 0 && i - candidate < WINDOW {
                chain -= 1;
                let n = data[candidate..candidate + limit]
                    .iter()
                    .zip(&data[i..i + limit])
                    .take_while(|(a, b)| a == b)
                    .count();
                if n > best_len {
                    best_len = n;
                    best_dist = i - candidate;
                    if n == limit {
                        break;
                    }
                }
                let next = prev[candidate % WINDOW];
                if next == usize::MAX || next >= candidate {
                    break;
                }
                candidate = next;
            }
        }
        let advance = if best_len >= MIN_MATCH {
            w.put_match(best_len as u32, best_dist as u32);
            best_len
        } else {
            w.put_literal(u32::from(data[i]));
            1
        };
        // Index every position consumed, so later matches can reach it.
        for _ in 0..advance {
            if len - i >= MIN_MATCH {
                let h = hash3(&data[i..]);
                prev[i % WINDOW] = head[h];
                head[h] = i;
            }
            i += 1;
        }
    }
    w.put_literal(256);
    if w.count != 0 {
        let pad = 8 - w.count;
        w.put_bits(0, pad);
    }
}

/// 1980-01-01 00:00, the timestamp the Rust writer's defaults produced.
const DOS_TIME: u16 = 0x0000;
const DOS_DATE: u16 = 0x0021;

#[derive(Debug)]
pub struct Zip {
    out: Vec<u8>,
    limit: usize,
    pub members: Vec<Member>,
    failed: bool,
}

fn le16(b: &mut Vec<u8>, v: u16) {
    b.extend_from_slice(&v.to_le_bytes());
}

fn le32(b: &mut Vec<u8>, v: u32) {
    b.extend_from_slice(&v.to_le_bytes());
}

impl Zip {
    pub fn new(limit: usize) -> Self {
        Self { out: Vec::new(), limit, members: Vec::new(), failed: false }
    }

    pub fn add(&mut self, name: &str, data: &[u8], method: Method) {
        let (Ok(size), Ok(offset)) = (u32::try_from(data.len()), u32::try_from(self.out.len()))
        else {
            self.failed = true;
            return;
        };
        if self.failed || self.members.len() == MAX_MEMBERS || name.len() >= 128 {
            self.failed = true;
            return;
        }
        let mut body = Vec::new();
        match method {
            Method::Deflated => deflate(data, &mut body),
            Method::Stored => body.extend_from_slice(data),
        }
        let Ok(compressed) = u32::try_from(body.len()) else {
            self.failed = true;
            return;
        };
        if self.out.len() + 30 + name.len() + body.len() > self.limit {
            self.failed = true;
            return;
        }
        let m =
            Member { name: name.to_owned(), crc: crc32(data), compressed, size, offset, method };
        let out = &mut self.out;
        le32(out, 0x0403_4b50);
        le16(out, 20);
        le16(out, 0x0800); // UTF-8 names
        le16(out, m.method as u16);
        le16(out, DOS_TIME);
        le16(out, DOS_DATE);
        le32(out, m.crc);
        le32(out, m.compressed);
        le32(out, m.size);
        le16(out, name.len() as u16);
        le16(out, 0);
        out.extend_from_slice(name.as_bytes());
        out.extend_from_slice(&body);
        self.members.push(m);
    }

    /// Writes the central directory and returns the archive; `None` if any
    /// member or the archive failed.
    pub fn finish(mut self) -> Option<Vec<u8>> {
        let directory = u32::try_from(self.out.len()).ok().filter(|_| !self.failed)?;
        let out = &mut self.out;
        for m in &self.members {
            le32(out, 0x0201_4b50);
            le16(out, 0x0314); // made by Unix, spec 2.0
            le16(out, 20);
            le16(out, 0x0800);
            le16(out, m.method as u16);
            le16(out, DOS_TIME);
            le16(out, DOS_DATE);
            le32(out, m.crc);
            le32(out, m.compressed);
            le32(out, m.size);
            le16(out, m.name.len() as u16);
            le16(out, 0);
            le16(out, 0);
            le16(out, 0);
            le16(out, 0);
            le32(out, 0o100_644 << 16); // regular file, rw-r--r--
            le32(out, m.offset);
            out.extend_from_slice(m.name.as_bytes());
        }
        let size = u32::try_from(out.len()).ok()? - directory;
        le32(out, 0x0605_4b50);
        le16(out, 0);
        le16(out, 0);
        le16(out, self.members.len() as u16);
        le16(out, self.members.len() as u16);
        le32(out, size);
        le32(out, directory);
        le16(out, 0);
        (out.len() <= self.limit).then_some(self.out)
    }
}
