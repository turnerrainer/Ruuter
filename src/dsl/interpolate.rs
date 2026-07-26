//! Constant interpolation — shared between the DSL parser, the
//! `dsl-lint` binary, and the WebSocket source-config loader.
//!
//! Two surface syntaxes are accepted, both resolving to the same
//! `constants.ini` key and producing identical output:
//!
//! - `[#NAME]` — original Java-Ruuter syntax. Retained indefinitely
//!   for backward compatibility.
//! - `#{NAME}` — alternate syntax added by task 067. Visually
//!   mirrors `${...}` (runtime variable) so the two interpolation
//!   forms read as a symmetric pair.
//!
//! Both forms accept the same character class inside their
//! delimiters (any character except the matching close delimiter).
//! Missing constants are handled by the caller — see the
//! `substitute` closure.
//!
//! Escape handling: **none**. `[#name]` and `#{name}` are always
//! matched. If a DSL author needs a literal `[#foo]` or `#{bar}`
//! that must survive interpolation, define the "constant" `foo` /
//! `bar` with the literal text as its value.

use once_cell::sync::Lazy;
use regex::{Captures, Regex};

/// One regex covering both syntaxes. Alternation groups:
///
/// - group 1 = `[#NAME]` form (character class `[^\]]+`)
/// - group 2 = `#{NAME}` form (character class `[^}]+`)
///
/// Only one group ever captures per match. The `key_of` helper
/// returns whichever is present.
static CONSTANT_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"\[#([^\]]+)\]|#\{([^}]+)\}").unwrap());

/// Replace every `[#NAME]` and `#{NAME}` reference in `input` using
/// `lookup`. When `lookup` returns `None` for a given key, the
/// original literal (`[#NAME]` or `#{NAME}` — whichever form was
/// used) is preserved unchanged.
pub fn substitute<F>(input: &str, mut lookup: F) -> String
where
    F: FnMut(&str) -> Option<String>,
{
    CONSTANT_RE
        .replace_all(input, |caps: &Captures| match lookup(key_of(caps)) {
            Some(v) => v,
            None => caps[0].to_string(),
        })
        .to_string()
}

/// Iterate every constant reference in `input`, yielding the key
/// name (without delimiters) and the exact matched literal
/// (including delimiters). Both syntaxes are yielded through the
/// same iterator.
///
/// Useful for the linter's "unresolved constant" scan — the caller
/// checks each yielded key against the constants map and reports
/// misses with the original literal so error messages show the
/// exact form the author used.
pub fn iter_refs(input: &str) -> impl Iterator<Item = ConstantRef<'_>> {
    CONSTANT_RE.captures_iter(input).map(|caps| ConstantRef {
        key: key_of_owned(&caps).into(),
        literal: caps[0].to_string().into(),
    })
}

/// A single `[#NAME]` or `#{NAME}` occurrence in DSL source.
pub struct ConstantRef<'a> {
    /// The key inside the delimiters — e.g. `NAME`.
    pub key: std::borrow::Cow<'a, str>,
    /// The exact matched literal including delimiters — e.g.
    /// `[#NAME]` or `#{NAME}`. Preserve when reporting so error
    /// messages show the author's syntax.
    pub literal: std::borrow::Cow<'a, str>,
}

fn key_of<'a>(caps: &'a Captures) -> &'a str {
    caps.get(1)
        .or_else(|| caps.get(2))
        .expect("regex guarantees one of the two groups matched")
        .as_str()
}

fn key_of_owned(caps: &Captures) -> String {
    key_of(caps).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn lookup_from<'a>(map: &'a HashMap<&'a str, &'a str>) -> impl Fn(&str) -> Option<String> + 'a {
        move |k| map.get(k).map(|v| v.to_string())
    }

    #[test]
    fn bracket_form_still_works() {
        let mut m = HashMap::new();
        m.insert("FOO", "bar");
        assert_eq!(substitute("hello [#FOO]!", lookup_from(&m)), "hello bar!");
    }

    #[test]
    fn brace_form_new_alias() {
        let mut m = HashMap::new();
        m.insert("FOO", "bar");
        assert_eq!(substitute("hello #{FOO}!", lookup_from(&m)), "hello bar!");
    }

    #[test]
    fn both_forms_produce_identical_output() {
        let mut m = HashMap::new();
        m.insert("x", "1");
        m.insert("y", "2");
        let a = substitute("[#x] and [#y]", lookup_from(&m));
        let b = substitute("#{x} and #{y}", lookup_from(&m));
        assert_eq!(a, b);
    }

    #[test]
    fn mixed_forms_in_one_input() {
        let mut m = HashMap::new();
        m.insert("a", "A");
        m.insert("b", "B");
        assert_eq!(substitute("[#a] then #{b}", lookup_from(&m)), "A then B");
    }

    #[test]
    fn missing_key_preserves_original_literal_bracket() {
        let m: HashMap<&str, &str> = HashMap::new();
        assert_eq!(substitute("keep [#NOPE]", lookup_from(&m)), "keep [#NOPE]");
    }

    #[test]
    fn missing_key_preserves_original_literal_brace() {
        let m: HashMap<&str, &str> = HashMap::new();
        assert_eq!(substitute("keep #{NOPE}", lookup_from(&m)), "keep #{NOPE}");
    }

    #[test]
    fn iter_refs_finds_both_forms() {
        let refs: Vec<(String, String)> = iter_refs("[#a] and #{b} and [#c]")
            .map(|r| (r.key.into_owned(), r.literal.into_owned()))
            .collect();
        assert_eq!(
            refs,
            vec![
                ("a".to_string(), "[#a]".to_string()),
                ("b".to_string(), "#{b}".to_string()),
                ("c".to_string(), "[#c]".to_string()),
            ]
        );
    }

    #[test]
    fn adjacent_refs_both_forms() {
        let mut m = HashMap::new();
        m.insert("a", "A");
        m.insert("b", "B");
        assert_eq!(substitute("[#a][#b]", lookup_from(&m)), "AB");
        assert_eq!(substitute("#{a}#{b}", lookup_from(&m)), "AB");
        assert_eq!(substitute("[#a]#{b}", lookup_from(&m)), "AB");
    }

    #[test]
    fn dollar_runtime_variables_untouched() {
        // ${...} is the runtime-variable syntax, evaluated per-request
        // by the JS engine. Constant substitution must never touch it.
        let m: HashMap<&str, &str> = HashMap::new();
        assert_eq!(
            substitute("prefix ${incoming.body.name} suffix", lookup_from(&m)),
            "prefix ${incoming.body.name} suffix"
        );
    }
}
