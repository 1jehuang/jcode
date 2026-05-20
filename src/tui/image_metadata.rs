pub(super) fn compact_path_label(path: &str) -> String {
    let trimmed = path.trim();
    std::path::Path::new(trimmed)
        .file_name()
        .and_then(|value| value.to_str())
        .filter(|value| !value.trim().is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| trimmed.to_string())
}

pub(super) fn compact_image_format(value: &str) -> String {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return "image".to_string();
    }

    let lower = trimmed
        .strip_prefix("image/")
        .unwrap_or(trimmed)
        .to_ascii_lowercase();
    match lower.as_str() {
        "jpg" | "jpeg" => "JPEG".to_string(),
        "png" => "PNG".to_string(),
        "webp" => "WebP".to_string(),
        "gif" => "GIF".to_string(),
        "bmp" => "BMP".to_string(),
        "ico" | "x-icon" => "ICO".to_string(),
        "svg" | "svg+xml" => "SVG".to_string(),
        _ => trimmed.to_string(),
    }
}

pub(super) fn format_dimensions(width: u32, height: u32) -> String {
    format!("{width}×{height}")
}

pub(super) fn aspect_ratio(width: u32, height: u32) -> Option<String> {
    if width == 0 || height == 0 {
        return None;
    }
    let divisor = gcd(width, height).max(1);
    Some(format!("{}:{}", width / divisor, height / divisor))
}

pub(super) fn format_byte_count(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    const GB: f64 = MB * 1024.0;

    if bytes == 1 {
        return "1 B".to_string();
    }
    if bytes < 1024 {
        return format!("{bytes} B");
    }

    let (value, unit) = if (bytes as f64) >= GB {
        (bytes as f64 / GB, "GB")
    } else if (bytes as f64) >= MB {
        (bytes as f64 / MB, "MB")
    } else {
        (bytes as f64 / KB, "KB")
    };
    let mut formatted = if value >= 100.0 {
        format!("{value:.0}")
    } else {
        format!("{value:.1}")
    };
    if formatted.ends_with(".0") {
        formatted.truncate(formatted.len().saturating_sub(2));
    }
    format!("{formatted} {unit}")
}

pub(super) fn estimate_base64_decoded_len(data: &str) -> Option<u64> {
    let trimmed = data.trim();
    let payload = if trimmed.starts_with("data:") {
        trimmed.split_once(',').map(|(_, payload)| payload)?
    } else {
        trimmed
    };

    let mut len = 0u64;
    let mut padding = 0u64;
    for ch in payload.chars() {
        if ch.is_whitespace() {
            continue;
        }
        len += 1;
        if ch == '=' {
            padding += 1;
        }
    }

    if len == 0 || len % 4 == 1 || padding > 2 {
        return None;
    }

    Some(((len * 3) / 4).saturating_sub(padding))
}

fn gcd(mut a: u32, mut b: u32) -> u32 {
    while b != 0 {
        let next = a % b;
        a = b;
        b = next;
    }
    a
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_byte_count_uses_compact_units() {
        assert_eq!(format_byte_count(1), "1 B");
        assert_eq!(format_byte_count(1536), "1.5 KB");
        assert_eq!(format_byte_count(2 * 1024 * 1024), "2 MB");
    }

    #[test]
    fn estimate_base64_decoded_len_handles_common_payloads() {
        assert_eq!(estimate_base64_decoded_len("TQ=="), Some(1));
        assert_eq!(estimate_base64_decoded_len("TWE="), Some(2));
        assert_eq!(estimate_base64_decoded_len("TWFu"), Some(3));
        assert_eq!(
            estimate_base64_decoded_len("data:image/png;base64,TWFu"),
            Some(3)
        );
    }

    #[test]
    fn aspect_ratio_reduces_dimensions() {
        assert_eq!(aspect_ratio(1024, 1024).as_deref(), Some("1:1"));
        assert_eq!(aspect_ratio(1792, 1024).as_deref(), Some("7:4"));
    }
}
