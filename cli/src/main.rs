//! `ecx` — the command-line eCash splitter.
//!
//! Deliberately thin. Every step lives in `ecx-split`, exactly as it does for the desktop app;
//! this binary parses arguments and prints [`SplitEvent`]s. If a behaviour differs between the
//! two frontends, that is a bug in one of them, not a feature of either.
//!
//! Like the GUI, it stops before signing.

use std::process::ExitCode;

use bitcoin::{Address, address::NetworkUnchecked};
use ecx_chain::{ChainProfile, EsploraChain, ScanReadiness};
use ecx_split::{Destination, SplitEvent, build_sweep, build_sweep_full, discover, resolve_signed};

const USAGE: &str = "\
ecx — split BTC to ECX from a hardware wallet

USAGE:
    ecx <COMMAND> [OPTIONS]

COMMANDS:
    devices                     List connected hardware wallets
    status                      Chain tip, whether it is caught up, and how far off the fork is
    discover                    Find accounts with coins on the connected device
    check [--account <PATH>]    Ask Bitcoin which coins are genuinely shared and need
                                splitting. Read-only
    build --account <PATH> --to <ADDRESS>
                                Build the sweep PSBT for one account and print it
    sign --to <ADDRESS> [--account <PATH>]
                                Build, show the PSBT, confirm, sign on the device, and
                                verify the result. Add --broadcast to publish it.
                                Prompts for the account unless --account is given
    broadcast --tx <HEX>        Publish a transaction signed earlier
    track --txid <TXID>         How deeply buried a broadcast transaction is

OPTIONS:
    --endpoint <URL>            eCash Esplora base URL (default: the ECX alpha preset)
    --bitcoin <URL>             Bitcoin Esplora for `check` (default: blockstream.info)
    --accounts <N>              Account indices to probe per address type (default: 3).
                                Raise this if an account was created further out; four
                                address types x N accounts are read from the device
    --gap <N>                   Unused addresses that end a scan within one account
                                (default: 20). Raising this does not find missing accounts
    --feerate <SAT_PER_VB>      Fee rate for `build`/`sign` (default: 1)
    --psbt-out <FILE>           Also write the unsigned PSBT here, to inspect elsewhere
    --broadcast                 Publish after signing. Refused unless the endpoint is
                                proven to be eCash by the fork probe
    --yes                       Skip the confirmation prompt (scripting only)
    -h, --help                  Show this help

Broadcasting is gated on the fork probe: an endpoint must be proven to be eCash and not Bitcoin,
which is impossible until the chains diverge at the fork block. `ecx status` says where that
stands. Signing works regardless — the transaction carries a locktime Bitcoin will never accept.
";

fn has(args: &[String], name: &str) -> bool {
    args.iter().any(|a| a == name)
}

fn flag(args: &[String], name: &str) -> Option<String> {
    let i = args.iter().position(|a| a == name)?;
    args.get(i + 1).cloned()
}

/// How far to look. Two independent depths, so both are exposed.
fn depth(args: &[String]) -> Result<ecx_wallet::DiscoveryDepth, String> {
    let mut d = ecx_wallet::DiscoveryDepth::default();
    if let Some(v) = flag(args, "--accounts") {
        d.accounts = v
            .parse()
            .map_err(|_| "--accounts must be a whole number".to_string())?;
        if d.accounts == 0 {
            return Err("--accounts must be at least 1".into());
        }
    }
    if let Some(v) = flag(args, "--gap") {
        d.stop_gap = v
            .parse()
            .map_err(|_| "--gap must be a whole number".to_string())?;
    }
    Ok(d)
}

fn profile(args: &[String]) -> ChainProfile {
    flag(args, "--endpoint")
        .map(|u| ChainProfile::custom(&u))
        .unwrap_or_default()
}

fn render(event: SplitEvent) {
    match event {
        SplitEvent::Connected(id) => {
            println!(
                "connected: {} {} · fingerprint {}",
                id.label, id.version, id.fingerprint
            );
        }
        SplitEvent::ReadingKeys { done, total, label } => {
            println!("  [{:>2}/{}] reading  {label}", done + 1, total);
        }
        SplitEvent::Scanning { done, total, label } => {
            println!("  [{:>2}/{}] scanning {label}", done + 1, total);
        }
    }
}

#[tokio::main]
async fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let command = args.first().map(String::as_str).unwrap_or("--help");

    if matches!(command, "-h" | "--help" | "help") {
        print!("{USAGE}");
        return ExitCode::SUCCESS;
    }

    match run(command, &args).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("error: {message}");
            ExitCode::FAILURE
        }
    }
}

fn parse_address(text: &str) -> Result<Address, String> {
    text.parse::<Address<NetworkUnchecked>>()
        .map_err(|_| format!("{text:?} is not a valid address"))?
        .require_network(bitcoin::Network::Bitcoin)
        .map_err(|_| "address is not a Bitcoin mainnet address".to_string())
}

fn find_account<'a>(
    accounts: &'a [ecx_wallet::DiscoveredAccount],
    path: &str,
) -> Result<&'a ecx_wallet::DiscoveredAccount, String> {
    let wanted = path.trim().trim_start_matches("m/");
    accounts
        .iter()
        .find(|a| a.candidate.path.to_string() == wanted)
        .ok_or_else(|| format!("no discovered account at {path}"))
}

fn print_accounts(
    accounts: &[ecx_wallet::DiscoveredAccount],
    profile: &ChainProfile,
    numbered: bool,
) {
    if numbered {
        println!(
            "{:<4} {:<16} {:<14} {:>14}  UTXOS",
            "#", "TYPE", "PATH", "BALANCE"
        );
    } else {
        println!("{:<16} {:<14} {:>14}  UTXOS", "TYPE", "PATH", "BALANCE");
    }
    for (i, a) in accounts.iter().enumerate() {
        let row = format!(
            "{:<16} {:<14} {:>14}  {}",
            a.candidate.kind.label(),
            a.candidate.path.to_string(),
            profile.format(a.balance),
            a.utxo_count
        );
        if numbered {
            // Accounts with history but nothing to spend are listed and marked, not hidden --
            // "we looked here and found nothing left" is information.
            let marker = if a.is_splittable() {
                format!("{:<4}", i + 1)
            } else {
                "  - ".into()
            };
            println!("{marker} {row}");
        } else {
            println!("{row}");
        }
    }
}

/// Pick an account interactively, so the user does not have to run `discover` first and copy a
/// path back in — which would mean reading twelve xpubs and scanning twelve accounts twice.
fn choose_account<'a>(
    accounts: &'a [ecx_wallet::DiscoveredAccount],
    profile: &ChainProfile,
) -> Result<&'a ecx_wallet::DiscoveredAccount, String> {
    use std::io::{Write, stdin, stdout};

    let splittable: Vec<usize> = accounts
        .iter()
        .enumerate()
        .filter(|(_, a)| a.is_splittable())
        .map(|(i, _)| i)
        .collect();

    if splittable.is_empty() {
        return Err("no account has anything to spend".into());
    }

    println!();
    print_accounts(accounts, profile, true);
    println!();

    if splittable.len() == 1 {
        let only = &accounts[splittable[0]];
        println!("Only one account has coins: {}", only.label());
        return Ok(only);
    }

    print!("Which account? [1-{}]: ", accounts.len());
    stdout().flush().map_err(|e| e.to_string())?;
    let mut answer = String::new();
    stdin().read_line(&mut answer).map_err(|e| e.to_string())?;

    let n: usize = answer
        .trim()
        .parse()
        .map_err(|_| "expected a number from the # column".to_string())?;
    let account = accounts
        .get(n.wrapping_sub(1))
        .ok_or_else(|| format!("there is no account {n}"))?;
    if !account.is_splittable() {
        return Err(format!("{} has nothing left to spend", account.label()));
    }
    Ok(account)
}

fn print_summary(
    account: &ecx_wallet::DiscoveredAccount,
    s: &ecx_wallet::SweepSummary,
    profile: &ChainProfile,
) {
    println!("from        : {}", account.label());
    println!("to          : {}", s.destination);
    println!("inputs      : {}", s.input_count);
    println!("total in    : {}", profile.format(s.total_in));
    println!("sending     : {}", profile.format(s.sending));
    println!("fee         : {}", profile.format(s.fee));
    println!("nLockTime   : {}", s.locktime);
    println!(
        "prev txs    : {}",
        if s.has_prev_txs { "present" } else { "MISSING" }
    );
}

/// Typed confirmation. Not a y/n — this spends every UTXO in an account.
fn confirm(summary: &ecx_wallet::SweepSummary, profile: &ChainProfile) -> Result<bool, String> {
    use std::io::{Write, stdin, stdout};
    println!();
    println!(
        "This sweeps {} to {} on eCash.",
        profile.format(summary.sending),
        summary.destination
    );
    println!("Your device will display \"Bitcoin\" — it has no way not to.");
    print!("Type 'sign' to continue: ");
    stdout().flush().map_err(|e| e.to_string())?;
    let mut answer = String::new();
    stdin().read_line(&mut answer).map_err(|e| e.to_string())?;
    Ok(answer.trim() == "sign")
}

/// Publish, with the fork probe as the gate.
async fn publish(
    chain: &EsploraChain,
    tx: &bitcoin::Transaction,
    profile: &ChainProfile,
) -> Result<(), String> {
    let readiness = ecx_split::broadcast_readiness(chain)
        .await
        .map_err(|e| e.to_string())?;
    if let Some(why) = readiness.explain() {
        return Err(format!("cannot broadcast — {why}"));
    }

    let txid = ecx_split::broadcast(chain, tx)
        .await
        .map_err(|e| e.to_string())?;
    println!();
    println!("BROADCAST to {}", profile.host());
    println!("txid        : {txid}");
    println!("explorer    : {}", profile.tx_url(&txid.to_string()));
    println!();
    println!(
        "Wait for {} confirmations before treating this as settled — post-fork difficulty resets\n\
         to minimum, so early reorg risk is elevated. Track it with:\n  ecx track --txid {txid}",
        ecx_split::MIN_CONFIRMATIONS
    );
    Ok(())
}

fn report_unbroadcast(tx: &bitcoin::Transaction, profile: &ChainProfile) {
    let txid = tx.compute_txid();
    println!();
    println!("explorer    : {}", profile.tx_url(&txid.to_string()));
    println!("              (nothing there yet — this has not been broadcast)");
    println!();
    println!(
        "NOT broadcast. Re-run with --broadcast, or publish it later with:\n  ecx broadcast --tx <hex>"
    );
}

async fn run(command: &str, args: &[String]) -> Result<(), String> {
    match command {
        "devices" => {
            let devices = ecx_signer::enumerate().await.map_err(|e| e.to_string())?;
            if devices.is_empty() {
                println!("no devices found");
            }
            for d in devices {
                println!("{:?}  {}", d.kind, d.label);
            }
            Ok(())
        }

        "status" => {
            let profile = profile(args);
            let chain = EsploraChain::new(profile.clone()).map_err(|e| e.to_string())?;
            let (tip, readiness) = ecx_split::chain_status(&chain)
                .await
                .map_err(|e| e.to_string())?;
            println!("endpoint : {}", profile.esplora_url);
            println!("tip      : {} (unix {})", tip.height, tip.time);
            match readiness {
                ScanReadiness::Ready { .. } => println!("status   : caught up"),
                // Golden Rule 9: an empty result from here would be indistinguishable from an
                // empty wallet, so say so rather than letting a script trust the output.
                ScanReadiness::Behind { age_secs, .. } => {
                    println!("status   : BEHIND — newest block is {age_secs}s old");
                    println!("           results from this endpoint cannot be trusted");
                }
            }
            println!(
                "fork     : block {} ({})",
                ecx_core::ECASH_HEIGHT,
                if tip.height >= ecx_core::ECASH_HEIGHT {
                    "activated"
                } else {
                    "not yet reached"
                }
            );

            // The fork probe, read-only. Says whether this endpoint is provably eCash before a
            // user tries to publish to it and finds out the hard way.
            match ecx_split::broadcast_readiness(&chain).await {
                Ok(readiness) => match readiness.explain() {
                    None => println!("broadcast : allowed — endpoint proven to be eCash"),
                    Some(why) => {
                        println!("broadcast : REFUSED");
                        println!("            {why}");
                    }
                },
                Err(e) => println!("broadcast : could not check — {e}"),
            }
            Ok(())
        }

        "discover" => {
            let profile = profile(args);
            let chain = EsploraChain::new(profile.clone()).map_err(|e| e.to_string())?;
            let signer = ecx_signer::connect_any().await.map_err(|e| e.to_string())?;
            let label = format!("{:?}", signer.kind());
            let (_, accounts) = discover(&chain, signer.as_ref(), label, depth(args)?, render)
                .await
                .map_err(|e| e.to_string())?;

            println!();
            if accounts.is_empty() {
                // "Nothing in what we searched" is not "nothing in your wallet", and the
                // difference matters on a screen about money.
                println!("no accounts with history in what was searched");
                println!(
                    "  searched {} candidates (4 address types x {} account indices, gap {})",
                    depth(args)?.candidate_count(),
                    depth(args)?.accounts,
                    depth(args)?.stop_gap
                );
                println!("  try --accounts <N> or --gap <N> to look further out");
                return Ok(());
            }
            print_accounts(&accounts, &profile, false);
            Ok(())
        }

        "build" => {
            let path = flag(args, "--account").ok_or("build needs --account <PATH>")?;
            let to = flag(args, "--to").ok_or("build needs --to <ADDRESS>")?;
            let feerate: u64 = flag(args, "--feerate")
                .as_deref()
                .unwrap_or("1")
                .parse()
                .map_err(|_| "--feerate must be a whole number of sat/vB")?;

            let address = parse_address(&to)?;

            let profile = profile(args);
            let chain = EsploraChain::new(profile.clone()).map_err(|e| e.to_string())?;
            let signer = ecx_signer::connect_any().await.map_err(|e| e.to_string())?;
            let label = format!("{:?}", signer.kind());
            let (identity, accounts) =
                discover(&chain, signer.as_ref(), label, depth(args)?, render)
                    .await
                    .map_err(|e| e.to_string())?;

            let account = find_account(&accounts, &path)?;

            // The CLI has no way to show a typed acknowledgement, so require it explicitly.
            let destination = Destination::Pasted {
                address,
                acknowledged: true,
            };

            let summary = build_sweep(&chain, account, &destination, identity.fingerprint, feerate)
                .await
                .map_err(|e| e.to_string())?;

            println!();
            println!("from        : {}", account.label());
            println!("to          : {}", summary.destination);
            println!("inputs      : {}", summary.input_count);
            println!("total in    : {}", summary.total_in);
            println!("sending     : {}", summary.sending);
            println!("fee         : {}", summary.fee);
            println!("nLockTime   : {}", summary.locktime);
            println!(
                "prev txs    : {}",
                if summary.has_prev_txs {
                    "present"
                } else {
                    "MISSING"
                }
            );
            println!();
            println!("{}", summary.psbt_base64);
            println!();
            println!(
                "Not signed. eCash has not activated at block {}.",
                ecx_core::ECASH_HEIGHT
            );
            Ok(())
        }

        "sign" => {
            let to = flag(args, "--to").ok_or("sign needs --to <ADDRESS>")?;
            let feerate: u64 = flag(args, "--feerate")
                .as_deref()
                .unwrap_or("1")
                .parse()
                .map_err(|_| "--feerate must be a whole number of sat/vB")?;
            let address = parse_address(&to)?;

            let profile = profile(args);
            let chain = EsploraChain::new(profile.clone()).map_err(|e| e.to_string())?;
            let signer = ecx_signer::connect_any().await.map_err(|e| e.to_string())?;
            let kind = signer.kind();
            let label = format!("{kind:?}");
            let (identity, accounts) =
                discover(&chain, signer.as_ref(), label, depth(args)?, render)
                    .await
                    .map_err(|e| e.to_string())?;
            // --account skips the prompt; without it, pick from the accounts we just scanned
            // rather than making the user run `discover` and repeat the whole sweep.
            let account = match flag(args, "--account") {
                Some(path) => find_account(&accounts, &path)?,
                None => choose_account(&accounts, &profile)?,
            };

            let destination = Destination::Pasted {
                address,
                acknowledged: true,
            };
            let built =
                build_sweep_full(&chain, account, &destination, identity.fingerprint, feerate)
                    .await
                    .map_err(|e| e.to_string())?;

            println!();
            print_summary(account, &built.summary, &profile);
            println!();
            println!("{}", built.summary.psbt_base64);

            if let Some(file) = flag(args, "--psbt-out") {
                std::fs::write(&file, ecx_signer::airgap::export_bytes(&built.psbt))
                    .map_err(|e| format!("could not write {file}: {e}"))?;
                println!();
                println!("unsigned PSBT written to {file}");
            }

            // Golden Rule 7 — nothing reaches the device without explicit confirmation.
            if !has(args, "--yes") && !confirm(&built.summary, &profile)? {
                println!("aborted; nothing was sent to the device");
                return Ok(());
            }

            // Ledger will not sign without its wallet policy, and the policy can only be set at
            // construction — so signing needs a second connection. The account is not known
            // until discovery has run, which is why it cannot simply be set on the first one.
            //
            // Drop the discovery connection first. A USB HID device can be open once; holding
            // both handles makes the second open fail with "device not found", which reads as
            // "unplugged" and is really "already in use by us".
            let policy = account.ledger_policy();
            drop(signer);

            println!();
            println!("confirm on your device…");
            let signing = ecx_signer::connect_for_signing(kind, &policy)
                .await
                .map_err(|e| e.to_string())?;

            let signed = signing.sign(&built.psbt).await.map_err(|e| e.to_string())?;
            let tx = resolve_signed(signed).map_err(|e| e.to_string())?;

            // Golden Rule 3 — the device's output is untrusted. Compare it against the intent
            // derived from the PSBT shown above, never one recomputed from these bytes.
            ecx_core::verify_signed(&tx, &built.intent)
                .map_err(|e| format!("VERIFICATION FAILED — not broadcasting: {e}"))?;

            println!();
            println!("verified against the reviewed transaction.");
            println!("txid        : {}", tx.compute_txid());
            println!("nLockTime   : {}", tx.lock_time.to_consensus_u32());
            println!();
            println!("{}", bitcoin::consensus::encode::serialize_hex(&tx));
            if has(args, "--broadcast") {
                publish(&chain, &tx, &profile).await
            } else {
                report_unbroadcast(&tx, &profile);
                Ok(())
            }
        }

        "check" => {
            let profile = profile(args);
            let chain = EsploraChain::new(profile.clone()).map_err(|e| e.to_string())?;
            let signer = ecx_signer::connect_any().await.map_err(|e| e.to_string())?;
            let label = format!("{:?}", signer.kind());
            let (_, accounts) = discover(&chain, signer.as_ref(), label, depth(args)?, render)
                .await
                .map_err(|e| e.to_string())?;

            let reference = match flag(args, "--bitcoin") {
                Some(url) => ecx_chain::BitcoinReference::new(&url),
                None => ecx_chain::BitcoinReference::from_env(),
            }
            .map_err(|e| e.to_string())?;

            let selected: Vec<_> = match flag(args, "--account") {
                Some(path) => vec![find_account(&accounts, &path)?],
                None => accounts.iter().filter(|a| a.is_splittable()).collect(),
            };
            if selected.is_empty() {
                println!("\nno account has anything to check");
                return Ok(());
            }

            println!();
            println!("Asking {} about each coin.", reference.host());
            println!("Read-only — but it does reveal these transaction ids to that operator.");

            for account in selected {
                println!();
                println!("{}", account.label());
                let check = ecx_split::check_against_bitcoin(&reference, account, |done, total| {
                    use std::io::Write;
                    print!("\r  checking {}/{}…", done + 1, total);
                    let _ = std::io::stdout().flush();
                })
                .await;
                print!("\r                              \r");

                for coin in &check.coins {
                    println!(
                        "  {}:{:<3} {:>16}  {}",
                        coin.utxo.outpoint.txid,
                        coin.utxo.outpoint.vout,
                        profile.format(coin.utxo.value),
                        coin.verdict.label()
                    );
                }

                println!(
                    "  shared, needs splitting : {}",
                    profile.format(check.shared_value())
                );
                println!(
                    "  already settled         : {}",
                    profile.format(check.settled_value())
                );
                if check.unverified_value() > bitcoin::Amount::ZERO {
                    // Never folded into "settled": not knowing is its own state.
                    println!(
                        "  COULD NOT CHECK         : {}",
                        profile.format(check.unverified_value())
                    );
                }
                if !check.worth_splitting() {
                    println!("  nothing here needs splitting.");
                }
            }
            Ok(())
        }

        "broadcast" => {
            let hex = flag(args, "--tx").ok_or("broadcast needs --tx <HEX>")?;
            let tx: bitcoin::Transaction = bitcoin::consensus::encode::deserialize_hex(hex.trim())
                .map_err(|e| format!("not a valid transaction: {e}"))?;
            let profile = profile(args);
            let chain = EsploraChain::new(profile.clone()).map_err(|e| e.to_string())?;
            publish(&chain, &tx, &profile).await
        }

        "track" => {
            let txid: bitcoin::Txid = flag(args, "--txid")
                .ok_or("track needs --txid <TXID>")?
                .trim()
                .parse()
                .map_err(|_| "that is not a valid txid".to_string())?;
            let profile = profile(args);
            let chain = EsploraChain::new(profile.clone()).map_err(|e| e.to_string())?;
            match ecx_split::track(&chain, txid)
                .await
                .map_err(|e| e.to_string())?
            {
                ecx_chain::TxState::Unknown => {
                    println!("not seen by {} — not broadcast, or dropped", profile.host());
                }
                ecx_chain::TxState::InMempool => println!("in the mempool, not yet mined"),
                ecx_chain::TxState::Confirmed {
                    height,
                    confirmations,
                } => {
                    println!("confirmed in block {height}, {confirmations} confirmation(s)");
                    if confirmations < ecx_split::MIN_CONFIRMATIONS {
                        // Post-fork difficulty resets to minimum, so reorg risk is elevated.
                        println!(
                            "  wait for {} — post-fork difficulty is at minimum, so early reorg risk is elevated",
                            ecx_split::MIN_CONFIRMATIONS
                        );
                    }
                }
            }
            println!("explorer    : {}", profile.tx_url(&txid.to_string()));
            Ok(())
        }

        other => Err(format!("unknown command {other:?} — try `ecx --help`")),
    }
}
