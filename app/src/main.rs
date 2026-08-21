//! eCash Splitter — desktop shell.
//!
//! This layer performs **no I/O**: no network, no device, no filesystem beyond app config. All of
//! it goes through the `ecx-*` crates, off-thread, reported back as messages
//! (`CLAUDE.md` Golden Rule 8 and §10). The UI never constructs a transaction, never sets a
//! locktime, and never calls a device directly.

mod state;
mod tasks;
mod ui;

use gpui::{
    App, AppContext as _, Application, Context, Entity, IntoElement, ParentElement, Render, Styled,
    Window, WindowOptions, div, px,
};
use gpui_component::{ActiveTheme as _, Root};

use ecx_chain::ChainProfile;
use state::{ChainStatus, DiscoveryPhase, Progress, Stage};

pub struct SplitterApp {
    profile: ChainProfile,
    chain: ChainStatus,
    stage: Stage,
    error: Option<String>,
}

impl SplitterApp {
    fn new(cx: &mut Context<Self>) -> Self {
        let mut this = Self {
            profile: ChainProfile::ECX_ALPHA,
            chain: ChainStatus::Unknown,
            stage: Stage::NeedsDevice,
            error: None,
        };
        // Kick off the first tip read so the header is honest immediately.
        this.refresh_tip(cx);
        this
    }

    pub fn profile(&self) -> ChainProfile {
        self.profile
    }
    pub fn chain(&self) -> &ChainStatus {
        &self.chain
    }
    pub fn stage(&self) -> &Stage {
        &self.stage
    }
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    // -- actions ----------------------------------------------------------

    pub fn set_profile(&mut self, profile: ChainProfile, cx: &mut Context<Self>) {
        if self.profile == profile {
            return;
        }
        self.profile = profile;
        self.chain = ChainStatus::Unknown;
        // Results from another chain must not linger.
        if matches!(self.stage, Stage::Accounts { .. }) {
            if let Some(session) = self.stage.session().cloned() {
                self.stage = Stage::Connected(session);
            }
        }
        self.refresh_tip(cx);
        cx.notify();
    }

    pub fn refresh_tip(&mut self, cx: &mut Context<Self>) {
        self.chain = ChainStatus::Checking;
        let profile = self.profile;
        let (tx, rx) = tokio::sync::oneshot::channel();
        tasks::rt().spawn(async move {
            let _ = tx.send(tasks::chain_tip(profile).await);
        });
        cx.spawn(async move |this, cx| {
            let Ok(result) = rx.await else { return };
            let _ = this.update(cx, |this, cx| {
                this.chain = match result {
                    Ok((tip, readiness)) => ChainStatus::Up { tip, readiness },
                    Err(message) => ChainStatus::Down { message },
                };
                cx.notify();
            });
        })
        .detach();
    }

    pub fn connect(&mut self, cx: &mut Context<Self>) {
        self.error = None;
        self.stage = Stage::Connecting;
        cx.notify();

        let (tx, rx) = tokio::sync::oneshot::channel();
        tasks::rt().spawn(async move {
            let _ = tx.send(tasks::connect().await);
        });
        cx.spawn(async move |this, cx| {
            let Ok(result) = rx.await else { return };
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(session) => this.stage = Stage::Connected(session),
                    Err(message) => {
                        this.stage = Stage::NeedsDevice;
                        this.error = Some(message);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub fn discover(&mut self, cx: &mut Context<Self>) {
        self.error = None;
        let profile = self.profile;
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Progress>();

        tasks::rt().spawn(async move { tasks::run_discovery(profile, tx).await });

        cx.spawn(async move |this, cx| {
            while let Some(progress) = rx.recv().await {
                let ok = this
                    .update(cx, |this, cx| {
                        this.apply_progress(progress);
                        cx.notify();
                    })
                    .is_ok();
                if !ok {
                    break;
                }
            }
        })
        .detach();
    }

    fn apply_progress(&mut self, progress: Progress) {
        let session = self.stage.session().cloned();
        match progress {
            Progress::Connected(session) => {
                self.stage = Stage::Discovering {
                    session,
                    phase: DiscoveryPhase::ReadingKeys,
                    scanned: 0,
                    total: 12,
                    current: String::new(),
                };
            }
            Progress::Step {
                phase,
                scanned,
                total,
                label,
            } => {
                if let Some(session) = session {
                    self.stage = Stage::Discovering {
                        session,
                        phase,
                        scanned,
                        total,
                        current: label,
                    };
                }
            }
            Progress::Done(accounts) => {
                if let Some(session) = session {
                    self.stage = Stage::Accounts {
                        session,
                        accounts,
                        selected: None,
                    };
                }
            }
            Progress::Failed(message) => {
                self.error = Some(message);
                self.stage = session.map(Stage::Connected).unwrap_or(Stage::NeedsDevice);
            }
        }
    }

    pub fn select_account(&mut self, index: usize, cx: &mut Context<Self>) {
        if let Stage::Accounts { selected, .. } = &mut self.stage {
            *selected = Some(index);
            cx.notify();
        }
    }

    pub fn dismiss_error(&mut self, cx: &mut Context<Self>) {
        self.error = None;
        cx.notify();
    }
}

impl Render for SplitterApp {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .child(ui::header(self, cx))
            .children(ui::profile_banner(self, cx))
            .children(ui::error_banner(self, cx))
            .child(
                div()
                    .flex_1()
                    .overflow_hidden()
                    .p_6()
                    .child(ui::body(self, cx)),
            )
            .child(ui::footer(self, cx))
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
                    gpui::size(px(940.0), px(720.0)),
                    cx,
                ))),
                ..Default::default()
            };

            cx.open_window(options, |window, cx| {
                let view: Entity<SplitterApp> = cx.new(SplitterApp::new);
                // Root is required as the first-level view: it enables dialogs and notifications.
                cx.new(|cx| Root::new(view, window, cx))
            })
            .expect("failed to open window");
        });
}
