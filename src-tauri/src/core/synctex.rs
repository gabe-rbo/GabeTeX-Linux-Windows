//! SyncTeX forward / inverse search through the `synctex` command-line tool: port of
//! `Sources/TeXToolchain/SyncTeX.swift`.
//!
//! Coordinate convention (verified on real pdfTeX output): the CLI uses PDF points with the origin at the
//! **top-left** of the page's media box. In `synctex view` records `h` is the left edge and `v` the **bottom** of
//! the box (baseline + depth), so the box spans `v - H … v` (y growing downward); `synctex edit -o page:x:y`
//! takes a top-left-origin point as well. Conversion to PDF page space (origin bottom-left):
//! `x = mediaBox.minX + h`, `y = mediaBox.maxY - v` (bottom edge), height `H`.
//!
//! The Swift version reads media boxes with `CGPDFDocument`; here [`pdf`] is a small self-contained PDF reader
//! (classic xref tables, xref streams, object streams, Flate + PNG predictors, page-tree inheritance, and a
//! rebuild pass for damaged files) that only extracts the pages' `MediaBox`es.
//!
//! The word-level refinement of the Mac app lives in the editor/preview layer (`SourceWordLocator`), not in
//! `SyncTeX.swift`, so it is not part of this module.

use std::collections::HashSet;
use std::time::Duration;

use crate::commands::{InverseResult, SyncBox};
use crate::core::distribution::{run, Distribution, RunOptions, Tool};
use crate::core::log_parser::{lexically_standardized, ShadowPathMapper};

// ---------------------------------------------------------------------------------------------------
// Records and pure helpers
// ---------------------------------------------------------------------------------------------------

/// One `synctex view` record (top-left origin, `v` = bottom edge).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ViewRecord {
    pub page: u32,
    pub h: f64,
    pub v: f64,
    pub width: f64,
    pub height: f64,
}

/// The first `synctex edit` record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditRecord {
    pub input: String,
    pub line: u32,
    pub column: Option<u32>,
}

/// A rectangle in PDF page space (origin bottom-left).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PageRect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// `synctex view` output: one record per `Page:` line, followed by `x: y: h: v: W: H:` fields.
pub fn parse_view_output(text: &str) -> Vec<ViewRecord> {
    #[derive(Default)]
    struct Fields {
        h: Option<f64>,
        v: Option<f64>,
        w: Option<f64>,
        hh: Option<f64>,
    }
    let mut records = Vec::new();
    let mut current = Fields::default();
    let mut page: Option<u32> = None;
    fn flush(records: &mut Vec<ViewRecord>, current: &mut Fields, page: Option<u32>) {
        if let (Some(page), Some(h), Some(v)) = (page, current.h, current.v) {
            records.push(ViewRecord { page, h, v, width: current.w.unwrap_or(0.0), height: current.hh.unwrap_or(0.0) });
        }
        *current = Fields::default();
    }
    for raw in text.split('\n') {
        let line = raw.trim_matches(|c: char| c == ' ' || c == '\t' || c == '\r');
        if line.is_empty() {
            continue;
        }
        if line.starts_with("SyncTeX result end") {
            flush(&mut records, &mut current, page);
            page = None;
            continue;
        }
        let Some(colon) = line.find(':') else { continue };
        let (key, value) = (&line[..colon], &line[colon + 1..]);
        if key == "Page" {
            flush(&mut records, &mut current, page);
            page = value.trim().parse::<u32>().ok();
        } else if matches!(key, "x" | "y" | "h" | "v" | "W" | "H") {
            if let Ok(number) = value.trim().parse::<f64>() {
                match key {
                    "h" => current.h = Some(number),
                    "v" => current.v = Some(number),
                    "W" => current.w = Some(number),
                    "H" => current.hh = Some(number),
                    _ => {}
                }
            }
        }
    }
    flush(&mut records, &mut current, page);
    records
}

/// `synctex edit` output: the first `Input:` / `Line:` / `Column:` record. Column -1 (or 0) -> `None`.
pub fn parse_edit_output(text: &str) -> Option<EditRecord> {
    let mut input: Option<String> = None;
    let mut line: Option<i64> = None;
    let mut column: Option<u32> = None;
    for raw in text.split('\n') {
        let raw = raw.strip_suffix('\r').unwrap_or(raw);
        if let Some(rest) = raw.strip_prefix("Input:") {
            if input.is_some() && line.is_some() {
                break; // a second record starts
            }
            input = Some(rest.to_string());
        } else if let Some(rest) = raw.strip_prefix("Line:") {
            line = rest.trim().parse::<i64>().ok();
        } else if let Some(rest) = raw.strip_prefix("Column:") {
            let c = rest.trim().parse::<i64>().unwrap_or(-1);
            column = if c > 0 { Some(c as u32) } else { None };
        }
    }
    let input = input.filter(|i| !i.is_empty())?;
    let line = line.filter(|l| *l >= 1)?;
    Some(EditRecord { input, line: line as u32, column })
}

/// `synctex view` box (top-left origin, `v` = bottom edge) -> PDF page-space rectangle.
pub fn page_space_rect(record: &ViewRecord, media_box: &pdf::Rect) -> PageRect {
    PageRect {
        x: media_box.min_x + record.h,
        y: media_box.max_y - record.v,
        width: record.width.max(0.0),
        height: record.height.max(0.0),
    }
}

/// PDF page-space point -> `synctex edit` point (top-left origin).
pub fn synctex_point(x: f64, y: f64, media_box: &pdf::Rect) -> (f64, f64) {
    (x - media_box.min_x, media_box.max_y - y)
}

fn format3(value: f64) -> String {
    format!("{value:.3}")
}

/// The on-disk spelling (letter case on case-insensitive volumes; Unicode normalization is left as typed) of a
/// project file, so paths compare equal to the ones the app got from its file tree. Returns `path` standardized
/// when nothing better is found. (`SyncTeX.canonicalSpelling`)
pub fn canonical_spelling(path: &str, project_directory: &str) -> String {
    let root = lexically_standardized(project_directory);
    let base = if root == "/" { String::new() } else { root.trim_end_matches('/').to_string() };
    let std = lexically_standardized(path);
    let prefix_len = base.len() + 1;
    let inside = std.len() > prefix_len
        && std.is_char_boundary(prefix_len)
        && std.as_bytes()[base.len()] == b'/'
        && if cfg!(windows) { std[..base.len()].eq_ignore_ascii_case(&base) } else { std[..base.len()] == base };
    if !inside {
        return std;
    }
    let mut current = base.clone();
    for wanted in std[prefix_len..].split('/').filter(|c| !c.is_empty()) {
        let listing = if current.is_empty() { "/" } else { current.as_str() };
        let names: Vec<String> = std::fs::read_dir(listing)
            .map(|rd| rd.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect())
            .unwrap_or_default();
        if names.iter().any(|n| n == wanted) {
            current.push('/');
            current.push_str(wanted);
            continue;
        }
        // A case-insensitive volume opened `Chapters/Intro.tex` for chapters/intro.tex: use the real case
        // (only when unambiguous: on a case-sensitive volume both spellings may exist).
        let folded = wanted.to_lowercase();
        let matches: Vec<&String> = names.iter().filter(|n| n.to_lowercase() == folded).collect();
        current.push('/');
        current.push_str(if matches.len() == 1 { matches[0] } else { wanted });
    }
    lexically_standardized(&current)
}

// ---------------------------------------------------------------------------------------------------
// The CLI wrapper
// ---------------------------------------------------------------------------------------------------

/// Forward/inverse search through the `synctex` command-line tool of a [`Distribution`].
pub struct SyncTeX<'a> {
    pub distribution: &'a Distribution,
}

const TIMEOUT: Duration = Duration::from_secs(10);

impl<'a> SyncTeX<'a> {
    pub fn new(distribution: &'a Distribution) -> Self {
        SyncTeX { distribution }
    }

    fn run_synctex(&self, args: &[String]) -> Option<String> {
        let exe = self.distribution.tool(Tool::Synctex)?;
        let options = RunOptions { env: self.distribution.process_environment(), timeout: Some(TIMEOUT), ..Default::default() };
        let output = run(exe, args, &options).ok()?;
        if output.cancelled || output.timed_out {
            return None;
        }
        Some(output.stdout_text())
    }

    /// Source -> PDF. `source_file` is a project path; `mapper` translates it to the shadow path recorded in the
    /// `.synctex.gz`. Returns boxes (possibly several) on the matched page(s); empty when nothing matches.
    pub fn forward(&self, pdf_path: &str, source_file: &str, line: u32, column: u32, mapper: &ShadowPathMapper) -> Vec<SyncBox> {
        if line < 1 || self.distribution.tool(Tool::Synctex).is_none() {
            return Vec::new();
        }
        let shadow_path = mapper.shadow_path(source_file);
        let arguments = ["view".to_string(), "-i".to_string(), format!("{line}:{column}:{shadow_path}"), "-o".to_string(), pdf_path.to_string()];
        let Some(text) = self.run_synctex(&arguments) else { return Vec::new() };
        let records = parse_view_output(&text);
        if records.is_empty() {
            return Vec::new();
        }
        let Some(boxes) = pdf::cached_media_boxes(pdf_path) else { return Vec::new() };
        let mut seen: HashSet<(u32, [u64; 4])> = HashSet::new();
        let mut result = Vec::new();
        for record in &records {
            let Some(media_box) = (record.page as usize).checked_sub(1).and_then(|i| boxes.get(i)) else { continue };
            let r = page_space_rect(record, media_box);
            if seen.insert((record.page, [r.x.to_bits(), r.y.to_bits(), r.width.to_bits(), r.height.to_bits()])) {
                result.push(SyncBox { page: record.page, x: r.x, y: r.y, width: r.width, height: r.height });
            }
        }
        result
    }

    /// PDF -> source. `(x, y)` is in PDF page space (origin bottom-left) of the 1-based `page`.
    pub fn inverse(&self, pdf_path: &str, page: u32, x: f64, y: f64, mapper: &ShadowPathMapper) -> Option<InverseResult> {
        if page < 1 || self.distribution.tool(Tool::Synctex).is_none() {
            return None;
        }
        let boxes = pdf::cached_media_boxes(pdf_path)?;
        let media_box = boxes.get(page as usize - 1)?;
        let (px, py) = synctex_point(x, y, media_box);
        let arguments = ["edit".to_string(), "-o".to_string(), format!("{page}:{}:{}:{pdf_path}", format3(px), format3(py))];
        let text = self.run_synctex(&arguments)?;
        let record = parse_edit_output(&text)?;
        let project_path = mapper.project_path(&record.input)?;
        Some(InverseResult {
            path: canonical_spelling(&project_path, &mapper.project_directory),
            line: record.line,
            column: record.column,
        })
    }
}

/// True when `path` looks like a PDF that [`pdf`] can read (used by callers to fail early).
pub fn pdf_page_count(pdf_path: &str) -> Option<usize> {
    pdf::cached_media_boxes(pdf_path).map(|b| b.len())
}

// ---------------------------------------------------------------------------------------------------
// Minimal PDF reader (media boxes only)
// ---------------------------------------------------------------------------------------------------

pub mod pdf {
    use std::cell::RefCell;
    use std::collections::{HashMap, HashSet};
    use std::sync::{Arc, Mutex};
    use std::time::SystemTime;

    /// A normalized rectangle (`CGRect` of `getBoxRect`).
    #[derive(Debug, Clone, Copy, PartialEq)]
    pub struct Rect {
        pub min_x: f64,
        pub min_y: f64,
        pub max_x: f64,
        pub max_y: f64,
    }

    impl Rect {
        pub fn width(&self) -> f64 {
            self.max_x - self.min_x
        }
        pub fn height(&self) -> f64 {
            self.max_y - self.min_y
        }
        /// US Letter: what Core Graphics assumes for a page without a media box.
        pub const LETTER: Rect = Rect { min_x: 0.0, min_y: 0.0, max_x: 612.0, max_y: 792.0 };
    }

    // ---- inflate (RFC 1950/1951, after zlib's puff.c) -------------------------------------------

    struct Huffman {
        count: [u16; 16],
        symbol: Vec<u16>,
    }

    impl Huffman {
        fn new(lengths: &[u8]) -> Huffman {
            let mut count = [0u16; 16];
            for &l in lengths {
                count[l as usize & 15] += 1;
            }
            count[0] = 0;
            let mut offs = [0u16; 16];
            for len in 1..15 {
                offs[len + 1] = offs[len] + count[len];
            }
            let mut symbol = vec![0u16; lengths.len()];
            for (sym, &l) in lengths.iter().enumerate() {
                if l != 0 {
                    let o = &mut offs[l as usize & 15];
                    if (*o as usize) < symbol.len() {
                        symbol[*o as usize] = sym as u16;
                    }
                    *o += 1;
                }
            }
            Huffman { count, symbol }
        }
    }

    struct Inflater<'a> {
        src: &'a [u8],
        pos: usize,
        bitbuf: u32,
        bitcnt: u32,
        out: Vec<u8>,
        limit: usize,
    }

    const LBASE: [u16; 29] = [3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115, 131, 163, 195, 227, 258];
    const LEXT: [u8; 29] = [0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0];
    const DBASE: [u16; 30] = [
        1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537, 2049, 3073, 4097, 6145, 8193, 12289, 16385, 24577,
    ];
    const DEXT: [u8; 30] = [0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13, 13];

    impl Inflater<'_> {
        fn bits(&mut self, need: u32) -> Option<u32> {
            while self.bitcnt < need {
                let b = *self.src.get(self.pos)?;
                self.pos += 1;
                self.bitbuf |= (b as u32) << self.bitcnt;
                self.bitcnt += 8;
            }
            let v = self.bitbuf & ((1u32 << need) - 1);
            self.bitbuf >>= need;
            self.bitcnt -= need;
            Some(v)
        }

        fn decode(&mut self, h: &Huffman) -> Option<u16> {
            let (mut code, mut first, mut index) = (0i32, 0i32, 0i32);
            for len in 1..16 {
                code |= self.bits(1)? as i32;
                let count = h.count[len] as i32;
                if code - count < first {
                    return h.symbol.get((index + (code - first)) as usize).copied();
                }
                index += count;
                first += count;
                first <<= 1;
                code <<= 1;
            }
            None
        }

        fn stored(&mut self) -> Option<()> {
            self.bitbuf = 0;
            self.bitcnt = 0;
            let h = self.src.get(self.pos..self.pos + 4)?;
            let len = u16::from_le_bytes([h[0], h[1]]) as usize;
            let nlen = u16::from_le_bytes([h[2], h[3]]);
            if len as u16 != !nlen {
                return None;
            }
            self.pos += 4;
            let data = self.src.get(self.pos..self.pos + len)?;
            if self.out.len() + len > self.limit {
                return None;
            }
            self.out.extend_from_slice(data);
            self.pos += len;
            Some(())
        }

        fn codes(&mut self, lencode: &Huffman, distcode: &Huffman) -> Option<()> {
            loop {
                let sym = self.decode(lencode)? as usize;
                if sym < 256 {
                    if self.out.len() >= self.limit {
                        return None;
                    }
                    self.out.push(sym as u8);
                } else if sym == 256 {
                    return Some(());
                } else {
                    let s = sym - 257;
                    if s >= 29 {
                        return None;
                    }
                    let len = LBASE[s] as usize + self.bits(LEXT[s] as u32)? as usize;
                    let d = self.decode(distcode)? as usize;
                    if d >= 30 {
                        return None;
                    }
                    let dist = DBASE[d] as usize + self.bits(DEXT[d] as u32)? as usize;
                    if dist > self.out.len() || self.out.len() + len > self.limit {
                        return None;
                    }
                    let start = self.out.len() - dist;
                    for i in 0..len {
                        let b = self.out[start + i];
                        self.out.push(b);
                    }
                }
            }
        }

        fn fixed(&mut self) -> Option<()> {
            let mut lengths = [0u8; 288];
            for (i, l) in lengths.iter_mut().enumerate() {
                *l = match i {
                    0..=143 => 8,
                    144..=255 => 9,
                    256..=279 => 7,
                    _ => 8,
                };
            }
            let lencode = Huffman::new(&lengths);
            let distcode = Huffman::new(&[5u8; 30]);
            self.codes(&lencode, &distcode)
        }

        fn dynamic(&mut self) -> Option<()> {
            const ORDER: [usize; 19] = [16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15];
            let nlen = self.bits(5)? as usize + 257;
            let ndist = self.bits(5)? as usize + 1;
            let ncode = self.bits(4)? as usize + 4;
            if nlen > 286 || ndist > 30 {
                return None;
            }
            let mut lengths = [0u8; 19];
            for &o in ORDER.iter().take(ncode) {
                lengths[o] = self.bits(3)? as u8;
            }
            let lencode = Huffman::new(&lengths);
            let mut lens = [0u8; 320];
            let mut index = 0;
            while index < nlen + ndist {
                let sym = self.decode(&lencode)?;
                if sym < 16 {
                    lens[index] = sym as u8;
                    index += 1;
                } else {
                    let (prev, rep) = match sym {
                        16 => {
                            if index == 0 {
                                return None;
                            }
                            (lens[index - 1], 3 + self.bits(2)? as usize)
                        }
                        17 => (0, 3 + self.bits(3)? as usize),
                        _ => (0, 11 + self.bits(7)? as usize),
                    };
                    if index + rep > nlen + ndist {
                        return None;
                    }
                    for _ in 0..rep {
                        lens[index] = prev;
                        index += 1;
                    }
                }
            }
            let lencode = Huffman::new(&lens[..nlen]);
            let distcode = Huffman::new(&lens[nlen..nlen + ndist]);
            self.codes(&lencode, &distcode)
        }
    }

    /// Raw DEFLATE data (no zlib header), at most `limit` output bytes.
    pub fn inflate_raw(data: &[u8], limit: usize) -> Option<Vec<u8>> {
        let mut inf = Inflater { src: data, pos: 0, bitbuf: 0, bitcnt: 0, out: Vec::new(), limit };
        loop {
            let last = inf.bits(1)?;
            match inf.bits(2)? {
                0 => inf.stored()?,
                1 => inf.fixed()?,
                2 => inf.dynamic()?,
                _ => return None,
            }
            if last == 1 {
                return Some(inf.out);
            }
        }
    }

    /// zlib stream (RFC 1950): 2-byte header, DEFLATE data, Adler-32 (not verified; PDF producers differ on it).
    pub fn inflate_zlib(data: &[u8], limit: usize) -> Option<Vec<u8>> {
        if data.len() >= 2 && data[0] & 0x0F == 8 && ((data[0] as u16) << 8 | data[1] as u16) % 31 == 0 {
            let skip = if data[1] & 0x20 != 0 { 6 } else { 2 };
            return inflate_raw(data.get(skip..)?, limit);
        }
        inflate_raw(data, limit)
    }

    /// PNG predictors (PDF `/Predictor` >= 10) of a Flate stream.
    fn undo_png_predictor(data: &[u8], columns: usize, colors: usize, bpc: usize) -> Option<Vec<u8>> {
        let row_len = (columns * colors * bpc).div_ceil(8);
        let bpp = (colors * bpc).div_ceil(8).max(1);
        if row_len == 0 {
            return None;
        }
        let mut out = Vec::with_capacity(data.len());
        let mut prev = vec![0u8; row_len];
        for chunk in data.chunks(row_len + 1) {
            let filter = chunk[0];
            let mut cur = chunk[1..].to_vec();
            cur.resize(row_len, 0);
            for i in 0..row_len {
                let a = if i >= bpp { cur[i - bpp] as i32 } else { 0 };
                let b = prev[i] as i32;
                let c = if i >= bpp { prev[i - bpp] as i32 } else { 0 };
                let add = match filter {
                    0 => 0,
                    1 => a,
                    2 => b,
                    3 => (a + b) / 2,
                    4 => {
                        let p = a + b - c;
                        let (pa, pb, pc) = ((p - a).abs(), (p - b).abs(), (p - c).abs());
                        if pa <= pb && pa <= pc {
                            a
                        } else if pb <= pc {
                            b
                        } else {
                            c
                        }
                    }
                    _ => return None,
                };
                cur[i] = (cur[i] as i32 + add) as u8;
            }
            out.extend_from_slice(&cur);
            prev = cur;
        }
        Some(out)
    }

    // ---- objects ----------------------------------------------------------------------------------

    type Dict = HashMap<String, Obj>;

    #[derive(Debug, Clone)]
    enum Obj {
        Null,
        Bool,
        Int(i64),
        Real(f64),
        Name(String),
        Str,
        Array(Vec<Obj>),
        Dict(Dict),
        Ref(u32),
    }

    impl Obj {
        fn num(&self) -> Option<f64> {
            match self {
                Obj::Int(i) => Some(*i as f64),
                Obj::Real(r) => Some(*r),
                _ => None,
            }
        }
        fn int(&self) -> Option<i64> {
            match self {
                Obj::Int(i) => Some(*i),
                Obj::Real(r) if r.fract() == 0.0 => Some(*r as i64),
                _ => None,
            }
        }
        fn dict(&self) -> Option<&Dict> {
            match self {
                Obj::Dict(d) => Some(d),
                _ => None,
            }
        }
        fn name(&self) -> Option<&str> {
            match self {
                Obj::Name(n) => Some(n),
                _ => None,
            }
        }
    }

    fn is_ws(b: u8) -> bool {
        matches!(b, 0 | 9 | 10 | 12 | 13 | 32)
    }

    fn is_delim(b: u8) -> bool {
        matches!(b, b'(' | b')' | b'<' | b'>' | b'[' | b']' | b'{' | b'}' | b'/' | b'%')
    }

    fn is_regular(b: u8) -> bool {
        !is_ws(b) && !is_delim(b)
    }

    fn skip_ws(d: &[u8], p: &mut usize) {
        loop {
            while *p < d.len() && is_ws(d[*p]) {
                *p += 1;
            }
            if *p < d.len() && d[*p] == b'%' {
                while *p < d.len() && d[*p] != 10 && d[*p] != 13 {
                    *p += 1;
                }
            } else {
                break;
            }
        }
    }

    fn read_token<'a>(d: &'a [u8], p: &mut usize) -> &'a [u8] {
        let start = *p;
        while *p < d.len() && is_regular(d[*p]) {
            *p += 1;
        }
        &d[start..*p]
    }

    fn read_uint(d: &[u8], p: &mut usize) -> Option<u64> {
        skip_ws(d, p);
        let start = *p;
        let mut v: u64 = 0;
        while *p < d.len() && d[*p].is_ascii_digit() {
            v = v.checked_mul(10)?.checked_add((d[*p] - b'0') as u64)?;
            *p += 1;
        }
        if *p == start { None } else { Some(v) }
    }

    fn hex(b: u8) -> Option<u8> {
        (b as char).to_digit(16).map(|v| v as u8)
    }

    fn parse_name(d: &[u8], p: &mut usize) -> Option<String> {
        if d.get(*p) != Some(&b'/') {
            return None;
        }
        *p += 1;
        let mut out = Vec::new();
        while *p < d.len() && is_regular(d[*p]) {
            if d[*p] == b'#' && *p + 2 < d.len() && hex(d[*p + 1]).is_some() && hex(*d.get(*p + 2)?).is_some() {
                out.push(hex(d[*p + 1])? * 16 + hex(d[*p + 2])?);
                *p += 3;
            } else {
                out.push(d[*p]);
                *p += 1;
            }
        }
        Some(String::from_utf8_lossy(&out).into_owned())
    }

    fn parse_obj(d: &[u8], p: &mut usize, depth: u32) -> Option<Obj> {
        if depth > 64 {
            return None;
        }
        skip_ws(d, p);
        let c = *d.get(*p)?;
        match c {
            b'/' => parse_name(d, p).map(Obj::Name),
            b'<' if d.get(*p + 1) == Some(&b'<') => {
                *p += 2;
                let mut dict = Dict::new();
                loop {
                    skip_ws(d, p);
                    if d.get(*p) == Some(&b'>') && d.get(*p + 1) == Some(&b'>') {
                        *p += 2;
                        return Some(Obj::Dict(dict));
                    }
                    let key = parse_name(d, p)?;
                    let value = parse_obj(d, p, depth + 1)?;
                    dict.insert(key, value);
                }
            }
            b'<' => {
                while *p < d.len() && d[*p] != b'>' {
                    *p += 1;
                }
                *p += 1;
                Some(Obj::Str)
            }
            b'[' => {
                *p += 1;
                let mut items = Vec::new();
                loop {
                    skip_ws(d, p);
                    if d.get(*p) == Some(&b']') {
                        *p += 1;
                        return Some(Obj::Array(items));
                    }
                    items.push(parse_obj(d, p, depth + 1)?);
                }
            }
            b'(' => {
                *p += 1;
                let mut nesting = 1;
                while *p < d.len() {
                    match d[*p] {
                        b'\\' => *p += 1,
                        b'(' => nesting += 1,
                        b')' => {
                            nesting -= 1;
                            if nesting == 0 {
                                *p += 1;
                                return Some(Obj::Str);
                            }
                        }
                        _ => {}
                    }
                    *p += 1;
                }
                None
            }
            b'0'..=b'9' | b'+' | b'-' | b'.' => {
                let start = *p;
                while *p < d.len() && (d[*p].is_ascii_digit() || matches!(d[*p], b'+' | b'-' | b'.')) {
                    *p += 1;
                }
                let text = std::str::from_utf8(&d[start..*p]).ok()?;
                if text.contains('.') {
                    return text.parse::<f64>().ok().map(Obj::Real);
                }
                let n: i64 = text.parse().ok()?;
                if n >= 0 {
                    // `N G R`
                    let save = *p;
                    if let Some(_gen) = read_uint(d, p) {
                        skip_ws(d, p);
                        if d.get(*p) == Some(&b'R') && d.get(*p + 1).is_none_or(|&b| !is_regular(b)) {
                            *p += 1;
                            return Some(Obj::Ref(n as u32));
                        }
                    }
                    *p = save;
                }
                Some(Obj::Int(n))
            }
            _ => match read_token(d, p) {
                b"true" => Some(Obj::Bool),
                b"false" => Some(Obj::Bool),
                b"null" => Some(Obj::Null),
                _ => None,
            },
        }
    }

    /// `N G obj <object> [stream…]` at `offset`: (object number, object, start of stream data).
    fn parse_indirect(d: &[u8], offset: usize) -> Option<(u32, Obj, Option<usize>)> {
        let mut p = offset;
        let num = read_uint(d, &mut p)?;
        read_uint(d, &mut p)?;
        skip_ws(d, &mut p);
        if read_token(d, &mut p) != b"obj" {
            return None;
        }
        let obj = parse_obj(d, &mut p, 0)?;
        skip_ws(d, &mut p);
        let mut stream = None;
        if matches!(obj, Obj::Dict(_)) && d[p.min(d.len())..].starts_with(b"stream") {
            p += 6;
            if d.get(p) == Some(&13) && d.get(p + 1) == Some(&10) {
                p += 2;
            } else if matches!(d.get(p), Some(&10) | Some(&13)) {
                p += 1;
            }
            stream = Some(p);
        }
        Some((num as u32, obj, stream))
    }

    fn find(d: &[u8], from: usize, needle: &[u8]) -> Option<usize> {
        if from >= d.len() {
            return None;
        }
        d[from..].windows(needle.len()).position(|w| w == needle).map(|i| i + from)
    }

    fn rfind(d: &[u8], needle: &[u8]) -> Option<usize> {
        d.windows(needle.len()).rposition(|w| w == needle)
    }

    // ---- document ----------------------------------------------------------------------------------

    #[derive(Debug, Clone, Copy)]
    enum Entry {
        Offset(usize),
        InStream(u32, u32),
    }

    struct ObjStream {
        data: Vec<u8>,
        first: usize,
        index: Vec<(u32, usize)>,
    }

    const STREAM_LIMIT: usize = 256 << 20;

    struct Doc<'a> {
        d: &'a [u8],
        xref: HashMap<u32, Entry>,
        root: Option<u32>,
        streams: RefCell<HashMap<u32, Option<std::rc::Rc<ObjStream>>>>,
    }

    impl<'a> Doc<'a> {
        fn new(d: &'a [u8]) -> Doc<'a> {
            Doc { d, xref: HashMap::new(), root: None, streams: RefCell::new(HashMap::new()) }
        }

        // -- xref ------------------------------------------------------------------------------------

        fn read_xref_chain(&mut self) -> Option<()> {
            let tail = rfind(self.d, b"startxref")?;
            let mut p = tail + 9;
            let mut next = read_uint(self.d, &mut p)? as usize;
            let mut visited = HashSet::new();
            loop {
                if !visited.insert(next) || next >= self.d.len() {
                    break;
                }
                match self.read_xref_section(next) {
                    Some(Some(prev)) => next = prev,
                    Some(None) => break,
                    None => {
                        if visited.len() == 1 {
                            return None;
                        }
                        break;
                    }
                }
            }
            Some(())
        }

        /// One xref table or stream; returns `Some(prev offset)`.
        fn read_xref_section(&mut self, offset: usize) -> Option<Option<usize>> {
            let d = self.d;
            let mut p = offset;
            skip_ws(d, &mut p);
            if d[p..].starts_with(b"xref") {
                p += 4;
                loop {
                    skip_ws(d, &mut p);
                    if d[p.min(d.len())..].starts_with(b"trailer") {
                        p += 7;
                        break;
                    }
                    let first = read_uint(d, &mut p)? as u32;
                    let count = read_uint(d, &mut p)? as u32;
                    for i in 0..count {
                        let off = read_uint(d, &mut p)? as usize;
                        read_uint(d, &mut p)?;
                        skip_ws(d, &mut p);
                        let kind = *d.get(p)?;
                        p += 1;
                        if kind == b'n' && off > 0 {
                            self.xref.entry(first.wrapping_add(i)).or_insert(Entry::Offset(off));
                        }
                    }
                }
                let trailer = parse_obj(d, &mut p, 0)?;
                let t = trailer.dict()?;
                self.note_root(t);
                if let Some(x) = t.get("XRefStm").and_then(Obj::int) {
                    let _ = self.read_xref_section(x as usize);
                }
                Some(t.get("Prev").and_then(Obj::int).map(|v| v as usize))
            } else {
                let (_, obj, stream) = parse_indirect(d, p)?;
                let dict = obj.dict()?;
                if dict.get("Type").and_then(Obj::name) != Some("XRef") {
                    return None;
                }
                self.note_root(dict);
                let data = self.stream_data(dict, stream?)?;
                let w: Vec<usize> = match dict.get("W")? {
                    Obj::Array(a) => a.iter().filter_map(|o| o.int()).map(|v| v.max(0) as usize).collect(),
                    _ => return None,
                };
                if w.len() != 3 || w.iter().any(|&x| x > 8) {
                    return None;
                }
                let size = dict.get("Size").and_then(Obj::int).unwrap_or(0).max(0) as u32;
                let index: Vec<(u32, u32)> = match dict.get("Index") {
                    Some(Obj::Array(a)) => a.chunks(2).filter_map(|c| Some((c.first()?.int()? as u32, c.get(1)?.int()? as u32))).collect(),
                    _ => vec![(0, size)],
                };
                let entry_len = w[0] + w[1] + w[2];
                if entry_len == 0 {
                    return None;
                }
                let field = |bytes: &[u8]| bytes.iter().fold(0u64, |acc, &b| (acc << 8) | b as u64);
                let mut cursor = 0usize;
                for (first, count) in index {
                    for i in 0..count {
                        let Some(rec) = data.get(cursor..cursor + entry_len) else { return Some(None) };
                        cursor += entry_len;
                        let kind = if w[0] == 0 { 1 } else { field(&rec[..w[0]]) };
                        let f2 = field(&rec[w[0]..w[0] + w[1]]);
                        let f3 = field(&rec[w[0] + w[1]..]);
                        let num = first.wrapping_add(i);
                        match kind {
                            1 => {
                                self.xref.entry(num).or_insert(Entry::Offset(f2 as usize));
                            }
                            2 => {
                                self.xref.entry(num).or_insert(Entry::InStream(f2 as u32, f3 as u32));
                            }
                            _ => {}
                        }
                    }
                }
                Some(dict.get("Prev").and_then(Obj::int).map(|v| v as usize))
            }
        }

        fn note_root(&mut self, trailer: &Dict) {
            if self.root.is_none() {
                if let Some(Obj::Ref(r)) = trailer.get("Root") {
                    self.root = Some(*r);
                }
            }
        }

        /// Rebuilds the cross-reference table by scanning for `N G obj` (damaged or truncated files).
        fn rebuild(&mut self) {
            let d = self.d;
            self.xref.clear();
            self.streams.borrow_mut().clear();
            let mut found: Vec<(u32, usize)> = Vec::new();
            let mut i = 0;
            while let Some(at) = find(d, i, b"obj") {
                i = at + 3;
                if at == 0 || !is_ws(d[at - 1]) || d.get(at + 3).is_some_and(|&b| b.is_ascii_alphabetic()) {
                    continue;
                }
                let mut s = at - 1;
                while s > 0 && is_ws(d[s]) {
                    s -= 1;
                }
                let gen_end = s + 1;
                while s > 0 && d[s].is_ascii_digit() {
                    s -= 1;
                }
                if gen_end == s + 1 || !is_ws(d[s]) {
                    continue;
                }
                while s > 0 && is_ws(d[s]) {
                    s -= 1;
                }
                let num_end = s + 1;
                while s > 0 && d[s].is_ascii_digit() {
                    s -= 1;
                }
                let start = if d[s].is_ascii_digit() { s } else { s + 1 };
                if start >= num_end {
                    continue;
                }
                let mut q = start;
                if let Some(n) = read_uint(d, &mut q) {
                    found.push((n as u32, start));
                }
            }
            for (n, off) in &found {
                self.xref.insert(*n, Entry::Offset(*off));
            }
            let mut catalog = None;
            for (n, off) in &found {
                let Some((_, obj, stream)) = parse_indirect(d, *off) else { continue };
                let Some(dict) = obj.dict() else { continue };
                match dict.get("Type").and_then(Obj::name) {
                    Some("Catalog") => catalog = Some(*n),
                    Some("ObjStm") => {
                        if let (Some(stream), Some(count)) = (stream, dict.get("N").and_then(Obj::int)) {
                            if let Some(os) = self.parse_objstm(dict, stream) {
                                for (idx, (num, _)) in os.index.iter().enumerate().take(count.max(0) as usize) {
                                    self.xref.entry(*num).or_insert(Entry::InStream(*n, idx as u32));
                                }
                            }
                        }
                    }
                    Some("XRef") => self.note_root(dict),
                    _ => {}
                }
                if dict.contains_key("Root") && dict.contains_key("Size") {
                    self.note_root(dict);
                }
            }
            if self.root.is_none() || self.get(self.root.unwrap_or(0)).and_then(|o| o.dict().cloned()).is_none() {
                self.root = catalog;
            }
        }

        // -- streams ---------------------------------------------------------------------------------

        fn resolve(&self, obj: &Obj) -> Obj {
            let mut cur = obj.clone();
            for _ in 0..8 {
                match cur {
                    Obj::Ref(n) => cur = self.get(n).unwrap_or(Obj::Null),
                    _ => break,
                }
            }
            cur
        }

        fn raw_stream<'b>(&'b self, dict: &Dict, start: usize) -> Option<&'b [u8]> {
            let d = self.d;
            if start > d.len() {
                return None;
            }
            let declared = dict.get("Length").map(|l| self.resolve(l)).and_then(|l| l.int()).filter(|&l| l >= 0).map(|l| l as usize);
            if let Some(len) = declared {
                if let Some(end) = start.checked_add(len).filter(|&e| e <= d.len()) {
                    let mut q = end;
                    skip_ws(d, &mut q);
                    if d[q.min(d.len())..].starts_with(b"endstream") {
                        return Some(&d[start..end]);
                    }
                }
            }
            // Missing or wrong /Length: up to the `endstream` keyword, minus the end-of-line before it.
            let mut end = find(d, start, b"endstream")?;
            if end > start && d[end - 1] == 10 {
                end -= 1;
            }
            if end > start && d[end - 1] == 13 {
                end -= 1;
            }
            Some(&d[start..end])
        }

        fn stream_data(&self, dict: &Dict, start: usize) -> Option<Vec<u8>> {
            let raw = self.raw_stream(dict, start)?;
            let filters: Vec<String> = match dict.get("Filter").map(|f| self.resolve(f)) {
                None | Some(Obj::Null) => Vec::new(),
                Some(Obj::Name(n)) => vec![n],
                Some(Obj::Array(a)) => a.iter().filter_map(|o| self.resolve(o).name().map(str::to_string)).collect(),
                _ => return None,
            };
            let parms: Option<Dict> = match dict.get("DecodeParms").or_else(|| dict.get("DP")).map(|f| self.resolve(f)) {
                Some(Obj::Dict(d)) => Some(d),
                Some(Obj::Array(a)) => a.first().map(|o| self.resolve(o)).and_then(|o| o.dict().cloned()),
                _ => None,
            };
            let mut data = raw.to_vec();
            for filter in &filters {
                match filter.as_str() {
                    "FlateDecode" | "Fl" => {
                        data = inflate_zlib(&data, STREAM_LIMIT)?;
                        if let Some(parms) = &parms {
                            let get = |k: &str, default: i64| parms.get(k).map(|o| self.resolve(o)).and_then(|o| o.int()).unwrap_or(default);
                            let predictor = get("Predictor", 1);
                            if predictor >= 10 {
                                data = undo_png_predictor(
                                    &data,
                                    get("Columns", 1).max(1) as usize,
                                    get("Colors", 1).max(1) as usize,
                                    get("BitsPerComponent", 8).max(1) as usize,
                                )?;
                            } else if predictor != 1 {
                                return None; // TIFF predictor: never used for the streams we read
                            }
                        }
                    }
                    _ => return None,
                }
            }
            Some(data)
        }

        fn parse_objstm(&self, dict: &Dict, start: usize) -> Option<ObjStream> {
            let data = self.stream_data(dict, start)?;
            let n = self.resolve(dict.get("N")?).int()?.max(0) as usize;
            let first = self.resolve(dict.get("First")?).int()?.max(0) as usize;
            let mut p = 0;
            let mut index = Vec::with_capacity(n.min(65_536));
            for _ in 0..n {
                let num = read_uint(&data, &mut p)? as u32;
                let off = read_uint(&data, &mut p)? as usize;
                index.push((num, off));
            }
            Some(ObjStream { data, first, index })
        }

        fn object_stream(&self, stm: u32) -> Option<std::rc::Rc<ObjStream>> {
            if let Some(cached) = self.streams.borrow().get(&stm) {
                return cached.clone();
            }
            let parsed = (|| {
                let Entry::Offset(off) = *self.xref.get(&stm)? else { return None };
                let (_, obj, stream) = parse_indirect(self.d, off)?;
                self.parse_objstm(obj.dict()?, stream?)
            })()
            .map(std::rc::Rc::new);
            self.streams.borrow_mut().insert(stm, parsed.clone());
            parsed
        }

        // -- objects ---------------------------------------------------------------------------------

        fn get(&self, num: u32) -> Option<Obj> {
            match *self.xref.get(&num)? {
                Entry::Offset(off) => {
                    let (n, obj, _) = parse_indirect(self.d, off)?;
                    if n == num { Some(obj) } else { None }
                }
                Entry::InStream(stm, idx) => {
                    let os = self.object_stream(stm)?;
                    let (_, off) = match os.index.get(idx as usize) {
                        Some(&(n, off)) if n == num => (n, off),
                        _ => *os.index.iter().find(|(n, _)| *n == num)?,
                    };
                    let mut p = os.first.checked_add(off)?;
                    parse_obj(&os.data, &mut p, 0)
                }
            }
        }

        fn rect_of(&self, obj: &Obj) -> Option<Rect> {
            let Obj::Array(a) = self.resolve(obj) else { return None };
            if a.len() < 4 {
                return None;
            }
            let n: Vec<f64> = a.iter().take(4).map(|o| self.resolve(o).num()).collect::<Option<_>>()?;
            Some(Rect { min_x: n[0].min(n[2]), min_y: n[1].min(n[3]), max_x: n[0].max(n[2]), max_y: n[1].max(n[3]) })
        }

        fn walk(&self, node: &Obj, inherited: Option<Rect>, out: &mut Vec<Rect>, visited: &mut HashSet<u32>, depth: u32) -> Option<()> {
            if depth > 64 || out.len() > 100_000 {
                return None;
            }
            if let Obj::Ref(n) = node {
                if !visited.insert(*n) {
                    return Some(());
                }
            }
            let resolved = self.resolve(node);
            let dict = resolved.dict()?;
            let media = dict.get("MediaBox").and_then(|m| self.rect_of(m)).or(inherited);
            match dict.get("Kids").map(|k| self.resolve(k)) {
                Some(Obj::Array(kids)) => {
                    for kid in &kids {
                        let _ = self.walk(kid, media, out, visited, depth + 1);
                    }
                }
                _ => {
                    if dict.get("Type").and_then(Obj::name) != Some("Pages") {
                        out.push(media.unwrap_or(Rect::LETTER));
                    }
                }
            }
            Some(())
        }

        fn page_boxes(&self) -> Option<Vec<Rect>> {
            let root = self.get(self.root?)?;
            let pages = root.dict()?.get("Pages")?.clone();
            let mut out = Vec::new();
            self.walk(&pages, None, &mut out, &mut HashSet::new(), 0)?;
            if out.is_empty() { None } else { Some(out) }
        }
    }

    /// The media box of every page of a PDF (in page order); `None` when the file is not a readable PDF.
    pub fn media_boxes(bytes: &[u8]) -> Option<Vec<Rect>> {
        let mut doc = Doc::new(bytes);
        let primary = doc.read_xref_chain().and_then(|_| doc.page_boxes());
        if primary.is_some() {
            return primary;
        }
        doc.rebuild();
        doc.page_boxes()
    }

    type CacheEntry = (String, u64, Option<SystemTime>, Arc<Vec<Rect>>);

    /// [`media_boxes`] of a file, cached by path, size and modification time (the preview asks once per click).
    pub fn cached_media_boxes(path: &str) -> Option<Arc<Vec<Rect>>> {
        static CACHE: Mutex<Vec<CacheEntry>> = Mutex::new(Vec::new());
        let meta = std::fs::metadata(path).ok()?;
        let (len, mtime) = (meta.len(), meta.modified().ok());
        {
            let cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(e) = cache.iter().find(|e| e.0 == path && e.1 == len && e.2 == mtime) {
                return Some(e.3.clone());
            }
        }
        let bytes = std::fs::read(path).ok()?;
        let boxes = Arc::new(media_boxes(&bytes)?);
        let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
        cache.retain(|e| e.0 != path);
        cache.push((path.to_string(), len, mtime, boxes.clone()));
        if cache.len() > 4 {
            cache.remove(0);
        }
        Some(boxes)
    }

    /// `PageGeometry.mediaBox(page:)`: the media box of a 1-based page.
    pub fn media_box(path: &str, page: u32) -> Option<Rect> {
        let boxes = cached_media_boxes(path)?;
        boxes.get((page as usize).checked_sub(1)?).copied()
    }
}

// ---------------------------------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::fs;
    use std::path::PathBuf;

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("gabetex-sync-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 0.001
    }

    const VIEW: &str = "This is SyncTeX command line utility, version 1.5\nSyncTeX result begin\nOutput:out/main.pdf\nPage:1\nx:125.878006\ny:66.655540\nh:56.692902\nv:68.592720\nW:481.889771\nH:8.855677\nbefore:\noffset:-1\nmiddle:\nafter:\nSyncTeX result end\n";
    const EDIT: &str = "This is SyncTeX command line utility, version 1.5\nSyncTeX result begin\nOutput:/x/out/main.pdf\nInput:/x/src/./sections/body.tex\nLine:5\nColumn:-1\nOffset:0\nContext:\nSyncTeX result end\n";

    fn a4() -> pdf::Rect {
        pdf::Rect { min_x: 0.0, min_y: 0.0, max_x: 595.276, max_y: 841.89 }
    }

    // ---- parsing and conversion (SyncTeX unit checks) -----------------------------------------------

    #[test]
    fn parses_real_view_output_and_converts_coordinates() {
        let records = parse_view_output(VIEW);
        assert_eq!(records, vec![ViewRecord { page: 1, h: 56.692902, v: 68.592720, width: 481.889771, height: 8.855677 }]);
        let rect = page_space_rect(&records[0], &a4());
        assert!(close(rect.y, 841.89 - 68.592720), "{rect:?}");
        assert!(close(rect.y + rect.height, 841.89 - 68.592720 + 8.855677));
        assert!(close(rect.x, 56.692902) && close(rect.width, 481.889771));
        let shifted = pdf::Rect { min_x: 10.0, min_y: 20.0, max_x: 610.0, max_y: 820.0 };
        let (x, y) = synctex_point(110.0, 720.0, &shifted);
        assert!(close(x, 100.0) && close(y, 100.0), "media box origin honoured: {x} {y}");
        let r = page_space_rect(&records[0], &shifted);
        assert!(close(r.x, 10.0 + 56.692902) && close(r.y, 820.0 - 68.592720));
        // Negative extents are clamped.
        let neg = ViewRecord { page: 1, h: 0.0, v: 0.0, width: -3.0, height: -1.0 };
        let r = page_space_rect(&neg, &a4());
        assert_eq!((r.width, r.height), (0.0, 0.0));
    }

    #[test]
    fn view_output_with_several_records_and_crlf() {
        let text = "SyncTeX result begin\r\nPage:2\r\nx:1\r\ny:2\r\nh:10\r\nv:20\r\nW:30\r\nH:4\r\nPage:3\r\nh:11\r\nv:21\r\nW:31\r\nH:5\r\nSyncTeX result end\r\n";
        let r = parse_view_output(text);
        assert_eq!(r.len(), 2);
        assert_eq!((r[0].page, r[0].h, r[0].v, r[0].width, r[0].height), (2, 10.0, 20.0, 30.0, 4.0));
        assert_eq!((r[1].page, r[1].h), (3, 11.0));
        // Missing W/H default to 0; missing h or v drops the record; garbage pages are ignored.
        let r = parse_view_output("Page:1\nh:1\nv:2\nPage:x\nh:5\nv:6\nPage:4\nh:7\nSyncTeX result end\n");
        assert_eq!(r, vec![ViewRecord { page: 1, h: 1.0, v: 2.0, width: 0.0, height: 0.0 }]);
        assert!(parse_view_output("SyncTeX result begin\nSyncTeX result end\n").is_empty());
        assert!(parse_view_output("").is_empty());
    }

    #[test]
    fn parses_edit_output() {
        assert_eq!(
            parse_edit_output(EDIT),
            Some(EditRecord { input: "/x/src/./sections/body.tex".into(), line: 5, column: None })
        );
        assert_eq!(parse_edit_output("SyncTeX result begin\nSyncTeX result end\n"), None);
        let with_col = "Input:/a b/c.tex\nLine:12\nColumn:7\n";
        assert_eq!(parse_edit_output(with_col), Some(EditRecord { input: "/a b/c.tex".into(), line: 12, column: Some(7) }));
        // Only the first record counts.
        let two = "Input:/a.tex\nLine:1\nColumn:0\nInput:/b.tex\nLine:2\nColumn:3\n";
        assert_eq!(parse_edit_output(two), Some(EditRecord { input: "/a.tex".into(), line: 1, column: None }));
        // CRLF output (Windows) does not leak `\r` into the path.
        let crlf = "Input:C:/p/a.tex\r\nLine:3\r\nColumn:-1\r\n";
        assert_eq!(parse_edit_output(crlf), Some(EditRecord { input: "C:/p/a.tex".into(), line: 3, column: None }));
        assert_eq!(parse_edit_output("Input:/a.tex\nLine:0\n"), None, "line must be >= 1");
        assert_eq!(parse_edit_output("Input:\nLine:4\n"), None);
        assert_eq!(parse_edit_output("Line:4\n"), None);
    }

    #[test]
    fn number_format_matches_printf() {
        assert_eq!(format3(100.0), "100.000");
        assert_eq!(format3(12.34567), "12.346");
        assert_eq!(format3(-0.0004), "-0.000");
    }

    #[test]
    fn canonical_spelling_uses_on_disk_case() {
        let dir = scratch("canon");
        fs::create_dir_all(dir.join("Chapters")).unwrap();
        fs::write(dir.join("Chapters").join("Intro.tex"), "x").unwrap();
        let root = dir.to_str().unwrap();
        let want = lexically_standardized(&format!("{root}/Chapters/Intro.tex"));
        assert_eq!(canonical_spelling(&format!("{root}/chapters/intro.tex"), root), want);
        assert_eq!(canonical_spelling(&format!("{root}/Chapters/./Intro.tex"), root), want);
        assert_eq!(canonical_spelling(&format!("{root}/Chapters/Intro.tex"), root), want);
        // Unknown files keep their spelling; outside the project nothing changes.
        assert_eq!(canonical_spelling(&format!("{root}/chapters/new.tex"), root), lexically_standardized(&format!("{root}/Chapters/new.tex")));
        assert_eq!(canonical_spelling("/elsewhere/Foo.tex", root), "/elsewhere/Foo.tex");
        // Ambiguous case-insensitive matches (case-sensitive volume with both spellings) stay as typed.
        #[cfg(unix)]
        {
            fs::write(dir.join("Chapters").join("INTRO.TEX"), "x").unwrap();
            let got = canonical_spelling(&format!("{root}/Chapters/Intro.TeX"), root);
            assert!(got.ends_with("/Chapters/Intro.TeX"), "{got}");
        }
        let _ = fs::remove_dir_all(&dir);
    }

    // ---- inflate -----------------------------------------------------------------------------------

    fn unhex(s: &str) -> Vec<u8> {
        (0..s.len() / 2).map(|i| u8::from_str_radix(&s[2 * i..2 * i + 2], 16).unwrap()).collect()
    }

    #[test]
    fn inflate_dynamic_fixed_and_stored_blocks() {
        // zlib.compress(level 9) of the text below (dynamic Huffman block).
        let dynamic = unhex("78daedd74d4e03310c05e03da7c81148329364ae036a454be9b4b4153d3ee2069f8497b3b7fcf3fc6c3f9f0ee75d7a4deb3edd3f76e9fa38bc7fa6b7eff5e79cf6eb331d1f5f97dbcbe9cf2683cd04368bc46a60546630aae269929c9a143704a52ac549b4a983512f124ed296da9a785aa873025327e64a4eb3845b884d3405344e9493849b05a721fd95d615e1e59070b31457a4bf83669cc65720c8e269089d1a714ec0ac82532616082f85bc43b0ecd2ba4e9e6a985158e2040181d9a2da4b4421ca1179690c72d868d290d3baa0c5432b8c9621ad555ad023ea64d0f1b13396c34e6b8b3af7241c488290982159d4c2a45a89128f244349d0d628894d629d647f0d7b45e8a95982be2c0169fbfab6af6ffbfafe7f507e01e3b2fe81");
        let expected: String = (0..120).map(|i: u32| format!("line {} of the quick brown fox jumps\n", (i * i) % 97)).collect();
        assert_eq!(pdf::inflate_zlib(&dynamic, 1 << 20).unwrap(), expected.as_bytes());
        // Z_FIXED stream.
        let fixed = unhex("7801cb48cdc9c957c840903a0a699915a9290a19a56969b989798a00cf2f0bff");
        assert_eq!(pdf::inflate_zlib(&fixed, 1 << 20).unwrap(), b"hello hello hello, fixed huffman!");
        // Stored block.
        assert_eq!(pdf::inflate_zlib(&zlib_stored(b"stored!"), 100).unwrap(), b"stored!");
        // Limits and corruption.
        assert!(pdf::inflate_zlib(&dynamic, 100).is_none(), "output limit");
        assert!(pdf::inflate_zlib(&dynamic[..60], 1 << 20).is_none(), "truncated");
        assert!(pdf::inflate_zlib(&[0x78, 0x9c, 0x07], 10).is_none(), "reserved block type");
    }

    // ---- PDF fixtures -------------------------------------------------------------------------------

    fn zlib_stored(data: &[u8]) -> Vec<u8> {
        let mut out = vec![0x78, 0x01];
        let mut chunks = data.chunks(65_535).peekable();
        if data.is_empty() {
            out.extend_from_slice(&[1, 0, 0, 0xFF, 0xFF]);
        }
        while let Some(chunk) = chunks.next() {
            out.push(if chunks.peek().is_none() { 1 } else { 0 });
            out.extend_from_slice(&(chunk.len() as u16).to_le_bytes());
            out.extend_from_slice(&(!(chunk.len() as u16)).to_le_bytes());
            out.extend_from_slice(chunk);
        }
        out.extend_from_slice(&[0, 0, 0, 0]);
        out
    }

    /// Page tree: root Pages with MediaBox A4; page 1 inherits it; page 2 has its own; page 3 sits in a
    /// nested Pages node with yet another box (given with reversed corners and an indirect number).
    fn page_objects() -> Vec<(u32, String)> {
        vec![
            (1, "<< /Type /Catalog /Pages 2 0 R >>".into()),
            (2, "<< /Type /Pages /Kids [3 0 R 4 0 R 6 0 R] /Count 3 /MediaBox [0 0 595.276 841.89] >>".into()),
            (3, "<< /Type /Page /Parent 2 0 R >>".into()),
            (4, "<< /Type /Page /Parent 2 0 R /MediaBox [10 20 610 820] >>".into()),
            (5, "<< /Type /Page /Parent 6 0 R >>".into()),
            (6, "<< /Type /Pages /Parent 2 0 R /Kids [5 0 R] /Count 1 /MediaBox [300 200 0 0] >>".into()),
        ]
    }

    fn expected_boxes() -> Vec<pdf::Rect> {
        vec![
            pdf::Rect { min_x: 0.0, min_y: 0.0, max_x: 595.276, max_y: 841.89 },
            pdf::Rect { min_x: 10.0, min_y: 20.0, max_x: 610.0, max_y: 820.0 },
            pdf::Rect { min_x: 0.0, min_y: 0.0, max_x: 300.0, max_y: 200.0 },
        ]
    }

    /// Classic cross-reference table.
    fn classic_pdf(objects: &[(u32, String)]) -> Vec<u8> {
        let mut out = b"%PDF-1.4\n%\xE2\xE3\xCF\xD3\n".to_vec();
        let mut offsets: BTreeMap<u32, usize> = BTreeMap::new();
        for (n, body) in objects {
            offsets.insert(*n, out.len());
            out.extend_from_slice(format!("{n} 0 obj\n{body}\nendobj\n").as_bytes());
        }
        let size = offsets.keys().max().unwrap() + 1;
        let xref = out.len();
        out.extend_from_slice(format!("xref\n0 {size}\n0000000000 65535 f \n").as_bytes());
        for n in 1..size {
            match offsets.get(&n) {
                Some(o) => out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes()),
                None => out.extend_from_slice(b"0000000000 00000 f \n"),
            }
        }
        out.extend_from_slice(format!("trailer\n<< /Size {size} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n").as_bytes());
        out
    }

    /// PDF 1.5 layout like pdfTeX's: page objects in an object stream, xref stream with a PNG-Up predictor.
    fn modern_pdf(objects: &[(u32, String)]) -> Vec<u8> {
        let mut out = b"%PDF-1.5\n%\xD0\xD4\xC5\xD8\n".to_vec();
        // Object stream 7 holds every object but the first (kept as a plain object).
        let (plain, packed): (Vec<_>, Vec<_>) = objects.iter().cloned().partition(|(n, _)| *n == 1);
        let mut header = String::new();
        let mut body = String::new();
        for (n, text) in &packed {
            header.push_str(&format!("{n} {} ", body.len()));
            body.push_str(text);
            body.push('\n');
        }
        let first = header.len();
        let stm_data = zlib_stored(format!("{header}{body}").as_bytes());
        let mut offsets: BTreeMap<u32, usize> = BTreeMap::new();
        for (n, text) in &plain {
            offsets.insert(*n, out.len());
            out.extend_from_slice(format!("{n} 0 obj\n{text}\nendobj\n").as_bytes());
        }
        offsets.insert(7, out.len());
        out.extend_from_slice(
            format!("7 0 obj\n<< /Type /ObjStm /N {} /First {first} /Length {} /Filter /FlateDecode >>\nstream\n", packed.len(), stm_data.len()).as_bytes(),
        );
        out.extend_from_slice(&stm_data);
        out.extend_from_slice(b"\nendstream\nendobj\n");
        let xref_off = out.len();
        offsets.insert(8, xref_off);
        // Entries: W [1 3 1], objects 0..=8.
        let mut rows: Vec<[u8; 5]> = Vec::new();
        for n in 0..=8u32 {
            let row = if n == 0 {
                [0, 0, 0, 0, 255]
            } else if let Some(idx) = packed.iter().position(|(m, _)| *m == n) {
                [2, 0, 0, 7, idx as u8]
            } else {
                let o = offsets[&n] as u32;
                [1, (o >> 16) as u8, (o >> 8) as u8, o as u8, 0]
            };
            rows.push(row);
        }
        // PNG "Up" predictor, 5 columns.
        let mut predicted = Vec::new();
        let mut prev = [0u8; 5];
        for row in &rows {
            predicted.push(2);
            for i in 0..5 {
                predicted.push(row[i].wrapping_sub(prev[i]));
            }
            prev = *row;
        }
        let xref_data = zlib_stored(&predicted);
        out.extend_from_slice(
            format!(
                "8 0 obj\n<< /Type /XRef /Size 9 /W [1 3 1] /Root 1 0 R /Filter /FlateDecode /DecodeParms << /Predictor 12 /Columns 5 >> /Length {} >>\nstream\n",
                xref_data.len()
            )
            .as_bytes(),
        );
        out.extend_from_slice(&xref_data);
        out.extend_from_slice(format!("\nendstream\nendobj\nstartxref\n{xref_off}\n%%EOF\n").as_bytes());
        out
    }

    fn assert_boxes(got: Option<Vec<pdf::Rect>>, want: &[pdf::Rect]) {
        let got = got.expect("boxes");
        assert_eq!(got.len(), want.len());
        for (g, w) in got.iter().zip(want) {
            assert!(close(g.min_x, w.min_x) && close(g.min_y, w.min_y) && close(g.max_x, w.max_x) && close(g.max_y, w.max_y), "{g:?} vs {w:?}");
        }
    }

    #[test]
    fn reads_media_boxes_from_classic_xref() {
        assert_boxes(pdf::media_boxes(&classic_pdf(&page_objects())), &expected_boxes());
    }

    #[test]
    fn reads_media_boxes_from_object_streams_and_xref_stream() {
        assert_boxes(pdf::media_boxes(&modern_pdf(&page_objects())), &expected_boxes());
    }

    #[test]
    fn rebuilds_damaged_files() {
        // Wrong startxref offset (e.g. a file with junk prepended), classic and modern.
        let mut classic = classic_pdf(&page_objects());
        let mut junk = b"JUNKJUNKJUNK\n".to_vec();
        junk.extend_from_slice(&classic);
        classic = junk;
        assert_boxes(pdf::media_boxes(&classic), &expected_boxes());
        let mut modern = b"JUNKJUNKJUNK\n".to_vec();
        modern.extend_from_slice(&modern_pdf(&page_objects()));
        assert_boxes(pdf::media_boxes(&modern), &expected_boxes());
        // Truncated before the xref: objects are still found.
        let full = classic_pdf(&page_objects());
        let cut = full.windows(4).position(|w| w == b"xref").unwrap();
        assert_boxes(pdf::media_boxes(&full[..cut]), &expected_boxes());
        // Not a PDF.
        assert!(pdf::media_boxes(b"hello world").is_none());
        assert!(pdf::media_boxes(b"").is_none());
    }

    #[test]
    fn page_without_media_box_is_letter_and_cycles_do_not_hang() {
        let objects = vec![
            (1, "<< /Type /Catalog /Pages 2 0 R >>".to_string()),
            (2, "<< /Type /Pages /Kids [3 0 R 2 0 R] /Count 1 >>".to_string()),
            (3, "<< /Type /Page /Parent 2 0 R >>".to_string()),
        ];
        assert_boxes(pdf::media_boxes(&classic_pdf(&objects)), &[pdf::Rect::LETTER]);
    }

    #[test]
    fn indirect_numbers_and_wrong_length_streams() {
        let objects = vec![
            (1, "<< /Type /Catalog /Pages 2 0 R >>".to_string()),
            (2, "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string()),
            (3, "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 4 0 R 5 0 R] /Contents 6 0 R >>".to_string()),
            (4, "300".to_string()),
            (5, "400.5".to_string()),
            (6, "<< /Length 3 >>\nstream\nBT ET (endstream in text? no) \nendstream".to_string()),
        ];
        assert_boxes(pdf::media_boxes(&classic_pdf(&objects)), &[pdf::Rect { min_x: 0.0, min_y: 0.0, max_x: 300.0, max_y: 400.5 }]);
    }

    #[test]
    fn media_box_cache_follows_the_file() {
        let dir = scratch("cache");
        let f = dir.join("a.pdf");
        fs::write(&f, classic_pdf(&page_objects())).unwrap();
        let path = f.to_str().unwrap();
        assert_eq!(pdf_page_count(path), Some(3));
        let b2 = pdf::media_box(path, 2).unwrap();
        assert!(close(b2.min_x, 10.0) && close(b2.max_y, 820.0));
        assert!(pdf::media_box(path, 0).is_none() && pdf::media_box(path, 4).is_none());
        fs::write(&f, classic_pdf(&page_objects()[..4])).unwrap();
        // 2 pages now (page 3 and the nested node are gone): different size -> cache refreshed.
        let fewer = vec![
            (1, "<< /Type /Catalog /Pages 2 0 R >>".to_string()),
            (2, "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 100 100] >>".to_string()),
            (3, "<< /Type /Page /Parent 2 0 R >>".to_string()),
        ];
        fs::write(&f, classic_pdf(&fewer)).unwrap();
        assert_eq!(pdf_page_count(path), Some(1));
        assert!(pdf_page_count("/no/such.pdf").is_none());
        let _ = fs::remove_dir_all(&dir);
    }

    // ---- CLI wrapper with a fake `synctex` ----------------------------------------------------------

    #[cfg(unix)]
    mod cli {
        use super::*;
        use std::path::Path;
        use crate::core::distribution::Kind;
        use std::os::unix::fs::PermissionsExt;

        fn fake_distribution(dir: &Path, script_body: &str) -> Distribution {
            let exe = dir.join("synctex");
            fs::write(&exe, format!("#!/bin/sh\n{script_body}\n")).unwrap();
            fs::set_permissions(&exe, fs::Permissions::from_mode(0o755)).unwrap();
            let mut tools = BTreeMap::new();
            tools.insert(Tool::Synctex, exe.to_string_lossy().into_owned());
            Distribution {
                name: "Fake".into(),
                year: None,
                kind: Kind::TeXLive,
                bin_dir: dir.to_string_lossy().into_owned(),
                texmf_root: None,
                user_writable: true,
                tools,
                version_description: None,
            }
        }

        #[test]
        fn forward_passes_shadow_path_and_converts_boxes() {
            let dir = scratch("fwd");
            let log = dir.join("args.txt");
            let script = format!(
                "echo \"$@\" > '{}'\ncat <<'EOF'\nSyncTeX result begin\nOutput:out/main.pdf\nPage:1\nx:125.8\ny:66.6\nh:56.692902\nv:68.592720\nW:481.889771\nH:8.855677\nbefore:\nSyncTeX result end\nPage:2\nh:10\nv:20\nW:5\nH:6\nPage:1\nh:56.692902\nv:68.592720\nW:481.889771\nH:8.855677\nPage:9\nh:1\nv:1\nW:1\nH:1\nSyncTeX result end\nEOF",
                log.display()
            );
            let dist = fake_distribution(&dir, &script);
            let pdf_file = dir.join("main.pdf");
            fs::write(&pdf_file, classic_pdf(&page_objects())).unwrap();
            let mapper = ShadowPathMapper::new("/home/u/Proj", "/cache/S/abc/src");
            let boxes = SyncTeX::new(&dist).forward(pdf_file.to_str().unwrap(), "/home/u/Proj/sections/a.tex", 12, 4, &mapper);
            let args = fs::read_to_string(&log).unwrap();
            assert_eq!(args.trim(), format!("view -i 12:4:/cache/S/abc/src/sections/a.tex -o {}", pdf_file.display()));
            // Page 1 (A4) once (duplicate record removed), page 2 (box origin 10,20), page 9 does not exist.
            assert_eq!(boxes.len(), 2, "{boxes:?}");
            assert_eq!(boxes[0].page, 1);
            assert!(close(boxes[0].x, 56.692902) && close(boxes[0].y, 841.89 - 68.592720) && close(boxes[0].width, 481.889771) && close(boxes[0].height, 8.855677));
            assert_eq!(boxes[1].page, 2);
            assert!(close(boxes[1].x, 20.0) && close(boxes[1].y, 800.0) && close(boxes[1].width, 5.0) && close(boxes[1].height, 6.0));
            assert!(SyncTeX::new(&dist).forward(pdf_file.to_str().unwrap(), "/home/u/Proj/a.tex", 0, 0, &mapper).is_empty(), "line < 1");
            let _ = fs::remove_dir_all(&dir);
        }

        #[test]
        fn forward_is_empty_without_output_or_pdf() {
            let dir = scratch("fwd-empty");
            let dist = fake_distribution(&dir, "echo 'SyncTeX result begin'\necho 'SyncTeX result end'");
            let pdf_file = dir.join("main.pdf");
            fs::write(&pdf_file, classic_pdf(&page_objects())).unwrap();
            let mapper = ShadowPathMapper::new("/p", "/s");
            assert!(SyncTeX::new(&dist).forward(pdf_file.to_str().unwrap(), "/p/a.tex", 1, 0, &mapper).is_empty());
            let dist = fake_distribution(&dir, "printf 'Page:1\\nh:1\\nv:2\\nW:3\\nH:4\\nSyncTeX result end\\n'");
            assert!(SyncTeX::new(&dist).forward("/no/such.pdf", "/p/a.tex", 1, 0, &mapper).is_empty(), "unreadable PDF");
            let mut none = dist.clone();
            none.tools.clear();
            assert!(SyncTeX::new(&none).forward(pdf_file.to_str().unwrap(), "/p/a.tex", 1, 0, &mapper).is_empty(), "no synctex tool");
            let _ = fs::remove_dir_all(&dir);
        }

        #[test]
        fn inverse_converts_the_point_and_maps_the_path_back() {
            let dir = scratch("inv");
            let project = dir.join("Proj");
            fs::create_dir_all(project.join("Sections")).unwrap();
            fs::write(project.join("Sections").join("Body.tex"), "x").unwrap();
            let log = dir.join("args.txt");
            let script = format!(
                "echo \"$@\" > '{}'\nprintf 'SyncTeX result begin\\nOutput:/x/main.pdf\\nInput:/cache/S/abc/src/./sections/body.tex\\nLine:5\\nColumn:-1\\nOffset:0\\nContext:\\nSyncTeX result end\\n'",
                log.display()
            );
            let dist = fake_distribution(&dir, &script);
            let pdf_file = dir.join("main.pdf");
            fs::write(&pdf_file, classic_pdf(&page_objects())).unwrap();
            let mapper = ShadowPathMapper::new(project.to_str().unwrap(), "/cache/S/abc/src");
            let hit = SyncTeX::new(&dist).inverse(pdf_file.to_str().unwrap(), 2, 110.0, 720.0, &mapper).expect("location");
            // Page 2 has media box (10, 20, 610, 820): the point becomes (100, 100) in top-left coordinates.
            assert_eq!(fs::read_to_string(&log).unwrap().trim(), format!("edit -o 2:100.000:100.000:{}", pdf_file.display()));
            assert_eq!(hit.line, 5);
            assert_eq!(hit.column, None);
            assert_eq!(hit.path, lexically_standardized(&format!("{}/Sections/Body.tex", project.display())), "on-disk spelling");
            assert!(SyncTeX::new(&dist).inverse(pdf_file.to_str().unwrap(), 0, 1.0, 1.0, &mapper).is_none());
            assert!(SyncTeX::new(&dist).inverse(pdf_file.to_str().unwrap(), 7, 1.0, 1.0, &mapper).is_none(), "page out of range");
            let _ = fs::remove_dir_all(&dir);
        }

        #[test]
        fn inverse_ignores_files_outside_the_shadow() {
            let dir = scratch("inv-out");
            let dist = fake_distribution(&dir, "printf 'Input:/usr/share/texmf/article.cls\\nLine:3\\nColumn:-1\\n'");
            let pdf_file = dir.join("main.pdf");
            fs::write(&pdf_file, classic_pdf(&page_objects())).unwrap();
            let mapper = ShadowPathMapper::new("/p", "/cache/S/abc/src");
            assert!(SyncTeX::new(&dist).inverse(pdf_file.to_str().unwrap(), 1, 10.0, 10.0, &mapper).is_none());
            let dist = fake_distribution(&dir, "exit 1");
            assert!(SyncTeX::new(&dist).inverse(pdf_file.to_str().unwrap(), 1, 10.0, 10.0, &mapper).is_none());
            let _ = fs::remove_dir_all(&dir);
        }
    }
}
