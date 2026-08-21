//! Rendering. Pure presentation — every value shown here was produced by an `ecx-*` crate.

use bitcoin::Amount;
use gpui::{
    AnyElement, App, Context, InteractiveElement as _, IntoElement, ParentElement, SharedString,
    StatefulInteractiveElement as _, Styled, div, prelude::FluentBuilder as _, px, relative,
};
use gpui_component::scroll::ScrollableElement as _;
use gpui_component::{
    ActiveTheme as _, Disableable as _, Icon, IconName, Sizable as _, button::Button,
    button::ButtonVariants as _, spinner::Spinner,
};

use ecx_chain::{ChainProfile, ProfileKind, ScanReadiness};
use ecx_core::{ALPHA_HEIGHT, ECASH_HEIGHT, Phase};
use ecx_wallet::{DiscoveredAccount, SweepSummary};

use crate::SplitterApp;
use crate::state::{ChainStatus, DestinationChoice, DiscoveryPhase, Stage, profile_notice};

/// Ticker for the selected chain. Small thing, but showing "BTC" while scanning Bitcoin and
/// "ECX" while scanning ECX removes a real source of confusion (§10).
fn unit(profile: &ChainProfile) -> &'static str {
    match profile.kind {
        ProfileKind::Ecx | ProfileKind::Custom => "ECX",
        ProfileKind::BitcoinReadOnly => "BTC",
    }
}

fn amount(value: Amount, profile: &ChainProfile) -> String {
    format!("{:.8} {}", value.to_btc(), unit(profile))
}

/// "3 hours", "2 days" — matches the phrasing `ecx-wallet` uses in its errors.
fn humanize(secs: u64) -> String {
    match secs {
        s if s < 120 => format!("{s} seconds"),
        s if s < 7_200 => format!("{} minutes", s / 60),
        s if s < 172_800 => format!("{} hours", s / 3_600),
        s => format!("{} days", s / 86_400),
    }
}

fn thousands(n: u32) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

// ---------------------------------------------------------------------------
// Header
// ---------------------------------------------------------------------------

pub fn header(app: &SplitterApp, cx: &mut Context<SplitterApp>) -> AnyElement {
    let active = app.profile();

    div()
        .flex()
        .flex_col()
        .w_full()
        .px_6()
        .pt_5()
        .pb_4()
        .gap_4()
        .border_b_1()
        .border_color(cx.theme().border)
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_3()
                        .child(
                            div()
                                .text_xl()
                                .font_weight(gpui::FontWeight::SEMIBOLD)
                                .child("eCash Splitter"),
                        )
                        .child(chain_chip(app, cx)),
                )
                .children(app.stage().session().map(|session| {
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .px_3()
                        .py_1()
                        .rounded_md()
                        .bg(cx.theme().secondary)
                        .child(
                            Icon::new(IconName::CircleCheck)
                                .small()
                                .text_color(cx.theme().success),
                        )
                        .child(div().text_xs().child(SharedString::from(format!(
                            "{} · {}",
                            session.label, session.fingerprint
                        ))))
                        .into_any_element()
                })),
        )
        .child(
            div()
                .w_full()
                .flex()
                .items_center()
                .gap_2()
                .children(
                    ChainProfile::presets()
                        .into_iter()
                        .enumerate()
                        .map(|(i, p)| {
                            let selected = p == *active;
                            let button =
                                Button::new(("preset", i)).label(p.name.to_string()).small();
                            let button = if selected {
                                button.primary()
                            } else {
                                button.outline()
                            };
                            button
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.set_profile(p.clone(), window, cx)
                                }))
                                .into_any_element()
                        }),
                )
                // These hosts move every phase (§6), so the URL is editable rather than a fixed
                // menu. Anything typed here is still gated by the fork probe before broadcast.
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.0))
                        .child(gpui_component::input::Input::new(app.endpoint_input()).small()),
                )
                .child(
                    Button::new("use-endpoint")
                        .outline()
                        .small()
                        .label("Use")
                        .on_click(cx.listener(|this, _, _window, cx| this.use_typed_endpoint(cx))),
                ),
        )
        .into_any_element()
}

/// The sync gate, rendered. Golden Rule 9 lives here as much as in `ecx-chain`.
fn chain_chip(app: &SplitterApp, cx: &App) -> AnyElement {
    // Long messages get clipped rather than shoving the rest of the header off-screen.
    let chip = |inner: AnyElement, color: gpui::Hsla| {
        div()
            .flex()
            .items_center()
            .gap_1p5()
            .px_2p5()
            .py_1()
            .max_w(px(420.0))
            .overflow_hidden()
            .rounded_md()
            .bg(cx.theme().secondary)
            .text_xs()
            .text_color(color)
            .child(inner)
            .into_any_element()
    };

    match app.chain() {
        ChainStatus::Unknown | ChainStatus::Checking => chip(
            div()
                .flex()
                .items_center()
                .gap_1p5()
                .child(Spinner::new().xsmall().color(cx.theme().muted_foreground))
                .child("checking chain…")
                .into_any_element(),
            cx.theme().muted_foreground,
        ),
        ChainStatus::Down { message } => chip(
            div()
                .flex()
                .items_center()
                .gap_1p5()
                .child(Icon::new(IconName::CircleX).xsmall())
                .child(SharedString::from(message.clone()))
                .into_any_element(),
            cx.theme().danger,
        ),
        ChainStatus::Up {
            readiness: ScanReadiness::Behind { tip, .. },
            ..
        } => chip(
            div()
                .flex()
                .items_center()
                .gap_1p5()
                .child(Icon::new(IconName::TriangleAlert).xsmall())
                .child(SharedString::from(format!(
                    "behind — block {}",
                    thousands(*tip)
                )))
                .into_any_element(),
            cx.theme().warning,
        ),
        ChainStatus::Up { tip, .. } => chip(
            div()
                .flex()
                .items_center()
                .gap_1p5()
                .child(Icon::new(IconName::CircleCheck).xsmall())
                .child(SharedString::from(format!(
                    "synced · block {}",
                    thousands(tip.height)
                )))
                .into_any_element(),
            cx.theme().success,
        ),
    }
}

// ---------------------------------------------------------------------------
// Banners
// ---------------------------------------------------------------------------

fn banner(
    cx: &App,
    icon: IconName,
    accent: gpui::Hsla,
    title: impl Into<SharedString>,
    body: impl Into<SharedString>,
) -> AnyElement {
    div()
        .flex()
        .gap_3()
        .mx_6()
        .mt_4()
        .p_3()
        .rounded_lg()
        .border_1()
        .border_color(accent.opacity(0.35))
        .bg(accent.opacity(0.08))
        .child(Icon::new(icon).small().text_color(accent).flex_shrink_0())
        .child(
            div()
                // Without an explicit min width a flex child refuses to shrink below its
                // content, so long copy runs off the right edge instead of wrapping.
                .flex_1()
                .min_w(px(0.0))
                .flex()
                .flex_col()
                .gap_0p5()
                .child(
                    div()
                        .text_sm()
                        .font_weight(gpui::FontWeight::MEDIUM)
                        .text_color(accent)
                        .child(title.into()),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(body.into()),
                ),
        )
        .into_any_element()
}

pub fn profile_banner(app: &SplitterApp, cx: &App) -> Option<AnyElement> {
    profile_notice(app.profile()).map(|notice| {
        banner(
            cx,
            IconName::Info,
            cx.theme().info,
            "Bitcoin, not ECX",
            notice,
        )
    })
}

pub fn error_banner(app: &SplitterApp, cx: &mut Context<SplitterApp>) -> Option<AnyElement> {
    let message = app.error()?.to_string();
    Some(
        div()
            .flex()
            .items_start()
            .gap_3()
            .mx_6()
            .mt_4()
            .p_3()
            .rounded_lg()
            .border_1()
            .border_color(cx.theme().danger.opacity(0.35))
            .bg(cx.theme().danger.opacity(0.08))
            .child(
                Icon::new(IconName::TriangleAlert)
                    .small()
                    .text_color(cx.theme().danger)
                    .flex_shrink_0(),
            )
            .child(
                div()
                    .flex_1()
                    .flex()
                    .flex_col()
                    .gap_0p5()
                    .child(
                        div()
                            .text_sm()
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .text_color(cx.theme().danger)
                            .child("Something went wrong"),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(SharedString::from(message)),
                    ),
            )
            .child(
                Button::new("dismiss")
                    .ghost()
                    .xsmall()
                    .icon(IconName::Close)
                    .on_click(cx.listener(|this, _, _window, cx| this.dismiss_error(cx))),
            )
            .into_any_element(),
    )
}

// ---------------------------------------------------------------------------
// Body
// ---------------------------------------------------------------------------

pub fn body(app: &SplitterApp, cx: &mut Context<SplitterApp>) -> AnyElement {
    match app.stage() {
        Stage::NeedsDevice => connect_card(cx),
        Stage::Connecting => busy_card(
            cx,
            "Connecting…",
            "Unlock your Ledger and open the Bitcoin app.",
        ),
        Stage::Connected(_) => ready_card(app, cx),
        Stage::Discovering {
            phase,
            scanned,
            total,
            current,
            ..
        } => discovering_card(cx, *phase, *scanned, *total, current),
        Stage::Accounts {
            accounts, selected, ..
        } => accounts_view(app, accounts, *selected, cx),
        Stage::ChoosingDestination {
            account, choice, ..
        } => destination_card(app, account, choice, cx),
        Stage::Building { .. } => busy_card(
            cx,
            "Building the transaction…",
            "Re-scanning the account and selecting every UTXO to sweep.",
        ),
        Stage::Review {
            account, summary, ..
        } => review_card(app, account, summary, cx),
    }
}

fn centered(cx: &App, inner: AnyElement) -> AnyElement {
    div()
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .child(
            div()
                .w(px(460.0))
                .flex()
                .flex_col()
                .items_center()
                .gap_4()
                .p_8()
                .rounded_xl()
                .border_1()
                .border_color(cx.theme().border)
                .bg(cx.theme().secondary.opacity(0.4))
                .child(inner),
        )
        .into_any_element()
}

fn connect_card(cx: &mut Context<SplitterApp>) -> AnyElement {
    centered(
        cx,
        div()
            .flex()
            .flex_col()
            .items_center()
            .gap_4()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_center()
                    .size(px(48.0))
                    .rounded_full()
                    .bg(cx.theme().primary.opacity(0.12))
                    .child(Icon::new(IconName::CircleUser).large().text_color(cx.theme().primary)),
            )
            .child(
                div()
                    .text_lg()
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .child("Connect your hardware wallet"),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child("Unlock the device and open the Bitcoin app. Your PIN is entered on the device — this app never asks for it."),
            )
            .child(
                Button::new("connect")
                    .primary()
                    .label("Connect device")
                    .on_click(cx.listener(|this, _, _window, cx| this.connect(cx))),
            )
            .into_any_element(),
    )
}

fn busy_card(cx: &App, title: &str, subtitle: &str) -> AnyElement {
    centered(
        cx,
        div()
            .flex()
            .flex_col()
            .items_center()
            .gap_3()
            .child(
                Icon::new(IconName::LoaderCircle)
                    .large()
                    .text_color(cx.theme().primary),
            )
            .child(
                div()
                    .text_lg()
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .child(SharedString::from(title.to_string())),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(SharedString::from(subtitle.to_string())),
            )
            .into_any_element(),
    )
}

fn ready_card(app: &SplitterApp, cx: &mut Context<SplitterApp>) -> AnyElement {
    let gated = !app.chain().may_report_balance();
    let blocked_reason = match app.chain() {
        ChainStatus::Up {
            readiness: ScanReadiness::Behind { tip, age_secs },
            ..
        } => Some(format!(
            "The indexer's newest block is {}, {} old. It has not caught up, so an empty result would be indistinguishable from an empty wallet — discovery stays disabled.",
            thousands(*tip),
            humanize(*age_secs)
        )),
        ChainStatus::Down { message } => Some(format!("Chain source unreachable — {message}")),
        ChainStatus::Unknown | ChainStatus::Checking => {
            Some("Still checking the chain source.".to_string())
        }
        _ => None,
    };

    let controls = depth_controls(app, cx);

    centered(
        cx,
        div()
            .flex()
            .flex_col()
            .items_center()
            .gap_4()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_center()
                    .size(px(48.0))
                    .rounded_full()
                    .bg(cx.theme().success.opacity(0.12))
                    .child(Icon::new(IconName::Search).large().text_color(cx.theme().success)),
            )
            .child(
                div()
                    .text_lg()
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .child("Find accounts with coins"),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child("Reads twelve candidate accounts from the device — four address types across three account indices — then scans each against the chain."),
            )
            .child(controls)
            .children(blocked_reason.map(|reason| {
                div()
                    .text_xs()
                    .text_color(cx.theme().warning)
                    .child(SharedString::from(reason))
                    .into_any_element()
            }))
            .child(
                Button::new("discover")
                    .primary()
                    .label("Search for accounts")
                    .disabled(gated)
                    .on_click(cx.listener(|this, _, _window, cx| this.discover(cx))),
            )
            .into_any_element(),
    )
}

fn discovering_card(
    cx: &App,
    phase: DiscoveryPhase,
    scanned: usize,
    total: usize,
    current: &str,
) -> AnyElement {
    // Two phases, each with its own bar, so "is it doing anything" is always answerable: the
    // step line names the phase, the bar moves per account, and the caption names the account.
    let fraction = if total == 0 {
        0.0
    } else {
        scanned as f32 / total as f32
    };

    centered(
        cx,
        div()
            .w_full()
            .flex()
            .flex_col()
            .gap_4()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(Spinner::new().color(cx.theme().primary))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_0p5()
                            .child(
                                div()
                                    .text_sm()
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .child(SharedString::from(phase.title().to_string())),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(SharedString::from(format!(
                                        "Step {} of 2 · account {} of {}",
                                        phase.step(),
                                        (scanned + 1).min(total.max(1)),
                                        total
                                    ))),
                            ),
                    ),
            )
            .child(
                div()
                    .w_full()
                    .h(px(6.0))
                    .rounded_full()
                    .overflow_hidden()
                    .bg(cx.theme().secondary)
                    .child(
                        div()
                            .h_full()
                            .w(relative(fraction))
                            .rounded_full()
                            .bg(cx.theme().primary),
                    ),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(SharedString::from(if current.is_empty() {
                        "starting…".to_string()
                    } else {
                        current.to_string()
                    })),
            )
            .children((phase == DiscoveryPhase::ReadingKeys).then(|| {
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child("Your device is being read directly — no confirmation needed for this step.")
                    .into_any_element()
            }))
            .into_any_element(),
    )
}

fn accounts_view(
    app: &SplitterApp,
    accounts: &[DiscoveredAccount],
    selected: Option<usize>,
    cx: &mut Context<SplitterApp>,
) -> AnyElement {
    if accounts.is_empty() {
        return centered(
            cx,
            div()
                .flex()
                .flex_col()
                .items_center()
                .gap_3()
                .child(Icon::new(IconName::Inbox).large().text_color(cx.theme().muted_foreground))
                .child(
                    div()
                        .text_lg()
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .child("No accounts with history"),
                )
                .child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child("All twelve standard accounts were scanned against a caught-up chain and none has any transactions. If you use a passphrase, that is a different wallet — reconnect with it and search again."),
                )
                .into_any_element(),
        );
    }

    let profile = app.profile();
    // Pre-fork these balances are literally Bitcoin — the fork block does not exist yet. Calling
    // them "ECX" is right about what they will become and wrong about what they are today.
    let pre_fork = app.chain().tip().is_some_and(|tip| tip < ECASH_HEIGHT);
    let chosen = selected.and_then(|i| accounts.get(i));
    let can_continue = chosen.is_some_and(|a| a.is_splittable());

    div()
        .size_full()
        .flex()
        .flex_col()
        .gap_3()
        .child(
            div()
                .flex()
                .items_baseline()
                .justify_between()
                .child(
                    div()
                        .text_base()
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .child(SharedString::from(format!(
                            "{} account{} with history",
                            accounts.len(),
                            if accounts.len() == 1 { "" } else { "s" }
                        ))),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child("Select the account to split"),
                ),
        )
        .child(scroll_area(
            "accounts",
            div()
                .flex()
                .flex_col()
                .gap_3()
                .pr_2()
                .children(pre_fork.then(|| {
                    div()
                        .flex()
                        .gap_2()
                        .px_3()
                        .py_2()
                        .rounded_md()
                        .bg(cx.theme().info.opacity(0.08))
                        .child(
                            Icon::new(IconName::Info)
                                .xsmall()
                                .text_color(cx.theme().info)
                                .flex_shrink_0(),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(0.0))
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child(SharedString::from(format!(
                                    "These are your Bitcoin balances. Block {} has not been mined yet, so nothing is splittable until it is — this is what you will be able to claim.",
                                    thousands(ECASH_HEIGHT)
                                ))),
                        )
                        .into_any_element()
                }))
                .children(
                    accounts
                        .iter()
                        .enumerate()
                        .map(|(i, account)| account_row(account, i, selected == Some(i), profile, cx)),
                )
                .into_any_element(),
        ))
        .child(action_bar(
            cx,
            div()
                .w_full()
                .flex()
                .items_center()
                .gap_3()
                .child(
                    Button::new("continue")
                        .primary()
                        .label("Continue")
                        .disabled(!can_continue)
                        .on_click(cx.listener(|this, _, _window, cx| this.confirm_account(cx))),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(SharedString::from(match chosen {
                            None => "Select an account above to continue.".to_string(),
                            Some(a) if !a.is_splittable() => {
                                "That account has history but nothing left to spend.".to_string()
                            }
                            Some(a) => format!("Splitting {}", a.label()),
                        })),
                )
                .into_any_element(),
        ))
        .into_any_element()
}
fn account_row(
    account: &DiscoveredAccount,
    index: usize,
    selected: bool,
    profile: &ChainProfile,
    cx: &mut Context<SplitterApp>,
) -> AnyElement {
    let border = if selected {
        cx.theme().primary
    } else {
        cx.theme().border
    };
    let splittable = account.is_splittable();

    div()
        .id(("account", index))
        .flex()
        .items_center()
        .justify_between()
        .gap_4()
        .p_4()
        .rounded_lg()
        .border_1()
        .border_color(border)
        .bg(if selected {
            cx.theme().primary.opacity(0.06)
        } else {
            cx.theme().secondary.opacity(0.35)
        })
        .hover(|s| s.border_color(cx.theme().primary.opacity(0.6)))
        .cursor_pointer()
        .on_click(cx.listener(move |this, _, _window, cx| this.select_account(index, cx)))
        .child(
            div()
                .flex()
                .flex_col()
                .gap_1()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(div().text_sm().font_weight(gpui::FontWeight::MEDIUM).child(
                            SharedString::from(account.candidate.kind.label().to_string()),
                        ))
                        .child(
                            div()
                                .px_1p5()
                                .py_0p5()
                                .rounded_sm()
                                .bg(cx.theme().secondary)
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child(SharedString::from(
                                    account.candidate.kind.prefix().to_string(),
                                )),
                        ),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(SharedString::from(account.candidate.path.to_string())),
                ),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .items_end()
                .gap_1()
                .child(
                    div()
                        .text_sm()
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .text_color(if splittable {
                            cx.theme().foreground
                        } else {
                            cx.theme().muted_foreground
                        })
                        .child(SharedString::from(amount(account.balance, profile))),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(SharedString::from(format!(
                            "{} UTXO{} · {} tx",
                            account.utxo_count,
                            if account.utxo_count == 1 { "" } else { "s" },
                            account.tx_count
                        ))),
                ),
        )
        .into_any_element()
}

// ---------------------------------------------------------------------------
// Footer
// ---------------------------------------------------------------------------

pub fn footer(app: &SplitterApp, cx: &App) -> AnyElement {
    let phase = Phase::at_height(ALPHA_HEIGHT);
    let durable = phase.coins_are_durable();

    div()
        .flex()
        .items_center()
        .justify_between()
        .w_full()
        .px_6()
        .py_3()
        .border_t_1()
        .border_color(cx.theme().border)
        .text_xs()
        .text_color(cx.theme().muted_foreground)
        .child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(SharedString::from(format!(
                    "Fork height {}",
                    thousands(ECASH_HEIGHT)
                )))
                .child("·")
                .children((!durable).then(|| {
                    div()
                        .text_color(cx.theme().warning)
                        .child("alpha coins are destroyed and re-issued at full launch")
                        .into_any_element()
                })),
        )
        .child(SharedString::from(
            app.chain()
                .tip()
                .map(|t| format!("{} · tip {}", app.profile().name, thousands(t)))
                .unwrap_or_else(|| app.profile().name.to_string()),
        ))
        .into_any_element()
}

// ---------------------------------------------------------------------------
// Step 5 — destination
// ---------------------------------------------------------------------------

fn field(cx: &App, label: &str, value: impl Into<SharedString>) -> AnyElement {
    div()
        .flex()
        .items_baseline()
        .justify_between()
        .gap_6()
        .py_1p5()
        .child(
            div()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .flex_shrink_0()
                .child(SharedString::from(label.to_string())),
        )
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .text_sm()
                .text_right()
                .child(value.into()),
        )
        .into_any_element()
}

/// Section header with the back action on the **left**, where a back control belongs.
fn section_header(title: &str, cx: &mut Context<SplitterApp>) -> AnyElement {
    div()
        .flex()
        .items_center()
        .gap_3()
        .child(
            Button::new("back")
                .outline()
                .small()
                .icon(IconName::ArrowLeft)
                .label("Back")
                .on_click(cx.listener(|this, _, _window, cx| this.back_to_accounts(cx))),
        )
        .child(
            div()
                .text_base()
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .child(SharedString::from(title.to_string())),
        )
        .into_any_element()
}

/// Scrolling content region with a visible scrollbar, so it is obvious there is more below.
fn scroll_area(id: &'static str, content: AnyElement) -> AnyElement {
    let _ = id;
    div()
        .flex_1()
        .min_h(px(0.0))
        .overflow_y_scrollbar()
        .child(content)
        .into_any_element()
}

/// Actions pinned below the scroll region, so a primary button is never hidden off-screen.
fn action_bar(cx: &App, content: AnyElement) -> AnyElement {
    // Deliberately NOT a flex row. A flex child sizes to its content and will happily overflow
    // the viewport; a block wrapper forces the row inside to the bar's own width, which is what
    // lets explanatory text beside a button wrap instead of running off the edge.
    div()
        .w_full()
        .flex_shrink_0()
        .pt_4()
        .mt_2()
        .border_t_1()
        .border_color(cx.theme().border)
        .child(content)
        .into_any_element()
}

fn destination_card(
    app: &SplitterApp,
    account: &DiscoveredAccount,
    choice: &DestinationChoice,
    cx: &mut Context<SplitterApp>,
) -> AnyElement {
    let profile = app.profile();
    let ready = choice.address().is_some();
    let pasted = choice.is_pasted();

    div()
        .size_full()
        .flex()
        .flex_col()
        .gap_4()
        .child(section_header("Where should the coins go?", cx))
        .child(scroll_area(
            "destination",
            div()
                .flex()
                .flex_col()
                .gap_4()
                .pr_2()
                .child(
                    div()
                        .p_4()
                        .rounded_lg()
                        .border_1()
                        .border_color(cx.theme().border)
                        .bg(cx.theme().secondary.opacity(0.3))
                        .child(field(cx, "Splitting from", account.label()))
                        .child(field(cx, "Balance", amount(account.balance, profile))),
                )
                .child(banner(
                    cx,
                    IconName::TriangleAlert,
                    cx.theme().warning,
                    "An ECX address looks exactly like a Bitcoin address",
                    "Nothing in the string identifies the chain, so we cannot warn you if you paste an exchange deposit address — and no exchange accepts ECX deposits. Coins sent there are unrecoverable. Paste from your own eCash wallet and verify it there.",
                ))
                .child(destination_choice_block(app, choice, cx))
                .into_any_element(),
        ))
        .child(action_bar(
            cx,
            div()
                .w_full()
                .flex()
                .items_center()
                .gap_3()
                .child(
                    Button::new("build")
                        .primary()
                        .label("Build transaction")
                        .disabled(!ready)
                        .on_click(cx.listener(|this, _, _window, cx| this.build_sweep(cx))),
                )
                .child(
                    Button::new("switch-dest")
                        .ghost()
                        .small()
                        .label(if pasted {
                            "Use an account on this device instead"
                        } else {
                            "Paste an address instead"
                        })
                        .on_click(cx.listener(move |this, _, _window, cx| {
                            if pasted {
                                this.use_device_destination(cx);
                            } else {
                                this.use_pasted_address(cx);
                            }
                        })),
                )
                .into_any_element(),
        ))
        .into_any_element()
}
fn destination_choice_block(
    app: &SplitterApp,
    choice: &DestinationChoice,
    cx: &mut Context<SplitterApp>,
) -> AnyElement {
    match choice {
        DestinationChoice::Pending => div()
            .flex()
            .items_center()
            .gap_3()
            .p_4()
            .rounded_lg()
            .border_1()
            .border_color(cx.theme().border)
            .child(Spinner::new().small().color(cx.theme().primary))
            .child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child("Reading the destination account from your device…"),
            )
            .into_any_element(),

        DestinationChoice::Device { address, path } => div()
            .flex()
            .flex_col()
            .gap_2()
            .p_4()
            .rounded_lg()
            .border_1()
            .border_color(cx.theme().success.opacity(0.5))
            .bg(cx.theme().success.opacity(0.06))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        Icon::new(IconName::CircleCheck)
                            .xsmall()
                            .text_color(cx.theme().success),
                    )
                    .child(
                        div()
                            .text_sm()
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .child("A fresh account on your device"),
                    ),
            )
            .child(div().text_sm().child(SharedString::from(address.to_string())))
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(SharedString::from(format!(
                        "{path} — never used on Bitcoin, and it must stay ECX-only forever"
                    ))),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().warning)
                    .child("Derived locally from the device xpub. Verifying it on the device screen needs a registered Ledger wallet policy, which is not implemented yet."),
            )
            .into_any_element(),

        DestinationChoice::Pasted { parsed, acknowledged } => {
            let ack = *acknowledged;
            div()
                .flex()
                .flex_col()
                .gap_3()
                .p_4()
                .rounded_lg()
                .border_1()
                .border_color(cx.theme().border)
                .child(
                    div()
                        .text_sm()
                        .font_weight(gpui::FontWeight::MEDIUM)
                        .child("Paste an address"),
                )
                .child(gpui_component::input::Input::new(app.address_input()))
                .child(
                    Button::new("parse")
                        .outline()
                        .small()
                        .label("Use this address")
                        .on_click(cx.listener(|this, _, _window, cx| this.use_pasted_address(cx))),
                )
                .children(parsed.as_ref().map(|addr| {
                    div()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .child(div().text_sm().child(SharedString::from(addr.to_string())))
                        .child(
                            Button::new("ack")
                                .map(|b| if ack { b.primary() } else { b.outline() })
                                .small()
                                .label(if ack {
                                    "Acknowledged — this is an eCash address I control"
                                } else {
                                    "I confirm this is an eCash address I control"
                                })
                                .on_click(cx.listener(|this, _, _window, cx| {
                                    this.toggle_acknowledgement(cx)
                                })),
                        )
                        .into_any_element()
                }))
                .into_any_element()
        }
    }
}

// ---------------------------------------------------------------------------
// Step 6 — review. "The confirmation screen is the product" (§10).
// ---------------------------------------------------------------------------

fn review_card(
    app: &SplitterApp,
    account: &DiscoveredAccount,
    summary: &SweepSummary,
    cx: &mut Context<SplitterApp>,
) -> AnyElement {
    let profile = app.profile();
    let psbt_for_clipboard = summary.psbt_base64.clone();
    let has_prev = summary.has_prev_txs;

    div()
        .size_full()
        .flex()
        .flex_col()
        .gap_4()
        .child(section_header("Review the transaction", cx))
        .child(scroll_area(
            "review",
            div()
                .flex()
                .flex_col()
                .gap_4()
                .pr_2()
                .child(banner(
                    cx,
                    IconName::TriangleAlert,
                    cx.theme().warning,
                    "This spends on eCash, not on Bitcoin",
                    "Your device will display \"Bitcoin\" and BTC amounts — it has no way not to, because ECX is byte-identical to Bitcoin. The locktime below is the only thing that makes this an eCash transaction.",
                ))
                .child(
                    div()
                        .p_4()
                        .rounded_lg()
                        .border_1()
                        .border_color(cx.theme().border)
                        .bg(cx.theme().secondary.opacity(0.3))
                        .child(field(cx, "From", account.label()))
                        .child(field(cx, "To", summary.destination.clone()))
                        .child(field(cx, "Inputs swept", format!("{}", summary.input_count)))
                        .child(field(cx, "Total in", amount(summary.total_in, profile)))
                        .child(field(cx, "Sending", amount(summary.sending, profile)))
                        .child(field(cx, "Fee", amount(summary.fee, profile)))
                        .child(field(cx, "Device", format!("{}", summary.fingerprint))),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .p_4()
                        .rounded_lg()
                        .border_1()
                        .border_color(cx.theme().primary.opacity(0.4))
                        .bg(cx.theme().primary.opacity(0.05))
                        .child(
                            div()
                                .flex()
                                .items_baseline()
                                .justify_between()
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(cx.theme().muted_foreground)
                                        .child("nLockTime"),
                                )
                                .child(
                                    div()
                                        .text_lg()
                                        .font_weight(gpui::FontWeight::SEMIBOLD)
                                        .child(SharedString::from(summary.locktime.to_string())),
                                ),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child("eCash treats this value as final; Bitcoin reads it as a block height ~500 million in the future and will never relay or mine it. That asymmetry is the replay protection."),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_1p5()
                                .child(
                                    Icon::new(if has_prev {
                                        IconName::CircleCheck
                                    } else {
                                        IconName::TriangleAlert
                                    })
                                    .xsmall()
                                    .flex_shrink_0()
                                    .text_color(if has_prev {
                                        cx.theme().success
                                    } else {
                                        cx.theme().danger
                                    }),
                                )
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w(px(0.0))
                                        .text_xs()
                                        .text_color(cx.theme().muted_foreground)
                                        .child(if has_prev {
                                            "Every non-taproot input carries its previous transaction"
                                        } else {
                                            "Missing previous transactions — a Trezor would refuse to sign this"
                                        }),
                                ),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .justify_between()
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(cx.theme().muted_foreground)
                                        .child("Unsigned PSBT (base64)"),
                                )
                                .child(
                                    Button::new("copy-psbt")
                                        .outline()
                                        .xsmall()
                                        .label("Copy")
                                        .on_click(move |_, _window, cx| {
                                            cx.write_to_clipboard(gpui::ClipboardItem::new_string(
                                                psbt_for_clipboard.clone(),
                                            ));
                                        }),
                                ),
                        )
                        .child(
                            div()
                                .p_3()
                                .rounded_md()
                                .border_1()
                                .border_color(cx.theme().border)
                                .bg(cx.theme().secondary.opacity(0.4))
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child(SharedString::from(summary.psbt_base64.clone())),
                        ),
                )
                .into_any_element(),
        ))
        .child(action_bar(
            cx,
            div()
                .w_full()
                .flex()
                .items_center()
                .gap_3()
                .child(
                    Button::new("sign")
                        .primary()
                        .label("Sign on device")
                        .disabled(true),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.0))
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(SharedString::from(format!(
                            "Signing is disabled until eCash activates at block {}. Until then the chains are identical, the fork probe cannot clear any endpoint, and a signed transaction would have nowhere valid to go.",
                            thousands(ECASH_HEIGHT)
                        ))),
                )
                .into_any_element(),
        ))
        .into_any_element()
}

/// Search-depth controls.
///
/// Two different depths, and conflating them wastes a scan: **accounts** is how many account
/// indices are probed per address type, so an account created beyond the default range is simply
/// invisible without raising it; **gap** is how many consecutive unused addresses end a scan
/// *within* an account, and raising it never finds a missing account.
fn depth_controls(app: &SplitterApp, cx: &mut Context<SplitterApp>) -> AnyElement {
    let depth = app.depth();
    let busy = app.stage().is_busy();

    let row = |label: &'static str, hint: SharedString, buttons: AnyElement| {
        div()
            .flex()
            .items_center()
            .justify_between()
            .gap_3()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .child(div().text_xs().child(label))
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(hint),
                    ),
            )
            .child(buttons)
            .into_any_element()
    };

    div()
        .w_full()
        .flex()
        .flex_col()
        .gap_2()
        .p_3()
        .rounded_lg()
        .border_1()
        .border_color(cx.theme().border)
        .child(row(
            "Accounts per address type",
            SharedString::from(format!(
                "{} candidates read from the device",
                depth.candidate_count()
            )),
            div()
                .flex()
                .gap_1()
                .children([3u32, 6, 10].into_iter().enumerate().map(|(i, n)| {
                    let selected = depth.accounts == n;
                    let b = Button::new(("accounts", i)).label(n.to_string()).xsmall();
                    let b = if selected { b.primary() } else { b.outline() };
                    b.disabled(busy)
                        .on_click(
                            cx.listener(move |this, _, _window, cx| {
                                this.set_accounts_probed(n, cx)
                            }),
                        )
                        .into_any_element()
                }))
                .into_any_element(),
        ))
        .child(row(
            "Address gap limit",
            SharedString::from("unused addresses that end a scan within one account"),
            div()
                .flex()
                .gap_1()
                .children([20usize, 50, 100].into_iter().enumerate().map(|(i, n)| {
                    let selected = depth.stop_gap == n;
                    let b = Button::new(("gap", i)).label(n.to_string()).xsmall();
                    let b = if selected { b.primary() } else { b.outline() };
                    b.disabled(busy)
                        .on_click(cx.listener(move |this, _, _window, cx| this.set_stop_gap(n, cx)))
                        .into_any_element()
                }))
                .into_any_element(),
        ))
        .into_any_element()
}
