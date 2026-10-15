pub const QUIET_ENV: &str = "JCODE_QUIET";

pub fn set_quiet_enabled(enabled: bool) {
    if enabled {
        crate::env::set_var(QUIET_ENV, "1");
    } else {
        crate::env::remove_var(QUIET_ENV);
    }
}

pub fn quiet_enabled() -> bool {
    std::env::var(QUIET_ENV)
        .map(|value| value == "1" || value.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

pub fn stderr_info(message: impl AsRef<str>) {
    if !quiet_enabled() {
        eprintln!("{}", crate::output_style::terminal_text(message.as_ref()));
    }
}

pub fn terminal_title(title: impl AsRef<str>) -> String {
    crate::output_style::terminal_text(title.as_ref()).into_owned()
}

pub fn stderr_blank_line() {
    if !quiet_enabled() {
        eprintln!();
    }
}

/// Print each line centered to the terminal width (CLI update/restart banners).
pub fn print_centered(msg: &str) {
    let msg = crate::output_style::terminal_text(msg);
    let width = crossterm::terminal::size()
        .map(|(w, _)| w as usize)
        .unwrap_or(80);
    for line in msg.lines() {
        let visible_len = unicode_display_width(line);
        if visible_len >= width {
            println!("{}", line);
        } else {
            let pad = (width - visible_len) / 2;
            println!("{:>pad$}{}", "", line, pad = pad);
        }
    }
}

fn unicode_display_width(s: &str) -> usize {
    use unicode_width::UnicodeWidthChar;
    let mut w = 0;
    let mut in_escape = false;
    for c in s.chars() {
        if in_escape {
            if c == 'm' {
                in_escape = false;
            }
            continue;
        }
        if c == '\x1b' {
            in_escape = true;
            continue;
        }
        w += UnicodeWidthChar::width(c).unwrap_or(0);
    }
    w
}
