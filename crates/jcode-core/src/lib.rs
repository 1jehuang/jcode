pub mod console;
pub mod env;
pub mod fs;
pub mod id;
pub mod output_style;
pub mod panic_util;
pub mod stdin_detect;
pub mod util;

pub fn invoked_program_name() -> &'static str {
    static NAME: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    NAME.get_or_init(|| {
        std::env::args_os()
            .next()
            .and_then(|a| {
                std::path::Path::new(&a)
                    .file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
            })
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "jcode".to_string())
    })
}
