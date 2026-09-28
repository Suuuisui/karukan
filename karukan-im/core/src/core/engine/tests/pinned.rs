//! A user-dictionary word at the head of the reading is pinned to its
//! surface and only what follows it goes to the model (`chunk::pin`).

use karukan_engine::{LearningCache, LearningConfig};

use super::*;

fn engine_with_user_dict(json: &str) -> InputMethodEngine {
    let mut engine = InputMethodEngine::new();
    engine.converters.kanji = None;
    engine.dicts.user = Some(dict_from_json(json));
    engine
}

const SHIHONO: &str = r#"[{"reading":"しほの","candidates":[{"surface":"偲称乃","score":1.0}]}]"#;

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
    let mut engine = engine_with_user_dict(SHIHONO);
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
    let mut engine = engine_with_user_dict(SHIHONO);
    type_string(&mut engine, "shihono");
    assert_eq!(
        chunk_pairs(&engine),
        vec![("しほの".to_string(), "偲称乃".to_string())]
    );
}

#[test]
fn word_followed_by_a_mark_is_pinned() {
    let mut engine = engine_with_user_dict(SHIHONO);
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
    let mut engine = engine_with_user_dict(
        r#"[{"reading":"いた","candidates":[{"surface":"伊田","score":1.0}]}]"#,
    );
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
    let mut engine = engine_with_user_dict(
        r#"[{"reading":"すい","candidates":[{"surface":"Sui","score":1.0}]}]"#,
    );
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
    let mut engine = engine_with_user_dict(SHIHONO);
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
fn learning_history_does_not_change_the_pinned_surface() {
    // What was committed before the word was registered (the bare kana,
    // a wrong kanji) is history, not a choice about the dictionary word.
    for learned in ["しほの", "志穂の"] {
        let mut engine = engine_with_user_dict(SHIHONO);
        let mut cache = LearningCache::new(LearningConfig::default());
        cache.record("しほの", learned);
        engine.learning = Some(cache);
        type_string(&mut engine, "shihonosann");
        assert_eq!(chunk_pairs(&engine)[0].1, "偲称乃", "learned {learned}");
    }
}

#[test]
fn a_tail_of_bare_suffixes_stays_kana_without_the_model() {
    let mut engine = engine_with_user_dict(SHIHONO);
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
    let mut engine = engine_with_user_dict(SHIHONO);
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
    let engine = engine_with_user_dict(SHIHONO);
    let chars: Vec<char> = "しほのさんとあった".chars().collect();
    assert_eq!(engine.beam_span_start(&chars), 3);
    let chars: Vec<char> = "しほの".chars().collect();
    assert_eq!(engine.beam_span_start(&chars), 3, "nothing left to beam");
}

#[test]
fn space_lists_the_pinned_conversion_first_and_the_models_after() {
    let mut engine = engine_with_user_dict(SHIHONO);
    // The model's own take on the whole reading follows the pin.
    seed_model_cache(&mut engine, "シホノサン", "", &["志保のさん", "志穂のさん"]);
    type_string(&mut engine, "shihonosann");
    engine.process_key(&press_key(Keysym::SPACE));
    let texts = conversion_texts(&engine);
    assert_eq!(&texts[..3], ["偲称乃さん", "志保のさん", "志穂のさん"]);
}

#[test]
fn space_on_the_exact_word_keeps_the_models_alternatives() {
    let mut engine = engine_with_user_dict(SHIHONO);
    seed_model_cache(&mut engine, "シホノ", "", &["志保の", "詩穂の"]);
    type_string(&mut engine, "shihono");
    engine.process_key(&press_key(Keysym::SPACE));
    let texts = conversion_texts(&engine);
    assert_eq!(&texts[..3], ["偲称乃", "志保の", "詩穂の"]);
}
