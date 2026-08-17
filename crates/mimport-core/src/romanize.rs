//! CJK script detection and MB alias picking. MB carries editor-curated
//! romanization aliases for artists/release-groups/recordings (locale/type
//! are often left unset, so we can't filter on them reliably), which
//! sidesteps kanji-reading ambiguity that a programmatic transliterator
//! would introduce. Orchestration (fetching aliases, applying manual
//! overrides, deciding what's still unresolved) lives in `crate::tags`.

use crate::mb::types::Alias;

/// Alias types that are MB search hints, not display names, and must never
/// be picked as a romanization.
const JUNK_ALIAS_TYPES: &[&str] = &["Search hint"];

/// True if `s` contains any CJK/Hangul script character, i.e. it needs
/// romanization before it can go into a filename/tag meant to stay Latin.
pub fn is_non_latin(s: &str) -> bool {
    return s.chars().any(is_non_latin_char);
}

fn is_non_latin_char(c: char) -> bool {
    let cp = c as u32;
    return matches!(cp,
        0x3040..=0x309F   // Hiragana
        | 0x30A0..=0x30FF // Katakana (incl. phonetic extensions)
        | 0x3400..=0x4DBF // CJK Unified Ideographs Extension A
        | 0x4E00..=0x9FFF // CJK Unified Ideographs
        | 0xF900..=0xFAFF // CJK Compatibility Ideographs
        | 0x3000..=0x303F // CJK punctuation/symbols
        | 0xFF00..=0xFFEF // Halfwidth/fullwidth forms
        | 0xAC00..=0xD7A3 // Hangul Syllables
        | 0x1100..=0x11FF // Hangul Jamo
        | 0x3130..=0x318F // Hangul Compatibility Jamo
    );
}

/// Picks the best Latin-script alias: junk hint types are excluded, primary
/// aliases win, then `en*`-locale aliases, then the first remaining in MB's
/// own order.
pub fn pick_alias(aliases: &[Alias]) -> Option<String> {
    let mut candidates: Vec<&Alias> = aliases
        .iter()
        .filter(|a| return !is_non_latin(&a.name))
        .filter(|a| return !a.alias_type.as_deref().is_some_and(|t| return JUNK_ALIAS_TYPES.contains(&t)))
        .collect();
    candidates.sort_by_key(|a| {
        let primary_rank = if a.primary == Some(true) { 0 } else { 1 };
        let locale_rank = if a.locale.as_deref().is_some_and(|l| return l.starts_with("en")) { 0 } else { 1 };
        return (primary_rank, locale_rank);
    });
    return candidates.first().map(|a| return a.name.clone());
}
