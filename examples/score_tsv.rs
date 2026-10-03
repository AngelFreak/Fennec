//! Scores `reference<TAB>before<TAB>after` lines: punctuation of before and
//! after against the reference, and how many words `after` changed.
use fennec::eval::{Punctuation, punctuation, word_errors};
fn main() {
    let text = std::fs::read_to_string(std::env::args().nth(1).unwrap()).unwrap();
    let (mut before, mut after) = (Punctuation::default(), Punctuation::default());
    let (mut changed, mut words, mut wer_b, mut wer_a) = (0, 0, (0, 0), (0, 0));
    for line in text.lines() {
        let f: Vec<&str> = line.split('\t').collect();
        before.add(punctuation(f[0], f[1]));
        after.add(punctuation(f[0], f[2]));
        let c = word_errors(f[1], f[2]);
        changed += c.errors;
        words += c.reference_len;
        let (b, a) = (word_errors(f[0], f[1]), word_errors(f[0], f[2]));
        wer_b = (wer_b.0 + b.errors, wer_b.1 + b.reference_len);
        wer_a = (wer_a.0 + a.errors, wer_a.1 + a.reference_len);
    }
    println!(
        "comma F1 {:.2} -> {:.2}; sentence-end F1 {:.2} -> {:.2}",
        before.commas.f1(),
        after.commas.f1(),
        before.ends.f1(),
        after.ends.f1()
    );
    println!(
        "WER vs reference {:.1}% -> {:.1}%; words changed {changed} of {words}",
        100.0 * wer_b.0 as f64 / wer_b.1 as f64,
        100.0 * wer_a.0 as f64 / wer_a.1 as f64
    );
}
