use serde::Deserialize;

/// A frontmatter value written either as a delimited string or a YAML list
/// (`allowed-tools`, `arguments`).
#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub(super) enum ListValue {
    Delimited(String),
    Sequence(Vec<String>),
}

impl ListValue {
    /// Items of a comma separated list (`allowed-tools`), trimmed. Entries may
    /// contain spaces, e.g. `Bash(git push *)`.
    pub(super) fn into_comma_list(self) -> Vec<String> {
        match self {
            Self::Delimited(text) => text
                .split(',')
                .map(|item| item.trim().to_string())
                .collect(),
            Self::Sequence(items) => items,
        }
    }

    /// Identifier names (`arguments`): a string splits on commas and spaces.
    pub(super) fn into_names(self) -> Vec<String> {
        match self {
            Self::Delimited(text) => text
                .split(|ch: char| ch == ',' || ch.is_whitespace())
                .filter(|item| !item.is_empty())
                .map(str::to_string)
                .collect(),
            Self::Sequence(items) => items,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::ListValue;

    #[test]
    fn names_split_on_commas_and_spaces() {
        let value = ListValue::Delimited("read, bash grep".to_string());
        assert_eq!(value.into_names(), ["read", "bash", "grep"]);
    }

    #[test]
    fn comma_list_keeps_inner_spaces() {
        let value = ListValue::Delimited("Read, Bash(git push *)".to_string());
        assert_eq!(value.into_comma_list(), ["Read", "Bash(git push *)"]);
    }
}
