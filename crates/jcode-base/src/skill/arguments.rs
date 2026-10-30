//! `$ARGUMENTS` substitution for skill bodies (Claude Code compatible).
//!
//! Supported placeholders, replaced in a single left-to-right pass so inserted
//! text is never expanded again:
//!
//! - `$ARGUMENTS`: the whole argument string as typed
//! - `$ARGUMENTS[N]` and `$N`: shell-style word `N`, counting from 0
//! - `$name`: a name declared in the skill's `arguments:` frontmatter list,
//!   bound to the word at the same index (missing word becomes empty)
//! - `${JCODE_SKILL_DIR}` / `${CLAUDE_SKILL_DIR}`: the skill's directory
//! - `${JCODE_PROJECT_DIR}` / `${CLAUDE_PROJECT_DIR}`: the session project root
//!
//! `\$` yields a literal `$`. A missing indexed word keeps its placeholder text.
//! A body with no placeholder is returned unchanged.

use super::Skill;
use std::path::Path;

/// Everything a render needs besides the template itself.
pub struct SkillRenderContext<'a> {
    pub arguments: &'a str,
    pub names: &'a [String],
    pub skill_dir: Option<&'a Path>,
    pub project_dir: Option<&'a Path>,
}

/// Split `input` into shell-style words: whitespace separated, with single or
/// double quotes grouping words and a backslash escaping inside double quotes.
pub fn split_words(input: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut current = String::new();
    let mut started = false;
    let mut quote: Option<char> = None;
    let mut chars = input.chars();
    while let Some(ch) = chars.next() {
        match quote {
            Some(q) if ch == q => quote = None,
            Some('"') if ch == '\\' => {
                if let Some(next) = chars.next() {
                    current.push(next);
                }
            }
            Some(_) => current.push(ch),
            None if ch == '"' || ch == '\'' => {
                quote = Some(ch);
                started = true;
            }
            None if ch == '\\' => {
                started = true;
                if let Some(next) = chars.next() {
                    current.push(next);
                }
            }
            None if ch.is_whitespace() => {
                if started {
                    words.push(std::mem::take(&mut current));
                    started = false;
                }
            }
            None => {
                current.push(ch);
                started = true;
            }
        }
    }
    if started {
        words.push(current);
    }
    words
}

fn path_text(path: Option<&Path>) -> String {
    path.map_or_else(String::new, |p| p.display().to_string())
}

/// Substitute placeholders in `template`.
pub fn render(template: &str, ctx: &SkillRenderContext<'_>) -> String {
    if !template.contains('$') {
        return template.to_string();
    }
    let words = split_words(ctx.arguments);
    let bytes = template.as_bytes();
    let mut out = String::with_capacity(template.len() + ctx.arguments.len());
    let mut i = 0;
    while i < bytes.len() {
        let rest = &template[i..];
        if let Some(after) = rest.strip_prefix("\\$") {
            out.push('$');
            i = template.len() - after.len();
            continue;
        }
        if !rest.starts_with('$') {
            let Some(ch) = rest.chars().next() else {
                break;
            };
            out.push(ch);
            i += ch.len_utf8();
            continue;
        }
        let (consumed, replacement) = expand_placeholder(rest, ctx, &words);
        match replacement {
            Some(text) => out.push_str(&text),
            None => out.push_str(&rest[..consumed]),
        }
        i += consumed;
    }
    out
}

/// Expand the placeholder at the start of `rest` (which begins with `$`).
/// Returns how many bytes were consumed and the replacement, or `None` to keep
/// the consumed text literally.
fn expand_placeholder(
    rest: &str,
    ctx: &SkillRenderContext<'_>,
    words: &[String],
) -> (usize, Option<String>) {
    if let Some(body) = rest.strip_prefix("${") {
        if let Some(end) = body.find('}') {
            let consumed = 2 + end + 1;
            let value = match &body[..end] {
                "JCODE_SKILL_DIR" | "CLAUDE_SKILL_DIR" => Some(path_text(ctx.skill_dir)),
                "JCODE_PROJECT_DIR" | "CLAUDE_PROJECT_DIR" => Some(path_text(ctx.project_dir)),
                _ => None,
            };
            return (consumed, value);
        }
        return (1, None);
    }
    let after = &rest[1..];
    let digits: usize = after.chars().take_while(char::is_ascii_digit).count();
    if digits > 0 {
        let word = match after[..digits].parse::<usize>() {
            Ok(n) => words.get(n).cloned(),
            Err(_) => None,
        };
        return (1 + digits, word);
    }
    let ident: usize = after
        .char_indices()
        .take_while(|(pos, ch)| {
            ch.is_ascii_alphabetic() || *ch == '_' || (*pos > 0 && ch.is_ascii_digit())
        })
        .map(|(pos, ch)| pos + ch.len_utf8())
        .last()
        .unwrap_or(0);
    if ident == 0 {
        return (1, None);
    }
    let name = &after[..ident];
    if name == "ARGUMENTS" {
        let tail = &after[ident..];
        if let Some(inner) = tail.strip_prefix('[')
            && let Some(close) = inner.find(']')
            && let Ok(n) = inner[..close].trim().parse::<usize>()
        {
            let consumed = 1 + ident + 1 + close + 1;
            return (consumed, words.get(n).cloned());
        }
        return (1 + ident, Some(ctx.arguments.to_string()));
    }
    if let Some(position) = ctx.names.iter().position(|declared| declared == name) {
        return (
            1 + ident,
            Some(words.get(position).cloned().unwrap_or_else(String::new)),
        );
    }
    (1 + ident, None)
}

impl Skill {
    /// The prompt with `$ARGUMENTS`-style placeholders substituted for a
    /// slash invocation typed with `arguments` in a session rooted at
    /// `project_dir`.
    pub fn render_prompt(&self, arguments: &str, project_dir: Option<&Path>) -> String {
        render(
            &self.get_prompt(),
            &SkillRenderContext {
                arguments,
                names: &self.arguments,
                skill_dir: self.path.parent(),
                project_dir,
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(template: &str, arguments: &str, names: &[&str]) -> String {
        let names: Vec<String> = names.iter().map(|n| (*n).to_string()).collect();
        render(
            template,
            &SkillRenderContext {
                arguments,
                names: &names,
                skill_dir: Some(Path::new("/skills/demo")),
                project_dir: Some(Path::new("/work/proj")),
            },
        )
    }

    #[test]
    fn whole_arguments_as_typed() {
        assert_eq!(
            run("Do: $ARGUMENTS.", "fix  the 'big bug'", &[]),
            "Do: fix  the 'big bug'."
        );
    }

    #[test]
    fn indexed_words_are_zero_based_and_quoted() {
        let out = run(
            "$ARGUMENTS[0]|$1|$ARGUMENTS[2]",
            "alpha \"two words\" gamma",
            &[],
        );
        assert_eq!(out, "alpha|two words|gamma");
    }

    #[test]
    fn missing_indexed_stays_literal_and_missing_named_is_empty() {
        assert_eq!(run("$3 $ARGUMENTS[7]", "a", &[]), "$3 $ARGUMENTS[7]");
        assert_eq!(
            run("[$file][$line]", "only.rs", &["file", "line"]),
            "[only.rs][]"
        );
    }

    #[test]
    fn named_arguments_bind_by_position() {
        assert_eq!(
            run("$issue in $repo", "42 jcode", &["issue", "repo"]),
            "42 in jcode"
        );
    }

    #[test]
    fn undeclared_names_stay_literal() {
        assert_eq!(run("cost $HOME $x", "a", &["name"]), "cost $HOME $x");
    }

    #[test]
    fn escape_yields_literal_dollar() {
        assert_eq!(
            run(r"price \$ARGUMENTS and $ARGUMENTS", "5", &[]),
            "price $ARGUMENTS and 5"
        );
    }

    #[test]
    fn arguments_are_never_reexpanded() {
        assert_eq!(
            run("$ARGUMENTS|$0", "$ARGUMENTS $1", &[]),
            "$ARGUMENTS $1|$ARGUMENTS"
        );
    }

    #[test]
    fn no_placeholder_is_unchanged() {
        assert_eq!(
            run("plain text, no vars", "ignored", &[]),
            "plain text, no vars"
        );
    }

    #[test]
    fn directory_variables_and_aliases() {
        assert_eq!(
            run(
                "${JCODE_SKILL_DIR} ${CLAUDE_SKILL_DIR} ${JCODE_PROJECT_DIR} ${CLAUDE_PROJECT_DIR} ${OTHER}",
                "",
                &[]
            ),
            "/skills/demo /skills/demo /work/proj /work/proj ${OTHER}"
        );
    }

    #[test]
    fn empty_arguments_clear_whole_placeholder() {
        assert_eq!(run("[$ARGUMENTS]", "", &[]), "[]");
    }

    #[test]
    fn trailing_dollar_and_unicode_are_safe() {
        assert_eq!(run("héllo $ wörld $", "x", &[]), "héllo $ wörld $");
    }

    #[test]
    fn skill_render_prompt_uses_frontmatter_names() {
        let dir = tempfile::tempdir().expect("tempdir");
        let skill_dir = dir.path().join("triage");
        std::fs::create_dir_all(&skill_dir).expect("mkdir");
        std::fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: triage\ndescription: Triage\narguments: [issue, repo]\n---\nIssue $issue in $repo (${JCODE_SKILL_DIR})\n",
        )
        .expect("write");
        let skill =
            super::super::SkillRegistry::parse_skill(&skill_dir.join("SKILL.md")).expect("parse");
        assert_eq!(skill.arguments, ["issue", "repo"]);
        let prompt = skill.render_prompt("42 jcode", None);
        assert!(prompt.contains(&format!("Issue 42 in jcode ({})", skill_dir.display())));
    }
}
