//! Mutation tests for every parser that reads data QuotaGlass does not control (vendor endpoints, Orca output,
//! login files). Each fixture is mutated thousands of times, by JSON structure and by raw bytes. A parser may
//! reject the input, but it must never panic (debug builds also panic on integer overflow), and a reading it
//! accepts must hold only finite percentages from 0 to 100.

use quotaglass_core::sources::{claude_direct, codex_direct, orca};
use quotaglass_core::{AccountKey, Reading, Service};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

const ROUNDS: usize = 3_000;
const NOW_MS: i64 = 1_790_000_000_000;

/// xorshift64: deterministic, so a failure reproduces with the same round number.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

fn fixture(name: &str) -> Vec<u8> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name);
    std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn hostile(rng: &mut Rng) -> Value {
    let choices = [
        Value::Null,
        json!(-1),
        json!(-0.0),
        json!(1e308),
        json!(-1e308),
        json!(i64::MAX),
        json!(i64::MIN),
        json!(u64::MAX),
        json!(101.5),
        json!(""),
        json!("not-a-date"),
        json!("2026-13-45T99:99:99Z"),
        json!(true),
        json!([]),
        json!({}),
    ];
    choices[rng.below(choices.len())].clone()
}

/// Every node in the tree, as a path of object keys and array indexes.
fn paths(v: &Value, here: &mut Vec<String>, out: &mut Vec<Vec<String>>) {
    out.push(here.clone());
    match v {
        Value::Object(m) => {
            for (k, child) in m {
                here.push(k.clone());
                paths(child, here, out);
                here.pop();
            }
        }
        Value::Array(a) => {
            for (i, child) in a.iter().enumerate() {
                here.push(i.to_string());
                paths(child, here, out);
                here.pop();
            }
        }
        _ => {}
    }
}

fn node_mut<'a>(v: &'a mut Value, path: &[String]) -> &'a mut Value {
    path.iter().fold(v, |node, step| match node {
        Value::Object(m) => m.get_mut(step).expect("path from the same tree"),
        Value::Array(a) => &mut a[step.parse::<usize>().expect("index")],
        _ => unreachable!("paths only descend into containers"),
    })
}

/// Replaces one to three random nodes with hostile values, or deletes an object key.
fn mutate_structure(base: &Value, rng: &mut Rng) -> Vec<u8> {
    let mut v = base.clone();
    for _ in 0..=rng.below(3) {
        let mut all = Vec::new();
        paths(&v, &mut Vec::new(), &mut all);
        let path = all[rng.below(all.len())].clone();
        if !path.is_empty() && rng.below(4) == 0 {
            let (last, parent) = path.split_last().expect("non-empty");
            if let Value::Object(m) = node_mut(&mut v, parent) {
                m.remove(last);
                continue;
            }
        }
        *node_mut(&mut v, &path) = hostile(rng);
    }
    serde_json::to_vec(&v).expect("serialize")
}

/// Flips, deletes, duplicates or truncates bytes.
fn mutate_bytes(base: &[u8], rng: &mut Rng) -> Vec<u8> {
    let mut b = base.to_vec();
    for _ in 0..=rng.below(4) {
        if b.is_empty() {
            break;
        }
        let i = rng.below(b.len());
        match rng.below(4) {
            0 => b[i] ^= 1 << rng.below(8),
            1 => {
                let end = (i + 1 + rng.below(16)).min(b.len());
                b.drain(i..end);
            }
            2 => {
                let end = (i + 1 + rng.below(16)).min(b.len());
                let chunk = b[i..end].to_vec();
                b.splice(i..i, chunk);
            }
            _ => b.truncate(i),
        }
    }
    b
}

fn assert_sane(readings: &[Reading], what: &str, round: usize) {
    for r in readings {
        for bucket in &r.buckets {
            assert!(
                bucket.used_percent.is_finite() && (0.0..=100.0).contains(&bucket.used_percent),
                "{what} round {round}: accepted used_percent {}",
                bucket.used_percent
            );
        }
    }
}

fn run(name: &str, seed: u64, mut parse: impl FnMut(&[u8]) -> Option<Vec<Reading>>) {
    let base_bytes = fixture(name);
    let base: Value = serde_json::from_slice(&base_bytes).expect("fixture is JSON");
    let mut rng = Rng(seed);
    for round in 0..ROUNDS {
        let input = if round % 2 == 0 {
            mutate_structure(&base, &mut rng)
        } else {
            mutate_bytes(&base_bytes, &mut rng)
        };
        if let Some(readings) = parse(&input) {
            assert_sane(&readings, name, round);
        }
    }
}

fn key(service: Service) -> AccountKey {
    AccountKey::new(service, Some("user@example.com"), "mutation")
}

#[test]
fn claude_usage_parser_survives_mutation() {
    for (i, name) in [
        "claude_usage.json",
        "claude_usage_model.json",
        "claude_usage_noisy.json",
    ]
    .into_iter()
    .enumerate()
    {
        run(name, 0x9E37_79B9_7F4A_7C15 + i as u64, |body| {
            claude_direct::parse_usage(
                body,
                key(Service::Claude),
                "Claude".into(),
                Some("pro".into()),
                NOW_MS,
            )
            .ok()
            .map(|r| vec![r])
        });
    }
}

#[test]
fn codex_usage_parser_survives_mutation() {
    for (i, name) in ["codex_usage_code_review.json", "codex_usage_free.json"]
        .into_iter()
        .enumerate()
    {
        run(name, 0xD1B5_4A32_D192_ED03 + i as u64, |body| {
            codex_direct::parse_usage(body, key(Service::Codex), "Codex".into(), NOW_MS)
                .ok()
                .map(|r| vec![r])
        });
    }
}

#[test]
fn orca_list_parser_survives_mutation() {
    run("orca_list.json", 0xA076_1D64_78BD_642F, |stdout| {
        orca::parse_list(stdout, NOW_MS).ok()
    });
}

/// A per-process scratch folder; removed when the test ends.
struct Scratch(PathBuf);

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn claude_login_files_survive_mutation() {
    let scratch =
        Scratch(std::env::temp_dir().join(format!("quotaglass-mutation-{}", std::process::id())));
    let home = scratch.0.clone();
    let claude_dir = home.join(".claude");
    std::fs::create_dir_all(&claude_dir).expect("scratch dir");

    let creds = fixture("claude_credentials.json");
    let config = fixture("claude_config.json");
    let creds_value: Value = serde_json::from_slice(&creds).expect("JSON");
    let config_value: Value = serde_json::from_slice(&config).expect("JSON");
    let mut rng = Rng(0xE703_7ED1_A0B4_28DB);
    for round in 0..ROUNDS / 3 {
        let (c, g) = if round % 2 == 0 {
            (
                mutate_structure(&creds_value, &mut rng),
                mutate_structure(&config_value, &mut rng),
            )
        } else {
            (
                mutate_bytes(&creds, &mut rng),
                mutate_bytes(&config, &mut rng),
            )
        };
        std::fs::write(claude_dir.join(".credentials.json"), c).expect("write");
        std::fs::write(home.join(".claude.json"), g).expect("write");
        let _ = claude_direct::read_login(&claude_dir, &home);
    }
}
