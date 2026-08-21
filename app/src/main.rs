//! eCash Splitter — desktop shell.
//!
//! This layer performs **no I/O**: no network, no device, no filesystem beyond app config. All of
//! it goes through the `ecx-*` crates behind channels (`CLAUDE.md` Golden Rule 8). The UI never
//! constructs a transaction, never sets a locktime, and never calls a device directly.

use gpui::{
    App, AppContext as _, Application, Context, IntoElement, ParentElement, Render, Styled, Window,
    WindowOptions, div, prelude::FluentBuilder as _, px,
};
use gpui_component::{ActiveTheme as _, Root};

use ecx_chain::ScanReadiness;
use ecx_core::{ECASH_HEIGHT, Phase};

/// Placeholder for the §7 flow. Real screens replace this: connect → discover → select →
/// destination → review → sign → broadcast.
struct SplitterApp {
    /// Tip of the ECX chain source. `None` until the first sync report arrives.
    tip: Option<u32>,
}

impl SplitterApp {
    fn new() -> Self {
        Self { tip: None }
    }

    fn readiness(&self) -> Option<ScanReadiness> {
        self.tip.map(ScanReadiness::at_tip)
    }
}

impl Render for SplitterApp {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let readiness = self.readiness();

        div()
            .size_full()
            .flex()
            .flex_col()
            .gap_4()
            .p_8()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .child(div().text_2xl().child("eCash Splitter"))
            .child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child("Split BTC to ECX from a hardware wallet."),
            )
            // Golden Rule 9: never state a balance from a chain source below the fork height.
            .child(match readiness {
                None => div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child("Connecting to an ECX chain source…"),
                Some(ScanReadiness::Syncing { tip, target }) => div()
                    .text_sm()
                    .text_color(cx.theme().warning)
                    .child(format!(
                        "Indexer syncing — {tip} / {target}. No balance can be shown yet."
                    )),
                Some(ScanReadiness::Ready) => div()
                    .text_sm()
                    .child(format!("Ready. Fork height {ECASH_HEIGHT}.")),
            })
            .when(cfg!(debug_assertions), |this| {
                this.child(
                    div()
                        .mt_4()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(format!(
                            "phase at fork height: {:?}",
                            Phase::at_height(ECASH_HEIGHT)
                        )),
                )
            })
    }
}

fn main() {
    Application::new()
        .with_assets(gpui_component_assets::Assets)
        .run(|cx: &mut App| {
            // MUST be first (gpui-component skill, §4).
            gpui_component::init(cx);

            let options = WindowOptions {
                window_bounds: Some(gpui::WindowBounds::Windowed(gpui::Bounds::centered(
                    None,
                    gpui::size(px(880.0), px(640.0)),
                    cx,
                ))),
                ..Default::default()
            };

            cx.open_window(options, |window, cx| {
                let view = cx.new(|_| SplitterApp::new());
                // Root is required as the first-level view: it enables dialogs and notifications.
                cx.new(|cx| Root::new(view, window, cx))
            })
            .expect("failed to open window");
        });
}
