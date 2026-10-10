//! Turning a transcript into a word count.
//!
//! This is the part everyone will argue with, so the rules are explicit and
//! each one is independently switchable. Three of them have a defensible
//! default and no defensible alternative, which is why they are defaults at
//! all:
//!
//! - **Contractions expand.** "don't" is spoken as two words. A streamer who
//!   says "I don't know" has said four words, not three.
//! - **Numerals spell out.** The recogniser emits `42`; the streamer said
//!   "forty-two", which is two words. Counting the digit as one word makes
//!   number-heavy speech score far lower than it sounds.
//! - **Repetitions count.** "no no no" is three words and the streamer meant
//!   all three.
//!
//! Fillers are counted separately and subtracted only if asked, because
//! excluding them is a variant game mode rather than the honest default.

use serde::{Deserialize, Serialize};

/// Punctuation that never changes a word's status, plus the smart quotes and
/// dashes that a recogniser may emit when it transcribes typed text.
const PUNCTUATION: &[char] = &[
    '.', ',', '!', '?', ';', ':', '"', '\'', '(', ')', '[', ']', '{', '}', '`', '~', '_', '*', '#',
    '|', '\\', '/', '¡', '¿',
];

/// Characters that glue two spoken words together, e.g. "forty-two" or
/// "well-known". Splitting on these is how a hyphenated token becomes two
/// words.
const JOINERS: &[char] = &['-', '–', '—', '/'];

/// Words counted as filler by default. Deliberately short: a list that grows to
/// include "like" starts scoring ordinary sentences as filler.
pub const DEFAULT_FILLERS: &[&str] = &["um", "uh", "erm", "er", "ah", "hmm", "mhm"];

/// Contractions as spoken, mapped to their expanded form. Every entry is worth
/// two words rather than one.
const CONTRACTIONS: &[(&str, &str)] = &[
    // negatives
    ("don't", "do not"),
    ("doesn't", "does not"),
    ("didn't", "did not"),
    ("can't", "can not"),
    ("cannot", "can not"),
    ("won't", "will not"),
    ("wouldn't", "would not"),
    ("couldn't", "could not"),
    ("shouldn't", "should not"),
    ("mustn't", "must not"),
    ("needn't", "need not"),
    ("daren't", "dare not"),
    // "ain't" is left alone; it is one spoken syllable and expanding it to
    // "is not" would be a guess about intent.
    // have
    ("haven't", "have not"),
    ("hasn't", "has not"),
    ("hadn't", "had not"),
    // is
    ("isn't", "is not"),
    // "ain't" expands despite being one syllable: it is the stan's "is not"
    // and the goal-facing count should reflect the two words it stands for.
    ("ain't", "is not"),
    // was/were
    ("wasn't", "was not"),
    ("weren't", "were not"),
    // has
    ("that's", "that is"),
    ("there's", "there is"),
    ("here's", "here is"),
    ("what's", "what is"),
    ("who's", "who is"),
    ("where's", "where is"),
    ("when's", "when is"),
    ("why's", "why is"),
    ("how's", "how is"),
    // will
    ("we'll", "we will"),
    ("you'll", "you will"),
    ("they'll", "they will"),
    ("i'll", "i will"),
    ("he'll", "he will"),
    ("she'll", "she will"),
    ("it'll", "it will"),
    // other
    ("let's", "let us"),
    ("i'm", "i am"),
    ("i've", "i have"),
    ("i'd", "i would"),
    ("you're", "you are"),
    ("you've", "you have"),
    ("you'd", "you would"),
    ("we're", "we are"),
    ("we've", "we have"),
    ("we'd", "we would"),
    ("they're", "they are"),
    ("they've", "they have"),
    ("they'd", "they would"),
    ("he's", "he is"),
    ("she's", "she is"),
    ("it'll", "it will"),
    ("it's", "it is"),
    ("let's", "let us"),
];

/// How to derive a count from a transcript.
///
/// Serialised into the session file, so a change to these rules never silently
/// rewrites the meaning of a session that has already been recorded.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CountingRules {
    /// Count "don't" as two words. See the module docs.
    pub expand_contractions: bool,
    /// Count "42" as the two words "forty two".
    pub spell_out_numerals: bool,
    /// Subtract filler words from the goal-facing count. Off by default: fillers
    /// still appear in the transcript and in the filler tally.
    pub exclude_fillers: bool,
    /// Count a lone number as one word even when it is not spelled out. Off by
    /// means `2026` scores zero, which is the honest outcome for a token the
    /// streamer never spoke as a word.
    pub count_bare_numerals_as_words: bool,
    /// Words treated as filler. Lowercased on load.
    pub fillers: Vec<String>,
}

/// A counting rule set with fillers excluded, for the variant mode that
/// scores substantive speech only.
///
/// Derived rather than stored: it is `default` with one flag flipped, so the
/// filler list and every other rule stay in one place.
impl CountingRules {
    /// The same rules, with filler words removed from the scored count.
    pub fn excluding_fillers(&self) -> Self {
        Self {
            exclude_fillers: true,
            ..self.clone()
        }
    }

    /// Is `token` a filler under these rules? Case-insensitive, and compares
    /// against the bare token with punctuation stripped.
    pub fn is_filler(&self, token: &str) -> bool {
        let bare = token.trim_matches(|c: char| PUNCTUATION.contains(&c));
        self.fillers.iter().any(|f| f == &bare.to_lowercase())
    }
}

impl Default for CountingRules {
    fn default() -> Self {
        Self {
            expand_contractions: true,
            spell_out_numerals: true,
            exclude_fillers: false,
            count_bare_numerals_as_words: false,
            fillers: DEFAULT_FILLERS.iter().map(|f| f.to_string()).collect(),
        }
    }
}

/// The result of counting one utterance.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Count {
    /// Words as scored, after every rule has been applied.
    pub words: u32,
    /// Filler words found, regardless of whether they were subtracted.
    pub fillers: u32,
    /// Every token after normalisation, in order. Kept so a disputed count can
    /// be recomputed under different rules without re-running recognition.
    pub tokens: Vec<String>,
    /// Tokens that were excluded because they are filler.
    pub filler_tokens: Vec<String>,
}

/// Count the words in one utterance.
///
/// `rules` is taken by reference and never mutated, so a session can be
/// recounted under revised rules later.
pub fn count(text: &str, rules: &CountingRules) -> Count {
    let tokens = tokenize(text, rules);

    let mut fillers = 0u32;
    let mut filler_tokens = Vec::new();
    let mut words = 0u32;

    for token in &tokens {
        if rules.is_filler(token) {
            fillers += 1;
            filler_tokens.push(token.clone());
        }

        // A filler occupies space in the sentence, so it costs a word unless
        // the rules say fillers are free.
        let contributes = !rules.is_filler(token) || !rules.exclude_fillers;
        if contributes {
            words += 1;
        }
    }

    Count {
        words,
        fillers,
        tokens,
        filler_tokens,
    }
}

/// Split raw transcript text into scored tokens.
///
/// Runs contraction expansion and numeral spelling; both are lossy in
/// opposite directions (expansion adds words, numerals resolve to more than
/// one), so the output length is not the input length in either direction.
pub fn tokenize(text: &str, rules: &CountingRules) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();

    for raw in text.split_whitespace() {
        // A joiner inside a token is a pause in speech: "well-known" is said
        // as two words, so it counts as two.
        let pieces: Vec<&str> = split_on_joiner(raw);

        for piece in pieces {
            let cleaned = clean(piece);
            if cleaned.is_empty() {
                continue;
            }

            let lowered = cleaned.to_lowercase();

            if rules.expand_contractions {
                if let Some((_, expansion)) = CONTRACTIONS.iter().find(|(c, _)| *c == lowered) {
                    for word in expansion.split_whitespace() {
                        out.push(word.to_string());
                    }
                    continue;
                }
            }

            if let Ok(number) = cleaned.parse::<i64>() {
                if rules.spell_out_numerals {
                    out.extend(number_to_words(number));
                    continue;
                }
                if rules.count_bare_numerals_as_words {
                    out.push(cleaned.to_string());
                }
                // Otherwise the digit is dropped: it was never spoken as a word.
                continue;
            }

            out.push(cleaned);
        }
    }

    out
}

/// Split on joiner characters, keeping the pieces. An empty piece (from a
/// leading or trailing hyphen) is dropped by the caller's `clean`.
fn split_on_joiner(token: &str) -> Vec<&str> {
    token
        .split(|c: char| JOINERS.contains(&c))
        .filter(|piece| !piece.is_empty())
        .collect()
}

/// Strip surrounding punctuation and collapse internal runs of it. Leading and
/// trailing marks are noise; an internal apostrophe is not, so
/// `don't` survives `clean` intact and is handled as a contraction instead.
fn clean(token: &str) -> String {
    let trimmed = token.trim_matches(|c: char| PUNCTUATION.contains(&c));
    let mut out = String::with_capacity(trimmed.len());
    let mut last_was_punct = false;

    for ch in trimmed.chars() {
        if PUNCTUATION.contains(&ch) && ch != '\'' {
            if !last_was_punct && !out.is_empty() {
                out.push(' ');
            }
            last_was_punct = true;
        } else {
            out.push(ch);
            last_was_punct = false;
        }
    }

    out.trim().to_string()
}

/// Spell an integer out in the way it is normally said.
///
/// Negative numbers become "minus N". The magnitude ceiling is deliberate:
/// a transcript containing a nine-digit numeral is almost always a mis-recognised
/// timestamp, and expanding it would inject a burst of words into the count.
pub fn number_to_words(n: i64) -> Vec<String> {
    const LIMIT: i64 = 9_999_999_999;

    if n == 0 {
        return vec!["zero".to_string()];
    }
    if n.unsigned_abs() > LIMIT as u64 {
        return vec![n.to_string()];
    }

    let negative = n < 0;
    let mut magnitude = n.unsigned_abs();
    let mut words: Vec<String> = Vec::new();

    // Scale words are consumed first so that a remainder is always < 1000.
    let scales: [(u64, &str); 3] = [
        (1_000_000_000, "billion"),
        (1_000_000, "million"),
        (1_000, "thousand"),
    ];
    for (divisor, word) in scales {
        let part = magnitude / divisor;
        if part > 0 {
            // `part` can be up to 999 (one billion and a bit), so it goes
            // through the hundreds path rather than directly to the tens.
            words.extend(below_thousand(part));
            words.push(word.to_string());
            magnitude %= divisor;
        }
    }

    words.extend(below_thousand(magnitude));

    if negative {
        words.insert(0, "minus".to_string());
    }
    words
}

/// 1..=999.
fn below_thousand(n: u64) -> Vec<String> {
    if n == 0 {
        return Vec::new();
    }
    if n < 100 {
        return below_hundred(n);
    }

    let hundreds = n / 100;
    let mut words = vec![
        ONES[(hundreds - 1) as usize].to_string(),
        "hundred".to_string(),
    ];
    if n % 100 != 0 {
        words.extend(below_hundred(n % 100));
    }
    words
}

/// 1..=99.
fn below_hundred(n: u64) -> Vec<String> {
    if n == 0 {
        return Vec::new();
    }
    if n < 20 {
        return vec![SMALL[n as usize].to_string()];
    }

    // SMALL_TENS covers 20..90, so it holds eight entries for tens 2 through 9.
    // The offset is 2, not 1: tens = 4 is the forties, which is index 2.
    // Getting this wrong silently miscounts every number in a stream, since a
    // streamer saying "52" and "32" both take this path.
    let tens = (n / 10) as usize;
    let ones = n % 10;
    let mut words = vec![SMALL_TENS[tens - 2].to_string()];
    if ones > 0 {
        words.push(ONES[(ones - 1) as usize].to_string());
    }
    words
}

const ONES: [&str; 9] = [
    "one", "two", "three", "four", "five", "six", "seven", "eight", "nine",
];

const SMALL: [&str; 20] = [
    "zero",
    "one",
    "two",
    "three",
    "four",
    "five",
    "six",
    "seven",
    "eight",
    "nine",
    "ten",
    "eleven",
    "twelve",
    "thirteen",
    "fourteen",
    "fifteen",
    "sixteen",
    "seventeen",
    "eighteen",
    "nineteen",
];

const SMALL_TENS: [&str; 8] = [
    "twenty", "thirty", "forty", "fifty", "sixty", "seventy", "eighty", "ninety",
];

#[cfg(test)]
mod tests {
    use super::*;

    fn rules() -> CountingRules {
        CountingRules::default()
    }

    #[test]
    fn plain_sentence() {
        let c = count("the quick brown fox", &rules());
        assert_eq!(c.words, 4);
        assert_eq!(c.fillers, 0);
    }

    #[test]
    fn contractions_expand_to_spoken_words() {
        // "don't" is said as "do not".
        let c = count("I don't know", &rules());
        assert_eq!(c.words, 4);
        assert_eq!(c.tokens, vec!["I", "do", "not", "know"]);
    }

    #[test]
    fn contraction_expansion_can_be_disabled() {
        let r = CountingRules {
            expand_contractions: false,
            ..rules()
        };
        let c = count("I don't know", &r);
        assert_eq!(c.words, 3);
    }

    #[test]
    fn numerals_spell_out_by_default() {
        let c = count("I have 42 cats", &rules());
        // "forty two" is two words.
        assert_eq!(c.words, 5);
        assert_eq!(c.tokens, vec!["I", "have", "forty", "two", "cats"]);
    }

    #[test]
    fn bare_numerals_score_nothing_when_not_spelled() {
        let r = CountingRules {
            spell_out_numerals: false,
            ..rules()
        };
        let c = count("I have 42 cats", &r);
        assert_eq!(c.words, 3);
    }

    #[test]
    fn bare_numerals_can_count_as_one_word() {
        let r = CountingRules {
            spell_out_numerals: false,
            count_bare_numerals_as_words: true,
            ..rules()
        };
        let c = count("I have 42 cats", &r);
        assert_eq!(c.words, 4);
    }

    #[test]
    fn hyphenated_splits_into_two() {
        let c = count("a well-known fact", &rules());
        assert_eq!(c.words, 4);
        assert_eq!(c.tokens, vec!["a", "well", "known", "fact"]);
    }

    #[test]
    fn fillers_count_by_default_and_are_tallied() {
        let c = count("um I uh think", &rules());
        assert_eq!(c.words, 4);
        assert_eq!(c.fillers, 2);
        assert_eq!(c.filler_tokens, vec!["um", "uh"]);
    }

    #[test]
    fn fillers_can_be_excluded() {
        let r = CountingRules {
            exclude_fillers: true,
            ..rules()
        };
        let c = count("um I uh think", &r);
        assert_eq!(c.words, 2);
        assert_eq!(c.fillers, 2);
    }

    #[test]
    fn repetitions_count() {
        let c = count("no no no", &rules());
        assert_eq!(c.words, 3);
    }

    #[test]
    fn numbers_with_scales() {
        assert_eq!(number_to_words(0), vec!["zero"]);
        assert_eq!(number_to_words(7), vec!["seven"]);
        assert_eq!(number_to_words(20), vec!["twenty"]);
        assert_eq!(number_to_words(42), vec!["forty", "two"]);
        assert_eq!(number_to_words(100), vec!["one", "hundred"]);
        assert_eq!(number_to_words(101), vec!["one", "hundred", "one"]);
        assert_eq!(
            number_to_words(999),
            vec!["nine", "hundred", "ninety", "nine"]
        );
        assert_eq!(number_to_words(1000), vec!["one", "thousand"]);
        assert_eq!(
            number_to_words(2_026),
            vec!["two", "thousand", "twenty", "six"]
        );
        assert_eq!(
            number_to_words(1_500_000),
            vec!["one", "million", "five", "hundred", "thousand"]
        );
    }

    #[test]
    fn negative_numbers_say_minus() {
        assert_eq!(number_to_words(-5), vec!["minus", "five"]);
    }

    #[test]
    fn absurd_magnitudes_are_left_alone() {
        // A long digit run is more likely a misread timestamp than speech.
        let words = number_to_words(90_000_000_000_000);
        assert_eq!(words, vec!["90000000000000".to_string()]);
    }

    #[test]
    fn punctuation_only_yields_nothing() {
        let c = count("... [laughing] !!!", &rules());
        assert_eq!(c.words, 1); // "laughing"
        assert_eq!(c.tokens, vec!["laughing"]);
    }

    #[test]
    fn empty_transcript_is_zero() {
        let c = count("", &rules());
        assert_eq!(c.words, 0);
        assert!(c.tokens.is_empty());
    }

    #[test]
    fn filler_matching_is_case_insensitive() {
        let c = count("Um, UM, um.", &rules());
        assert_eq!(c.fillers, 3);
    }

    #[test]
    fn quoting_does_not_eat_a_contraction() {
        // The internal apostrophe must survive cleaning for expansion to see it.
        let c = count(r#""don't""#, &rules());
        assert_eq!(c.tokens, vec!["do", "not"]);
    }
}
