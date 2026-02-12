#[cfg(all(not(target_arch = "wasm32"), unix))]
pub(crate) mod backend;
#[cfg(all(not(target_arch = "wasm32"), not(unix)))]
pub(crate) mod backend_desktop_stub;
#[cfg(target_arch = "wasm32")]
pub(crate) mod backend_web;
pub(crate) mod model;
mod ui;

fn main() {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let window = dioxus::desktop::WindowBuilder::new().with_title("jcode");
        let cfg = dioxus::desktop::Config::new()
            .with_window(window)
            .with_menu(None::<dioxus::desktop::muda::Menu>);

        dioxus::LaunchBuilder::desktop()
            .with_cfg(cfg)
            .launch(ui::app::app);
    }

    #[cfg(target_arch = "wasm32")]
    {
        dioxus::LaunchBuilder::web().launch(ui::app::app);
    }
}
