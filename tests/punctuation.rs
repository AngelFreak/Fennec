//! The punctuation model on real dictation. Needs the model (Settings →
//! Dictation → Add punctuation downloads it):
//!   cargo test --release --test punctuation -- --ignored

use fennec::punctuation::{DIR, Punctuator};

fn punctuator() -> Punctuator {
    Punctuator::load(&fennec::config::Paths::user().models().join(DIR)).unwrap()
}

#[test]
#[ignore = "needs the downloaded punctuation model"]
fn a_real_dictated_paragraph_gets_its_sentences_back() {
    // Edda's text for a real dictation, its utterances joined.
    let heard = "Okay så hvordan kører det her nu hvor at ville jeg lige prøve at lave nogle rettelser i \
                 systemet er det blevet bedre eller værre umiddelbart ser Recordinggrafen i hvert fald meget pænere ud";
    let heard = heard.split_whitespace().collect::<Vec<_>>().join(" ");
    assert_eq!(
        punctuator().punctuate(&heard).unwrap(),
        "Okay, så hvordan kører det her nu hvor at ville jeg lige prøve at lave nogle rettelser i systemet. \
         Er det blevet bedre eller værre? Umiddelbart ser Recordinggrafen i hvert fald meget pænere ud."
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
    );
}

#[test]
#[ignore = "needs the downloaded punctuation model"]
fn words_and_existing_marks_never_change() {
    let p = punctuator();
    let heard = "det ser sådan ud og Det er meget meget positivt. Denne graf hakker stadig, så det skal der gøres noget ved";
    let out = p.punctuate(heard).unwrap();
    let words = |t: &str| fennec::punctuation::model_words(t);
    assert_eq!(words(&out), words(heard));
    assert!(out.contains("positivt.") && out.contains("stadig,"), "{out}");
    assert!(out.contains("og det er"), "{out}");
}

#[test]
#[ignore = "needs the downloaded punctuation model"]
fn a_paragraph_takes_a_fraction_of_a_second() {
    let p = punctuator();
    let text =
        "vi prøver lige at snakke en lille smule mere her for at finde ud af om den nye kommando virker "
            .repeat(4);
    p.punctuate(&text).unwrap();
    let t = std::time::Instant::now();
    p.punctuate(&text).unwrap();
    assert!(t.elapsed().as_millis() < 1_500, "{:?}", t.elapsed());
}
