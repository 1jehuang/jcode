//! Last-line secret scanning for model-authored Discovery text.
//!
//! `discover_tools` sends a model-written `query` and `reason` to a hosted
//! endpoint, so the client refuses anything that recognizably contains a
//! credential or personal identifier. This is a high-confidence backstop, not a
//! substitute for the schema instruction to summarize the need rather than
//! copy user data: it should almost never fire in normal use.

/// A deliberately high-confidence last-line defense before model-authored
/// Discovery text leaves the client. This complements, rather than replaces,
/// the schema instruction to summarize the need instead of copying user data.
pub(super) fn contains_recognizable_secret(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    if (lower.contains("-----begin ") && lower.contains("private key-----"))
        || contains_credential_assignment(&lower)
        || contains_email_address(value)
        || contains_ssn(value)
        || contains_credential_url(value)
        || contains_international_phone_number(value)
    {
        return true;
    }

    if contains_prefixed_secret(value) || contains_payment_card_sequence(value) {
        return true;
    }

    value.split_whitespace().any(|token| {
        let token = token.trim_matches(|c: char| {
            matches!(
                c,
                '"' | '\'' | '`' | '(' | ')' | '[' | ']' | '{' | '}' | ',' | ';'
            )
        });
        looks_like_jwt(token)
    }) || contains_bearer_token(&lower)
}

fn contains_prefixed_secret(value: &str) -> bool {
    const SECRET_PREFIXES: &[&str] = &[
        "sk_live_",
        "rk_live_",
        "sk_test_",
        "rk_test_",
        "sk-proj-",
        "ghp_",
        "gho_",
        "ghu_",
        "ghs_",
        "github_pat_",
        "xoxb-",
        "xoxp-",
        "xoxa-",
        "xoxr-",
        "npm_",
        "jck_live_",
    ];
    value.split_whitespace().any(|token| {
        let token = token.trim_matches(|c: char| !c.is_ascii_alphanumeric() && !"_-".contains(c));
        let lower = token.to_ascii_lowercase();
        SECRET_PREFIXES
            .iter()
            .any(|prefix| lower.starts_with(prefix) && token.len() >= prefix.len() + 8)
            || (token.starts_with("AKIA") && token.len() == 20)
            || (token.starts_with("AIza") && token.len() >= 35)
    })
}

fn contains_credential_assignment(lower: &str) -> bool {
    const LABELS: &[&str] = &[
        "api_key",
        "api-key",
        "apikey",
        "access_token",
        "auth_token",
        "client_secret",
        "secret_key",
        "password",
        "passwd",
    ];
    LABELS.iter().any(|label| {
        lower.match_indices(label).any(|(index, _)| {
            let rest = &lower[index + label.len()..];
            let rest = rest.trim_start();
            let Some(rest) = rest.strip_prefix(['=', ':']) else {
                return false;
            };
            let candidate =
                rest.trim_start_matches(|c: char| c.is_whitespace() || "'\"`".contains(c));
            candidate
                .split(|c: char| c.is_whitespace() || "'\"`,;".contains(c))
                .next()
                .is_some_and(|token| token.len() >= 8)
        })
    })
}

fn contains_bearer_token(lower: &str) -> bool {
    lower.match_indices("bearer ").any(|(index, _)| {
        lower[index + "bearer ".len()..]
            .split_whitespace()
            .next()
            .is_some_and(|token| token.trim_matches(|c: char| ",;.'\"`".contains(c)).len() >= 12)
    })
}

fn contains_email_address(value: &str) -> bool {
    value.split_whitespace().any(|token| {
        let token = token.trim_matches(|c: char| ",;:()[]{}<>\"'`".contains(c));
        let Some((local, domain)) = token.split_once('@') else {
            return false;
        };
        !local.is_empty()
            && domain
                .rsplit_once('.')
                .is_some_and(|(host, suffix)| !host.is_empty() && suffix.len() >= 2)
    })
}

fn contains_ssn(value: &str) -> bool {
    value.split_whitespace().any(|token| {
        let token = token.trim_matches(|c: char| !c.is_ascii_digit() && c != '-');
        let parts: Vec<&str> = token.split('-').collect();
        parts.len() == 3
            && parts[0].len() == 3
            && parts[1].len() == 2
            && parts[2].len() == 4
            && parts
                .iter()
                .all(|part| part.chars().all(|c| c.is_ascii_digit()))
    })
}

fn contains_credential_url(value: &str) -> bool {
    value.split_whitespace().any(|token| {
        let Some((_, rest)) = token.split_once("://") else {
            return false;
        };
        let authority = rest.split('/').next().unwrap_or_default();
        authority.contains('@')
            && authority
                .split('@')
                .next()
                .is_some_and(|user| user.contains(':'))
    })
}

fn contains_international_phone_number(value: &str) -> bool {
    value.split_whitespace().any(|token| {
        if !token.starts_with('+') {
            return false;
        }
        let digits = token.chars().filter(|c| c.is_ascii_digit()).count();
        (10..=15).contains(&digits)
            && token
                .chars()
                .all(|c| c.is_ascii_digit() || "+-().".contains(c))
    })
}

fn looks_like_jwt(token: &str) -> bool {
    token.len() >= 40 && token.starts_with("eyJ") && token.matches('.').count() == 2
}

fn contains_payment_card_sequence(value: &str) -> bool {
    value
        .split(|c: char| !c.is_ascii_digit() && c != '-' && c != ' ')
        .any(|candidate| looks_like_payment_card(candidate.trim()))
}

fn looks_like_payment_card(candidate: &str) -> bool {
    let digits: String = candidate.chars().filter(|c| c.is_ascii_digit()).collect();
    if !(13..=19).contains(&digits.len())
        || candidate
            .chars()
            .any(|c| !c.is_ascii_digit() && c != '-' && c != ' ')
    {
        return false;
    }
    let mut sum = 0u32;
    let parity = digits.len() % 2;
    for (index, byte) in digits.bytes().enumerate() {
        let mut digit = u32::from(byte - b'0');
        if index % 2 == parity {
            digit *= 2;
            if digit > 9 {
                digit -= 9;
            }
        }
        sum += digit;
    }
    sum.is_multiple_of(10)
}
