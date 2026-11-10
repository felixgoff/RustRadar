//! JavaScript-semantics text helpers (doc 05 §14).
//!
//! | JavaScript | here |
//! |---|---|
//! | `n.toLocaleString("en-GB")`, and `toLocaleString()` (English everywhere, plan §7) | [`group_thousands`] |
//! | `n.toLocaleString(…, { minimumFractionDigits: d, maximumFractionDigits: d })` | [`group_thousands_fixed`] |
//! | `train-list.ts` `fold` | [`fold`] |
//! | `new Intl.Collator(undefined, { numeric: true, sensitivity: "base" }).compare` | [`natural_cmp`] |
//! | `a.localeCompare(b)` | [`locale_cmp`] |
//!
//! The two comparisons stand in for ICU without a locale (plan §7: lists sort with a natural
//! compare, no locale tailoring): they agree with ICU's root collation on Latin text with
//! accents, digits, spaces and punctuation, which is what train, ship and operator names hold.

use std::cmp::Ordering;

use unicode_normalization::UnicodeNormalization;
use unicode_normalization::char::is_combining_mark;

/// Lower case without accents, so "helsinki" finds "Helsinki" and "oslo s" finds "Oslo S"
/// (`train-list.ts` `fold`: `normalize("NFD")`, strip `\p{M}`, `toLowerCase()`).
pub fn fold(s: &str) -> String {
    s.nfd().filter(|c| !is_combining_mark(*c)).collect::<String>().to_lowercase()
}

/// `s` (digits with an optional fraction, no sign, no exponent) rounded half up to `digits`
/// decimals, as a decimal string with exactly that many.
fn round_decimal(s: &str, digits: usize) -> String {
    let (int, frac) = s.split_once('.').unwrap_or((s, ""));
    let mut out: Vec<u8> = int.bytes().collect();
    out.extend(frac.bytes().chain(std::iter::repeat(b'0')).take(digits));
    if frac.as_bytes().get(digits).is_some_and(|d| *d >= b'5') {
        let mut carry = true;
        for b in out.iter_mut().rev() {
            if *b == b'9' {
                *b = b'0';
            } else {
                *b += 1;
                carry = false;
                break;
            }
        }
        if carry {
            out.insert(0, b'1');
        }
    }
    let mut text = String::from_utf8(out).expect("ASCII digits");
    if digits > 0 {
        text.insert(text.len() - digits, '.');
    }
    text
}

/// Commas between the thousands of a string of digits.
fn group(int: &str) -> String {
    let mut out = String::with_capacity(int.len() + int.len() / 3);
    for (i, c) in int.chars().enumerate() {
        if i > 0 && (int.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// The shared body of the two formatters: `None` keeps up to three decimals, `Some(d)` exactly `d`.
fn english(x: f64, fixed: Option<usize>) -> String {
    if x.is_nan() {
        return "NaN".to_owned();
    }
    let sign = if x.is_sign_negative() { "-" } else { "" };
    if x.is_infinite() {
        return format!("{sign}∞");
    }
    // Intl rounds the shortest decimal form, not the exact binary value: 1.0005 gives 1.001
    // (where `toFixed(3)` gives "1.000"). Rust's `{}` is that shortest form, never an exponent.
    let rounded = round_decimal(&format!("{}", x.abs()), fixed.unwrap_or(3));
    let (int, frac) = rounded.split_once('.').unwrap_or((&rounded, ""));
    let frac = if fixed.is_some() { frac } else { frac.trim_end_matches('0') };
    let dot = if frac.is_empty() { "" } else { "." };
    format!("{sign}{}{dot}{frac}", group(int))
}

/// `x.toLocaleString("en-GB")`: commas between thousands and at most three decimals, a half
/// rounding away from zero (`1240 → "1,240"`, `1234.5678 → "1,234.568"`). As in JavaScript, a
/// negative zero and a negative value that rounds to zero print as `"-0"`, NaN as `"NaN"` and
/// infinity as `"∞"`. Pass a count as `n as f64`.
pub fn group_thousands(x: f64) -> String {
    english(x, None)
}

/// `x.toLocaleString("en-GB", { minimumFractionDigits: digits, maximumFractionDigits: digits })`:
/// as [`group_thousands`] with exactly `digits` decimals (`1234.5, 1 → "1,234.5"`, `2.5, 0 → "3"`).
pub fn group_thousands_fixed(x: f64, digits: usize) -> String {
    english(x, Some(digits))
}

/// One element of a collation key: `(class, digit count, text)`. The classes order as ICU's
/// root collation does: white space, then punctuation and symbols, then digits, then letters.
/// A number is a run of ASCII digits without its leading zeros: the shorter one is the smaller.
type Element = (u8, usize, String);

/// The key the two comparisons order by at "base" strength: folded, with the letters that
/// ICU treats as a base letter or a pair written out (`ß` = `ss`, `æ` = `ae`, `ø` = `o`, …).
fn collation_key(s: &str, numeric: bool) -> Vec<Element> {
    let mut letters = String::with_capacity(s.len());
    for c in fold(s).chars() {
        match c {
            'ß' => letters.push_str("ss"),
            'æ' => letters.push_str("ae"),
            'œ' => letters.push_str("oe"),
            'ø' => letters.push('o'),
            'ł' => letters.push('l'),
            'đ' | 'ð' => letters.push('d'),
            '\u{a0}' | '\u{202f}' | '\u{2007}' => letters.push(' '),
            c => letters.push(c),
        }
    }
    let mut key: Vec<Element> = Vec::new();
    let mut chars = letters.chars().peekable();
    while let Some(c) = chars.next() {
        if c.is_ascii_digit() {
            let mut run = String::from(c);
            if numeric {
                while let Some(d) = chars.peek().copied().filter(char::is_ascii_digit) {
                    run.push(d);
                    chars.next();
                }
                let trimmed = run.trim_start_matches('0');
                run = if trimmed.is_empty() { "0".to_owned() } else { trimmed.to_owned() };
            }
            key.push((2, run.len(), run));
        } else {
            let class = if c.is_whitespace() {
                0
            } else if c.is_alphabetic() {
                3
            } else if c.is_numeric() {
                2
            } else {
                1
            };
            key.push((class, 0, c.to_string()));
        }
    }
    key
}

/// `new Intl.Collator(undefined, { numeric: true, sensitivity: "base" }).compare(a, b)` without
/// a locale: natural order (`"IC 2"` before `"IC 10"`), case and accents ignored (`"Å"` equals
/// `"a"`), a space before punctuation before digits before letters. `Equal` means "the same at
/// base strength", not "the same string": callers fall back to an id, as the TypeScript does.
pub fn natural_cmp(a: &str, b: &str) -> Ordering {
    collation_key(a, true).cmp(&collation_key(b, true))
}

/// `a.localeCompare(b)` without a locale: letters first (digits one by one: `"a10"` before
/// `"a9"`), then accents (`"a"` before `"á"`), then case (`"a"` before `"A"`).
pub fn locale_cmp(a: &str, b: &str) -> Ordering {
    collation_key(a, false)
        .cmp(&collation_key(b, false))
        .then_with(|| {
            // accents: the decomposed lower-case forms, where a mark sorts after no mark
            let marks = |s: &str| s.nfd().collect::<String>().to_lowercase();
            marks(a).cmp(&marks(b))
        })
        .then_with(|| {
            // case: at the first place where it differs, lower case first
            a.chars()
                .zip(b.chars())
                .find(|(x, y)| x != y && x.is_uppercase() != y.is_uppercase())
                .map_or(Ordering::Equal, |(x, _)| if x.is_uppercase() { Ordering::Greater } else { Ordering::Less })
        })
        .then_with(|| a.cmp(b))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cmp::Ordering::{Equal, Greater, Less};

    #[test]
    fn fold_lower_cases_and_strips_accents() {
        assert_eq!(fold("Helsinki"), "helsinki");
        assert_eq!(fold("Åre Östersund"), "are ostersund");
        assert_eq!(fold("Málaga"), "malaga");
        assert_eq!(fold("Łódź"), "łodz");
        // already decomposed input folds the same
        assert_eq!(fold("e\u{301}"), "e");
        assert_eq!(fold("Oslo S"), "oslo s");
        assert_eq!(fold(""), "");
    }

    #[test]
    fn fold_keeps_letters_that_do_not_decompose() {
        // as in JavaScript: these are letters of their own, not a base letter plus a mark
        assert_eq!(fold("Tromsø"), "tromsø");
        assert_eq!(fold("Ærø"), "ærø");
        assert_eq!(fold("ß"), "ß");
        // the dot of İ is a mark once decomposed, so it goes before the lower-casing
        assert_eq!(fold("İstanbul"), "istanbul");
        // a final sigma, and a title-case digraph
        assert_eq!(fold("ΣΑΣ"), "σας");
        assert_eq!(fold("ǅ"), "ǆ");
    }

    #[test]
    fn group_thousands_groups_the_english_way() {
        assert_eq!(group_thousands(1240.0), "1,240");
        assert_eq!(group_thousands(999.0), "999");
        assert_eq!(group_thousands(1000.0), "1,000");
        assert_eq!(group_thousands(0.0), "0");
        assert_eq!(group_thousands(1_234_567.0), "1,234,567");
        assert_eq!(group_thousands(1e6), "1,000,000");
        assert_eq!(group_thousands(123_456_789_012.0), "123,456,789,012");
        assert_eq!(group_thousands(-1234.5), "-1,234.5");
        assert_eq!(group_thousands(35_000.0), "35,000");
        assert_eq!(group_thousands(1e21), "1,000,000,000,000,000,000,000");
    }

    #[test]
    fn group_thousands_keeps_at_most_three_decimals() {
        assert_eq!(group_thousands(1234.5678), "1,234.568");
        assert_eq!(group_thousands(12345.6789), "12,345.679");
        assert_eq!(group_thousands(0.5), "0.5");
        assert_eq!(group_thousands(999.9996), "1,000");
        // Intl rounds the shortest decimal form: a half goes up even where the double is below it
        assert_eq!(group_thousands(0.0005), "0.001");
        assert_eq!(group_thousands(1.0005), "1.001");
        assert_eq!(group_thousands(0.0015), "0.002");
        assert_eq!(crate::num::to_fixed(1.0005, 3), "1.000");
    }

    #[test]
    fn group_thousands_zero_sign_and_the_values_that_are_not_numbers() {
        assert_eq!(group_thousands(-0.0), "-0");
        assert_eq!(group_thousands(-0.0001), "-0");
        assert_eq!(group_thousands(f64::NAN), "NaN");
        assert_eq!(group_thousands(f64::INFINITY), "∞");
        assert_eq!(group_thousands(f64::NEG_INFINITY), "-∞");
    }

    #[test]
    fn group_thousands_fixed_keeps_exactly_the_decimals_asked_for() {
        assert_eq!(group_thousands_fixed(1234.5, 1), "1,234.5");
        assert_eq!(group_thousands_fixed(2.5, 0), "3");
        assert_eq!(group_thousands_fixed(1234.25, 1), "1,234.3");
        assert_eq!(group_thousands_fixed(-0.04, 1), "-0.0");
        assert_eq!(group_thousands_fixed(35000.0, 0), "35,000");
        assert_eq!(group_thousands_fixed(0.125, 2), "0.13");
        assert_eq!(group_thousands_fixed(7.0, 2), "7.00");
        assert_eq!(group_thousands_fixed(999.95, 1), "1,000.0");
    }

    #[test]
    fn natural_cmp_orders_numbers_by_value() {
        assert_eq!(natural_cmp("IC 2", "IC 10"), Less);
        assert_eq!(natural_cmp("x10", "x9"), Greater);
        assert_eq!(natural_cmp("9", "10"), Less);
        assert_eq!(natural_cmp("1.5", "1.10"), Less);
        // leading zeros do not count
        assert_eq!(natural_cmp("IC 02", "IC 2"), Equal);
        assert_eq!(natural_cmp("007", "7"), Equal);
        assert_eq!(natural_cmp("a01b", "a1a"), Greater);
        // longer than any integer type
        assert_eq!(natural_cmp("a123456789012345678901234567890", "a123456789012345678901234567891"), Less);
        // where the code-point order goes the other way
        assert_eq!("IC 2".cmp("IC 10"), Greater);
        let mut names = vec!["IC 10", "P 50", "IC 2", "H 9"];
        names.sort_by(|a, b| natural_cmp(a, b));
        assert_eq!(names, ["H 9", "IC 2", "IC 10", "P 50"]);
    }

    #[test]
    fn natural_cmp_ignores_case_and_accents() {
        assert_eq!(natural_cmp("a", "B"), Less);
        assert_eq!(natural_cmp("Z", "a"), Greater);
        assert_eq!(natural_cmp("Å", "a"), Equal);
        assert_eq!(natural_cmp("é", "E"), Equal);
        assert_eq!(natural_cmp("IC 2", "ic 2"), Equal);
        assert_eq!(natural_cmp("ö", "z"), Less);
        assert_eq!(natural_cmp("", "a"), Less);
        assert_eq!(natural_cmp("", ""), Equal);
        // where the code-point order goes the other way
        assert_eq!("Z".cmp("a"), Less);
        assert_eq!("ö".cmp("z"), Greater);
    }

    #[test]
    fn natural_cmp_treats_special_letters_as_icu_does() {
        assert_eq!(natural_cmp("ß", "ss"), Equal);
        assert_eq!(natural_cmp("æ", "ae"), Equal);
        assert_eq!(natural_cmp("æ", "af"), Less);
        assert_eq!(natural_cmp("œ", "oe"), Equal);
        assert_eq!(natural_cmp("ø", "o"), Equal);
        assert_eq!(natural_cmp("ø", "oz"), Less);
        assert_eq!(natural_cmp("ł", "l"), Equal);
        assert_eq!(natural_cmp("ł", "lz"), Less);
        assert_eq!(natural_cmp("đ", "d"), Equal);
        assert_eq!(natural_cmp("ð", "dz"), Less);
        assert_eq!(natural_cmp("þ", "z"), Greater);
    }

    #[test]
    fn natural_cmp_puts_spaces_before_punctuation_before_digits_before_letters() {
        assert_eq!(natural_cmp("a b", "ab"), Less);
        assert_eq!(natural_cmp("a-b", "ab"), Less);
        assert_eq!(natural_cmp("a b", "a-b"), Less);
        assert_eq!(natural_cmp("a_b", "a b"), Greater);
        assert_eq!(natural_cmp("a 1", "a1"), Less);
        assert_eq!(natural_cmp("a1", "aa"), Less);
        assert_eq!(natural_cmp("1a", "a"), Less);
        assert_eq!(natural_cmp("a.b", "a1"), Less);
        // a colon is after the digits by code point, before them in a collation
        assert_eq!(natural_cmp("a:b", "a1"), Less);
        assert_eq!(natural_cmp("a+b", "a1"), Less);
        assert_eq!(natural_cmp("IC10", "IC 2"), Greater);
        assert_eq!(natural_cmp("a  b", "a b"), Less);
        assert_eq!(natural_cmp("a\u{a0}b", "a b"), Equal);
    }

    #[test]
    fn locale_cmp_orders_by_letters_then_accents_then_case() {
        assert_eq!(locale_cmp("a", "A"), Less);
        assert_eq!(locale_cmp("A", "a"), Greater);
        assert_eq!(locale_cmp("a", "á"), Less);
        // an accent weighs more than case
        assert_eq!(locale_cmp("A", "á"), Less);
        assert_eq!(locale_cmp("b", "A"), Greater);
        assert_eq!(locale_cmp("Z", "a"), Greater);
        assert_eq!(locale_cmp("resume", "résumé"), Less);
        assert_eq!(locale_cmp("VR", "vr"), Greater);
        assert_eq!(locale_cmp("abc", "abd"), Less);
        // letters come before accents and case
        assert_eq!(locale_cmp("áb", "ac"), Less);
        assert_eq!(locale_cmp("Ab", "aC"), Less);
        assert_eq!(locale_cmp("", "a"), Less);
        assert_eq!(locale_cmp("a", "a"), Equal);
        // not numeric: digit by digit
        assert_eq!(locale_cmp("a10", "a9"), Less);
        // where the code-point order goes the other way
        assert_eq!("a".cmp("A"), Greater);
    }

    #[test]
    fn the_comparisons_are_total_orders_for_sorting() {
        let words = ["IC 10", "ic 2", "IC 2", "Å", "a", "A", "á", "", "ß", "ss", "x9", "x10", "x 9", "x-9", "Z"];
        for a in words {
            assert_eq!(natural_cmp(a, a), Equal);
            assert_eq!(locale_cmp(a, a), Equal);
            for b in words {
                assert_eq!(natural_cmp(a, b), natural_cmp(b, a).reverse(), "{a:?} {b:?}");
                assert_eq!(locale_cmp(a, b), locale_cmp(b, a).reverse(), "{a:?} {b:?}");
                for c in words {
                    if natural_cmp(a, b) != Greater && natural_cmp(b, c) != Greater {
                        assert_ne!(natural_cmp(a, c), Greater, "{a:?} {b:?} {c:?}");
                    }
                    if locale_cmp(a, b) != Greater && locale_cmp(b, c) != Greater {
                        assert_ne!(locale_cmp(a, c), Greater, "{a:?} {b:?} {c:?}");
                    }
                }
            }
        }
    }

    #[test]
    fn regex_word_boundary_and_digit_are_ascii_in_javascript() {
        // /^sj\b/i matches "sjö" in JavaScript: ö is not a word character there
        let ascii = regex::Regex::new(r"(?i)^sj(?-u:\b)").unwrap();
        assert!(ascii.is_match("sjö"));
        assert!(ascii.is_match("SJ Nord"));
        assert!(!ascii.is_match("sjx"));
        assert!(regex::Regex::new(r"(?i)^sl(?-u:\b)").unwrap().is_match("slå"));
        // /^\d+$/ does not match other scripts' digits in JavaScript
        assert!(regex::Regex::new(r"^[0-9]+$").unwrap().is_match("123"));
        assert!(!regex::Regex::new(r"^[0-9]+$").unwrap().is_match("١٢٣"));
        // where the regex crate's defaults go the other way
        assert!(!regex::Regex::new(r"(?i)^sj\b").unwrap().is_match("sjö"));
        assert!(regex::Regex::new(r"^\d+$").unwrap().is_match("١٢٣"));
        // the `i` flag still folds å ä ö, as in JavaScript
        assert!(regex::Regex::new(r"(?i)^skånetrafiken(?-u:\b)").unwrap().is_match("SKÅNETRAFIKEN"));
    }

    #[test]
    fn string_lengths_and_slices_are_utf16_units_in_javascript() {
        // "å".length is 1 in JavaScript and 2 bytes here; an emoji is 2 units and 4 bytes
        assert_eq!("å".len(), 2);
        assert_eq!("å".encode_utf16().count(), 1);
        assert_eq!("✈️".chars().count(), 2);
        assert_eq!("😀".encode_utf16().count(), 2);
        // `language.slice(0, 2)` is only safe as a byte slice on ASCII: take characters instead
        assert_eq!("sv-SE".chars().take(2).collect::<String>(), "sv");
        assert_eq!("åland".chars().take(2).collect::<String>(), "ål");
        assert!(!"åland".is_char_boundary(1));
    }
}
