//! Word, heading and formula counts of a whole LaTeX project, per outline section (port of
//! `Statistics/*`: `DocumentStatistics`, `StatisticsEngine` (+Text, +Environments), `StatisticsRules`,
//! `WordClassifier`, `PageAttribution`).
//!
//! ## Counting rules (close to `texcount`'s defaults)
//!
//! **What counts as a word.** A run of letters and digits. Accented letters (typed as UTF-8 or as LaTeX
//! accents: `ação`, `Poincar\'e`, `\c{c}\~ao`, `Stra\ss e`, `Sch"on`), hyphenated words (`state-of-the-art`,
//! also `hy\-phen`), contractions (`don't`, `d’água`), numbers (`1990`, `3.14`, `1,5`) and mixed forms
//! (`COVID-19`, `2nd`) are one word each. `--` and `---` separate (`10--20` is two numbers), as do `~`,
//! commands and all punctuation. Han and kana count one word per character.
//!
//! **Counted:** running text, list items, captions, footnotes, theorem titles, the text arguments of
//! formatting commands, the titles of `\part … \subparagraph` and beamer frames (as *heading* words).
//!
//! **Not counted:** the preamble (everything before `\begin{document}`; a document without one counts
//! entirely) and anything after `\end{document}`; comments; verbatim, listings and `\verb`; formulas (which are
//! counted as formulas instead); commands and their non-text arguments; `tikzpicture` & co. and the
//! bibliography; URLs; tables, `tabbing`, `titlepage` and everything inside a float except its captions.
//!
//! **Sections.** Front matter holds what precedes the first heading. A section's `own` counts run from its
//! heading to the next heading of any level; `subtree` adds its sub-sections. A file included twice is counted
//! once, where it first appears. A subfile (own `\documentclass` and `\begin{document}`) counts only what lies
//! between its `\begin{document}` and `\end{document}`.

use super::arguments as args;
use super::masked::MaskedSource;
use super::models::*;
use super::syntax::*;
use crate::core::project_index::{self as pi, ExistsFn, ReadFn};
use crate::core::scanner::latex_scanner as ls;
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::ops::{Add, AddAssign, Range, Sub};
use std::rc::Rc;

// ---------------------------------------------------------------------------------------------------
// Models
// ---------------------------------------------------------------------------------------------------

/// Counters for one piece of a document: a section, the front matter or the whole document.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StatisticsCounts {
    /// Words of running text (paragraphs, list items, captions, footnotes …).
    pub text_words: usize,
    /// Words of headings (`\section{…}`, `\chapter{…}`, beamer frame titles …).
    pub heading_words: usize,
    /// Inline formulas: `$…$`, `\(…\)`, `\begin{math}`.
    pub inline_math: usize,
    /// Displayed formulas: `$$…$$`, `\[…\]`, `equation`, `align`, `gather` … (one per environment).
    pub display_math: usize,
}

impl StatisticsCounts {
    /// Text and heading words together: the number shown as "N words".
    pub fn words(&self) -> usize {
        self.text_words + self.heading_words
    }
    /// Nothing counted at all.
    pub fn is_empty(&self) -> bool {
        self.words() == 0 && self.inline_math == 0 && self.display_math == 0
    }
    pub const ZERO: StatisticsCounts = StatisticsCounts { text_words: 0, heading_words: 0, inline_math: 0, display_math: 0 };
}

impl Add for StatisticsCounts {
    type Output = StatisticsCounts;
    fn add(self, r: StatisticsCounts) -> StatisticsCounts {
        StatisticsCounts {
            text_words: self.text_words + r.text_words,
            heading_words: self.heading_words + r.heading_words,
            inline_math: self.inline_math + r.inline_math,
            display_math: self.display_math + r.display_math,
        }
    }
}
impl Sub for StatisticsCounts {
    type Output = StatisticsCounts;
    fn sub(self, r: StatisticsCounts) -> StatisticsCounts {
        StatisticsCounts {
            text_words: self.text_words.saturating_sub(r.text_words),
            heading_words: self.heading_words.saturating_sub(r.heading_words),
            inline_math: self.inline_math.saturating_sub(r.inline_math),
            display_math: self.display_math.saturating_sub(r.display_math),
        }
    }
}
impl AddAssign for StatisticsCounts {
    fn add_assign(&mut self, r: StatisticsCounts) {
        *self = *self + r;
    }
}

pub const FRONT_MATTER_ID: &str = "front-matter";

/// The counts of one outline section (part … subparagraph or a beamer frame), or of the front matter.
///
/// `own` is what lies directly in the section: from its heading to the next heading of ANY level. `subtree`
/// adds the sections nested inside it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SectionStatistics {
    /// `None` for the front matter (everything before the first heading).
    pub kind: Option<OutlineKind>,
    /// Heading title as the outline shows it (empty for the front matter).
    pub title: String,
    pub is_starred: bool,
    /// File and 1-based line of the heading command (the root file, line 1, for the front matter).
    pub file: FilePath,
    pub line: usize,
    /// 1-based line where the heading's title ends (equals `line` for one-line headings).
    pub heading_end_line: usize,
    /// UTF-16 offset of the heading command in its file.
    pub offset: usize,
    /// Nesting level (`OutlineKind::level`; -1 for the front matter).
    pub level: i32,
    pub own: StatisticsCounts,
    pub subtree: StatisticsCounts,
    /// Equals `OutlineNode::id` of the outline node of this heading.
    pub id: String,
    /// Survives edits that only move the heading: file, kind, title and the n-th heading with that triple.
    pub stable_key: String,
}

impl SectionStatistics {
    pub fn is_front_matter(&self) -> bool {
        self.kind.is_none()
    }

    fn heading(entry: &OutlineEntry, heading_end_line: usize, own: StatisticsCounts, occurrence: usize) -> Self {
        SectionStatistics {
            kind: Some(entry.kind),
            title: entry.title.clone(),
            is_starred: entry.is_starred,
            file: entry.file.clone(),
            line: entry.line,
            heading_end_line: heading_end_line.max(entry.line),
            offset: entry.offset,
            level: entry.kind.level(),
            own,
            subtree: own,
            id: format!("{}#{}#{}#{}", entry.file, entry.line, entry.kind.raw(), entry.title),
            stable_key: format!("{}#{}#{}#{}", entry.file, entry.kind.raw(), entry.title, occurrence),
        }
    }

    fn front_matter(root: &str, own: StatisticsCounts) -> Self {
        SectionStatistics {
            kind: None,
            title: String::new(),
            is_starred: false,
            file: FilePath::from(root),
            line: 1,
            heading_end_line: 1,
            offset: 0,
            level: -1,
            own,
            subtree: own,
            id: FRONT_MATTER_ID.to_string(),
            stable_key: FRONT_MATTER_ID.to_string(),
        }
    }
}

/// Word, heading and formula counts of a whole LaTeX project, per outline section.
#[derive(Debug, Clone, PartialEq)]
pub struct DocumentStatistics {
    pub root_file: FilePath,
    /// Everything between `\begin{document}` and the first heading: the title block, the abstract …
    pub front_matter: SectionStatistics,
    /// Headings and beamer frames in reading order (the outline entries without its labels).
    pub sections: Vec<SectionStatistics>,
    /// Front matter plus every section. Always equals the sum of the `own` counts.
    pub totals: StatisticsCounts,
    /// The .tex files that were walked, in reading order (root first).
    pub files: Vec<FilePath>,
    index_by_id: HashMap<String, usize>,
}

impl DocumentStatistics {
    /// Front matter first, then the sections.
    pub fn all_sections(&self) -> Vec<&SectionStatistics> {
        std::iter::once(&self.front_matter).chain(self.sections.iter()).collect()
    }

    /// The section whose id is `id` (an `OutlineNode::id`, or [`FRONT_MATTER_ID`]).
    pub fn section(&self, id: &str) -> Option<&SectionStatistics> {
        if id == FRONT_MATTER_ID {
            return Some(&self.front_matter);
        }
        self.index_by_id.get(id).map(|&p| &self.sections[p])
    }
}

/// Counts the words and formulas of the project rooted at `root_file` (a standardized path). Pass the text
/// of unsaved buffers from `read`.
pub fn compute(root_file: &str, read: ReadFn, file_exists: ExistsFn) -> DocumentStatistics {
    Engine::new(pi::standardize(root_file), read, file_exists, false).run()
}

/// The counts of a loose piece of LaTeX (a selection): no sections, no includes, nothing is a preamble.
pub fn counts_in(text: &str) -> StatisticsCounts {
    let read = |_: &str| Some(text.to_string());
    let exists = |_: &str| false;
    Engine::new("/selection.tex".to_string(), &read, &exists, true).run().totals
}

// ---------------------------------------------------------------------------------------------------
// Page attribution
// ---------------------------------------------------------------------------------------------------

/// Where a section heading landed in the PDF (from a SyncTeX forward search of its line).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HeadingPlacement {
    /// 1-based PDF page.
    pub page: i64,
    /// The heading sits at the top of its page, so the previous section ends on the page before.
    pub is_at_page_top: bool,
}

/// The pages a section occupies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PageSpan {
    pub first: i64,
    pub last: i64,
    /// The section starts on a page it shares with the next section (so "its" pages are only approximate).
    pub is_approximate: bool,
}

impl PageSpan {
    pub fn page_count(&self) -> i64 {
        self.last - self.first + 1
    }
}

/// A heading whose top edge is in the upper part of the page counts as "at the top".
pub const TOP_FRACTION: f64 = 0.72;

/// Whether a box whose top edge is at `max_y` (PDF page space, origin bottom-left) in a media box that spans
/// `media_min_y ..< media_min_y + media_height` lies in the top part of its page.
pub fn is_at_page_top(max_y: f64, media_min_y: f64, media_height: f64) -> bool {
    if media_height <= 0.0 {
        return false;
    }
    (max_y - media_min_y) / media_height >= TOP_FRACTION
}

/// Page spans for sections in reading order (see `PageAttribution.spans`): a section spans from the page of its
/// heading to the page of the next heading (shared unless that heading is at the top of its page).
pub fn page_spans(placements: &[Option<HeadingPlacement>], total_pages: i64) -> Vec<Option<PageSpan>> {
    let mut result: Vec<Option<PageSpan>> = vec![None; placements.len()];
    if total_pages <= 0 {
        return result;
    }
    let mut usable: Vec<Option<HeadingPlacement>> = vec![None; placements.len()];
    let mut lowest = 1;
    for (index, placement) in placements.iter().enumerate() {
        let Some(p) = placement else { continue };
        if p.page < lowest || p.page > total_pages {
            continue;
        }
        usable[index] = Some(*p);
        lowest = p.page;
    }
    let mut next: Option<HeadingPlacement> = None;
    for index in (0..placements.len()).rev() {
        let Some(own) = usable[index] else { continue };
        if let Some(nx) = next {
            if nx.page > own.page {
                let last = if nx.is_at_page_top { nx.page - 1 } else { nx.page };
                result[index] = Some(PageSpan { first: own.page, last: own.page.max(last), is_approximate: false });
            } else {
                result[index] = Some(PageSpan { first: own.page, last: own.page, is_approximate: true });
            }
        } else {
            result[index] = Some(PageSpan { first: own.page, last: total_pages, is_approximate: false });
        }
        next = Some(own);
    }
    result
}

/// Widens every section's span over the sections nested in it (`levels` = `SectionStatistics::level`).
pub fn subtree_spans(levels: &[i32], spans: &[Option<PageSpan>]) -> Vec<Option<PageSpan>> {
    let mut result = spans.to_vec();
    let count = levels.len().min(spans.len());
    for index in 0..count {
        let Some(own) = spans[index] else { continue };
        if levels[index] < 0 {
            continue;
        }
        let mut last = own.last;
        let mut next = index + 1;
        while next < count && levels[next] > levels[index] {
            if let Some(nested) = spans[next] {
                if nested.last > last {
                    last = nested.last;
                }
            }
            next += 1;
        }
        if last > own.last {
            result[index] = Some(PageSpan { first: own.first, last, is_approximate: false });
        }
    }
    result
}

// ---------------------------------------------------------------------------------------------------
// Word classification
// ---------------------------------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CharacterClass {
    Letter,
    Digit,
    /// Combining marks and invisible format characters: they continue a word and never start one.
    Mark,
    /// Han and kana: every character counts as a word of its own.
    Ideograph,
    Hyphen,
    Apostrophe,
    Other,
}

/// Ranges (inclusive) of combining marks (general categories Mn, Mc, Me) of the common scripts.
const MARK_RANGES: &[(u32, u32)] = &[
    (0x0300, 0x036F), (0x0483, 0x0489), (0x0591, 0x05BD), (0x05BF, 0x05BF), (0x05C1, 0x05C2), (0x05C4, 0x05C5),
    (0x05C7, 0x05C7), (0x0610, 0x061A), (0x064B, 0x065F), (0x0670, 0x0670), (0x06D6, 0x06DC), (0x06DF, 0x06E4),
    (0x06E7, 0x06E8), (0x06EA, 0x06ED), (0x0711, 0x0711), (0x0730, 0x074A), (0x07A6, 0x07B0), (0x07EB, 0x07F3),
    (0x0816, 0x0819), (0x081B, 0x0823), (0x0825, 0x0827), (0x0829, 0x082D), (0x0859, 0x085B), (0x08D3, 0x08E1),
    (0x08E3, 0x0903), (0x093A, 0x093C), (0x093E, 0x094F), (0x0951, 0x0957), (0x0962, 0x0963), (0x0981, 0x0983),
    (0x09BC, 0x09BC), (0x09BE, 0x09C4), (0x09C7, 0x09C8), (0x09CB, 0x09CD), (0x09D7, 0x09D7), (0x09E2, 0x09E3),
    (0x0A01, 0x0A03), (0x0A3C, 0x0A51), (0x0A70, 0x0A71), (0x0A75, 0x0A75), (0x0A81, 0x0A83), (0x0ABC, 0x0ABC),
    (0x0ABE, 0x0ACD), (0x0B01, 0x0B03), (0x0B3C, 0x0B3C), (0x0B3E, 0x0B57), (0x0B82, 0x0B82), (0x0BBE, 0x0BCD),
    (0x0BD7, 0x0BD7), (0x0C00, 0x0C04), (0x0C3E, 0x0C56), (0x0C81, 0x0C83), (0x0CBC, 0x0CBC), (0x0CBE, 0x0CD6),
    (0x0D00, 0x0D03), (0x0D3B, 0x0D3C), (0x0D3E, 0x0D4D), (0x0D57, 0x0D57), (0x0D82, 0x0D83), (0x0DCA, 0x0DDF),
    (0x0E31, 0x0E31), (0x0E34, 0x0E3A), (0x0E47, 0x0E4E), (0x0EB1, 0x0EB1), (0x0EB4, 0x0EBC), (0x0EC8, 0x0ECD),
    (0x0F18, 0x0F19), (0x0F35, 0x0F35), (0x0F37, 0x0F37), (0x0F39, 0x0F39), (0x0F3E, 0x0F3F), (0x0F71, 0x0F84),
    (0x0F86, 0x0F87), (0x0F8D, 0x0FBC), (0x102B, 0x103E), (0x1056, 0x1059), (0x135D, 0x135F), (0x1712, 0x1714),
    (0x17B4, 0x17D3), (0x180B, 0x180D), (0x1AB0, 0x1AFF), (0x1B00, 0x1B04), (0x1B34, 0x1B44), (0x1DC0, 0x1DFF),
    (0x20D0, 0x20FF), (0x2CEF, 0x2CF1), (0x2DE0, 0x2DFF), (0x302A, 0x302F), (0x3099, 0x309A), (0xA66F, 0xA672),
    (0xA674, 0xA67D), (0xA69E, 0xA69F), (0xA6F0, 0xA6F1), (0xA802, 0xA802), (0xA806, 0xA806), (0xA80B, 0xA80B),
    (0xA823, 0xA827), (0xA880, 0xA881), (0xA8B4, 0xA8C5), (0xFB1E, 0xFB1E), (0xFE00, 0xFE0F), (0xFE20, 0xFE2F),
    (0x1D165, 0x1D169), (0x1D16D, 0x1D172), (0xE0100, 0xE01EF),
];

/// Start code points of the blocks of ten decimal digits (general category Nd) beyond ASCII.
const DIGIT_BLOCKS: &[u32] = &[
    0x0660, 0x06F0, 0x07C0, 0x0966, 0x09E6, 0x0A66, 0x0AE6, 0x0B66, 0x0BE6, 0x0C66, 0x0CE6, 0x0D66, 0x0DE6, 0x0E50,
    0x0ED0, 0x0F20, 0x1040, 0x1090, 0x17E0, 0x1810, 0x1946, 0x19D0, 0x1A80, 0x1A90, 0x1B50, 0x1BB0, 0x1C40, 0x1C50,
    0xA620, 0xA8D0, 0xA900, 0xA9D0, 0xA9F0, 0xAA50, 0xABF0, 0xFF10, 0x1D7CE, 0x1D7D8, 0x1D7E2, 0x1D7EC, 0x1D7F6,
];

fn is_mark(cp: u32) -> bool {
    MARK_RANGES.binary_search_by(|&(lo, hi)| {
        if cp < lo {
            std::cmp::Ordering::Greater
        } else if cp > hi {
            std::cmp::Ordering::Less
        } else {
            std::cmp::Ordering::Equal
        }
    })
    .is_ok()
}

fn is_decimal_digit(cp: u32) -> bool {
    if cp < 128 {
        return (0x30..=0x39).contains(&cp);
    }
    DIGIT_BLOCKS.iter().any(|&s| cp >= s && cp < s + 10)
}

/// What a character does to the word being built. (Unicode general categories are approximated with
/// `char::is_alphabetic` plus built-in mark and digit tables, since the crate has no Unicode database.)
pub fn classify(ch: char) -> CharacterClass {
    let value = ch as u32;
    match value {
        0x2010 | 0x2011 => return CharacterClass::Hyphen,
        0x2019 | 0x02BC => return CharacterClass::Apostrophe,
        0x00AD | 0x200C | 0x200D | 0xFEFF => return CharacterClass::Mark,
        0x30FB => return CharacterClass::Other,
        0x3041..=0x30FF | 0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF | 0x20000..=0x2FA1F => return CharacterClass::Ideograph,
        _ => {}
    }
    if is_mark(value) {
        return CharacterClass::Mark;
    }
    if is_decimal_digit(value) {
        return CharacterClass::Digit;
    }
    // Letter-like numbers (Roman numerals) and circled letters are alphabetic in Rust but not letters in Unicode.
    if matches!(value, 0x2160..=0x2188 | 0x24B6..=0x24E9 | 0x3007 | 0x3021..=0x3029 | 0x3038..=0x303A) {
        return CharacterClass::Other;
    }
    if ch.is_alphabetic() {
        return CharacterClass::Letter;
    }
    CharacterClass::Other
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Joiner {
    Hyphen,
    Apostrophe,
    NumberSeparator,
}

/// Tracks whether the characters seen so far form one word, so that "state-of-the-art", "don’t", "3.14" and
/// "COVID-19" count once while "pages 10--20" counts twice.
#[derive(Debug, Clone, Copy, Default)]
pub struct WordState {
    in_word: bool,
    last_was_digit: bool,
    pending: Option<Joiner>,
}

impl WordState {
    /// True while the word is made of letters only and nothing is waiting to be joined.
    pub fn is_inside_letters(&self) -> bool {
        self.in_word && !self.last_was_digit && self.pending.is_none()
    }

    /// A letter arrived. Returns true when it starts a new word.
    pub fn letter(&mut self) -> bool {
        let mut started = !self.in_word;
        if self.pending == Some(Joiner::NumberSeparator) {
            started = true; // "3.a": the number ended at the dot
        }
        self.in_word = true;
        self.last_was_digit = false;
        self.pending = None;
        started
    }

    /// A digit arrived. Returns true when it starts a new word.
    pub fn digit(&mut self) -> bool {
        let started = !self.in_word;
        self.in_word = true;
        self.last_was_digit = true;
        self.pending = None;
        started
    }

    /// A character that joins what is on both sides of it, when a letter or digit follows.
    pub fn joiner(&mut self, kind: Joiner) {
        if self.in_word && self.pending.is_none() && (kind != Joiner::NumberSeparator || self.last_was_digit) {
            self.pending = Some(kind);
        } else {
            self.separator();
        }
    }

    /// Whitespace, punctuation, a command, a formula … the word (if any) is over.
    pub fn separator(&mut self) {
        self.in_word = false;
        self.last_was_digit = false;
        self.pending = None;
    }
}

// ---------------------------------------------------------------------------------------------------
// Rules
// ---------------------------------------------------------------------------------------------------

#[derive(Debug, Clone, Copy)]
enum Rule {
    /// An ordinary command: it ends the word before it, an attached `[option]` or `<overlay>` is dropped and its
    /// `{arguments}` are read as text.
    Unknown,
    /// Ends the word and nothing else (`\item`, whose `[label]` is text).
    Separator,
    Heading,
    FrameTitle,
    Begin,
    End,
    Include,
    IgnoreArguments(usize),
    Definition(usize),
    PlainDefinition,
    NewTheorem,
    DeclareTheorem,
    Letter,
    Accent,
    TexOrPdfString,
    Cmidrule,
    Caption(usize),
    SubCaption,
}

fn rule_for(name: &str) -> Rule {
    if sectioning_kind(name).is_some() {
        return Rule::Heading;
    }
    match name {
        "title" => Rule::Heading,
        "frametitle" | "framesubtitle" => Rule::FrameTitle,
        "begin" => Rule::Begin,
        "end" => Rule::End,
        "input" | "include" | "subfile" | "import" | "subimport" => Rule::Include,
        "item" => Rule::Separator,
        // Commands whose first argument(s) are names, keys, lengths or files — never text.
        "label" | "ref" | "eqref" | "pageref" | "autoref" | "Autoref" | "nameref" | "vref" | "Vref" | "cref" | "Cref"
        | "labelcref" | "cpageref" | "Cpageref" | "subref" | "nocite" | "bibliography" | "bibliographystyle"
        | "addbibresource" | "addglobalbib" | "addsectionbib" | "includegraphics" | "includepdf" | "includesvg"
        | "includestandalone" | "graphicspath" | "usepackage" | "RequirePackage" | "RequirePackageWithOptions"
        | "LoadClass" | "documentclass" | "usetikzlibrary" | "usepgflibrary" | "usepgfplotslibrary" | "lstinputlisting"
        | "pagestyle" | "thispagestyle" | "pagenumbering" | "stepcounter" | "refstepcounter" | "newcounter"
        | "newlength" | "hypersetup" | "geometry" | "newgeometry" | "vspace" | "hspace" | "vphantom" | "hphantom"
        | "phantom" | "color" | "pagecolor" | "selectlanguage" | "babelprovide" | "index" | "glossary" | "hyphenation"
        | "enlargethispage" | "markright" | "captionsetup" | "lstset" | "tikzset" | "pgfplotsset" | "includeonly"
        | "bibitem" | "url" | "nolinkurl" | "href" | "hypertarget" | "hyperlink" | "textcolor" | "colorbox"
        | "cellcolor" | "rowcolor" | "columncolor" | "arrayrulecolor" | "cline" | "foreignlanguage" | "scalebox"
        | "rotatebox" | "usetheme" | "usecolortheme" | "usefonttheme" | "useoutertheme" | "useinnertheme"
        | "setbeameroption" | "theoremstyle" | "fancyhead" | "fancyfoot" | "fancyhf" | "lhead" | "chead" | "rhead"
        | "lfoot" | "cfoot" | "rfoot" | "setmainfont" | "setsansfont" | "setmonofont" | "setmathfont"
        | "setdefaultlanguage" | "setotherlanguage" | "bookmark" | "author" | "date" | "parbox" | "raisebox"
        | "special" | "typeout" | "typein" | "newsavebox" | "usebox" | "usecounter" | "fontfamily" | "fontseries"
        | "addvspace" | "eqlabel" | "alph" | "arabic" | "fnsymbol" | "roman" | "value" | "mint" => Rule::IgnoreArguments(1),
        "setcounter" | "addtocounter" | "setlength" | "addtolength" | "settowidth" | "settoheight" | "colorlet"
        | "markboth" | "pdfbookmark" | "addtocontents" | "setkeys" | "rule" | "resizebox" | "multicolumn" | "newfont"
        | "fontsize" | "sbox" | "savebox" | "settodepth" | "mintinline" | "multirow" | "crefrange" | "Crefrange"
        | "cpagerefrange" | "inputminted" | "fcolorbox" | "setbeamercolor" | "setbeamerfont" | "setbeamertemplate"
        | "numberwithin" | "newfontfamily" => Rule::IgnoreArguments(2),
        "definecolor" | "addcontentsline" => Rule::IgnoreArguments(3),
        // `\printbibliography[heading=…]` only has options: they are dropped as attached options too.
        "printbibliography" => Rule::IgnoreArguments(0),
        "newcommand" | "renewcommand" | "providecommand" | "DeclareRobustCommand" | "newrobustcmd" | "renewrobustcmd"
        | "providerobustcmd" | "DeclareMathOperator" | "NewEnviron" | "RenewEnviron" | "newtcolorbox" | "renewtcolorbox"
        | "newmdtheoremenv" => Rule::Definition(1),
        "newenvironment" | "renewenvironment" | "NewDocumentCommand" | "RenewDocumentCommand" | "ProvideDocumentCommand"
        | "DeclareDocumentCommand" | "NewExpandableDocumentCommand" | "RenewExpandableDocumentCommand"
        | "ProvideExpandableDocumentCommand" | "DeclareExpandableDocumentCommand" | "DeclarePairedDelimiter"
        | "NewTColorBox" | "DeclareTColorBox" => Rule::Definition(2),
        "NewDocumentEnvironment" | "RenewDocumentEnvironment" | "ProvideDocumentEnvironment"
        | "DeclareDocumentEnvironment" | "DeclarePairedDelimiterX" | "newtcbtheorem" => Rule::Definition(3),
        "def" | "gdef" | "edef" | "xdef" => Rule::PlainDefinition,
        "newtheorem" => Rule::NewTheorem,
        "declaretheorem" => Rule::DeclareTheorem,
        "ss" | "ae" | "AE" | "oe" | "OE" | "aa" | "AA" | "o" | "O" | "l" | "L" | "i" | "j" | "dh" | "DH" | "th" | "TH"
        | "ng" | "NG" | "dj" | "DJ"
        // Logos that print as one word (texcount counts `\LaTeX` and `\TeX` too).
        | "LaTeX" | "LaTeXe" | "TeX" | "BibTeX" | "XeTeX" | "LuaTeX" | "XeLaTeX" | "LuaLaTeX" => Rule::Letter,
        "c" | "v" | "u" | "H" | "k" | "r" | "b" | "d" | "t" => Rule::Accent,
        "texorpdfstring" => Rule::TexOrPdfString,
        "cmidrule" => Rule::Cmidrule,
        "caption" | "subcaption" => Rule::Caption(0),
        "captionof" => Rule::Caption(1),
        "subfloat" | "subfigure" | "subtable" => Rule::SubCaption,
        _ => {
            if is_cite_like(name) {
                Rule::IgnoreArguments(1)
            } else {
                Rule::Unknown
            }
        }
    }
}

/// Environments whose whole body is code, data or layout, not prose.
fn is_skipped_environment(name: &str) -> bool {
    matches!(
        name,
        "tikzpicture"
            | "pgfpicture"
            | "picture"
            | "pspicture"
            | "tikzcd"
            | "circuitikz"
            | "forest"
            | "axis"
            | "semilogxaxis"
            | "semilogyaxis"
            | "loglogaxis"
            | "pgfonlayer"
            | "scope"
            | "thebibliography"
            | "asy"
            | "chessboard"
            | "tikzpicture*"
            | "tabular"
            | "tabular*"
            | "tabularx"
            | "tabulary"
            | "tabu"
            | "longtabu"
            | "tblr"
            | "NiceTabular"
            | "NiceTabular*"
            | "supertabular"
            | "xtabular"
            | "mpsupertabular"
            | "tabbing"
            | "titlepage"
            | "lrbox"
    )
}

/// Floats: the body is not prose but the captions inside count.
fn is_float_environment(name: &str) -> bool {
    matches!(
        name,
        "figure"
            | "figure*"
            | "table"
            | "table*"
            | "float"
            | "wrapfigure"
            | "wraptable"
            | "sidewaysfigure"
            | "sidewaysfigure*"
            | "sidewaystable"
            | "sidewaystable*"
            | "subfigure"
            | "subtable"
            | "longtable"
            | "longtable*"
            | "SCfigure"
            | "SCtable"
            | "marginfigure"
            | "margintable"
    )
}

/// Mandatory arguments right after `\begin{name}` that are specifications, not text.
fn environment_arguments(name: &str) -> usize {
    match name {
        "longtable" | "array" | "minipage" | "multicols" | "multicols*" | "subfigure" | "subtable" | "adjustbox"
        | "column" | "spacing" | "otherlanguage" | "otherlanguage*" | "minted" => 1,
        "wrapfigure" | "wraptable" | "adjustwidth" | "list" => 2,
        _ => 0,
    }
}

const DEFAULT_TITLED_ENVIRONMENTS: [&str; 22] = [
    "theorem", "lemma", "proposition", "corollary", "definition", "remark", "example", "proof", "conjecture", "claim",
    "fact", "exercise", "problem", "solution", "note", "axiom", "observation", "question", "notation", "assumption",
    "hypothesis", "principle",
];

// ---------------------------------------------------------------------------------------------------
// Engine
// ---------------------------------------------------------------------------------------------------

struct FileData {
    source: MaskedSource,
    /// Headings and frames (no labels), in file order — the entries of the outline.
    entries: Vec<OutlineEntry>,
    /// Includes that the outline follows.
    includes: Vec<IncludeReference>,
    /// Resolved .tex file of each include (None when it does not resolve to a .tex file).
    targets: Vec<Option<String>>,
    has_document_class: bool,
    has_begin_document: bool,
}

struct Accumulator {
    entry: Option<OutlineEntry>,
    counts: StatisticsCounts,
    heading_end_line: usize,
}

struct Engine<'a> {
    root: String,
    read: ReadFn<'a>,
    file_exists: ExistsFn<'a>,
    is_fragment: bool,

    cache: HashMap<String, Rc<FileData>>,
    visited: HashSet<String>,
    order: Vec<FilePath>,

    // The file being walked
    file: String,
    cur: Rc<FileData>,
    entry_index: usize,
    include_index: usize,
    /// Included files that carry their own `\documentclass` contribute only what lies between their
    /// `\begin{document}` and `\end{document}`.
    is_envelope: bool,
    /// Text of the current file is not part of the document right now (an envelope's own preamble).
    file_suppressed: bool,
    depth: usize,

    // Counting state
    accumulators: Vec<Accumulator>,
    current: usize,
    /// Before `\begin{document}`: nothing counts.
    in_preamble: bool,
    /// After `\end{document}`: nothing counts.
    document_ended: bool,
    /// > 0 while walking code that is not typeset.
    suppressed: i32,
    /// > 0 inside a float: its contents are not prose. Only captions count there.
    excluded: i32,
    in_heading: bool,
    word: WordState,
    titled_environments: HashSet<String>,
}

fn empty_file_data() -> Rc<FileData> {
    Rc::new(FileData {
        source: MaskedSource::new(""),
        entries: Vec::new(),
        includes: Vec::new(),
        targets: Vec::new(),
        has_document_class: false,
        has_begin_document: false,
    })
}

/// True when the masked source holds `\begin{document}`.
pub fn contains_begin_document(u: &[u16]) -> bool {
    let n = u.len();
    let mut i = 0;
    while i + 6 < n {
        if u[i] != 92 {
            i += 1;
            continue;
        }
        // \begin
        if !(u[i + 1] == 98 && u[i + 2] == 101 && u[i + 3] == 103 && u[i + 4] == 105 && u[i + 5] == 110) {
            i += 1;
            continue;
        }
        let mut k = i + 6;
        if k < n && is_command_letter(u[k]) {
            i = k;
            continue;
        }
        while k < n && is_hspace(u[k]) {
            k += 1;
        }
        if let Some((name, _)) = environment_name(u, k) {
            if utf16_string(u, name) == "document" {
                return true;
            }
        }
        i = k;
    }
    false
}

impl<'a> Engine<'a> {
    fn new(root: String, read: ReadFn<'a>, file_exists: ExistsFn<'a>, is_fragment: bool) -> Self {
        Engine {
            file: root.clone(),
            root,
            read,
            file_exists,
            is_fragment,
            cache: HashMap::new(),
            visited: HashSet::new(),
            order: Vec::new(),
            cur: empty_file_data(),
            entry_index: 0,
            include_index: 0,
            is_envelope: false,
            file_suppressed: false,
            depth: 0,
            accumulators: Vec::new(),
            current: 0,
            in_preamble: false,
            document_ended: false,
            suppressed: 0,
            excluded: 0,
            in_heading: false,
            word: WordState::default(),
            titled_environments: DEFAULT_TITLED_ENVIRONMENTS.iter().map(|s| s.to_string()).collect(),
        }
    }

    fn is_counting(&self) -> bool {
        !self.in_preamble && !self.document_ended && self.suppressed == 0 && self.excluded == 0 && !self.file_suppressed
    }

    fn run(mut self) -> DocumentStatistics {
        self.accumulators = vec![Accumulator { entry: None, counts: StatisticsCounts::default(), heading_end_line: 1 }];
        self.current = 0;
        // A document counts from `\begin{document}` on. When no file reachable from the root has one (a
        // fragment, a document still being started), everything counts.
        self.in_preamble = if self.is_fragment { false } else { self.any_file_has_begin_document() };
        let root = self.root.clone();
        self.walk_file(&root);
        self.assemble()
    }

    /// Reads, masks and scans a file once.
    fn load(&mut self, url: &str) -> Rc<FileData> {
        let key = pi::path_key(url).into_owned();
        if let Some(c) = self.cache.get(&key) {
            return c.clone();
        }
        let text = (self.read)(url).unwrap_or_default();
        let masked = MaskedSource::new(&text);
        let scan = ls::scan(&masked, url, ls::ScanParts::INCLUDES.union(ls::ScanParts::OUTLINE));
        let mut references: Vec<IncludeReference> = Vec::new();
        let mut targets: Vec<Option<String>> = Vec::new();
        for reference in scan.includes {
            if !reference.kind.follows_include() {
                continue;
            }
            let target = pi::resolve(&reference, url, &self.root, self.file_exists);
            references.push(reference);
            targets.push(target.filter(|t| pi::extension_lower(t) == "tex"));
        }
        let data = Rc::new(FileData {
            has_document_class: scan.document_class.is_some(),
            has_begin_document: contains_begin_document(&masked.masked),
            entries: scan.outline.into_iter().filter(|e| e.kind != OutlineKind::Label).collect(),
            includes: references,
            targets,
            source: masked,
        });
        self.cache.insert(key, data.clone());
        data
    }

    fn any_file_has_begin_document(&mut self) -> bool {
        let mut seen: HashSet<String> = HashSet::new();
        seen.insert(pi::path_key(&self.root).into_owned());
        let mut stack: Vec<String> = vec![self.root.clone()];
        while let Some(url) = stack.pop() {
            let data = self.load(&url);
            if data.has_begin_document {
                return true;
            }
            for target in data.targets.iter().flatten() {
                if seen.insert(pi::path_key(target).into_owned()) {
                    stack.push(target.clone());
                }
            }
        }
        false
    }

    // ----- Walking a file -----

    fn walk_file(&mut self, url: &str) {
        let data = self.load(url);
        self.visited.insert(pi::path_key(url).into_owned());
        self.order.push(FilePath::from(url));

        let saved = (
            std::mem::replace(&mut self.file, url.to_string()),
            self.cur.clone(),
            self.entry_index,
            self.include_index,
            self.is_envelope,
            self.file_suppressed,
        );
        self.cur = data.clone();
        self.entry_index = 0;
        self.include_index = 0;
        self.is_envelope = self.depth > 0 && data.has_document_class && data.has_begin_document && !self.in_preamble;
        self.file_suppressed = self.is_envelope;
        self.depth += 1;

        self.walk(0, data.source.masked.len());
        self.sweep(usize::MAX);

        self.depth -= 1;
        (self.file, self.cur, self.entry_index, self.include_index, self.is_envelope, self.file_suppressed) = saved;
    }

    /// Opens the sections whose heading lies at or before `position` and walks (uncounted) the includes that
    /// lie before it and were skipped over.
    fn sweep(&mut self, position: usize) {
        loop {
            let cur = self.cur.clone();
            let has_entry = self.entry_index < cur.entries.len();
            let has_include = self.include_index < cur.includes.len();
            let next_entry = if has_entry { cur.entries[self.entry_index].offset } else { usize::MAX };
            let next_include = if has_include { cur.includes[self.include_index].range.start } else { usize::MAX };
            if has_entry && next_entry <= position && next_entry <= next_include {
                // A heading the walk reaches as a command opens its section. One it jumped over is not typeset:
                // it exists for the outline's sake but takes no text.
                self.open_section(cur.entries[self.entry_index].clone(), next_entry >= position);
                self.entry_index += 1;
            } else if has_include && next_include < position && next_include < next_entry {
                let target = cur.targets[self.include_index].clone();
                self.include_index += 1;
                let section = self.current;
                self.descend(target.as_deref(), false);
                self.current = section;
            } else {
                break;
            }
        }
    }

    fn descend(&mut self, target: Option<&str>, counted: bool) {
        let Some(target) = target else { return };
        if self.visited.contains(pi::path_key(target).as_ref()) {
            return;
        }
        if !counted {
            self.suppressed += 1;
        }
        self.walk_file(target);
        if !counted {
            self.suppressed -= 1;
        }
    }

    fn open_section(&mut self, entry: OutlineEntry, make_current: bool) {
        self.word.separator();
        let line = entry.line;
        self.accumulators.push(Accumulator { entry: Some(entry), counts: StatisticsCounts::default(), heading_end_line: line });
        if make_current {
            self.current = self.accumulators.len() - 1;
        }
    }

    // ----- Counting -----

    #[inline]
    fn count_word(&mut self) {
        if !self.is_counting() {
            return;
        }
        let acc = &mut self.accumulators[self.current].counts;
        if self.in_heading {
            acc.heading_words += 1;
        } else {
            acc.text_words += 1;
        }
    }

    fn count_math(&mut self, display: bool) {
        if !self.is_counting() {
            return;
        }
        let acc = &mut self.accumulators[self.current].counts;
        if display {
            acc.display_math += 1;
        } else {
            acc.inline_math += 1;
        }
    }

    /// Records where the heading that opened the current section ends.
    fn note_heading_end(&mut self, command_start: usize, end_offset: usize) {
        let Some(entry) = &self.accumulators[self.current].entry else { return };
        if entry.offset != command_start || pi::path_key(&entry.file) != pi::path_key(&self.file) {
            return;
        }
        let line = self.cur.source.line_at(command_start.max(end_offset.saturating_sub(1)));
        self.accumulators[self.current].heading_end_line = line;
    }

    fn begin_document(&mut self) {
        if self.file_suppressed {
            self.file_suppressed = false;
        } else {
            self.in_preamble = false;
        }
    }

    fn end_document(&mut self) {
        if self.is_envelope {
            self.file_suppressed = true;
        } else {
            self.document_ended = true;
        }
    }

    // ----- Result -----

    fn assemble(self) -> DocumentStatistics {
        let mut iter = self.accumulators.into_iter();
        let first = iter.next().expect("front matter accumulator");
        let front = SectionStatistics::front_matter(&self.root, first.counts);
        let mut sections: Vec<SectionStatistics> = Vec::new();
        let mut occurrences: HashMap<String, usize> = HashMap::new();
        for acc in iter {
            let Some(entry) = acc.entry else { continue };
            let key = format!("{}#{}#{}", entry.file, entry.kind.raw(), entry.title);
            let occ = occurrences.entry(key).or_insert(0);
            let occurrence = *occ;
            *occ += 1;
            sections.push(SectionStatistics::heading(&entry, acc.heading_end_line, acc.counts, occurrence));
        }

        // A section's subtree runs up to the next heading of the same or a shallower level.
        let mut prefix = vec![StatisticsCounts::ZERO; sections.len() + 1];
        for (position, section) in sections.iter().enumerate() {
            prefix[position + 1] = prefix[position] + section.own;
        }
        let mut next = vec![sections.len(); sections.len()];
        let mut stack: Vec<usize> = Vec::new();
        for position in (0..sections.len()).rev() {
            while let Some(&top) = stack.last() {
                if sections[top].level > sections[position].level {
                    stack.pop();
                } else {
                    break;
                }
            }
            next[position] = stack.last().copied().unwrap_or(sections.len());
            stack.push(position);
        }
        for position in 0..sections.len() {
            sections[position].subtree = prefix[next[position]] - prefix[position];
        }

        let mut index_by_id: HashMap<String, usize> = HashMap::new();
        for (position, section) in sections.iter().enumerate() {
            index_by_id.entry(section.id.clone()).or_insert(position);
        }
        DocumentStatistics {
            root_file: FilePath::from(self.root.as_str()),
            totals: front.own + prefix[sections.len()],
            front_matter: front,
            sections,
            files: self.order,
            index_by_id,
        }
    }

    // ===== Text walk =====

    /// Walks `[start, end)` of the current file (the masked text: comments and verbatim are blank).
    fn walk(&mut self, start: usize, end: usize) {
        let cur = self.cur.clone();
        let u: &[u16] = &cur.source.masked;
        let limit = end.min(u.len());
        let mut i = start;
        while i < limit {
            let c = u[i];
            if c < 128 {
                if is_ascii_letter(c) {
                    if self.word.letter() {
                        self.count_word();
                    }
                    i += 1;
                } else if is_ascii_digit(c) {
                    if self.word.digit() {
                        self.count_word();
                    }
                    i += 1;
                } else {
                    match c {
                        92 => {
                            let next = self.command(i);
                            i = if next > i { next } else { i + 1 };
                        }
                        36 => {
                            let next = self.dollar(i);
                            i = if next > i { next } else { i + 1 };
                        }
                        123 | 125 => i += 1, // braces never split a word: Poincar{\'e}, \textbf{foo}bar
                        45 => {
                            self.word.joiner(Joiner::Hyphen);
                            i += 1;
                        }
                        39 => {
                            self.word.joiner(Joiner::Apostrophe);
                            i += 1;
                        }
                        44 | 46 => {
                            self.word.joiner(Joiner::NumberSeparator);
                            i += 1;
                        }
                        34 => i = self.double_quote(i),
                        _ => {
                            self.word.separator();
                            i += 1;
                        }
                    }
                }
                continue;
            }

            let mut value = c as u32;
            let mut length = 1;
            if is_high_surrogate(c) && i + 1 < limit && is_low_surrogate(u[i + 1]) {
                value = 0x10000 + (((c as u32) - 0xD800) << 10) + ((u[i + 1] as u32) - 0xDC00);
                length = 2;
            }
            match char::from_u32(value) {
                Some(ch) => match classify(ch) {
                    CharacterClass::Letter => {
                        if self.word.letter() {
                            self.count_word();
                        }
                    }
                    CharacterClass::Digit => {
                        if self.word.digit() {
                            self.count_word();
                        }
                    }
                    CharacterClass::Mark => {}
                    CharacterClass::Ideograph => {
                        self.word.separator();
                        self.count_word();
                        self.word.separator();
                    }
                    CharacterClass::Hyphen => self.word.joiner(Joiner::Hyphen),
                    CharacterClass::Apostrophe => self.word.joiner(Joiner::Apostrophe),
                    CharacterClass::Other => self.word.separator(),
                },
                None => self.word.separator(),
            }
            i += length;
        }
    }

    /// Counts the words of a heading argument as heading words.
    fn walk_heading(&mut self, range: Range<usize>) {
        let was_heading = self.in_heading;
        self.word.separator();
        self.in_heading = true;
        self.walk(range.start, range.end);
        self.in_heading = was_heading;
        self.word.separator();
    }

    /// Babel shorthands keep a word together: `Sch"on` ("a "o "u "s), `Wort"=Trenner` ("- "= "|). Elsewhere a
    /// straight quote is punctuation.
    fn double_quote(&mut self, i: usize) -> usize {
        let cur = self.cur.clone();
        let u: &[u16] = &cur.source.masked;
        if self.word.is_inside_letters() && i + 1 < u.len() {
            let next = u[i + 1];
            if is_ascii_letter(next) {
                return i + 1;
            }
            if next == 45 || next == 61 || next == 124 {
                self.word.joiner(Joiner::Hyphen);
                return i + 2;
            }
        }
        self.word.separator();
        i + 1
    }

    // ----- Commands -----

    fn command(&mut self, i: usize) -> usize {
        let cur = self.cur.clone();
        let u: &[u16] = &cur.source.masked;
        let n = u.len();
        if i + 1 >= n {
            self.word.separator();
            return n;
        }
        let d = u[i + 1];
        if !is_command_letter(d) {
            return self.control_symbol(i, d);
        }
        let mut j = i + 2;
        while j < n && is_command_letter(u[j]) {
            j += 1;
        }
        self.sweep(i);
        let mut buf = [0u8; 48];
        let name = ascii_name(u, (i + 1)..j, &mut buf).into_owned();
        self.control_word(&name, i, j)
    }

    fn control_word(&mut self, name: &str, i: usize, j: usize) -> usize {
        let cur = self.cur.clone();
        let u: &[u16] = &cur.source.masked;
        let n = u.len();
        match rule_for(name) {
            Rule::Unknown => {
                self.word.separator();
                self.skip_attached_options(j)
            }
            Rule::Separator => {
                self.word.separator();
                self.skip_overlay(j)
            }
            Rule::Heading => {
                let Some(call) = args::parse(u, j, 1, true) else {
                    self.word.separator();
                    return j;
                };
                self.walk_heading(call.mandatory[0].clone());
                self.note_heading_end(i, call.end);
                call.end
            }
            Rule::FrameTitle => {
                self.word.separator();
                let mut p = self.skip_overlay(j);
                if p < n && u[p] == 91 {
                    if let Some((_, e)) = args::bracketed(u, p) {
                        p = e;
                    }
                }
                if p < n && u[p] == 123 {
                    if let Some((content, e)) = args::braced(u, p) {
                        self.walk_heading(content);
                        self.note_heading_end(i, e);
                        return e;
                    }
                }
                p
            }
            Rule::Begin => self.begin_environment(i, j),
            Rule::End => {
                self.word.separator();
                let mut k = j;
                while k < n && is_hspace(u[k]) {
                    k += 1;
                }
                let Some((name_range, end)) = environment_name(u, k) else { return j };
                if utf16_string(u, name_range) == "document" {
                    self.end_document();
                }
                end
            }
            Rule::Include => self.include_command(i, j, if name == "import" || name == "subimport" { 2 } else { 1 }),
            Rule::IgnoreArguments(count) => {
                self.word.separator();
                self.skip_arguments(j, count, true)
            }
            Rule::Definition(bodies) => {
                self.word.separator();
                self.skip_definition(j, bodies).unwrap_or(j)
            }
            Rule::PlainDefinition => {
                self.word.separator();
                self.skip_plain_definition(j).unwrap_or(j)
            }
            Rule::NewTheorem => {
                self.word.separator();
                self.handle_new_theorem(j)
            }
            Rule::DeclareTheorem => {
                self.word.separator();
                self.handle_declare_theorem(j)
            }
            Rule::Letter => {
                if self.word.letter() {
                    self.count_word();
                }
                self.skip_space_no_par(j)
            }
            Rule::Accent => {
                let p = self.skip_space_no_par(j);
                if let Some(end) = self.accent_base(p) {
                    if self.word.letter() {
                        self.count_word();
                    }
                    return end;
                }
                self.word.separator();
                j
            }
            Rule::TexOrPdfString => {
                self.word.separator();
                let Some(call) = args::parse(u, j, 2, true) else { return j };
                if call.mandatory.len() != 2 {
                    return j;
                }
                self.walk(call.mandatory[0].start, call.mandatory[0].end);
                self.word.separator();
                call.end
            }
            Rule::Caption(type_arguments) => {
                self.word.separator();
                self.caption_text(j, type_arguments)
            }
            Rule::SubCaption => {
                self.word.separator();
                self.sub_caption_text(j)
            }
            Rule::Cmidrule => {
                self.word.separator();
                let mut p = j;
                if p < n && u[p] == 91 {
                    if let Some((_, e)) = args::bracketed_limit(u, p, 40) {
                        p = e;
                    }
                }
                if p < n && u[p] == 40 {
                    let mut k = p + 1;
                    while k < n && k < p + 12 && u[k] != 41 {
                        k += 1;
                    }
                    if k < n && u[k] == 41 {
                        p = k + 1;
                    }
                }
                if p < n && u[p] == 123 {
                    if let Some((_, e)) = args::braced_limit(u, p, 40) {
                        p = e;
                    }
                }
                p
            }
        }
    }

    /// `\caption*[short title]{text}` and `\captionof{figure}[short title]{text}`: the text counts, also inside
    /// a figure or a table, whose other contents do not.
    fn caption_text(&mut self, j: usize, type_arguments: usize) -> usize {
        let cur = self.cur.clone();
        let u: &[u16] = &cur.source.masked;
        let n = u.len();
        let mut p = j;
        if p < n && u[p] == 42 {
            p += 1;
        }
        for _ in 0..type_arguments {
            let q = self.skip_space_no_par(p);
            if q < n && u[q] == 123 {
                if let Some((_, e)) = args::braced(u, q) {
                    p = e;
                    continue;
                }
            }
            return j;
        }
        let mut q = self.skip_space_no_par(p);
        if q < n && u[q] == 91 {
            if let Some((_, e)) = args::bracketed(u, q) {
                q = self.skip_space_no_par(e);
            }
        }
        if q < n && u[q] == 123 {
            if let Some((content, e)) = args::braced(u, q) {
                self.walk_caption(content);
                return e;
            }
        }
        p
    }

    /// `\subfloat[caption]{…}`, `\subfloat[list entry][caption]{…}`, `\subfigure[caption]{…}`: the caption is the
    /// last `[option]`; the braced content is read by the walk as it is (inside a float: not counted).
    fn sub_caption_text(&mut self, j: usize) -> usize {
        let cur = self.cur.clone();
        let u: &[u16] = &cur.source.masked;
        let n = u.len();
        let mut end = j;
        let mut caption: Option<Range<usize>> = None;
        let mut q = self.skip_space_no_par(j);
        let mut found = 0;
        while found < 2 && q < n && u[q] == 91 {
            let Some((content, e)) = args::bracketed(u, q) else { break };
            caption = Some(content);
            end = e;
            q = self.skip_space_no_par(e);
            found += 1;
        }
        if let Some(c) = caption {
            self.walk_caption(c);
        }
        end
    }

    /// Reads a caption's text as counted text, wherever it sits.
    fn walk_caption(&mut self, range: Range<usize>) {
        let saved = self.excluded;
        self.excluded = 0;
        self.word.separator();
        self.walk(range.start, range.end);
        self.word.separator();
        self.excluded = saved;
    }

    /// `\input{file}` & co.: walks the file here, in reading order.
    fn include_command(&mut self, i: usize, j: usize, arguments: usize) -> usize {
        self.word.separator();
        let cur = self.cur.clone();
        let u: &[u16] = &cur.source.masked;
        let mut end = self.skip_arguments(j, arguments, true);
        if self.include_index >= cur.includes.len() {
            return end;
        }
        let reference = &cur.includes[self.include_index];
        let location = reference.range.start;
        if location <= i {
            return end;
        }
        let mut belongs_here = location < end;
        if !belongs_here && end == j {
            // The TeX primitive form `\input file.tex`.
            let mut k = j;
            while k < u.len() && is_tex_whitespace(u[k]) {
                k += 1;
            }
            belongs_here = k == location;
        }
        if !belongs_here {
            return end;
        }
        let target = cur.targets[self.include_index].clone();
        self.include_index += 1;
        end = end.max(location + reference.range.len());
        self.descend(target.as_deref(), true);
        end
    }

    // ----- Control symbols -----

    fn control_symbol(&mut self, i: usize, d: u16) -> usize {
        let cur = self.cur.clone();
        let u: &[u16] = &cur.source.masked;
        let n = u.len();
        match d {
            92 => {
                // \\ and its star and [length]
                self.word.separator();
                let mut p = i + 2;
                if p < n && u[p] == 42 {
                    p += 1;
                }
                if p < n && u[p] == 91 {
                    if let Some((_, e)) = args::bracketed_limit(u, p, 100) {
                        p = e;
                    }
                }
                p
            }
            40 => self.delimited_math(i, 41, false), // \(
            91 => self.delimited_math(i, 93, true),  // \[
            45 | 47 | 64 => i + 2,                   // \-  \/  \@ are invisible: the word goes on
            39 | 96 | 94 | 34 | 126 | 61 | 46 => {
                // accents  \'  \`  \^  \"  \~  \=  \.
                if let Some(end) = self.accent_base(i + 2) {
                    if self.word.letter() {
                        self.count_word();
                    }
                    return end;
                }
                self.word.separator();
                i + 2
            }
            _ => {
                self.word.separator();
                if is_high_surrogate(d) && i + 2 < n && is_low_surrogate(u[i + 2]) {
                    return i + 3;
                }
                i + 2
            }
        }
    }

    /// The letter an accent sits on: `e`, `{e}`, `\i`, `{\i}`. Returns the offset after it.
    fn accent_base(&self, p: usize) -> Option<usize> {
        let u: &[u16] = &self.cur.source.masked;
        let n = u.len();
        if p >= n {
            return None;
        }
        let c = u[p];
        if is_ascii_letter(c) {
            return Some(p + 1);
        }
        if c >= 128 && !is_high_surrogate(c) && !is_low_surrogate(c) {
            if let Some(ch) = char::from_u32(c as u32) {
                if classify(ch) == CharacterClass::Letter {
                    return Some(p + 1);
                }
            }
        }
        if c == 92 {
            return dotless_base(u, p);
        }
        if c == 123 {
            let mut k = p + 1;
            while k < n && is_hspace(u[k]) {
                k += 1;
            }
            let after = if k < n && is_ascii_letter(u[k]) {
                Some(k + 1)
            } else if k < n && u[k] == 92 {
                dotless_base(u, k)
            } else {
                None
            };
            let mut m = after?;
            while m < n && is_hspace(u[m]) {
                m += 1;
            }
            if m < n && u[m] == 125 {
                return Some(m + 1);
            }
            return None;
        }
        None
    }

    // ----- Arguments -----

    /// Skips spaces and at most one line break (TeX's own rule: a blank line is a paragraph, not a gap).
    fn skip_space_no_par(&self, from: usize) -> usize {
        let u: &[u16] = &self.cur.source.masked;
        let n = u.len();
        let mut p = from;
        while p < n && is_hspace(u[p]) {
            p += 1;
        }
        if !(p < n && is_line_break(u[p])) || self.cur.source.is_paragraph_break(p) {
            return p;
        }
        let mut q = p + 1;
        if u[p] == 13 && q < n && u[q] == 10 {
            q += 1;
        }
        while q < n && is_hspace(u[q]) {
            q += 1;
        }
        q
    }

    /// `<1-3>` right after a command name (beamer overlay specification).
    fn overlay_end(&self, open: usize) -> Option<usize> {
        let u: &[u16] = &self.cur.source.masked;
        let n = u.len();
        if open >= n || u[open] != 60 {
            return None;
        }
        let mut k = open + 1;
        let stop = n.min(open + 40);
        while k < stop {
            let c = u[k];
            if c == 62 {
                return if k > open + 1 { Some(k + 1) } else { None };
            }
            // digits, letters, - + , | : @ . * and space
            let allowed = is_ascii_letter(c) || is_ascii_digit(c) || matches!(c, 45 | 43 | 44 | 124 | 58 | 64 | 46 | 42 | 32);
            if !allowed {
                return None;
            }
            k += 1;
        }
        None
    }

    fn skip_overlay(&self, j: usize) -> usize {
        let u: &[u16] = &self.cur.source.masked;
        if j < u.len() && u[j] == 60 {
            if let Some(end) = self.overlay_end(j) {
                return end;
            }
        }
        j
    }

    /// `[options]` and `<overlays>` glued to the command name (`\includegraphics[width=3cm]`, `\only<2>`).
    fn skip_attached_options(&self, j: usize) -> usize {
        let u: &[u16] = &self.cur.source.masked;
        let n = u.len();
        let mut p = j;
        while p < n {
            if u[p] == 60 {
                if let Some(end) = self.overlay_end(p) {
                    p = end;
                    continue;
                }
            }
            if u[p] == 91 {
                if let Some((content, e)) = args::bracketed_limit(u, p, 300) {
                    if !u[content].iter().any(|&c| is_line_break(c)) {
                        p = e;
                        continue;
                    }
                }
            }
            break;
        }
        p
    }

    /// Skips `[*]`, the options and `count` mandatory `{…}` arguments of a command whose arguments are not
    /// text. Returns the offset after what was skipped (just `j` when the arguments are missing).
    fn skip_arguments(&self, j: usize, count: usize, trailing_options: bool) -> usize {
        let u: &[u16] = &self.cur.source.masked;
        let n = u.len();
        let mut p = j;
        if p < n && u[p] == 42 {
            p += 1;
        }
        let mut done = j;
        let mut found = 0;
        while found < count {
            let mut q = self.skip_space_no_par(p);
            while q < n {
                if u[q] == 91 {
                    if let Some((_, e)) = args::bracketed(u, q) {
                        q = self.skip_space_no_par(e);
                        continue;
                    }
                }
                if u[q] == 60 {
                    if let Some(e) = self.overlay_end(q) {
                        q = self.skip_space_no_par(e);
                        continue;
                    }
                }
                break;
            }
            if q < n && u[q] == 123 {
                if let Some((_, e)) = args::braced(u, q) {
                    p = e;
                    done = e;
                    found += 1;
                    continue;
                }
            }
            break;
        }
        if found == count && trailing_options {
            done = self.skip_attached_options(done);
        }
        done
    }

    // ----- Definitions -----

    /// `\newcommand{\foo}[2][x]{body}` & co. Returns the offset after the last body, or `None` when malformed.
    fn skip_definition(&self, j: usize, bodies: usize) -> Option<usize> {
        let u: &[u16] = &self.cur.source.masked;
        let n = u.len();
        let mut p = args::skip_whitespace(u, j);
        if p < n && u[p] == 42 {
            p = args::skip_whitespace(u, p + 1);
        }
        if p < n && u[p] == 91 {
            if let Some((_, e)) = args::bracketed_limit(u, p, 200) {
                p = args::skip_whitespace(u, e); // \newmdtheoremenv[options]{name}
            }
        }
        if p < n && u[p] == 123 {
            let (_, e) = args::braced_limit(u, p, 300)?;
            p = e;
        } else if p < n && u[p] == 92 && args::control_sequence(u, p).is_some() {
            p = args::control_sequence(u, p)?.1;
        } else {
            return None;
        }
        p = args::skip_whitespace(u, p);
        let mut optionals = 0;
        while optionals < 3 && p < n && u[p] == 91 {
            match args::bracketed_limit(u, p, 400) {
                Some((_, e)) => {
                    p = args::skip_whitespace(u, e);
                    optionals += 1;
                }
                None => break,
            }
        }
        let mut end = p;
        let mut remaining = bodies;
        let mut last_end: Option<usize> = None;
        while remaining > 0 {
            if !(end < n && u[end] == 123) {
                break;
            }
            let Some((_, e)) = args::braced(u, end) else { break };
            last_end = Some(e);
            end = args::skip_whitespace(u, e);
            remaining -= 1;
        }
        last_end
    }

    /// `\def\foo#1#2{body}`.
    fn skip_plain_definition(&self, j: usize) -> Option<usize> {
        let u: &[u16] = &self.cur.source.masked;
        let n = u.len();
        let p = args::skip_whitespace(u, j);
        if p >= n || u[p] != 92 {
            return None;
        }
        let (_, cs_end) = args::control_sequence(u, p)?;
        let mut k = cs_end;
        let stop = n.min(k + 120);
        while k < stop && u[k] != 123 {
            if is_line_break(u[k]) && self.cur.source.is_paragraph_break(k) {
                return None;
            }
            k += 1;
        }
        if k >= stop {
            return None;
        }
        args::braced(u, k).map(|(_, e)| e)
    }

    /// `\newtheorem{name}[counter]{Title}[within]`: `name` becomes an environment with a titled `[option]`.
    fn handle_new_theorem(&mut self, j: usize) -> usize {
        let cur = self.cur.clone();
        let u: &[u16] = &cur.source.masked;
        let n = u.len();
        let mut p = args::skip_whitespace(u, j);
        if p < n && u[p] == 42 {
            p = args::skip_whitespace(u, p + 1);
        }
        if !(p < n && u[p] == 123) {
            return j;
        }
        let Some((content, name_end)) = args::braced_limit(u, p, 200) else { return j };
        let trimmed = args::trimmed(u, content);
        if !trimmed.is_empty() {
            self.titled_environments.insert(utf16_string(u, trimmed));
        }
        let mut end = name_end;
        p = args::skip_whitespace(u, end);
        if p < n && u[p] == 91 {
            if let Some((_, e)) = args::bracketed_limit(u, p, 200) {
                end = e;
                p = args::skip_whitespace(u, end);
            }
        }
        if p < n && u[p] == 123 {
            if let Some((_, e)) = args::braced_limit(u, p, 400) {
                end = e;
                p = args::skip_whitespace(u, end);
                if p < n && u[p] == 91 {
                    if let Some((_, e2)) = args::bracketed_limit(u, p, 200) {
                        end = e2;
                    }
                }
            }
        }
        end
    }

    /// `\declaretheorem[options]{name, name}` (thmtools).
    fn handle_declare_theorem(&mut self, j: usize) -> usize {
        let cur = self.cur.clone();
        let u: &[u16] = &cur.source.masked;
        let n = u.len();
        let mut p = args::skip_whitespace(u, j);
        if p < n && u[p] == 42 {
            p = args::skip_whitespace(u, p + 1);
        }
        if p < n && u[p] == 91 {
            if let Some((_, e)) = args::bracketed_limit(u, p, 600) {
                p = args::skip_whitespace(u, e);
            }
        }
        if !(p < n && u[p] == 123) {
            return j;
        }
        let Some((content, end)) = args::braced_limit(u, p, 300) else { return j };
        for item in args::split_list(u, content) {
            for part in utf16_string(u, item).split([' ', '\t', '\n']).filter(|s| !s.is_empty()) {
                self.titled_environments.insert(part.to_string());
            }
        }
        end
    }

    // ===== Environments and formulas =====

    /// `\begin{name}` at `i`; returns where the walk continues (the environment's body is read as text unless
    /// it is a formula, a picture or a bibliography).
    fn begin_environment(&mut self, i: usize, j: usize) -> usize {
        self.word.separator();
        let cur = self.cur.clone();
        let u: &[u16] = &cur.source.masked;
        let n = u.len();
        let mut k = j;
        while k < n && is_hspace(u[k]) {
            k += 1;
        }
        let Some((name_range, after_name)) = environment_name(u, k) else { return j };
        let name = utf16_string(u, name_range);

        if name == "document" {
            self.begin_document();
            return after_name;
        }

        if is_math_environment(&name) {
            // A blank line cannot occur inside a formula (TeX would stop with an error): an `\end` that only
            // follows after one means the environment is still being typed, and its body counts as text.
            if let Some(range) = self.find_environment_end(&name, after_name, false, true) {
                self.count_math(name != "math");
                return range.end;
            }
            return after_name;
        }

        if is_skipped_environment(&name) {
            if let Some(range) = self.find_environment_end(&name, after_name, true, false) {
                return range.end;
            }
            return after_name;
        }

        if name == "frame" {
            return self.begin_frame(i, after_name);
        }

        if is_float_environment(&name) {
            if let Some(range) = self.find_environment_end(&name, after_name, true, false) {
                // Only the captions inside count. A float that is never closed (still being typed) is read as text.
                let body_start = range.start.min(self.arguments_end(&name, after_name));
                self.excluded += 1;
                self.walk(body_start, range.start);
                self.excluded -= 1;
                self.word.separator();
                return range.end;
            }
        }

        self.arguments_end(&name, after_name)
    }

    /// Where the body of an ordinary environment starts: its `[options]` and specification arguments are not text.
    fn arguments_end(&self, name: &str, after_name: usize) -> usize {
        let mut p = self.skip_overlay(after_name);
        let titled = self.titled_environments.contains(name)
            || (name.ends_with('*') && self.titled_environments.contains(&name[..name.len() - 1]));
        if !titled {
            p = self.skip_attached_options(p);
        }
        let specification = environment_arguments(name);
        if specification > 0 {
            p = self.skip_arguments(p, specification, !matches!(name, "multicols" | "multicols*"));
        }
        p
    }

    /// `\begin{frame}<overlay>[options]{Title}{Subtitle}`: titles are heading words.
    fn begin_frame(&mut self, i: usize, after_name: usize) -> usize {
        let cur = self.cur.clone();
        let u: &[u16] = &cur.source.masked;
        let n = u.len();
        let mut p = self.skip_overlay(after_name);
        if p < n && u[p] == 91 {
            if let Some((_, e)) = args::bracketed(u, p) {
                p = e;
            }
        }
        let mut titles = 0;
        while titles < 2 {
            let q = self.skip_space_no_par(p);
            if !(q < n && u[q] == 123) {
                break;
            }
            let Some((content, e)) = args::braced(u, q) else { break };
            self.walk_heading(content);
            self.note_heading_end(i, e);
            p = e;
            titles += 1;
        }
        p
    }

    /// `$…$` and `$$…$$` at `i`.
    fn dollar(&mut self, i: usize) -> usize {
        self.word.separator();
        let cur = self.cur.clone();
        let u: &[u16] = &cur.source.masked;
        let display = i + 1 < u.len() && u[i + 1] == 36;
        let body_start = i + if display { 2 } else { 1 };
        if let Some(end) = self.find_dollar_close(body_start, display) {
            self.count_math(display);
            return end;
        }
        body_start // never closed (still being typed): not a formula, the text goes on
    }

    /// `\(…\)` (`close` = `)`) and `\[…\]` (`close` = `]`) at `i`.
    fn delimited_math(&mut self, i: usize, close: u16, display: bool) -> usize {
        self.word.separator();
        let cur = self.cur.clone();
        let u: &[u16] = &cur.source.masked;
        let n = u.len();
        let mut k = i + 2;
        while k < n {
            let c = u[k];
            if c == 92 {
                if k + 1 < n && u[k + 1] == close {
                    self.count_math(display);
                    return k + 2;
                }
                k += 2;
                continue;
            }
            if is_line_break(c) && cur.source.is_paragraph_break(k) {
                break;
            }
            k += 1;
        }
        i + 2
    }

    /// Offset after the `$` / `$$` that closes a formula opened before `from`. A `$` inside braces belongs to a
    /// nested text group (`$x \text{if $y$}$`); a blank line ends the search.
    fn find_dollar_close(&self, from: usize, display: bool) -> Option<usize> {
        let u: &[u16] = &self.cur.source.masked;
        let n = u.len();
        let mut depth = 0;
        let mut k = from;
        while k < n {
            let c = u[k];
            if c == 92 {
                k += 2;
                continue;
            }
            if c == 123 {
                depth += 1;
            } else if c == 125 {
                if depth > 0 {
                    depth -= 1;
                }
            } else if c == 36 && depth == 0 {
                if !display {
                    return Some(k + 1);
                }
                if k + 1 < n && u[k + 1] == 36 {
                    return Some(k + 2);
                }
            } else if is_line_break(c) && self.cur.source.is_paragraph_break(k) {
                return None;
            }
            k += 1;
        }
        None
    }

    /// The range of the `\end{name}` that closes an environment whose body starts at `from`.
    fn find_environment_end(&self, name: &str, from: usize, nested: bool, stop_at_paragraph: bool) -> Option<Range<usize>> {
        let u: &[u16] = &self.cur.source.masked;
        let n = u.len();
        let mut depth = 1;
        let mut k = from;
        while k < n {
            let c = u[k];
            if c == 92 {
                if k + 1 >= n {
                    return None;
                }
                if !is_command_letter(u[k + 1]) {
                    k += 2;
                    continue;
                }
                let mut e = k + 2;
                while e < n && is_command_letter(u[e]) {
                    e += 1;
                }
                let length = e - k - 1;
                let is_end = length == 3 && u[k + 1] == 101 && u[k + 2] == 110 && u[k + 3] == 100;
                let is_begin = length == 5 && u[k + 1] == 98 && u[k + 2] == 101 && u[k + 3] == 103 && u[k + 4] == 105 && u[k + 5] == 110;
                if is_end || (nested && is_begin) {
                    let mut q = e;
                    while q < n && is_hspace(u[q]) {
                        q += 1;
                    }
                    if let Some((nr, pe)) = environment_name(u, q) {
                        if utf16_string(u, nr) == name {
                            if is_end {
                                depth -= 1;
                                if depth == 0 {
                                    return Some(k..pe);
                                }
                            } else {
                                depth += 1;
                            }
                            k = pe;
                            continue;
                        }
                    }
                }
                k = e;
                continue;
            }
            if stop_at_paragraph && is_line_break(c) && self.cur.source.is_paragraph_break(k) {
                return None;
            }
            k += 1;
        }
        None
    }
}

/// `\i` and `\j` (the dotless letters) at `p`.
fn dotless_base(u: &[u16], p: usize) -> Option<usize> {
    let n = u.len();
    if !(p + 1 < n && u[p] == 92 && (u[p + 1] == 105 || u[p + 1] == 106)) {
        return None;
    }
    if p + 2 < n && is_command_letter(u[p + 2]) {
        return None;
    }
    Some(p + 2)
}
