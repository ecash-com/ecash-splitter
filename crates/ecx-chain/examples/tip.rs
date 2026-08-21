//! Smoke-test the Esplora backend against every configured profile.
use ecx_chain::{ChainProfile, ChainSource, EsploraChain};

#[tokio::main]
async fn main() {
    for profile in ChainProfile::ALL {
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
