//! Live check against the running Antigravity app. Run by hand:
//! `cargo test -p quotaglass-core --test live_antigravity -- --ignored --nocapture`.
//! Prints only bucket names, percentages, windows and reset times: never an email, a plan or a token.

use quotaglass_core::sources::antigravity_app::AntigravityApp;
use quotaglass_core::{Env, Provider};

#[tokio::test]
#[ignore = "needs the Antigravity app running on this PC"]
async fn live_antigravity_fetch() {
    let env = Env::from_system().expect("env");
    let readings = match AntigravityApp.fetch(&env).await {
        Ok(readings) => readings,
        Err(e) => panic!("fetch error with the app running: {e:?}"),
    };
    let mut buckets = 0;
    for reading in readings {
        for b in reading.buckets {
            buckets += 1;
            println!(
                "{} | used {:.1}% | window {:?} s | resets_at_ms {:?}",
                b.name, b.used_percent, b.window_secs, b.resets_at_ms
            );
        }
    }
    assert!(buckets > 0, "the running app returned no buckets");
}
