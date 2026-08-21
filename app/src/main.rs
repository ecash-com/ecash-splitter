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
use state::{
    BuildOutcome, ChainStatus, DestinationChoice, DestinationOutcome, DiscoveryPhase, Progress,
    SignOutcome, Stage,
};

pub struct SplitterApp {
    profile: ChainProfile,
    chain: ChainStatus,
    stage: Stage,
    error: Option<String>,
    /// Text field for a pasted destination address.
    address_input: Entity<gpui_component::input::InputState>,
    /// Editable Esplora endpoint. These hosts move every phase, so a fixed menu is not enough.
    endpoint_input: Entity<gpui_component::input::InputState>,
    /// How far to look. An account created beyond the default range is otherwise invisible.
    depth: ecx_wallet::DiscoveryDepth,
}

impl SplitterApp {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let address_input = cx.new(|cx| {
            gpui_component::input::InputState::new(window, cx)
                .placeholder("bc1… — an ECX address you control")
        });
        let default_profile = ChainProfile::ecx_alpha();
        let endpoint_input = cx.new(|cx| {
            gpui_component::input::InputState::new(window, cx)
                .placeholder("https://host/api")
                .default_value(default_profile.esplora_url.to_string())
        });
        let mut this = Self {
            profile: default_profile,
            chain: ChainStatus::Unknown,
            stage: Stage::NeedsDevice,
            error: None,
            address_input,
            endpoint_input,
            depth: ecx_wallet::DiscoveryDepth::default(),
        };
        // Kick off the first tip read so the header is honest immediately.
        this.refresh_tip(cx);
        this
    }

    pub fn profile(&self) -> &ChainProfile {
        &self.profile
    }
    pub fn endpoint_input(&self) -> &Entity<gpui_component::input::InputState> {
        &self.endpoint_input
    }
    pub fn depth(&self) -> ecx_wallet::DiscoveryDepth {
        self.depth
    }

    /// How many account indices to probe per address type.
    pub fn set_accounts_probed(&mut self, accounts: u32, cx: &mut Context<Self>) {
        self.depth.accounts = accounts.max(1);
        cx.notify();
    }

    /// Consecutive unused addresses that end a scan within one account.
    pub fn set_stop_gap(&mut self, stop_gap: usize, cx: &mut Context<Self>) {
        self.depth.stop_gap = stop_gap.max(1);
        cx.notify();
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
    pub fn address_input(&self) -> &Entity<gpui_component::input::InputState> {
        &self.address_input
    }

    // -- actions ----------------------------------------------------------

    pub fn set_profile(
        &mut self,
        profile: ChainProfile,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.profile == profile {
            return;
        }
        // Keep the endpoint field showing whatever the app is actually pointed at.
        let url = profile.esplora_url.to_string();
        self.endpoint_input
            .update(cx, |state, cx| state.set_value(url, window, cx));
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

    /// Point the app at whatever URL is in the endpoint field.
    pub fn use_typed_endpoint(&mut self, cx: &mut Context<Self>) {
        let url = self.endpoint_input.read(cx).value().trim().to_string();
        if url.is_empty() {
            return;
        }
        let profile = ChainProfile::custom(&url);
        if profile == self.profile {
            // Same endpoint — treat the button as a retry.
            self.refresh_tip(cx);
            return;
        }
        self.profile = profile;
        self.chain = ChainStatus::Unknown;
        self.error = None;
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
        let profile = self.profile.clone();
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
        let profile = self.profile.clone();
        let depth = self.depth;
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Progress>();

        tasks::rt().spawn(async move { tasks::run_discovery(profile, depth, tx).await });

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
                    total: self.depth.candidate_count(),
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

    /// Step 4 → 5: the user picked an account; go choose where the coins land.
    pub fn confirm_account(&mut self, cx: &mut Context<Self>) {
        let Stage::Accounts {
            session,
            accounts,
            selected: Some(i),
        } = &self.stage
        else {
            return;
        };
        let Some(account) = accounts.get(*i).cloned() else {
            return;
        };
        let session = session.clone();
        self.error = None;
        // Pasting is the default. Most people splitting want the coins in a *different* wallet
        // -- a dedicated ECX wallet -- not a second account on the same seed. And until Ledger
        // wallet-policy registration lands, a device-derived address cannot be verified on the
        // device screen, so it is the *less* checkable of the two: an address pasted out of your
        // own ECX wallet is one you can verify there. (Revised 2026-08-21; CLAUDE.md §7.5.)
        self.stage = Stage::ChoosingDestination {
            session,
            account: Box::new(account),
            choice: DestinationChoice::Pasted {
                parsed: None,
                acknowledged: false,
            },
        };
        cx.notify();
    }

    /// Read the ECX destination account's xpub from the device and derive its first address.
    pub fn derive_device_destination(&mut self, cx: &mut Context<Self>) {
        let (tx, rx) = tokio::sync::oneshot::channel();
        tasks::rt().spawn(async move {
            let outcome = match tasks::derive_destination().await {
                Ok((address, path)) => DestinationOutcome::Derived { address, path },
                Err(message) => DestinationOutcome::Failed(message),
            };
            let _ = tx.send(outcome);
        });
        cx.spawn(async move |this, cx| {
            let Ok(outcome) = rx.await else { return };
            let _ = this.update(cx, |this, cx| {
                if let Stage::ChoosingDestination { choice, .. } = &mut this.stage {
                    match outcome {
                        DestinationOutcome::Derived { address, path } => {
                            *choice = DestinationChoice::Device { address, path };
                        }
                        DestinationOutcome::Failed(message) => {
                            this.error = Some(message);
                            *choice = DestinationChoice::Pasted {
                                parsed: None,
                                acknowledged: false,
                            };
                        }
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Parse whatever is in the address field and switch to the pasted path.
    pub fn use_pasted_address(&mut self, cx: &mut Context<Self>) {
        let text = self.address_input.read(cx).value().trim().to_string();
        let parsed = text
            .parse::<bitcoin::Address<bitcoin::address::NetworkUnchecked>>()
            .ok()
            .and_then(|a| a.require_network(bitcoin::Network::Bitcoin).ok());
        if parsed.is_none() {
            self.error = Some(format!("\"{text}\" is not a valid address"));
        } else {
            self.error = None;
        }
        if let Stage::ChoosingDestination { choice, .. } = &mut self.stage {
            *choice = DestinationChoice::Pasted {
                parsed,
                acknowledged: false,
            };
        }
        cx.notify();
    }

    /// The typed acknowledgement §7.5 requires before a pasted address can be used.
    pub fn toggle_acknowledgement(&mut self, cx: &mut Context<Self>) {
        if let Stage::ChoosingDestination {
            choice: DestinationChoice::Pasted { acknowledged, .. },
            ..
        } = &mut self.stage
        {
            *acknowledged = !*acknowledged;
            cx.notify();
        }
    }

    pub fn use_device_destination(&mut self, cx: &mut Context<Self>) {
        self.error = None;
        if let Stage::ChoosingDestination { choice, .. } = &mut self.stage {
            *choice = DestinationChoice::Pending;
        }
        cx.notify();
        self.derive_device_destination(cx);
    }

    /// Step 5 → 6: build the sweep PSBT and show the review screen.
    pub fn build_sweep(&mut self, cx: &mut Context<Self>) {
        let Stage::ChoosingDestination {
            session,
            account,
            choice,
        } = &self.stage
        else {
            return;
        };
        let Some(destination) = choice.address().cloned() else {
            return;
        };
        let (session, account) = (session.clone(), account.clone());
        let profile = self.profile.clone();
        let fingerprint = session.fingerprint;

        self.error = None;
        self.stage = Stage::Building {
            session: session.clone(),
            account: account.clone(),
        };
        cx.notify();

        let account_for_task = (*account).clone();
        let (tx, rx) = tokio::sync::oneshot::channel();
        tasks::rt().spawn(async move {
            let outcome = match tasks::build_sweep_summary(
                profile,
                account_for_task,
                destination,
                fingerprint,
            )
            .await
            {
                Ok(built) => BuildOutcome::Ready(Box::new(built)),
                Err(message) => BuildOutcome::Failed(message),
            };
            let _ = tx.send(outcome);
        });

        cx.spawn(async move |this, cx| {
            let Ok(outcome) = rx.await else { return };
            let _ = this.update(cx, |this, cx| {
                match outcome {
                    BuildOutcome::Ready(built) => {
                        this.stage = Stage::Review {
                            session,
                            account,
                            built,
                        };
                    }
                    BuildOutcome::Failed(message) => {
                        this.error = Some(message);
                        this.stage = Stage::ChoosingDestination {
                            session,
                            account,
                            choice: DestinationChoice::Pending,
                        };
                        this.derive_device_destination(cx);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Step 7: sign on the device, then re-verify what it returned (Golden Rule 3).
    ///
    /// Signing pre-fork is harmless — the transaction carries a locktime Bitcoin will never
    /// accept — so this runs today. Broadcasting does not.
    pub fn sign(&mut self, cx: &mut Context<Self>) {
        let Stage::Review {
            session,
            account,
            built,
        } = &self.stage
        else {
            return;
        };
        let (session, account, built) = (session.clone(), account.clone(), built.clone());
        let kind = session.kind;
        let policy = account.ledger_policy();

        self.error = None;
        self.stage = Stage::Signing {
            session: session.clone(),
            account: account.clone(),
            built: built.clone(),
        };
        cx.notify();

        let payload = (*built).clone();
        let (tx, rx) = tokio::sync::oneshot::channel();
        tasks::rt().spawn(async move {
            let outcome = match tasks::sign_reviewed(kind, policy, payload).await {
                Ok((txid, raw_hex)) => SignOutcome::Verified { txid, raw_hex },
                Err(message) => SignOutcome::Failed(message),
            };
            let _ = tx.send(outcome);
        });

        cx.spawn(async move |this, cx| {
            let Ok(outcome) = rx.await else { return };
            let _ = this.update(cx, |this, cx| {
                match outcome {
                    SignOutcome::Verified { txid, raw_hex } => {
                        this.stage = Stage::Signed {
                            session,
                            account,
                            built,
                            txid,
                            raw_hex,
                        };
                    }
                    SignOutcome::Failed(message) => {
                        this.error = Some(message);
                        this.stage = Stage::Review {
                            session,
                            account,
                            built,
                        };
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Back out of the destination or review step, to pick a different account.
    pub fn back_to_accounts(&mut self, cx: &mut Context<Self>) {
        let session = self.stage.session().cloned();
        self.error = None;
        if let Some(session) = session {
            self.stage = Stage::Connected(session);
        }
        cx.notify();
        self.discover(cx);
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
                    gpui::size(px(960.0), px(760.0)),
                    cx,
                ))),
                // Below this the review screen's figures start colliding; there is no useful
                // layout for a wallet-sized window here.
                window_min_size: Some(gpui::size(px(720.0), px(560.0))),
                ..Default::default()
            };

            // macOS keeps an app alive after its last window closes, which for a single-window
            // tool reads as "it didn't quit" — the user closes it and has to right-click the
            // Dock icon to actually exit. There is nothing to come back to here, so quit.
            cx.on_window_closed(|cx| {
                if cx.windows().is_empty() {
                    cx.quit();
                }
            })
            .detach();

            cx.open_window(options, |window, cx| {
                let view: Entity<SplitterApp> = cx.new(|cx| SplitterApp::new(window, cx));
                // Root is required as the first-level view: it enables dialogs and notifications.
                cx.new(|cx| Root::new(view, window, cx))
            })
            .expect("failed to open window");
        });
}
