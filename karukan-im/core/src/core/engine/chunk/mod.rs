//! Live-conversion chunking of the composing buffer.
//!
//! Splitting bounds each model call and freezes settled text: once a
//! boundary is behind the caret that chunk's reading and lctx stop
//! changing, so it stays a cache hit and its display no longer flickers.
//! The user-facing rationale is `docs/chunking.md`.
//!
//! Every keystroke re-chunks the whole buffer from scratch. The conversion
//! cache turns unchanged chunks into lookups, so only chunks whose reading
//! or left context actually changed reach the model.
//!
//! Where the boundaries fall is [`split`], which knows nothing of the
//! engine. A user-dictionary word at the head of the reading becomes a
//! chunk of its own whose text is pinned to the dictionary surface instead
//! of going through the model ([`pin`]), so 「しほのさん」 converts as
//! 偲称乃 + さん rather than one model guess.
mod pin;
mod split;

use tracing::debug;

use split::{ChunkLimits, group_chunks};

pub(super) use split::is_japanese;

use super::*;

/// One planned chunk of the reading: the text the model converts, or a
/// user-dictionary word whose surface is pinned (no model call). The plan
/// is what both live conversion and the explicit conversion's grid walk,
/// so a pinned word is a wall for the beam span the same way a manual
/// break is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ChunkPlan {
    pub reading: String,
    /// The surface this chunk is pinned to, when a user-dictionary word
    /// covers it; `None` sends the reading to the model.
    pub pinned: Option<String>,
}

impl InputMethodEngine {
    /// Auto-suggest over the composing buffer via [`Self::convert_chunks`],
    /// storing the chunks for display. Returns the concatenated conversion,
    /// or `None` when it equals the raw reading (no useful suggestion).
    /// Input no longer than one chunk — the common case — is a single
    /// whole-buffer model call.
    pub(super) fn chunked_auto_suggest(&mut self) -> Option<String> {
        let full_reading = self.input_buf.reading();
        if full_reading.is_empty() {
            self.chunks.clear();
            return None;
        }
        let text: Vec<char> = full_reading.chars().collect();
        let base_ctx = self.truncate_context_for_api();

        let chunks = self.convert_chunks(&text, &base_ctx, true);
        let combined: String = chunks.iter().map(|c| c.converted.as_str()).collect();

        self.chunks = chunks;
        self.log_chunk_state("convert");

        (combined != full_reading).then_some(combined)
    }

    /// The single implementation of the chunk-grid conversion: split with
    /// [`group_chunks`], each chunk built with the converted text of the
    /// preceding chunks as its left context. Live conversion and the
    /// explicit conversion's grid replay share it, so the two can never
    /// disagree about where the boundaries are. With `pin`, a
    /// user-dictionary word at the head of the reading is pinned to its
    /// surface (see [`Self::plan_chunks`]); without it the grid is the
    /// model's alone.
    fn convert_chunks(&mut self, chars: &[char], base_ctx: &str, pin: bool) -> Vec<ComposingChunk> {
        let plan = self.plan_chunks(chars, pin);
        let mut chunks: Vec<ComposingChunk> = Vec::new();
        let mut combined = String::new();
        for ChunkPlan { reading, pinned } in plan {
            let new = match pinned {
                Some(converted) => ComposingChunk { reading, converted },
                None => self.convert_new_chunk(reading, base_ctx, &combined),
            };
            combined.push_str(&new.converted);
            chunks.push(new);
        }
        chunks
    }

    /// The chunk plan for `chars`: the split-rule groups, with the head of
    /// the reading (and the head of every manually broken-off chunk)
    /// carved into a pinned user-dictionary chunk when one applies. Only
    /// those heads are tried: a dictionary with hundreds of thousands of
    /// words matches somewhere inside almost any sentence, and chopping the
    /// model's input there would do more harm than the pin does good.
    pub(super) fn plan_chunks(&self, chars: &[char], pin: bool) -> Vec<ChunkPlan> {
        let mut plan = Vec::new();
        let mut offset = 0;
        for reading in self.split_chunks(chars) {
            let len = reading.chars().count();
            let at_head = offset == 0 || self.chunk_breaks.contains(&offset);
            let pinned = (pin && at_head)
                .then(|| self.pin_user_word(&reading))
                .flatten();
            match pinned {
                Some((word_len, surface)) => {
                    let (word, rest) = reading.split_at(word_len);
                    plan.push(ChunkPlan {
                        reading: word.to_string(),
                        pinned: Some(surface),
                    });
                    if !rest.is_empty() {
                        // A tail of bare suffixes stays as typed; anything
                        // longer goes to the model with the word as context.
                        let pinned = self.is_suffix_run(rest).then(|| rest.to_string());
                        plan.push(ChunkPlan {
                            reading: rest.to_string(),
                            pinned,
                        });
                    }
                }
                None => plan.push(ChunkPlan {
                    reading,
                    pinned: None,
                }),
            }
            offset += len;
        }
        plan
    }

    /// Start of the trailing run of Japanese chunks that fits `budget`
    /// chars, always at least the last chunk. Snapped to chunk boundaries,
    /// so a digit or symbol run is never swallowed into the span, and a
    /// manual break is a wall: the user froze everything left of it. With
    /// `pin`, a pinned dictionary chunk is a wall too — its text is settled,
    /// so the beam covers only what follows it (nothing, when the reading
    /// ends on the word).
    pub(super) fn trailing_chunks_start(&self, chars: &[char], budget: usize, pin: bool) -> usize {
        let chunks = self.plan_chunks(chars, pin);
        let mut start = chars.len();
        let mut taken = 0;
        for chunk in chunks.iter().rev() {
            // A chunk with no Japanese has nothing to convert and must never
            // reach the model, so it walls the span off — including when it
            // is the last chunk, which then leaves the span empty. So does
            // a pinned word: the dictionary already converted it.
            if chunk.pinned.is_some() || !chunk.reading.chars().any(is_japanese) {
                break;
            }
            let len = chunk.reading.chars().count();
            if taken > 0 && taken + len > budget {
                break;
            }
            taken += len;
            start -= len;
            if self.chunk_breaks.contains(&start) {
                break;
            }
        }
        start
    }

    /// Split `chars` with the engine's configured rules. The single place
    /// the settings and the manual breaks meet [`group_chunks`], so a new
    /// rule reaches every caller at once.
    fn split_chunks(&self, chars: &[char]) -> Vec<String> {
        let limits = ChunkLimits {
            chars: self.chunk_chars(),
            symbols: self.config.chunk_symbols,
            digits: self.config.chunk_digits,
            alphabets: self.config.chunk_alphabets,
        };
        group_chunks(chars, limits, &self.chunk_breaks)
    }

    /// Top-1 conversion of `chars` on the same chunk grid live conversion
    /// uses: chunks containing Japanese go through the model (cache hits
    /// while the user types), purely non-Japanese chunks pass through
    /// verbatim, and each chunk's lctx is `base_ctx` plus the converted text
    /// before it. `pin` as in [`Self::convert_chunks`].
    pub(super) fn convert_on_chunk_grid(
        &mut self,
        chars: &[char],
        base_ctx: &str,
        pin: bool,
    ) -> String {
        self.convert_chunks(chars, base_ctx, pin)
            .into_iter()
            .map(|c| c.converted)
            .collect()
    }

    /// Build one converted chunk: a reading containing any Japanese goes to
    /// the model with `base_ctx` + `combined` as its left context (absorbed
    /// marks included); a purely non-Japanese one passes through verbatim,
    /// so digit and symbol runs stay exact.
    fn convert_new_chunk(
        &mut self,
        reading: String,
        base_ctx: &str,
        combined: &str,
    ) -> ComposingChunk {
        let converted = if reading.chars().any(is_japanese) {
            let lctx = self.lctx_for(base_ctx, combined);
            self.convert_chunk(&reading, &lctx)
        } else {
            reading.clone()
        };
        ComposingChunk { reading, converted }
    }

    /// Configured maximum chunk length in chars, clamped to at least 1.
    pub(super) fn chunk_chars(&self) -> usize {
        self.config.chunk_chars.max(1)
    }

    /// The left context (lctx) a chunk is built with: `base` (editor
    /// surrounding text) + preceding converted text, truncated to the API
    /// budget. Defined once so conversion and the aux display can't drift.
    pub(super) fn lctx_for(&self, base: &str, preceding_converted: &str) -> String {
        self.truncate_context(&format!("{base}{preceding_converted}"))
    }

    /// Left context for the chunk at `index`, derived on demand from the
    /// chunks to its left.
    pub(super) fn chunk_lctx(&self, index: usize) -> String {
        let base = self.truncate_context_for_api();
        let preceding: String = self.chunks[..index.min(self.chunks.len())]
            .iter()
            .map(|c| c.converted.as_str())
            .collect();
        self.lctx_for(&base, &preceding)
    }

    /// Model conversion of one chunk's `reading` given `lctx`, falling back to
    /// the reading itself when the model yields nothing.
    fn convert_chunk(&mut self, reading: &str, lctx: &str) -> String {
        self.run_kana_kanji_conversion(reading, lctx, 1)
            .into_iter()
            .next()
            .unwrap_or_else(|| reading.to_string())
    }

    /// Insert a manual chunk boundary at the caret, then reconvert. A
    /// boundary at the end of the reading takes effect as more text is
    /// typed: the settled text stops reconverting and the next keystroke
    /// starts a fresh chunk.
    pub(super) fn insert_chunk_break(&mut self) -> EngineResult {
        let pos = self.input_buf.reading_cursor();
        if pos > 0 && !self.chunk_breaks.contains(&pos) {
            self.chunk_breaks.push(pos);
            self.chunk_breaks.sort_unstable();
        }
        self.refresh_input_state()
    }

    /// Run a buffer edit, shifting the manual chunk boundaries with the text
    /// around the caret. Boundaries at or left of the edit stay put (text
    /// typed right after a break belongs to the new chunk); boundaries to
    /// the right move by the reading-length delta. Boundaries that fall off
    /// the reading are dropped.
    pub(super) fn edit_with_chunk_breaks<R>(&mut self, edit: impl FnOnce(&mut Self) -> R) -> R {
        if self.chunk_breaks.is_empty() {
            return edit(self);
        }
        let old_len = self.input_buf.reading().chars().count();
        let old_pos = self.input_buf.reading_cursor();
        let out = edit(self);
        let new_len = self.input_buf.reading().chars().count();
        let pos = old_pos.min(self.input_buf.reading_cursor());
        for b in &mut self.chunk_breaks {
            if *b <= pos {
                continue;
            }
            *b = if new_len >= old_len {
                *b + (new_len - old_len)
            } else {
                // A deletion can pull a boundary past the edit point; it
                // stops there rather than moving left of it.
                b.saturating_sub(old_len - new_len).max(pos)
            };
        }
        self.chunk_breaks.retain(|&b| b > 0 && b <= new_len);
        self.chunk_breaks.dedup();
        out
    }

    /// Index of the chunk the cursor sits in, by walking the actual chunk
    /// lengths (chunks are variable-length, so `cursor / chunk_chars` is
    /// wrong). Right after a manual break at the end of the reading this is
    /// `chunks.len()` — one past the end — because the new chunk is still
    /// empty. 0 for an empty buffer or a cursor at the very start.
    pub(super) fn current_chunk_index(&self) -> usize {
        let lengths: Vec<usize> = self
            .chunks
            .iter()
            .map(|c| c.reading.chars().count())
            .collect();
        self.caret_chunk_index(&lengths)
    }

    /// The walk itself, over the chunk lengths in order, so the grid and a
    /// fresh split answer the question the same way.
    fn caret_chunk_index(&self, lengths: &[usize]) -> usize {
        let cursor = self.input_buf.reading_cursor();
        let at_break = self.chunk_breaks.contains(&cursor);
        let pos = if at_break {
            cursor
        } else {
            cursor.saturating_sub(1)
        };
        let mut end = 0;
        for (i, len) in lengths.iter().enumerate() {
            end += len;
            if pos < end {
                return i;
            }
        }
        if at_break {
            lengths.len()
        } else {
            lengths.len().saturating_sub(1)
        }
    }

    /// The caret's chunk read off a fresh split of the buffer, so it stands
    /// whatever state the grid is in: typing inside a filtered view
    /// suppresses the suggestion, which clears `self.chunks`, and the aux
    /// counter has to survive that. Empty while the caret is past the last
    /// chunk (a break armed at the end of the reading), which restarts the
    /// counter at 0 as the composing line does.
    pub(super) fn caret_chunk_reading(&self) -> String {
        let chars: Vec<char> = self.input_buf.reading().chars().collect();
        let plan = self.plan_chunks(&chars, true);
        let lengths: Vec<usize> = plan.iter().map(|c| c.reading.chars().count()).collect();
        plan.into_iter()
            .nth(self.caret_chunk_index(&lengths))
            .map(|c| c.reading)
            .unwrap_or_default()
    }

    /// Debug-log the current chunking; `at` labels the call site
    /// (`"convert"`, `"cursor"`, …).
    pub(super) fn log_chunk_state(&self, at: &str) {
        let current = self.current_chunk_index();
        let current_len = self
            .chunks
            .get(current)
            .map(|chunk| chunk.reading.chars().count())
            .unwrap_or(0);
        debug!(
            "chunks [{}]: {} chunk(s); cursor at pos {} in chunk {} ({} char(s))",
            at,
            self.chunks.len(),
            self.input_buf.reading_cursor(),
            current,
            current_len
        );
    }
}
