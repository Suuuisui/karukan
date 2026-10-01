//! A user-dictionary word at the head of the reading is pinned to its
//! surface and only what follows it goes to the model (`chunk::pin`).

use karukan_engine::{LearningCache, LearningConfig};

use super::*;

/// Engine with `pins` registered by hand (and in the user dictionary, as
/// apply.sh puts them in both).
fn engine_with_pins(pins: &[(&str, &str)]) -> InputMethodEngine {
    let mut engine = InputMethodEngine::new();
    engine.converters.kanji = None;
    let json: Vec<String> = pins
        .iter()
        .map(|(r, s)| {
            format!(r#"{{"reading":"{r}","candidates":[{{"surface":"{s}","score":1.0}}]}}"#)
        })
        .collect();
    engine.dicts.user = Some(dict_from_json(&format!("[{}]", json.join(","))));
    engine.dicts.pin = pins
        .iter()
        .map(|(r, s)| (r.to_string(), s.to_string()))
        .collect();
    engine
}

const SHIHONO: &[(&str, &str)] = &[("しほの", "偲称乃")];

fn type_string(engine: &mut InputMethodEngine, s: &str) {
    for ch in s.chars() {
        engine.process_key(&press(ch));
    }
}

fn chunk_pairs(engine: &InputMethodEngine) -> Vec<(String, String)> {
    engine
        .chunks
        .iter()
        .map(|c| (c.reading.clone(), c.converted.clone()))
        .collect()
}

fn conversion_texts(engine: &InputMethodEngine) -> Vec<String> {
    engine
        .state()
        .candidates()
        .map(|cl| cl.candidates().iter().map(|c| c.text.clone()).collect())
        .unwrap_or_default()
}

#[test]
fn word_followed_by_honorific_is_pinned() {
    let mut engine = engine_with_pins(SHIHONO);
    type_string(&mut engine, "shihonosann");
    assert_eq!(engine.input_buf.reading(), "しほのさん");
    // No model: the tail falls back to its reading.
    assert_eq!(
        chunk_pairs(&engine),
        vec![
            ("しほの".to_string(), "偲称乃".to_string()),
            ("さん".to_string(), "さん".to_string())
        ]
    );
}

#[test]
fn exact_word_is_pinned_in_live_conversion() {
    let mut engine = engine_with_pins(SHIHONO);
    type_string(&mut engine, "shihono");
    assert_eq!(
        chunk_pairs(&engine),
        vec![("しほの".to_string(), "偲称乃".to_string())]
    );
}

#[test]
fn word_followed_by_a_mark_is_pinned() {
    let mut engine = engine_with_pins(SHIHONO);
    type_string(&mut engine, "shihono,");
    assert_eq!(engine.input_buf.reading(), "しほの、");
    assert_eq!(
        chunk_pairs(&engine),
        vec![
            ("しほの".to_string(), "偲称乃".to_string()),
            ("、".to_string(), "、".to_string())
        ]
    );
}

#[test]
fn word_followed_by_something_else_is_left_to_the_model() {
    // 「いた」 is registered but 「いたい」 is not a name + suffix: the
    // model converts the whole reading.
    let mut engine = engine_with_pins(&[("いた", "伊田")]);
    type_string(&mut engine, "itai");
    assert_eq!(
        chunk_pairs(&engine),
        vec![("いたい".to_string(), "いたい".to_string())]
    );
}

#[test]
fn longer_system_word_blocks_the_pin() {
    // 「すい」→ Sui is registered, but 「すいません」 is a system word, so
    // typing it must not become Sui + ません even though 「ませ」… no,
    // even though a suffix rule could match: the longest word wins.
    let mut engine = engine_with_pins(&[("すい", "Sui")]);
    engine.dicts.system = Some(dict_from_json(
        r#"[{"reading":"すいません","candidates":[{"surface":"すいません","score":1.0}]}]"#,
    ));
    engine.config.dict_suffixes.push("ませ".to_string());
    type_string(&mut engine, "suimasenn");
    assert_eq!(
        chunk_pairs(&engine),
        vec![("すいません".to_string(), "すいません".to_string())]
    );
}

#[test]
fn word_in_the_middle_is_not_pinned() {
    let mut engine = engine_with_pins(SHIHONO);
    type_string(&mut engine, "kyouhashihonosann");
    assert_eq!(
        chunk_pairs(&engine),
        vec![(
            "きょうはしほのさん".to_string(),
            "きょうはしほのさん".to_string()
        )]
    );
}

#[test]
fn learned_kanji_does_not_change_the_pinned_surface() {
    // A wrong kanji committed before the word was registered is history,
    // not a choice about the registered word.
    let mut engine = engine_with_pins(SHIHONO);
    let mut cache = LearningCache::new(LearningConfig::default());
    cache.record("しほの", "志穂の");
    cache.record("しほの", "志穂の");
    engine.learning = Some(cache);
    type_string(&mut engine, "shihonosann");
    assert_eq!(chunk_pairs(&engine)[0].1, "偲称乃");
}

#[test]
fn a_tail_of_bare_suffixes_stays_kana_without_the_model() {
    let mut engine = engine_with_pins(SHIHONO);
    // Were the tail sent to the model, this is what it would answer.
    seed_model_cache(&mut engine, "サンハ", "偲称乃", &["讃は"]);
    type_string(&mut engine, "shihonosannha");
    assert_eq!(
        chunk_pairs(&engine),
        vec![
            ("しほの".to_string(), "偲称乃".to_string()),
            ("さんは".to_string(), "さんは".to_string())
        ]
    );
    let chars: Vec<char> = "しほのさんは".chars().collect();
    assert_eq!(engine.beam_span_start(&chars), 6, "nothing left to beam");
}

#[test]
fn a_longer_tail_goes_to_the_model_with_the_word_as_context() {
    let mut engine = engine_with_pins(SHIHONO);
    seed_model_cache(&mut engine, "サントアッタ", "偲称乃", &["さんと会った"]);
    type_string(&mut engine, "shihonosanntoatta");
    assert_eq!(
        chunk_pairs(&engine),
        vec![
            ("しほの".to_string(), "偲称乃".to_string()),
            ("さんとあった".to_string(), "さんと会った".to_string())
        ]
    );
}

#[test]
fn pinned_word_walls_the_beam_span() {
    let engine = engine_with_pins(SHIHONO);
    let chars: Vec<char> = "しほのさんとあった".chars().collect();
    assert_eq!(engine.beam_span_start(&chars), 3);
    let chars: Vec<char> = "しほの".chars().collect();
    assert_eq!(engine.beam_span_start(&chars), 3, "nothing left to beam");
}

#[test]
fn space_lists_the_pinned_conversion_first_and_the_models_after() {
    let mut engine = engine_with_pins(SHIHONO);
    // The model's own take on the whole reading follows the pin.
    seed_model_cache(&mut engine, "シホノサン", "", &["志保のさん", "志穂のさん"]);
    type_string(&mut engine, "shihonosann");
    engine.process_key(&press_key(Keysym::SPACE));
    let texts = conversion_texts(&engine);
    assert_eq!(&texts[..3], ["偲称乃さん", "志保のさん", "志穂のさん"]);
}

#[test]
fn space_on_the_exact_word_keeps_the_models_alternatives() {
    let mut engine = engine_with_pins(SHIHONO);
    seed_model_cache(&mut engine, "シホノ", "", &["志保の", "詩穂の"]);
    type_string(&mut engine, "shihono");
    engine.process_key(&press_key(Keysym::SPACE));
    let texts = conversion_texts(&engine);
    assert_eq!(&texts[..3], ["偲称乃", "志保の", "詩穂の"]);
}

#[test]
fn bulk_user_dictionary_words_are_never_pinned() {
    // Phrase-shaped noise in a bulk dictionary must not take over live
    // text: only the hand-registered list pins.
    let mut engine = engine_with_pins(SHIHONO);
    engine.dicts.user = Some(dict_from_json(
        r#"[{"reading":"どうする","candidates":[{"surface":"どうする？","score":1.0}]}]"#,
    ));
    type_string(&mut engine, "dousuruno");
    assert_eq!(
        chunk_pairs(&engine),
        vec![("どうするの".to_string(), "どうするの".to_string())]
    );
}

#[test]
fn everyday_word_reading_is_left_to_the_model() {
    // 沙希 is registered, but さき is also 先: 「さきに」 must stay the
    // model's to convert.
    let mut engine = engine_with_pins(&[("さき", "沙希")]);
    engine.dicts.system = Some(dict_from_json(
        r#"[{"reading":"さき","candidates":[{"surface":"サキ","score":5776.0},{"surface":"先","score":5836.0}]}]"#,
    ));
    type_string(&mut engine, "sakini");
    assert_eq!(
        chunk_pairs(&engine),
        vec![("さきに".to_string(), "さきに".to_string())]
    );
}

#[test]
fn rare_system_entries_do_not_block_the_pin() {
    // The system dictionary knows しほの only as rare names, and katakana
    // spellings never count: the registered word still pins.
    let mut engine = engine_with_pins(SHIHONO);
    engine.dicts.system = Some(dict_from_json(
        r#"[{"reading":"しほの","candidates":[{"surface":"シホノ","score":3000.0},{"surface":"志歩乃","score":10000.0}]}]"#,
    ));
    type_string(&mut engine, "shihonosann");
    assert_eq!(chunk_pairs(&engine)[0].1, "偲称乃");
}

#[test]
fn reading_the_user_keeps_as_kana_is_not_pinned() {
    // ほんま is registered as 本間, but the user mostly leaves it as kana.
    let mut engine = engine_with_pins(&[("ほんま", "本間")]);
    let mut cache = LearningCache::new(LearningConfig::default());
    for _ in 0..5 {
        cache.record("ほんま", "ほんま");
    }
    cache.record("ほんま", "本間");
    engine.learning = Some(cache);
    type_string(&mut engine, "honnmani");
    assert_eq!(
        chunk_pairs(&engine),
        vec![("ほんまに".to_string(), "ほんまに".to_string())]
    );
}

#[test]
fn recent_commits_of_the_pin_do_not_outvote_the_kana_habit() {
    // The pinned surface committed lately (it was what live conversion
    // showed) still loses to a long kana habit.
    let mut engine = engine_with_pins(&[("ほんま", "本間")]);
    let mut cache = LearningCache::new(LearningConfig::default());
    for _ in 0..30 {
        cache.record("ほんま", "ほんま");
    }
    for _ in 0..7 {
        cache.record("ほんま", "本間");
    }
    engine.learning = Some(cache);
    type_string(&mut engine, "honnmani");
    assert_eq!(chunk_pairs(&engine)[0].0, "ほんまに");
}

#[test]
fn pin_words_file_parses_first_surface_per_reading() {
    let map = crate::core::engine::init::parse_pin_words(
        "# comment\nしほの\t偲称乃\t人名\nしほの\t志保乃\n\t空\nわるい行\n",
    );
    assert_eq!(map.len(), 1);
    assert_eq!(map["しほの"], "偲称乃");
}
