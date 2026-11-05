// SPDX-License-Identifier: Apache-2.0

//! Untrusted text from clients (window titles, app ids).

use crate::MAX_TEXT_BYTES;

/// Text from a client, validated once. Deliberately without `Debug`, `Display` or any
/// way back to the original bytes apart from [`ClientText::as_str`] inside this crate,
/// so it cannot be logged by accident (a test checks that the traits are absent).
#[derive(Clone, Default, PartialEq, Eq)]
pub struct ClientText {
    text: String,
    /// The input was longer than MAX_TEXT_BYTES.
    cut: bool,
}

/// Input beyond this is never looked at (UTF-8 repair of the kept part needs at most a
/// few bytes past the cap).
const LOOK_AT: usize = MAX_TEXT_BYTES + 4;

impl ClientText {
    /// Invalid UTF-8 becomes U+FFFD; control characters become spaces; bidi controls
    /// (embeddings, overrides, isolates) are removed, since sash-text does not reorder;
    /// the result is cut at a character boundary to at most MAX_TEXT_BYTES bytes.
    pub fn new(bytes: &[u8]) -> Self {
        let looked = &bytes[..bytes.len().min(LOOK_AT)];
        let lossy = String::from_utf8_lossy(looked);
        let mut text = String::with_capacity(lossy.len().min(MAX_TEXT_BYTES));
        let mut cut = bytes.len() > LOOK_AT;
        for ch in lossy.chars() {
            let ch = match ch {
                c if c.is_control() => ' ',
                '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}' => continue,
                c => c,
            };
            if text.len() + ch.len_utf8() > MAX_TEXT_BYTES {
                cut = true;
                break;
            }
            text.push(ch);
        }
        Self { text, cut }
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.text
    }

    /// Whether the input was cut to MAX_TEXT_BYTES.
    pub fn was_cut(&self) -> bool {
        self.cut
    }

    /// The validated length in bytes.
    pub fn len(&self) -> usize {
        self.text.len()
    }

    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }
}

impl From<&str> for ClientText {
    fn from(text: &str) -> Self {
        Self::new(text.as_bytes())
    }
}
