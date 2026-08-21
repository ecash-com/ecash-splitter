//! Rendering. Pure presentation — every value shown here was produced by an `ecx-*` crate.

use bitcoin::Amount;
use gpui::{
    AnyElement, App, Context, InteractiveElement as _, IntoElement, ParentElement, SharedString,
    StatefulInteractiveElement as _, Styled, div, px, relative,
};
use gpui_component::{
    ActiveTheme as _, Disableable as _, Icon, IconName, Sizable as _, button::Button,
    button::ButtonVariants as _, spinner::Spinner,
};

use ecx_chain::{ChainProfile, ProfileKind, ScanReadiness};
use ecx_core::{ALPHA_HEIGHT, ECASH_HEIGHT, Phase};
use ecx_wallet::DiscoveredAccount;

use crate::SplitterApp;
use crate::state::{ChainStatus, DiscoveryPhase, Stage, profile_notice};

/// Ticker for the selected chain. Small thing, but showing "BTC" while scanning Bitcoin and
/// "ECX" while scanning ECX removes a real source of confusion (§10).
fn unit(profile: ChainProfile) -> &'static str {
    match profile.kind {
        ProfileKind::Ecx => "ECX",
        ProfileKind::BitcoinReadOnly => "BTC",
    }
}

fn amount(value: Amount, profile: ChainProfile) -> String {
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
    let profile = app.profile();
    let active = profile;

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
        .child(div().flex().items_center().gap_2().children(
            ChainProfile::ALL.into_iter().enumerate().map(|(i, p)| {
                let selected = p == active;
                let mut button = Button::new(("profile", i)).label(p.name).small();
                button = if selected {
                    button.primary()
                } else {
                    button.outline()
                };
                button
                    .on_click(cx.listener(move |this, _, _window, cx| this.set_profile(p, cx)))
                    .into_any_element()
            }),
        ))
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
                        .child("All twelve standard accounts were scanned against a fully-synced chain and none has any transactions. If you use a passphrase, this is a different wallet — reconnect with it and search again."),
                )
                .into_any_element(),
        );
    }

    let profile = app.profile();
    // Pre-fork these balances are literally Bitcoin — the fork block does not exist yet. Calling
    // them "ECX" is right about what they will become and wrong about what they are today, so
    // say so rather than letting the ticker imply the split already happened.
    let pre_fork = app.chain().tip().is_some_and(|tip| tip < ECASH_HEIGHT);
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
        .children(pre_fork.then(|| {
            div()
                .flex()
                .items_center()
                .gap_2()
                .px_3()
                .py_2()
                .rounded_md()
                .bg(cx.theme().info.opacity(0.08))
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child(Icon::new(IconName::Info).xsmall().text_color(cx.theme().info))
                .child(SharedString::from(format!(
                    "These are your Bitcoin balances. Block {} has not been mined yet, so nothing is splittable until it is — this is what you will be able to claim.",
                    thousands(ECASH_HEIGHT)
                )))
                .into_any_element()
        }))
        .children(
            accounts
                .iter()
                .enumerate()
                .map(|(i, account)| account_row(account, i, selected == Some(i), profile, cx)),
        )
        .into_any_element()
}

fn account_row(
    account: &DiscoveredAccount,
    index: usize,
    selected: bool,
    profile: ChainProfile,
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
