//! Smoke-test the Esplora backend. Pass a URL to check a custom endpoint.
use ecx_chain::{ChainProfile, ChainSource, EsploraChain};

#[tokio::main]
async fn main() {
    let mut profiles = ChainProfile::presets();
    profiles.extend(std::env::args().skip(1).map(|u| ChainProfile::custom(&u)));

    for profile in profiles {
        print!("{:<28} ", profile.name);
        match EsploraChain::new(profile) {
            Err(e) => println!("BUILD FAILED: {e}"),
            Ok(chain) => match chain.tip_height().await {
                Ok(tip) => println!("tip {tip}"),
                Err(e) => println!("ERR {e}"),
            },
        }
    }
}
