//! Tag parsing: tags are lowercase letters, digits and `- _ . /`.

use std::collections::BTreeSet;

pub const MAX_TAG_LEN: usize = 32;
pub const MAX_TAGS: usize = 16;

fn normalize_tag(raw: &str) -> Result<String, String> {
    let tag = raw.strip_prefix('#').unwrap_or(raw).to_lowercase();
    if tag.is_empty() {
        return Err("a tag can't be just '#'".into());
    }
    if tag.chars().count() > MAX_TAG_LEN {
        return Err(format!(
            "tag '{tag}' is longer than {MAX_TAG_LEN} characters"
        ));
    }
    if let Some(bad) = tag
        .chars()
        .find(|c| !(c.is_alphanumeric() || matches!(c, '-' | '_' | '.' | '/')))
    {
        return Err(format!(
            "tag '{tag}' contains '{bad}'; use letters, digits and - _ . /"
        ));
    }
    Ok(tag)
}

/// Tags from user text: separated by commas and/or whitespace, an optional
/// leading `#` is ignored. The result is lowercase, sorted and de-duplicated.
/// Empty input means "no tags".
pub fn parse_tags(text: &str) -> Result<Vec<String>, String> {
    let mut set = BTreeSet::new();
    for token in text
        .split(|c: char| c == ',' || c.is_whitespace())
        .filter(|t| !t.is_empty())
    {
        set.insert(normalize_tag(token)?);
    }
    if set.len() > MAX_TAGS {
        return Err(format!("at most {MAX_TAGS} tags per feed"));
    }
    Ok(set.into_iter().collect())
}

/// A folder name: trimmed, non-empty, no control characters.
pub fn parse_folder(text: &str) -> Result<String, String> {
    let name = text.trim();
    if name.is_empty() {
        return Err("a folder needs a name".into());
    }
    if name.chars().count() > MAX_TAG_LEN {
        return Err(format!("folder names are at most {MAX_TAG_LEN} characters"));
    }
    if name.chars().any(char::is_control) {
        return Err("folder names can't contain control characters".into());
    }
    Ok(name.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_commas_spaces_hashes_and_case() {
        assert_eq!(
            parse_tags("Rust, #news  go").unwrap(),
            ["go", "news", "rust"]
        );
        assert_eq!(parse_tags("a a A").unwrap(), ["a"]);
        assert!(parse_tags("").unwrap().is_empty());
    }

    #[test]
    fn rejects_bad_tags() {
        assert!(parse_tags("#").is_err());
        assert!(parse_tags("a:b").is_err());
        assert!(parse_tags(&"x".repeat(MAX_TAG_LEN + 1)).is_err());
        assert!(
            parse_tags(
                &(0..=MAX_TAGS)
                    .map(|i| format!("t{i}"))
                    .collect::<Vec<_>>()
                    .join(" ")
            )
            .is_err()
        );
    }

    #[test]
    fn folder_names_are_validated() {
        assert_eq!(parse_folder("  News ").unwrap(), "News");
        assert!(parse_folder("   ").is_err());
        assert!(parse_folder("a\tb").is_err());
    }
}
