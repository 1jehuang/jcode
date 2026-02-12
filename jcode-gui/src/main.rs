use dioxus::prelude::*;

fn main() {
    #[cfg(not(target_arch = "wasm32"))]
    {
        dioxus::LaunchBuilder::desktop().launch(app);
    }

    #[cfg(target_arch = "wasm32")]
    {
        dioxus::LaunchBuilder::web().launch(app);
    }
}

fn app() -> Element {
    rsx! {
        document::Title { "jcode GUI" }
        div {
            style: "padding: 2rem; font-family: ui-sans-serif, system-ui;",
            h1 { "jcode GUI" }
            p { "Dioxus client scaffold is in place. Next commit wires protocol + UI state." }
        }
    }
}
