//! Conversion of Rust identifiers to Lisp keyword names.

/// Convert a `snake_case` or `CamelCase` identifier to kebab-case.
pub(crate) fn to_kebab(name: &str) -> String {
    let chars: Vec<char> = name.chars().collect();
    let mut out = String::with_capacity(name.len() + 4);
    for (i, &c) in chars.iter().enumerate() {
        if c == '_' {
            out.push('-');
        } else if c.is_uppercase() {
            if i > 0 {
                let prev = chars[i - 1];
                let next_is_lower = chars.get(i + 1).is_some_and(|n| n.is_lowercase());
                let after_word = prev.is_lowercase() || prev.is_ascii_digit();
                let ends_acronym = prev.is_uppercase() && next_is_lower;
                if after_word || ends_acronym {
                    out.push('-');
                }
            }
            out.extend(c.to_lowercase());
        } else {
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::to_kebab;

    #[test]
    fn snake_case_fields() {
        assert_eq!(to_kebab("line_number"), "line-number");
        assert_eq!(to_kebab("path"), "path");
    }

    #[test]
    fn camel_case_variants() {
        assert_eq!(to_kebab("Stop"), "stop");
        assert_eq!(to_kebab("SearchResult"), "search-result");
        assert_eq!(to_kebab("V2Result"), "v2-result");
    }

    #[test]
    fn acronyms_stay_together() {
        assert_eq!(to_kebab("HTTPServer"), "http-server");
        assert_eq!(to_kebab("ParseURL"), "parse-url");
    }

    #[test]
    fn kebab_input_is_unchanged() {
        assert_eq!(to_kebab("already-kebab"), "already-kebab");
    }

    #[test]
    fn empty_and_underscores() {
        assert_eq!(to_kebab(""), "");
        assert_eq!(to_kebab("_private_Name"), "-private-name");
    }
}
