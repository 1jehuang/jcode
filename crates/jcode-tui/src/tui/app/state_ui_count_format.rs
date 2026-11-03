pub(super) fn cache_ratio_pct(numerator: u64, denominator: u64) -> u8 {
    if denominator == 0 {
        0
    } else {
        ((numerator as f64 / denominator as f64) * 100.0)
            .round()
            .clamp(0.0, 100.0) as u8
    }
}

pub(super) fn grouped_u64(value: u64) -> String {
    let raw = value.to_string();
    let mut grouped = String::with_capacity(raw.len() + raw.len() / 3);
    for (index, ch) in raw.chars().enumerate() {
        if index > 0 && (raw.len() - index).is_multiple_of(3) {
            grouped.push(',');
        }
        grouped.push(ch);
    }
    grouped
}

pub(super) fn trim_decimal_zeros(mut value: String) -> String {
    if value.contains('.') {
        while value.ends_with('0') {
            value.pop();
        }
        if value.ends_with('.') {
            value.pop();
        }
    }
    value
}

pub(super) fn compact_count(value: u64) -> String {
    let Some((unit, suffix)) = [(1_000_000_000_u64, "b"), (1_000_000, "m"), (1_000, "k")]
        .into_iter()
        .find(|(unit, _)| value >= *unit)
    else {
        return value.to_string();
    };

    let scaled = value as f64 / unit as f64;
    let decimals = if scaled >= 10.0 { 1 } else { 2 };
    format!(
        "{}{}",
        trim_decimal_zeros(format!("{scaled:.decimals$}")),
        suffix
    )
}

pub(super) fn human_count(value: u64) -> String {
    if value < 1_000 {
        value.to_string()
    } else {
        format!("{} ({})", compact_count(value), grouped_u64(value))
    }
}

pub(super) fn bold_count(value: u64) -> String {
    human_count(value).to_string()
}

pub(super) fn bold_count_usize(value: usize) -> String {
    bold_count(value as u64)
}

pub(super) fn opt_u64(value: Option<u64>) -> String {
    value.map(human_count).unwrap_or_else(|| "None".to_string())
}

pub(super) fn opt_usize(value: Option<usize>) -> String {
    value
        .map(|value| value.to_string())
        .unwrap_or_else(|| "None".to_string())
}

pub(super) fn opt_string(value: Option<&str>) -> String {
    value
        .map(|value| value.to_string())
        .unwrap_or_else(|| "None".to_string())
}
