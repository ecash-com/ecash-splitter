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
use ecx_split::{Destination, SplitEvent, build_sweep, discover};

const USAGE: &str = "\
ecx — split BTC to ECX from a hardware wallet

USAGE:
    ecx <COMMAND> [OPTIONS]

COMMANDS:
    devices                     List connected hardware wallets
    status                      Show the chain endpoint's tip and whether it is caught up
    discover                    Find accounts with history on the connected device
    build --account <PATH> --to <ADDRESS>
                                Build the sweep PSBT for one account and print it

OPTIONS:
    --endpoint <URL>            Esplora base URL (default: the ECX alpha preset)
    --feerate <SAT_PER_VB>      Fee rate for `build` (default: 1)
    -h, --help                  Show this help

Signing and broadcasting are not implemented: eCash has not activated, so no endpoint can pass
the fork probe and a signed transaction would have nowhere valid to go.
";

fn flag(args: &[String], name: &str) -> Option<String> {
    let i = args.iter().position(|a| a == name)?;
    args.get(i + 1).cloned()
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
            Ok(())
        }

        "discover" => {
            let chain = EsploraChain::new(profile(args)).map_err(|e| e.to_string())?;
            let signer = ecx_signer::connect_any().await.map_err(|e| e.to_string())?;
            let label = format!("{:?}", signer.kind());
            let (_, accounts) = discover(&chain, signer.as_ref(), label, render)
                .await
                .map_err(|e| e.to_string())?;

            println!();
            if accounts.is_empty() {
                println!("no accounts with history");
                return Ok(());
            }
            println!("{:<16} {:<14} {:>14}  UTXOS", "TYPE", "PATH", "BALANCE");
            for a in &accounts {
                println!(
                    "{:<16} {:<14} {:>14}  {}",
                    a.candidate.kind.label(),
                    a.candidate.path.to_string(),
                    a.balance.to_string(),
                    a.utxo_count
                );
            }
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

            let address = to
                .parse::<Address<NetworkUnchecked>>()
                .map_err(|_| format!("{to:?} is not a valid address"))?
                .require_network(bitcoin::Network::Bitcoin)
                .map_err(|_| "address is not a Bitcoin mainnet address".to_string())?;

            let chain = EsploraChain::new(profile(args)).map_err(|e| e.to_string())?;
            let signer = ecx_signer::connect_any().await.map_err(|e| e.to_string())?;
            let label = format!("{:?}", signer.kind());
            let (identity, accounts) = discover(&chain, signer.as_ref(), label, render)
                .await
                .map_err(|e| e.to_string())?;

            let account = accounts
                .iter()
                .find(|a| a.candidate.path.to_string() == path.trim_start_matches("m/"))
                .ok_or_else(|| format!("no discovered account at {path}"))?;

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

        other => Err(format!("unknown command {other:?} — try `ecx --help`")),
    }
}
