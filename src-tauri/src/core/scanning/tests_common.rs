//! Shared helpers of the scanner tests (port of `TeXCoreChecks/Helpers.swift`).
#![allow(dead_code)]

use crate::core::scanner::syntax::units;
use std::ops::Range;

/// File path helper (all checks use fake absolute paths under /proj).
pub fn url(path: &str) -> String {
    format!("/proj/{path}")
}

/// The substring of `text` covered by the UTF-16 `range`.
pub fn substring(text: &str, range: Range<usize>) -> String {
    String::from_utf16_lossy(&units(text)[range])
}

/// A deterministic LCG so the large inputs are reproducible.
pub struct Lcg(pub u64);

impl Lcg {
    pub fn next(&mut self) -> usize {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        (self.0 >> 33) as usize
    }
}
