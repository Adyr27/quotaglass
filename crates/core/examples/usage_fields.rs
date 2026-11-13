//! Developer-only check (v0.4.0 plan Step 5a). Prints the JSON key paths and value types of the live Claude usage
//! response and of the Codex `rate_limit_reset_credits` object. It never prints a value, and it never refreshes a
//! token: an expired login stops with "expired" and nothing is sent.
//!
//! Run: `cargo run -p quotaglass-core --example usage_fields`

use quotaglass_core::sources::{claude_direct, codex_direct};
use quotaglass_core::{Env, FetchError, HeaderValueSource, Host};
use serde_json::Value;

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let env = match Env::from_system() {
        Ok(env) => env,
        Err(e) => return println!("environment: {}", describe(&e)),
    };

    println!("Claude GET /api/oauth/usage");
    print_paths(claude_usage(&env).await, None);

    println!("Codex GET /backend-api/wham/usage: rate_limit_reset_credits");
    print_paths(codex_usage(&env).await, Some("rate_limit_reset_credits"));
}

async fn claude_usage(env: &Env) -> Result<Vec<u8>, FetchError> {
    let login = claude_direct::read_login(&env.default_claude_dir, &env.home)?;
    if login.expires_at_ms <= (env.now_ms)() {
        return Err(FetchError::Expired);
    }
    env.http
        .get_json(
            Host::Anthropic,
            "/api/oauth/usage",
            &[
                ("authorization", HeaderValueSource::Bearer(&login.token)),
                (
                    "anthropic-beta",
                    HeaderValueSource::Static("oauth-2025-04-20"),
                ),
                ("accept", HeaderValueSource::Static("application/json")),
            ],
        )
        .await
}

async fn codex_usage(env: &Env) -> Result<Vec<u8>, FetchError> {
    let auth = codex_direct::read_auth(&env.default_codex_dir)?;
    let exp_ms =
        codex_direct::jwt_exp_ms(auth.token.expose()).ok_or(FetchError::Format("token exp"))?;
    if exp_ms <= (env.now_ms)() {
        return Err(FetchError::Expired);
    }
    let mut headers = vec![
        ("authorization", HeaderValueSource::Bearer(&auth.token)),
        ("accept", HeaderValueSource::Static("application/json")),
    ];
    if let Some(id) = auth.account_id.as_deref() {
        headers.push(("chatgpt-account-id", HeaderValueSource::Plain(id)));
    }
    env.http
        .get_json(Host::ChatGpt, "/backend-api/wham/usage", &headers)
        .await
}

/// Prints the paths under the top-level `key`, or under the whole body when `key` is `None`.
fn print_paths(body: Result<Vec<u8>, FetchError>, key: Option<&str>) {
    let body = match body {
        Ok(body) => body,
        Err(e) => return println!("  {}", describe(&e)),
    };
    // Only the error kind is printed, never serde's message, so no body text can reach the screen.
    let Ok(root) = serde_json::from_slice::<Value>(&body) else {
        return println!("  not JSON");
    };
    match key.map_or(Some(&root), |k| root.get(k)) {
        Some(v) => key_paths(v).iter().for_each(|line| println!("  {line}")),
        None => println!("  (absent)"),
    }
}

/// The error kind only. `FetchError` holds fixed strings and numbers, never a path or a value.
fn describe(e: &FetchError) -> String {
    match e {
        FetchError::Expired => "expired".to_string(),
        FetchError::NotDetected => "no login file".to_string(),
        other => format!("{other:?}"),
    }
}

/// One `path: type` line per leaf, sorted. Array items share the path `name[]`. Never contains a value.
fn key_paths(v: &Value) -> Vec<String> {
    fn walk(v: &Value, path: &str, out: &mut Vec<String>) {
        let kind = match v {
            Value::Object(map) if !map.is_empty() => {
                for (k, child) in map {
                    let p = if path.is_empty() {
                        k.clone()
                    } else {
                        format!("{path}.{k}")
                    };
                    walk(child, &p, out);
                }
                return;
            }
            Value::Array(items) if !items.is_empty() => {
                items
                    .iter()
                    .for_each(|item| walk(item, &format!("{path}[]"), out));
                return;
            }
            Value::Object(_) => "object",
            Value::Array(_) => "array",
            Value::String(_) => "string",
            Value::Number(_) => "number",
            Value::Bool(_) => "boolean",
            Value::Null => "null",
        };
        let path = if path.is_empty() { "(root)" } else { path };
        out.push(format!("{path}: {kind}"));
    }
    let mut out = Vec::new();
    walk(v, "", &mut out);
    out.sort();
    out.dedup();
    out
}

#[cfg(test)]
mod tests {
    use super::key_paths;
    use serde_json::json;

    #[test]
    fn key_paths_show_types_never_values() {
        let fixture = json!({
            "five_hour": {"utilization": 41.5, "resets_at": "SECRET-2026-11-14"},
            "seven_day_opus": null,
            "credits": [{"id": "SECRET-ID-1", "type": "SECRET-TYPE"}, {"id": "SECRET-ID-2"}],
            "enabled": true,
            "extra": {},
            "tags": []
        });
        let out = key_paths(&fixture);
        assert_eq!(
            out,
            [
                "credits[].id: string",
                "credits[].type: string",
                "enabled: boolean",
                "extra: object",
                "five_hour.resets_at: string",
                "five_hour.utilization: number",
                "seven_day_opus: null",
                "tags: array",
            ]
        );
        let text = out.join("\n");
        for value in ["SECRET", "41.5", "true"] {
            assert!(!text.contains(value), "value text {value:?} leaked");
        }
    }
}
