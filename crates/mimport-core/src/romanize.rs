use crate::mb::types::Alias;

const JUNK_ALIAS_TYPES: &[&str] = &["Search hint"];

pub fn is_non_latin(s: &str) -> bool {
    return s.chars().any(is_non_latin_char);
}

fn is_non_latin_char(c: char) -> bool {
    let cp = c as u32;
    return matches!(cp,
        0x3040..=0x309F
        | 0x30A0..=0x30FF
        | 0x3400..=0x4DBF
        | 0x4E00..=0x9FFF
        | 0xF900..=0xFAFF
        | 0x3000..=0x303F
        | 0xFF00..=0xFFEF
        | 0xAC00..=0xD7A3
        | 0x1100..=0x11FF
        | 0x3130..=0x318F
    );
}

pub fn pick_alias(aliases: &[Alias]) -> Option<String> {
    let mut candidates: Vec<&Alias> = aliases
        .iter()
        .filter(|a| return !is_non_latin(&a.name))
        .filter(|a| {
            return !a
                .alias_type
                .as_deref()
                .is_some_and(|t| return JUNK_ALIAS_TYPES.contains(&t));
        })
        .filter(|a| {
            return !a
                .locale
                .as_deref()
                .is_some_and(|l| return l.starts_with("en"));
        })
        .collect();
    candidates.sort_by_key(|a| {
        let romaji_rank = if a
            .locale
            .as_deref()
            .is_some_and(|l| return l.ends_with("Latn"))
        {
            0
        } else {
            1
        };
        let primary_rank = if a.primary == Some(true) { 0 } else { 1 };
        return (romaji_rank, primary_rank);
    });
    return candidates.first().map(|a| return a.name.clone());
}
