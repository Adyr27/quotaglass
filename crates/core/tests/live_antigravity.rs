//! Live check against the running Antigravity app. Run by hand:
//! `cargo test -p quotaglass-core --test live_antigravity -- --ignored --nocapture`.
//! Prints only bucket names, percentages and reset times: never an email, a plan or a token.

use quotaglass_core::sources::antigravity_app::AntigravityApp;
use quotaglass_core::{Env, Provider};

#[tokio::test]
#[ignore = "needs the Antigravity app running on this PC"]
async fn live_antigravity_fetch() {
    let env = Env::from_system().expect("env");
    println!("detect: {}", AntigravityApp.detect(&env));
    match AntigravityApp.fetch(&env).await {
        Ok(readings) => {
            for reading in readings {
                for b in reading.buckets {
                    println!(
                        "{} | used {:.1}% | window {:?} s | resets_at_ms {:?}",
                        b.name, b.used_percent, b.window_secs, b.resets_at_ms
                    );
                }
            }
        }
        Err(e) => println!("fetch error: {e:?}"),
    }
}
