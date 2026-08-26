//! Turns a `.env` into a `sniper.json`.
//!
//! ```text
//! cargo run -p sniper --bin gen-config -- --env /path/to/.env --out sniper.json
//! ```
//!
//! Rules:
//!
//! * a provider whose API key is missing from the environment is left out entirely, so
//!   adding a provider later is one line in `.env` and a re-run;
//! * `<PROVIDER>_REGIONS` picks which regional endpoints to fire at — `all` (the default)
//!   uses every endpoint the provider publishes. Firing at every region is free, because
//!   all of them share one durable nonce and only the first to land can succeed;
//! * anything already set in the environment (`SNIPER_BUY_LAMPORTS`, tips, CU price) wins
//!   over the defaults.

use std::collections::HashMap;

use serde_json::{json, Map, Value};
use sniper::providers::{Auth, PROVIDERS};

fn main() {
    let mut env_path = ".env".to_string();
    let mut out_path = "sniper.json".to_string();
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--env" => env_path = args.next().expect("--env needs a path"),
            "--out" => out_path = args.next().expect("--out needs a path"),
            other => panic!("unknown argument {other}"),
        }
    }

    let env = read_env(&env_path);
    let get = |k: &str| env.get(k).map(|s| s.trim().to_string()).unwrap_or_default();
    let get_or = |k: &str, d: &str| {
        let v = get(k);
        if v.is_empty() {
            d.to_string()
        } else {
            v
        }
    };
    let num = |k: &str, d: u64| get(k).parse::<u64>().unwrap_or(d);

    // NODE_REGION is deliberately not consulted here. It says where the box is; it does not
    // say which endpoints to fire at, and those are different questions — firing at every
    // region is free because all variants share one durable nonce, so only the first to land
    // can succeed. The region filter is `<PROVIDER>_REGIONS`, defaulting to `all`.
    let default_tip = num("SNIPER_TIP_LAMPORTS", 2_000_000);
    let default_cu_price = num("SNIPER_CU_PRICE", 6_000_000);

    let mut providers = Vec::new();
    let mut skipped = Vec::new();

    for spec in PROVIDERS {
        let key = if spec.env_key.is_empty() {
            String::new()
        } else {
            let k = get(spec.env_key);
            if k.is_empty() {
                skipped.push((spec.name, spec.env_key, spec.signup));
                continue;
            }
            k
        };

        // regions: explicit list, else every endpoint this provider has
        let wanted = get_or(spec.env_regions, "all");
        let hosts: Vec<String> = spec
            .hosts
            .iter()
            .filter(|(region, _)| {
                wanted == "all"
                    || wanted
                        .split(',')
                        .map(|r| r.trim())
                        .any(|r| r == *region)
            })
            .map(|(_, host)| host.to_string())
            .collect();
        if hosts.is_empty() {
            eprintln!(
                "{}: no endpoint matches {}={wanted}, skipping",
                spec.name, spec.env_regions
            );
            continue;
        }

        let path = spec.path.replace("{KEY}", &key);
        let health_path = spec.health_path.replace("{KEY}", &key);

        let mut headers = Vec::new();
        if let Auth::Header(name) = spec.auth {
            headers.push(json!([name, key]));
        }

        let tip = num(
            &format!("{}_TIP_LAMPORTS", spec.name.to_uppercase().replace('-', "_")),
            default_tip.max(spec.min_tip),
        );
        let cu_price = num(
            &format!("{}_CU_PRICE", spec.name.to_uppercase().replace('-', "_")),
            default_cu_price,
        );

        let mut p = Map::new();
        p.insert("name".into(), json!(spec.name));
        p.insert("hosts".into(), json!(hosts));
        p.insert("port".into(), json!(spec.port));
        p.insert("path".into(), json!(path));
        if spec.tls {
            p.insert("tls".into(), json!(true));
        }
        p.insert("body".into(), json!(spec.body));
        if !headers.is_empty() {
            p.insert("headers".into(), json!(headers));
        }
        p.insert("health_path".into(), json!(health_path));
        p.insert("tip_lamports".into(), json!(tip));
        p.insert("cu_price".into(), json!(cu_price));
        p.insert("tip_accounts".into(), json!(spec.tips));
        providers.push(Value::Object(p));
    }

    let nonce_accounts: Vec<String> = get_or("SNIPER_NONCE_ACCOUNTS", &get("NONCE_PUBLIC_KEY"))
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();

    let cfg = json!({
        "_generated": "by `cargo run -p sniper --bin gen-config`; contains API keys, never commit",
        "keypair_path": get_or("SNIPER_KEYPAIR", "/etc/sniper/keypair.json"),
        "whitelist_path": get_or("SNIPER_WHITELIST", "whitelist.txt"),
        "whitelist_disabled": get("SNIPER_WHITELIST_DISABLED") == "1",
        "rpc_url": get_or("SNIPER_RPC_URL", &get_or("RPC_ENDPOINT", "http://127.0.0.1:8899")),
        "nonce_accounts": nonce_accounts,
        "nonce_refresh_ms": num("SNIPER_NONCE_REFRESH_MS", 300),
        "sender_spin_micros": num("SNIPER_SENDER_SPIN_MICROS", 2_000_000),
        "buy_lamports": num("SNIPER_BUY_LAMPORTS", 1_000_000_000),
        "max_dev_buy_lamports": num("SNIPER_MAX_DEV_BUY_LAMPORTS", 2_900_000_000),
        "haircut_bps": num("SNIPER_HAIRCUT_BPS", 30),
        "slippage_bps": num("SNIPER_SLIPPAGE_BPS", 100),
        "cu_limit": num("SNIPER_CU_LIMIT", 90_000),
        "sync_mode": get_or("SNIPER_SYNC_MODE", "1") != "0",
        "test_mode": get("SNIPER_TEST_MODE") == "1",
        "ghost_mode": get("SNIPER_GHOST_MODE") == "1",
        "hold_ms": num("SNIPER_HOLD_MS", 1_600),
        "position_poll_ms": num("SNIPER_POSITION_POLL_MS", 200),
        "buy_timeout_ms": num("SNIPER_BUY_TIMEOUT_MS", 30_000),
        "dry_run": get("SNIPER_DRY_RUN") == "1",
        "providers": providers,
    });

    std::fs::write(&out_path, serde_json::to_string_pretty(&cfg).unwrap())
        .unwrap_or_else(|e| panic!("write {out_path}: {e}"));

    let endpoint_total: usize = cfg["providers"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["hosts"].as_array().map(|h| h.len()).unwrap_or(0))
        .sum();

    println!("wrote {out_path}");
    println!(
        "enabled: {} providers, {} endpoints",
        cfg["providers"].as_array().unwrap().len(),
        endpoint_total
    );
    for p in cfg["providers"].as_array().unwrap() {
        println!(
            "  {:<14} {} endpoints  tip {}  cu_price {}",
            p["name"].as_str().unwrap(),
            p["hosts"].as_array().unwrap().len(),
            p["tip_lamports"],
            p["cu_price"]
        );
    }
    if !skipped.is_empty() {
        println!("\nnot enabled (no key in {env_path}):");
        for (name, key, signup) in skipped {
            println!("  {name:<14} set {key:<18} {signup}");
        }
    }
    if nonce_accounts.is_empty() {
        println!("\nWARNING: no nonce accounts. Set SNIPER_NONCE_ACCOUNTS or NONCE_PUBLIC_KEY.");
    }
}

/// Minimal `.env` reader: `KEY=value`, `#` comments, no interpolation.
fn read_env(path: &str) -> HashMap<String, String> {
    let mut out = HashMap::new();
    let Ok(raw) = std::fs::read_to_string(path) else {
        eprintln!("warning: could not read {path}, using defaults only");
        return out;
    };
    for line in raw.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((k, v)) = line.split_once('=') {
            out.insert(k.trim().to_string(), v.trim().to_string());
        }
    }
    out
}
