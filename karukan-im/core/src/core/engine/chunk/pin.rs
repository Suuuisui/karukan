//! Pinning a user-dictionary word at the head of a chunk.
//!
//! The model converts a reading as a whole, so a registered name followed
//! by anything — 「しほのさん」, 「はーもにーの」 — never met the dictionary:
//! only a reading equal to the whole input did. Here the head of the
//! reading is matched against the user dictionary and, when the match
//! looks like a word boundary, carved off as a chunk pinned to the
//! dictionary surface. The rest is converted by the model with that
//! surface as its left context, as any later chunk is.
//!
//! What counts as a boundary is deliberately narrow. The word must be the
//! longest dictionary reading at the head — the system dictionary
//! included, so 「すいません」 is never cut into Sui + ません — and what
//! follows it must be nothing, a non-Japanese character, or one of the
//! configured suffixes (particles and honorifics). Anything else is left
//! to the model: a dictionary of hundreds of thousands of words matches
//! the head of most sentences somewhere (「いた」 in 「いたい」), and
//! cutting there would be worse than the pin is good.
//!
//! The pinned surface is the dictionary's own, never the learning
//! history's: the word was registered on purpose, and the history for a
//! reading the dictionary did not yet cover is what the user committed
//! while fighting the model (the bare kana, its katakana, a wrong kanji).
//! Space still leads with the history, as it always did.

use super::split::is_japanese;
use super::*;

/// Shortest reading that is ever pinned: a single kana matches too much.
const MIN_PIN_CHARS: usize = 2;

impl InputMethodEngine {
    /// The user-dictionary word at the head of `reading` to pin, as
    /// `(byte length of the word, surface)`, or `None` when the head is
    /// not a word boundary the pin rules accept.
    pub(in crate::core::engine) fn pin_user_word(&self, reading: &str) -> Option<(usize, String)> {
        let user = self.dicts.user.as_ref()?;
        let best = user
            .common_prefix_search(reading)
            .into_iter()
            .max_by_key(|hit| hit.reading.len())?;
        let word = best.reading;
        if word.chars().count() < MIN_PIN_CHARS {
            return None;
        }
        // Longest match across both dictionaries: a longer system word
        // starting here means the head is part of it, not a word.
        if let Some(system) = &self.dicts.system
            && system
                .common_prefix_search(reading)
                .iter()
                .any(|hit| hit.reading.len() > word.len())
        {
            return None;
        }
        let rest = &reading[word.len()..];
        let boundary = match rest.chars().next() {
            None => true,
            Some(c) if !is_japanese(c) => true,
            Some(_) => self
                .config
                .dict_suffixes
                .iter()
                .any(|suffix| !suffix.is_empty() && rest.starts_with(suffix.as_str())),
        };
        if !boundary {
            return None;
        }
        let surface = best.candidates.first()?.surface.clone();
        Some((word.len(), surface))
    }

    /// Whether `rest` is nothing but configured suffixes back to back
    /// (「さん」, 「さんは」, 「です」). Such a tail is kana as typed, and
    /// asking the model for it alone, with only the pinned name as context,
    /// is how 「さん」 comes back as 讃.
    pub(in crate::core::engine) fn is_suffix_run(&self, rest: &str) -> bool {
        let mut pos = 0;
        while pos < rest.len() {
            let Some(len) = self
                .config
                .dict_suffixes
                .iter()
                .filter(|s| !s.is_empty() && rest[pos..].starts_with(s.as_str()))
                .map(String::len)
                .max()
            else {
                return false;
            };
            pos += len;
        }
        !rest.is_empty()
    }
}
