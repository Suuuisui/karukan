//! Pinning a hand-registered word at the head of a chunk.
//!
//! The model converts a reading as a whole, so a registered name followed
//! by anything — 「しほのさん」, 「はーもにーの」 — never met the dictionary:
//! only a reading equal to the whole input did. Here the head of the
//! reading is matched against the words the user registered by hand
//! (`pin_words.tsv`, not the bulk user dictionary) and, when the match
//! looks like a word boundary, carved off as a chunk pinned to that
//! surface. The rest is converted by the model with the surface as its
//! left context, as any later chunk is.
//!
//! What counts as a boundary is deliberately narrow, because a wrong pin
//! takes over the live text:
//! - the word must be the longest reading at the head across the system
//!   and user dictionaries, so 「すいません」 is never cut into Sui + ません;
//! - what follows must be nothing, a non-Japanese character, or one of the
//!   configured suffixes (particles and honorifics);
//! - the reading must not be an everyday word in its own right: when the
//!   system dictionary has a common entry for it (さき → 先) the model is
//!   left to pick, so 「さきに」 stays 先に and never becomes 沙希に;
//! - when the user's own history keeps this reading as bare kana
//!   (ほんま → ほんま) more than it picks the registered word, likewise.

use super::split::is_japanese;
use super::*;

/// Shortest reading that is ever pinned: a single kana matches too much.
const MIN_PIN_CHARS: usize = 2;

/// System-dictionary cost below which an entry counts as an everyday word
/// (-500·ln p scale: 今日 ≈ 4100, 私 ≈ 6300, 先 ≈ 5800, while rare names
/// sit near 7500–10000).
const COMMON_WORD_COST: f32 = 7000.0;

impl InputMethodEngine {
    /// The hand-registered word at the head of `reading` to pin, as
    /// `(byte length of the word, surface)`, or `None` when the head is
    /// not a word boundary the pin rules accept.
    pub(in crate::core::engine) fn pin_user_word(&self, reading: &str) -> Option<(usize, String)> {
        if self.dicts.pin.is_empty() {
            return None;
        }
        // Longest registered reading at the head.
        let (word, surface) = reading
            .char_indices()
            .map(|(i, c)| i + c.len_utf8())
            .rev()
            .find_map(|end| {
                let head = &reading[..end];
                self.dicts.pin.get(head).map(|s| (head, s.clone()))
            })?;
        if word.chars().count() < MIN_PIN_CHARS {
            return None;
        }
        // A longer dictionary word starting here means the head is part
        // of it, not a word.
        let longer = |dict: &Option<Dictionary>| {
            dict.as_ref().is_some_and(|d| {
                d.common_prefix_search(reading)
                    .iter()
                    .any(|hit| hit.reading.len() > word.len())
            })
        };
        if longer(&self.dicts.system) || longer(&self.dicts.user) {
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
        if !boundary || self.is_everyday_word(word) || self.kept_as_kana(word, &surface) {
            return None;
        }
        Some((word.len(), surface))
    }

    /// Whether the system dictionary has a common entry for `word` that is
    /// a real alternative — not just the reading spelled in katakana or
    /// latin letters, which every reading gets.
    fn is_everyday_word(&self, word: &str) -> bool {
        let Some(hit) = self
            .dicts
            .system
            .as_ref()
            .and_then(|d| d.exact_match_search(word))
        else {
            return false;
        };
        hit.candidates.iter().any(|c| {
            c.score < COMMON_WORD_COST
                && !karukan_engine::is_pure_full_katakana(&c.surface)
                && !c.surface.is_ascii()
        })
    }

    /// Whether the learning history keeps this reading as bare kana more
    /// often than it picks the registered surface: the user habitually
    /// leaves this one as kana (ほんま 30 times vs 本間 7). Counted by
    /// frequency, not the recency-weighted score — a few recent commits of
    /// the pinned surface must not outvote the habit.
    fn kept_as_kana(&self, word: &str, surface: &str) -> bool {
        let Some(cache) = &self.learning else {
            return false;
        };
        cache.frequency(word, word) > cache.frequency(word, surface)
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
