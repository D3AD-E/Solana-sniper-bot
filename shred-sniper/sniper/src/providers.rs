//! The provider catalogue: every endpoint, tip account and auth style, in one place.
//!
//! Nothing here is used at runtime — `gen_config` turns this table plus a `.env` into a
//! `sniper.json`. Keeping it as data means adding a provider or a region is a one-line
//! change, and a provider with no API key in the environment simply does not appear in the
//! generated config.
//!
//! Hostnames were resolved before being listed. Where a provider has no point of presence
//! in a region, the region is absent rather than aliased to a distant one — sending to
//! every listed endpoint is free (they share a nonce, so only one can land), but pretending
//! a New York host is a Tokyo host is not.

/// How the API key reaches the provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Auth {
    /// no key at all
    None,
    /// substituted into the path where `{KEY}` appears
    Query,
    /// sent as a header with this name
    Header(&'static str),
}

pub struct ProviderSpec {
    pub name: &'static str,
    /// environment variable holding the API key. Empty means the provider needs none.
    pub env_key: &'static str,
    /// environment variable holding a comma separated region list, e.g. `fra,ams,ny`.
    /// `all` selects every endpoint below.
    pub env_regions: &'static str,
    /// what `env_regions` means when it is not set. `all` for anything proven; empty for a
    /// transport that has never been verified against a live fire, so it stays off until
    /// someone opts in explicitly.
    pub default_regions: &'static str,
    pub auth: Auth,
    pub port: u16,
    pub tls: bool,
    /// `{KEY}` is replaced with the API key
    pub path: &'static str,
    pub health_path: &'static str,
    /// one of json_rpc | wrapped | wrapped_blox | plain_tx | batch | binary |
    /// len_prefixed_binary | udp_raw
    pub body: &'static str,
    /// documented minimum tip, lamports
    pub min_tip: u64,
    /// where to get a key
    pub signup: &'static str,
    /// (region, host) — a host may carry its own path suffix
    pub hosts: &'static [(&'static str, &'static str)],
    pub tips: &'static [&'static str],
}

pub const JITO_TIPS: &[&str] = &[
    "96gYZGLnJYVFmbjzopPSU6QiEV5fGqZNyN9nmNhvrZU5",
    "HFqU5x63VTqvQss8hp11i4wVV8bD44PvwucfZ2bU7gRe",
    "Cw8CFyM9FkoMi7K7Crf6HNQqf4uEMzpKw6QNghXLvLkY",
    "ADaUMid9yfUytqMBgopwjb2DTLSokTSzL1zt6iGPaS49",
    "DfXygSm4jCyNCybVYYK6DwvWqjKee8pbDmJGcLWNDXjh",
    "ADuUkR4vqLUMWXxW9gh6D6L8pMSawimctcNZ5pGwDcEt",
    "DttWaMuVvTiduZRnguLF7jNxTgiMBZ1hyAumKUiL2KRL",
    "3AVi9Tg9Uo68tJfuvoKvqKNWKkC5wPdSSdeBnizKZ6jT",
];

pub const HELIUS_TIPS: &[&str] = &[
    "4ACfpUFoaSD9bfPdeu6DBt89gB6ENTeHBXCAi87NhDEE",
    "D2L6yPZ2FmmmTKPgzaMKdhu6EWZcTpLy1Vhx8uvZe7NZ",
    "9bnz4RShgq1hAnLnZbP8kbgBg1kEmcJBYQq3gQbmnSta",
    "5VY91ws6B2hMmBFRsXkoAAdsPHBJwRfBht4DXox3xkwn",
    "2nyhqdwKcJZR2vcqCyrYsaPVdAnFoJjiksCXJ7hfEYgD",
    "2q5pghRs6arqVjRvT5gfgWfWcHWmw1ZuCzphgd5KfWGJ",
    "wyvPkWjVZz1M8fHQnMMCDTQDbkManefNNhweYk5WkcF",
    "3KCKozbAaF75qEU33jtzozcJ29yJuaLJTy2jFdzUY8bT",
    "4vieeGHPYPG2MmyPRcYjdiDmmhN3ww7hsFNap8pVN3Ey",
    "4TQLFNWK8AovT1gFvda5jfw2oJeRMKEmw7aH6MGBJ3or",
];

/// All 21 accounts 0slot publishes, not the 5 we used to carry. None of the old five were
/// wrong — they are all still in the published table — but rotating a launch across 5
/// accounts instead of 21 concentrates the write lock, which is the same contention
/// bloXroute explicitly asks callers to spread. `tip_accounts` rotates per launch.
pub const ZEROSLOT_TIPS: &[&str] = &[
    "6fQaVhYZA4w3MBSXjJ81Vf6W1EDYeUPXpgVQ6UQyU1Av",
    "4HiwLEP2Bzqj3hM2ENxJuzhcPCdsafwiet3oGkMkuQY4",
    "7toBU3inhmrARGngC7z6SjyP85HgGMmCTEwGNRAcYnEK",
    "8mR3wB1nh4D6J9RUCugxUpc6ya8w38LPxZ3ZjcBhgzws",
    "6SiVU5WEwqfFapRuYCndomztEwDjvS5xgtEof3PLEGm9",
    "TpdxgNJBWZRL8UXF5mrEsyWxDWx9HQexA9P1eTWQ42p",
    "D8f3WkQu6dCF33cZxuAsrKHrGsqGP2yvAHf8mX6RXnwf",
    "GQPFicsy3P3NXxB5piJohoxACqTvWE9fKpLgdsMduoHE",
    "Ey2JEr8hDkgN8qKJGrLf2yFjRhW7rab99HVxwi5rcvJE",
    "4iUgjMT8q2hNZnLuhpqZ1QtiV8deFPy2ajvvjEpKKgsS",
    "3Rz8uD83QsU8wKvZbgWAPvCNDU6Fy8TSZTMcPm3RB6zt",
    "DiTmWENJsHQdawVUUKnUXkconcpW4Jv52TnMWhkncF6t",
    "HRyRhQ86t3H4aAtgvHVpUJmw64BDrb61gRiKcdKUXs5c",
    "J9BMEWFbCBEjtQ1fG5Lo9kouX1HfrKQxeUxetwXrifBw",
    "8U1JPQh3mVQ4F5jwRdFTBzvNRQaYFQppHQYoH38DJGSQ",
    "7y4whZmw388w1ggjToDLSBLv47drw5SUXcLk6jtmwixd",
    "Eb2KpSC8uMt9GmzyAEm5Eb1AAAgTjRaXWFjKyFXHZxF3",
    "FCjUJZ1qozm1e8romw216qyfQMaaWKxWsuySnumVCCNe",
    "ENxTEjSQ1YabmUpXAdCgevnHQ9MHdLv8tzFiuiYJqa13",
    "6rYLG55Q9RpsPGvqdPNJs4z5WTxJVatMB8zV3WJhs5EK",
    "Cix2bHfqPcKcM233mzxbLk14kSggUUiz2A87fJtGivXr",
];

pub const ASTRALANE_TIPS: &[&str] = &[
    "astrazznxsGUhWShqgNtAdfrzP2G83DzcWVJDxwV9bF",
    "astra4uejePWneqNaJKuFFA8oonqCE1sqF6b45kDMZm",
    "astra9xWY93QyfG6yM8zwsKsRodscjQ2uU2HKNL5prk",
    "astraRVUuTHjpwEVvNBeQEgwYx9w9CFyfxjYoobCZhL",
    "astraEJ2fEj8Xmy6KLG7B3VfbKfsHXhHrNdCQx7iGJK",
    "astraubkDw81n4LuutzSQ8uzHCv4BhPVhfvTcYv8SKC",
    "astraZW5GLFefxNPAatceHhYjfA1ciq9gvfEg2S47xk",
    "astrawVNP4xDBKT7rAdxrLYiTSTdqtUr63fSMduivXK",
    "AstrA1ejL4UeXC2SBP4cpeEmtcFPZVLxx3XGKXyCW6to",
    "AsTra79FET4aCKWspPqeSFvjJNyp96SvAnrmyAxqg5b7",
    "AstrABAu8CBTyuPXpV4eSCJ5fePEPnxN8NqBaPKQ9fHR",
    "AsTRADtvb6tTmrsqULQ9Wji9PigDMjhfEMza6zkynEvV",
    "AsTRAEoyMofR3vUPpf9k68Gsfb6ymTZttEtsAbv8Bk4d",
    "AStrAJv2RN2hKCHxwUMtqmSxgdcNZbihCwc1mCSnG83W",
    "Astran35aiQUF57XZsmkWMtNCtXGLzs8upfiqXxth2bz",
    "AStRAnpi6kFrKypragExgeRoJ1QnKH7pbSjLAKQVWUum",
    "ASTRaoF93eYt73TYvwtsv6fMWHWbGmMUZfVZPo3CRU9C",
];

pub const NODE1_TIPS: &[&str] = &[
    "node1PqAa3BWWzUnTHVbw8NJHC874zn9ngAkXjgWEej",
    "node1UzzTxAAeBTpfZkQPJXBAqixsbdth11ba1NXLBG",
    "node1Qm1bV4fwYnCurP8otJ9s5yrkPq7SPZ5uhj3Tsv",
    "node1PUber6SFmSQgvf2ECmXsHP5o3boRSGhvJyPMX1",
    "node1AyMbeqiVN6eoQzEAwCA6Pk826hrdqdAHR7cdJ3",
    "node1YtWCoTwwVYTFLfS19zquRQzYX332hs1HEuRBjC",
];

pub const NEXTBLOCK_TIPS: &[&str] = &[
    "NextbLoCkVtMGcV47JzewQdvBpLqT9TxQFozQkN98pE",
    "NexTbLoCkWykbLuB1NkjXgFWkX9oAtcoagQegygXXA2",
    "NeXTBLoCKs9F1y5PJS9CKrFNNLU1keHW71rfh7KgA1X",
    "NexTBLockJYZ7QD7p2byrUa6df8ndV2WSd8GkbWqfbb",
    "neXtBLock1LeC67jYd1QdAa32kbVeubsfPNTJC1V5At",
    "nEXTBLockYgngeRmRrjDV31mGSekVPqZoMGhQEZtPVG",
    "NEXTbLoCkB51HpLBLojQfpyVAMorm3zzKg7w9NFdqid",
    "nextBLoCkPMgmG8ZgJtABeScP35qLa2AMCNKntAP7Xc",
];

pub const NOZOMI_TIPS: &[&str] = &[
    "TEMPaMeCRFAS9EKF53Jd6KpHxgL47uWLcpFArU1Fanq",
    "noz3jAjPiHuBPqiSPkkugaJDkJscPuRhYnSpbi8UvC4",
    "noz3str9KXfpKknefHji8L1mPgimezaiUyCHYMDv1GE",
    "noz6uoYCDijhu1V7cutCpwxNiSovEwLdRHPwmgCGDNo",
    "noz9EPNcT7WH6Sou3sr3GGjHQYVkN3DNirpbvDkv9YJ",
    "nozc5yT15LazbLTFVZzoNZCwjh3yUtW86LoUyqsBu4L",
    "nozFrhfnNGoyqwVuwPAW4aaGqempx4PU6g6D9CJMv7Z",
    "nozievPk7HyK1Rqy1MPJwVQ7qQg2QoJGyP71oeDwbsu",
    "noznbgwYnBLDHu8wcQVCEw6kDrXkPdKkydGJGNXGvL7",
    "nozNVWs5N8mgzuD3qigrCG2UoKxZttxzZ85pvAQVrbP",
    "nozpEGbwx4BcGp6pvEdAh1JoC2CQGZdU6HbNP1v2p6P",
    "nozrhjhkCr3zXT3BiT4WCodYCUFeQvcdUkM7MqhKqge",
    "nozrwQtWhEdrA6W8dkbt9gnUaMs52PdAv5byipnadq3",
    "nozUacTVWub3cL4mJmGCYjKZTnE9RbdY5AP46iQgbPJ",
    "nozWCyTPppJjRuw2fpzDhhWbW355fzosWSzrrMYB1Qk",
    "nozWNju6dY353eMkMqURqwQEoM3SFgEKC6psLCSfUne",
    "nozxNBgWohjR75vdspfxR5H9ceC7XXH99xpxhVGt3Bb",
];

/// The 17 addresses in bloXroute's published tip table. The two that used to head this list
/// are not in it: `95cfoy47...` appears nowhere in their docs at all, and `HWEoBxYs...`
/// survives only in SDK code samples that themselves say "as of 2/12/2024 ... check docs to
/// see latest up to date tip wallet". A tip to a superseded address still leaves the wallet
/// but buys no priority, so it is a silent fee leak.
/// bloXroute asks for rotation across the list to avoid write-lock contention, which is
/// what `tip_accounts` already does per launch.
pub const BLOX_TIPS: &[&str] = &[
    "3UQUKjhMKaY2S6bjcQD6yHB7utcZt5bfarRCmctpRtUd",
    "FogxVNs6Mm2w9rnGL1vkARSwJxvLE8mujTv3LK8RnUhF",
    "bLx7MvxGaKdKL7mEbpk9tC79z6MnBSJoJkuaEAPu6Nd",
    "bLx7XBqSg3LUPVf1bRgCnkJmgVZR8QEgDJBPqcRLHvp",
    "bLx8KeZxinPwy6kkUgyzMLeqb2ARNsWjADG1dhSsVba",
    "bLxADBknoNj8WAGw2W6GBYeq848Xx6ajhaymV1YvrHm",
    "bLxAc88vRBwvcUQJEgcxNfBLvHPikY4csNsUmPeWea2",
    "bLxQ88oCiTsL8Xj4YWekKi1hjrgmbE3J3FFZ2xZHR3h",
    "bLxS7NoLuynNRJ4mCnEE2YbtwJFttYsEyp2ME7rp2yt",
    "bLxW6mCov7VEbrKc3S9tcBRcfSzRnLCbNp3Dfn3SJG5",
    "bLxXSGXs4mYPTC5okZXed1qzvjNwNJ48QJ82hT2V7w7",
    "bLxYi3vojbbB7hVzVDVTdBLVPhp7GJ3ZB3BwdK5sFXi",
    "bLxhLPgBXtUpX4b1bH3HatuMGMSKT9GnwtuCGiMSAqe",
    "bLxpY1mniuFW4PgkNA4JiNxoeKHFszryi6tNgyZAiAA",
    "bLxuETxd2tgWxBALNwPzAfHhsik4BzD3nrEBCiPNZQD",
    "bLxuL2gK5FW7xfahvwLrxLyW76vcCpNsKQY2CmnE6kV",
    "bLxv4Hnub7nDJWHs8s17o9bGU65Bnx6Yqp2fqtMgHmm",
];

pub const FLASHBLOCK_TIPS: &[&str] = &[
    "FLaShB3iXXTWE1vu9wQsChUKq3HFtpMAhb8kAh1pf1wi",
    "FLashhsorBmM9dLpuq6qATawcpqk1Y2aqaZfkd48iT3W",
    "FLaSHJNm5dWYzEgnHJWWJP5ccu128Mu61NJLxUf7mUXU",
    "FLaSHR4Vv7sttd6TyDF4yR1bJyAxRwWKbohDytEMu3wL",
    "FLASHRzANfcAKDuQ3RXv9hbkBy4WVEKDzoAgxJ56DiE4",
    "FLasHstqx11M8W56zrSEqkCyhMCCpr6ze6Mjdvqope5s",
    "FLAShWTjcweNT4NSotpjpxAkwxUr2we3eXQGhpTVzRwy",
    "FLasHXTqrbNvpWFB6grN47HGZfK6pze9HLNTgbukfPSk",
    "FLAshyAyBcKb39KPxSzXcepiS8iDYUhDGwJcJDPX4g2B",
    "FLAsHZTRcf3Dy1APaz6j74ebdMC6Xx4g6i9YxjyrDybR",
];

/// Falcon (Corvus Labs). Their tip rules are stricter than most: the tip must be ONE
/// top-level System `transfer` (not `transferWithSeed`, not a CPI, not split across two
/// instructions) of at least 1,000,000 lamports, to an account present in the transaction's
/// STATIC keys — an address-lookup-table entry does not count. Our template's instruction 1
/// is exactly that, so it already satisfies all four rules.
pub const FALCON_TIPS: &[&str] = &[
    "Fa1con11xLjPddfzRwRUB16sbFZggp2JeJkCeWREyR8X",
    "Fa1con11TM1RuAQzbQzYjTy4Ekfap9Lnc9fnEbQYEd6Q",
    "Fa1con113Bvi76nS5AzUiRDC2fqjfzkNMUNRLgQybMYt",
    "Fa1con1QGHJK232s8yZpzZZwqPexnAKcoyKj626LNsMv",
    "Fa1con1zUzb6qJVFz5tNkPq1Ahm8H1qKW7Q48252QbkQ",
    "Fa1con16d3MSwd3SAiwvr2LwgkpE7ot8zntbpuec8HAx",
    "Fa1con1i7mpa7Qc6epYJ6r4P9AbU77DFFz173r59Df1x",
    "Fa1con18nWn8TdAGL7JX8PertfMUGVSc899NawokJ4Bq",
    "Fa1con1GKusK2EqsfzrDzGPaYZSxQtFGzJiRMMU9Zm2g",
    "Fa1con1RDwVwM9VrJ53CwVefD3VU9c58EMpDawV7fLMi",
];

pub const BLOCKRAZOR_TIPS: &[&str] = &[
    "FjmZZrFvhnqqb9ThCuMVnENaM3JGVuGWNyCAxRJcFpg9",
    "6No2i3aawzHsjtThw81iq1EXPJN6rh8eSJCLaYZfKDTG",
    "A9cWowVAiHe9pJfKAj3TJiN9VpbzMUq6E4kEvf5mUT22",
    "Gywj98ophM7GmkDdaWs4isqZnDdFCW7B46TXmKfvyqSm",
    "68Pwb4jS7eZATjDfhmTXgRJjCiZmw1L7Huy4HNpnxJ3o",
    "4ABhJh5rZPjv63RBJBuyWzBK3g9gWMUQdTZP2kiW31V9",
    "B2M4NG5eyZp5SBQrSdtemzk5TqVuaWGQnowGaCBt8GyM",
    "5jA59cXMKQqZAVdtopv8q3yyw9SYfiE3vUCbt7p8MfVf",
    "5YktoWygr1Bp9wiS1xtMtUki1PeYuuzuCF98tqwYxf61",
    "295Avbam4qGShBYK7E9H5Ldew4B3WyJGmgmXfiWdeeyV",
    "EDi4rSy2LZgKJX74mbLTFk4mxoTgT6F7HxxzG2HBAFyK",
    "BnGKHAC386n4Qmv9xtpBVbRaUTKixjBe3oagkPFKtoy6",
    "Dd7K2Fp7AtoN8xCghKDRmyqr5U169t48Tw5fEd3wT9mq",
    "AP6qExwrbRgBAVaehg4b5xHENX815sMabtBzUzVB4v8S",
];

pub const PROVIDERS: &[ProviderSpec] = &[
    ProviderSpec {
        name: "jito",
        env_key: "",
        env_regions: "JITO_REGIONS",
        default_regions: "all",
        auth: Auth::None,
        port: 443,
        tls: true,
        path: "/api/v1/transactions?bundleOnly=true",
        // jito publishes no health endpoint — every path 404s. That is still a perfectly
        // good keep-alive: the probe exists to put bytes on the socket and read a reply, and
        // a 404 comes back on a connection the server leaves open. Do not "fix" this to a
        // 200-returning path; there isn't one.
        health_path: "/",
        body: "json_rpc",
        min_tip: 1_000,
        signup: "https://docs.jito.wtf/ (no key required)",
        hosts: &[
            ("ny", "ny.mainnet.block-engine.jito.wtf"),
            ("fra", "frankfurt.mainnet.block-engine.jito.wtf"),
            ("ams", "amsterdam.mainnet.block-engine.jito.wtf"),
            ("dublin", "dublin.mainnet.block-engine.jito.wtf"),
            ("slc", "slc.mainnet.block-engine.jito.wtf"),
            ("tyo", "tokyo.mainnet.block-engine.jito.wtf"),
            ("sgp", "singapore.mainnet.block-engine.jito.wtf"),
            ("lon", "london.mainnet.block-engine.jito.wtf"),
        ],
        tips: JITO_TIPS,
    },
    ProviderSpec {
        name: "helius-sender",
        env_key: "",
        env_regions: "HELIUS_SENDER_REGIONS",
        default_regions: "all",
        auth: Auth::None,
        port: 80,
        tls: false,
        path: "/fast",
        // GET /ping -> 200 on every sender region. helius hangs up after only TEN seconds
        // idle — the tightest window of any provider here and what sets KEEPALIVE_SECS.
        health_path: "/ping",
        body: "json_rpc",
        min_tip: 1_000_000,
        signup: "https://www.helius.dev/docs/sending-transactions/sender (key optional)",
        hosts: &[
            ("ny", "ewr-sender.helius-rpc.com"),
            ("fra", "fra-sender.helius-rpc.com"),
            ("ams", "ams-sender.helius-rpc.com"),
            ("lon", "lon-sender.helius-rpc.com"),
            ("slc", "slc-sender.helius-rpc.com"),
            ("tyo", "tyo-sender.helius-rpc.com"),
            ("sgp", "sg-sender.helius-rpc.com"),
        ],
        tips: HELIUS_TIPS,
    },
    ProviderSpec {
        name: "0slot",
        env_key: "SLOT_CONNECTION_KEY",
        env_regions: "SLOT_REGIONS",
        default_regions: "all",
        auth: Auth::Query,
        port: 80,
        tls: false,
        // `/txb` is 0slot's Binary-Tx path: the request body is the raw serialized
        // transaction and nothing else ("this eliminates unnecessary both client-side and
        // server-side encoding/decoding"). It needs no special headers at all — not even the
        // content type — but sending octet-stream is harmless and keeps it on the same
        // `BodyFormat::Binary` code path as blockrazor, astralane and falcon.
        // The older `/` JSON-RPC route and the `/txn` base64-plaintext route both still work
        // and are both strictly larger on the wire.
        path: "/txb?api-key={KEY}",
        // NOT `/?api-key=...`. 0slot rate-limits at 5 TPS on the standard plan and an
        // authenticated probe every KEEPALIVE_SECS spends that budget on nothing. Their
        // keep-alive doc is explicit that a request with the key omitted "does not count
        // toward TPS calculations", and `/health` is the endpoint they name for it.
        health_path: "/health",
        body: "binary",
        min_tip: 1_000_000,
        signup: "https://0slot.trade/",
        // Four of these resolve to one Cloudflare anycast pair (172.66.40.254 /
        // 172.66.43.2): `ny`, `ams`, `jp` and `la` are proxied, exactly like nozomi's
        // digit-less aliases. Where 0slot runs a direct host we use it — `de2` and `ny2` are
        // bare metal (64.130.32.201 and 207.148.24.122) and both answer on plain :80. There
        // is no published `ams2`/`jp2`/`la2`, so those three stay on the proxy until 0slot
        // gives us direct names; they are the slowest entries in this table.
        hosts: &[
            ("ny", "ny2.0slot.trade"),
            ("ny", "ny.0slot.trade"),
            ("fra", "de2.0slot.trade"),
            ("ams", "ams.0slot.trade"),
            ("tyo", "jp.0slot.trade"),
            ("la", "la.0slot.trade"),
        ],
        tips: ZEROSLOT_TIPS,
    },
    ProviderSpec {
        name: "astralane",
        env_key: "ASTRA_KEY",
        env_regions: "ASTRA_REGIONS",
        default_regions: "all",
        auth: Auth::Query,
        port: 80,
        tls: false,
        // `/irisb` is astralane's binary route: `Content-Type: application/octet-stream`,
        // the body is the raw serialized transaction, and the operation is chosen with a
        // `method` query parameter rather than a JSON envelope. Their own reasoning for it is
        // ours: it removes "Base64 Encoding/Decoding Overhead" and "Packet Splitting due to
        // reduced data size". `/iris2` (base64 plaintext) sits between this and `/iris`.
        //
        // Deliberately NOT set: `mev-protect=true` and `swqos-only=true`. Both default false.
        // mev-protect routes around validators, which costs a slot; swqos-only narrows to a
        // single path. Either would trade landing speed for protection we do not want on a
        // create-block snipe.
        path: "/irisb?api-key={KEY}&method=sendTransaction",
        // NOT `/iris?api-key={KEY}`. The probe is a GET, and a GET to `/iris` answers
        // `400 Bad Request` **with `Connection: close`** — so every keep-alive tick tore down
        // the very connection it exists to preserve, and astralane paid a fresh TCP handshake
        // on every launch. `--reuse-after` catches it; a plain reachability probe does not,
        // because closing is not an error.
        //
        // A GET to the binary route answers `405 Method Not Allowed` with
        // `Connection: keep-alive` and an empty body, which is exactly what a probe wants: it
        // exercises the same route we submit to, and the reply is a few dozen bytes. The
        // status does not matter (jito and nextblock 404 by design) — the Connection header
        // does.
        health_path: "/irisb?api-key={KEY}&method=getHealth",
        body: "binary",
        min_tip: 10_000,
        signup: "https://astralane.gitbook.io/docs (portal.astralane.io)",
        // Five of these were missing. Frankfurt and Amsterdam each run a second datacenter
        // (`fr2`, and `ams2` on Cherry Servers) and each is its own race entry, the way
        // blockrazor's three Frankfurts are. Limburg and Lithuania are their own metros, not
        // aliases. All ten resolve to distinct addresses and match the IPv4s astralane
        // publishes.
        //
        // `edge.astralane.io` is deliberately absent even though their docs recommend
        // broadcasting to it: it resolves to Cloudflare (104.20.38.198 / 172.66.145.104), so
        // it is a proxy hop, not a submission host. bloxroute's `global` is kept because it
        // is the opposite case — it answers on five of bloXroute's own addresses.
        hosts: &[
            ("ny", "ny.gateway.astralane.io"),
            ("fra", "fr.gateway.astralane.io"),
            ("fra", "fr2.gateway.astralane.io"),
            ("ams", "ams.gateway.astralane.io"),
            ("ams", "ams2.gateway.astralane.io"),
            ("lim", "lim.gateway.astralane.io"),
            ("lit", "lit.gateway.astralane.io"),
            ("tyo", "jp.gateway.astralane.io"),
            ("sgp", "sg.gateway.astralane.io"),
            ("la", "la.gateway.astralane.io"),
        ],
        tips: ASTRALANE_TIPS,
    },
    ProviderSpec {
        name: "node1",
        env_key: "NODE_ONE_KEY",
        env_regions: "NODE1_REGIONS",
        default_regions: "all",
        auth: Auth::Header("api-key"),
        port: 80,
        tls: false,
        path: "/",
        health_path: "/ping",
        body: "json_rpc",
        min_tip: 1_000_000,
        signup: "https://node1.me/",
        hosts: &[
            ("ny", "ny.node1.me"),
            ("fra", "fra.node1.me"),
            ("ams", "ams.node1.me"),
            ("lon", "lon.node1.me"),
            ("tyo", "tk.node1.me"),
        ],
        tips: NODE1_TIPS,
    },
    ProviderSpec {
        name: "nextblock",
        env_key: "NEXTBLOCK_KEY",
        env_regions: "NEXTBLOCK_REGIONS",
        default_regions: "all",
        auth: Auth::Header("Authorization"),
        port: 80,
        tls: false,
        path: "/api/v2/submit",
        // /health exists but 401s for our key; `/` 404s and answers `Connection: keep-alive`,
        // which is all the probe needs. Any reply keeps the socket warm.
        health_path: "/",
        body: "wrapped",
        min_tip: 1_000_000,
        signup: "https://docs.nextblock.io/ (t.me/nextblock_support)",
        hosts: &[
            ("ny", "ny.nextblock.io"),
            ("fra", "fra.nextblock.io"),
            ("ams", "ams.nextblock.io"),
            ("dublin", "dublin.nextblock.io"),
            ("slc", "slc.nextblock.io"),
            ("tyo", "tokyo.nextblock.io"),
            ("sgp", "sgp.nextblock.io"),
            ("lon", "london.nextblock.io"),
            // nextblock publishes nine regions and we carried eight. Vilnius is a real host
            // (88.216.197.109), not an alias of Frankfurt or Dublin.
            ("vln", "vilnius.nextblock.io"),
        ],
        tips: NEXTBLOCK_TIPS,
    },
    ProviderSpec {
        name: "nozomi",
        env_key: "NOZOMI_KEY",
        env_regions: "NOZOMI_REGIONS",
        default_regions: "all",
        auth: Auth::Query,
        port: 80,
        tls: false,
        // Nozomi publishes three submission routes and this is the one they name as fastest,
        // in their own words: "Use Batch Send over a direct `http://` endpoint … Plain HTTP
        // avoids per-transaction TLS encryption; batch avoids JSON and per-request overhead",
        // and it is "the fastest option even for a single transaction".
        //
        //   POST /                      JSON-RPC, base64 + envelope   (what we used to send)
        //   POST /api/sendTransaction2  base64 as text/plain
        //   POST /api/sendBatch         [u16 BE len][tx bytes], octet-stream   <- this
        //
        // For our ~1,043 byte transaction that is 1,045 bytes on the wire against ~1,450, and
        // no base64 pass in the hot path. Documented limits are 16 transactions, 66..=1232
        // bytes each, 19,744 byte body; we send one. The reply is an empty 200 and carries no
        // signature, which costs us nothing — we already know the signature and never read it
        // back.
        path: "/api/sendBatch?c={KEY}",
        // nozomi closes an idle connection after 65s and publishes a lightweight GET /ping
        // for exactly this. Without it the sender's probe skips nozomi and every launch
        // pays a fresh TCP handshake.
        health_path: "/ping",
        body: "len_prefixed_binary",
        min_tip: 1_000_000,
        signup: "https://use.temporal.xyz/ (dashboard issues the key)",
        hosts: &[
            ("ny", "ewr1.nozomi.temporal.xyz"),
            ("fra", "fra2.nozomi.temporal.xyz"),
            ("ams", "ams1.nozomi.temporal.xyz"),
            ("lon", "lon1.nozomi.temporal.xyz"),
            ("la", "lax1.nozomi.temporal.xyz"),
            ("tyo", "tyo1.nozomi.temporal.xyz"),
            ("sgp", "sgp1.nozomi.temporal.xyz"),
            ("pit", "pit1.nozomi.temporal.xyz"),
            ("ash", "ash1.nozomi.temporal.xyz"),
        ],
        tips: NOZOMI_TIPS,
    },
    ProviderSpec {
        name: "bloxroute",
        env_key: "BLOXROUTE_KEY",
        env_regions: "BLOXROUTE_REGIONS",
        default_regions: "all",
        // the auth header is the raw base64 `accountID:secret` value, no `Bearer` prefix
        auth: Auth::Header("Authorization"),
        // plain HTTP, like every other keyed provider here: rustls owns its own record
        // framing, so a TLS endpoint cannot go through the io_uring batch writer and costs
        // one write syscall per region instead of one batched submit for all of them. The
        // tradeoff is that the auth header crosses the wire in the clear — flip both fields
        // back to 443/true to undo it.
        port: 80,
        tls: false,
        path: "/api/v2/submit",
        // GET /health returns `ok` on every region and needs no auth
        health_path: "/health",
        // not plain `wrapped`: bloXroute's submitProtection default holds the transaction
        // for up to four slots, and this format also carries `useStakedRPCs`, which is what
        // makes bloxroute worth a race slot at all. See BodyFormat::WrappedBlox.
        body: "wrapped_blox",
        min_tip: 1_000_000,
        signup: "https://bloxroute.com/products/solana-trader-api/",
        // `la.solana.dex.blxrbdn.com` is gone from the published region table and resolves
        // to the same address as `ny` — firing at it was a duplicate send to New York, not
        // a Los Angeles entry. `global` is the edge endpoint that routes to the nearest
        // submission POP; worth keeping while we are not colocated.
        hosts: &[
            ("ny", "ny.solana.dex.blxrbdn.com"),
            ("fra", "germany.solana.dex.blxrbdn.com"),
            ("ams", "amsterdam.solana.dex.blxrbdn.com"),
            ("lon", "uk.solana.dex.blxrbdn.com"),
            ("tyo", "tokyo.solana.dex.blxrbdn.com"),
            ("global", "global.solana.dex.blxrbdn.com"),
        ],
        tips: BLOX_TIPS,
    },
    ProviderSpec {
        name: "flashblock",
        env_key: "FLASHBLOCK_KEY",
        env_regions: "FLASHBLOCK_REGIONS",
        default_regions: "all",
        auth: Auth::Header("Authorization"),
        port: 80,
        tls: false,
        path: "/api/v2/submit-batch",
        // flashblock documents `GET /` as the keep-alive and caps a connection at 30s idle
        health_path: "/",
        body: "batch",
        // documented floor is 0.0001 SOL, same as blockrazor
        min_tip: 100_000,
        signup: "https://flashblock.trade/ (doc.flashblock.trade, t.me/FlashBlock_support)",
        hosts: &[
            ("ny", "ny.flashblock.trade"),
            ("fra", "fra.flashblock.trade"),
            ("ams", "ams.flashblock.trade"),
            ("lon", "london.flashblock.trade"),
            ("slc", "slc.flashblock.trade"),
            ("tyo", "tokyo.flashblock.trade"),
            ("sgp", "singapore.flashblock.trade"),
        ],
        tips: FLASHBLOCK_TIPS,
    },
    ProviderSpec {
        name: "blockrazor",
        env_key: "BLOCKRAZOR_KEY",
        env_regions: "BLOCKRAZOR_REGIONS",
        default_regions: "all",
        // the key goes in BOTH places on purpose: `/v2/sendBinaryTransaction` reads it from
        // the query string, while the `/health` keep-alive is a GET that only carries
        // headers and 403s without `apikey`.
        auth: Auth::Header("apikey"),
        // plain HTTP on :443. That is genuinely what blockrazor publishes, not a typo.
        port: 443,
        tls: false,
        // binary submission: raw transaction bytes, no base64 and no JSON envelope, which is
        // ~26% fewer bytes on the wire than `/sendTransaction` and no encode in the hot path.
        path: "/v2/sendBinaryTransaction?auth={KEY}",
        health_path: "/health",
        body: "binary",
        // blockrazor's documented floor is 0.0001 SOL, the lowest of any provider here
        min_tip: 100_000,
        signup: "https://blockrazor.io/ (docs.blockrazor.io)",
        // several metros run more than one datacenter and each is its own race entry
        hosts: &[
            ("ny", "newyork.solana.blockrazor.xyz"),
            ("fra", "frankfurt.solana.blockrazor.xyz"),
            ("fra", "frankfurt-allnodes.solana.blockrazor.xyz"),
            ("fra", "frankfurt-cherryservers.solana.blockrazor.xyz"),
            ("ams", "amsterdam.solana.blockrazor.xyz"),
            ("ams", "amsterdam-cherryservers.solana.blockrazor.xyz"),
            ("lon", "london.solana.blockrazor.xyz"),
            ("tyo", "tokyo.solana.blockrazor.xyz"),
            ("sgp", "singapore.solana.blockrazor.xyz"),
            ("la", "losangeles.solana.blockrazor.xyz"),
            ("tor", "toronto.solana.blockrazor.xyz"),
        ],
        tips: BLOCKRAZOR_TIPS,
    },
    ProviderSpec {
        name: "falcon",
        env_key: "FALCON_KEY",
        env_regions: "FALCON_REGIONS",
        default_regions: "all",
        // `?api-key=<uuid>` on every HTTP route. No header form exists.
        auth: Auth::Query,
        port: 80,
        tls: false,
        // Falcon publishes four transports; this is the fastest one that fits a warm HTTP
        // connection. `/binary` takes the serialized transaction as the entire body — no
        // base64, no JSON envelope — so it reuses BodyFormat::Binary exactly as blockrazor
        // does. The alternatives: `/plaintext` (base64, bigger) and JSON-RPC (base64 plus an
        // envelope, bigger still).
        //
        // Strictly faster still is their native UDP on :9000 — one datagram of
        // `16-byte raw UUID || transaction`, no envelope and no reply, ever. It is not wired
        // up because it needs a new non-stream transport in `Conn`, and because it answers
        // nothing there is no way to verify it short of a funded live fire. The win over
        // this route is mostly tail latency (no TCP stall or retransmit), not median.
        path: "/binary?api-key={KEY}",
        // GET /health answers 200 on every region and needs no key
        health_path: "/health",
        body: "binary",
        min_tip: 1_000_000,
        signup: "https://docs.corvus-labs.io/falcon (dashboard issues the UUID)",
        hosts: &[
            ("fra", "fra.falcon.wtf"),
            ("ams", "ams.falcon.wtf"),
            ("lon", "lon.falcon.wtf"),
            ("ny", "nyc.falcon.wtf"),
            ("tyo", "tyo.falcon.wtf"),
            ("dublin", "dub.falcon.wtf"),
            ("sgp", "sgp.falcon.wtf"),
            ("slc", "slc.falcon.wtf"),
            // Šiauliai, Lithuania — their own metro, not an alias of anything
            ("sqq", "sqq.falcon.wtf"),
        ],
        tips: FALCON_TIPS,
    },
    // Falcon's native UDP transport, the fastest thing any provider in this table publishes:
    // one datagram of `16-byte raw UUID || transaction` straight to :9000. No HTTP, no
    // envelope, no request head, no TCP — and **no reply, ever**.
    //
    // That last property is why this is its own provider entry and why it ships DISABLED.
    //   * It cannot be probed. A keep-alive is meaningless on a connectionless socket, and
    //     `every_provider_declares_a_keepalive_path` exempts UDP for that reason.
    //   * It cannot be verified short of a funded live fire. A wrong key, a wrong prefix
    //     length or a silently truncated datagram all look identical to success: the sender
    //     writes the bytes, the kernel accepts them, and nothing ever comes back.
    // Running it alongside the TCP `/binary` entry above costs one extra datagram per region
    // and cannot lose a race the TCP path would have won — both carry the same durable
    // nonce, so at most one lands.
    //
    // To turn it on: set `FALCON_UDP_REGIONS` (e.g. `all`, or a short list) in `.env`, fire
    // once in SNIPER_TEST_MODE, and confirm on-chain that the buy landed from this path
    // before trusting it. Leaving the variable unset omits the provider entirely.
    ProviderSpec {
        name: "falcon-udp",
        env_key: "FALCON_KEY",
        env_regions: "FALCON_UDP_REGIONS",
        // unset means off — the opposite of every other provider here, on purpose
        default_regions: "",
        // the key is the datagram prefix, not a header or a query parameter
        auth: Auth::None,
        port: 9000,
        tls: false,
        path: "",
        // a datagram socket has no connection to keep warm
        health_path: "",
        body: "udp_raw",
        min_tip: 1_000_000,
        signup: "https://docs.corvus-labs.io/falcon (same UUID as the HTTP route)",
        hosts: &[
            ("fra", "fra.falcon.wtf"),
            ("ams", "ams.falcon.wtf"),
            ("lon", "lon.falcon.wtf"),
            ("ny", "nyc.falcon.wtf"),
            ("tyo", "tyo.falcon.wtf"),
            ("dublin", "dub.falcon.wtf"),
            ("sgp", "sgp.falcon.wtf"),
            ("slc", "slc.falcon.wtf"),
            ("sqq", "sqq.falcon.wtf"),
        ],
        tips: FALCON_TIPS,
    },
];

#[cfg(test)]
mod tests {
    use super::*;
    use solana_sdk::pubkey::Pubkey;
    use std::str::FromStr;

    #[test]
    fn every_tip_account_is_a_valid_pubkey() {
        for p in PROVIDERS {
            assert!(!p.tips.is_empty(), "{} has no tip accounts", p.name);
            for t in p.tips {
                assert!(
                    Pubkey::from_str(t).is_ok(),
                    "{}: {t} is not a valid pubkey",
                    p.name
                );
            }
        }
    }

    #[test]
    fn every_provider_has_endpoints_and_a_known_body_format() {
        for p in PROVIDERS {
            assert!(!p.hosts.is_empty(), "{} has no hosts", p.name);
            assert!(
                matches!(
                    p.body,
                    "json_rpc"
                        | "wrapped"
                        | "wrapped_blox"
                        | "plain_tx"
                        | "batch"
                        | "binary"
                        | "len_prefixed_binary"
                        | "udp_raw"
                ),
                "{}: unknown body format {}",
                p.name,
                p.body
            );
        }
    }

    /// A key-taking provider must actually place the key somewhere.
    #[test]
    fn keyed_providers_declare_where_the_key_goes() {
        for p in PROVIDERS {
            if p.env_key.is_empty() {
                assert_eq!(p.auth, Auth::None, "{} needs no key", p.name);
                continue;
            }
            // a datagram has nowhere to put a header or a query string: the key IS the first
            // 16 bytes of the payload, which gen-config emits as `udp_prefix`
            if p.body == "udp_raw" {
                assert_eq!(p.auth, Auth::None, "{}: UDP auth is the prefix", p.name);
                continue;
            }
            match p.auth {
                Auth::Query => assert!(
                    p.path.contains("{KEY}"),
                    "{}: query auth but no {{KEY}} in path",
                    p.name
                ),
                Auth::Header(h) => assert!(!h.is_empty(), "{}: empty header name", p.name),
                Auth::None => panic!("{}: has an env key but no auth", p.name),
            }
        }
    }

    /// nozomi closes any connection idle for more than 65s. The sender only sends its
    /// keep-alive probe to endpoints that declare a `health_path`, so an empty one here
    /// silently costs a TCP handshake on every launch - the opposite of the point.
    #[test]
    fn nozomi_declares_a_keepalive_path() {
        let nozomi = PROVIDERS
            .iter()
            .find(|p| p.name == "nozomi")
            .expect("nozomi missing from the catalogue");
        assert_eq!(nozomi.health_path, "/ping");
        assert_eq!(nozomi.hosts.len(), 9, "nozomi publishes 9 direct regions");
        for (region, host) in nozomi.hosts {
            assert!(
                host.ends_with(".nozomi.temporal.xyz"),
                "{region}: {host} is not a nozomi host"
            );
            let sub = host.split('.').next().unwrap();
            assert!(
                sub.chars().last().unwrap().is_ascii_digit(),
                "{region}: {host} is a cloudflare alias (https only), not a direct endpoint"
            );
        }
    }

    /// Two things here were silently wrong and neither would have surfaced as an error:
    /// tips went to addresses bloXroute has retired (the lamports leave, the priority does
    /// not arrive), and `la` resolved to the New York host, so a sixth of the fan-out was a
    /// duplicate send. Both are the kind of fault that only shows up as "we keep losing
    /// races", so pin them.
    #[test]
    fn bloxroute_tips_and_regions_match_the_published_set() {
        let blox = PROVIDERS
            .iter()
            .find(|p| p.name == "bloxroute")
            .expect("bloxroute missing from the catalogue");

        assert_eq!(blox.tips.len(), 17, "bloXroute's published tip table has 17 addresses");
        for superseded in ["HWEoBxYs7ssKuudEjzjmpfJVX7Dvi7wescFsVx2L5yoY",
                           "95cfoy472fcQHaw4tPGBTKpn6ZQnfEPfBgDQx6gcRmRg"] {
            assert!(
                !blox.tips.contains(&superseded),
                "{superseded} is not in the published tip table"
            );
        }

        assert!(
            !blox.hosts.iter().any(|(r, _)| *r == "la"),
            "bloxroute has no LA endpoint - la.solana.dex.blxrbdn.com is the NY host"
        );
        for (region, host) in blox.hosts {
            assert!(
                host.ends_with(".solana.dex.blxrbdn.com"),
                "{region}: {host} is not a bloXroute submission endpoint"
            );
        }

        // /health is what keeps the connection warm; an empty value skips the probe entirely
        assert_eq!(blox.health_path, "/health");
        // plain HTTP so the endpoints stay eligible for the io_uring batch writer
        assert_eq!((blox.port, blox.tls), (80, false));
    }

    /// blockrazor's key has to appear twice — the binary submit path reads it from the query
    /// string, the `/health` keep-alive only sends headers and 403s without `apikey`. Drop
    /// either half and the failure is quiet: no keep-alive, or no sends.
    #[test]
    fn blockrazor_carries_its_key_in_both_places() {
        let br = PROVIDERS
            .iter()
            .find(|p| p.name == "blockrazor")
            .expect("blockrazor missing from the catalogue");

        assert_eq!(br.path, "/v2/sendBinaryTransaction?auth={KEY}");
        assert!(br.path.contains("{KEY}"), "binary submit needs the key in the query");
        assert_eq!(br.auth, Auth::Header("apikey"), "/health needs the apikey header");
        assert_eq!(br.body, "binary");
        assert_eq!(br.health_path, "/health");
        // plain HTTP on 443 is what blockrazor publishes; TLS here would fail the handshake
        assert_eq!((br.port, br.tls), (443, false));
        assert_eq!(br.hosts.len(), 11);
    }

    /// The whole point of giving bloxroute its own body format.
    #[test]
    fn bloxroute_does_not_share_nextblocks_body() {
        let blox = PROVIDERS.iter().find(|p| p.name == "bloxroute").unwrap();
        let nextblock = PROVIDERS.iter().find(|p| p.name == "nextblock").unwrap();
        assert_eq!(blox.body, "wrapped_blox");
        assert_eq!(nextblock.body, "wrapped");
    }

    /// The sender only probes endpoints that declare a `health_path`, so an empty one means
    /// that provider's connections go cold and every launch pays a TCP handshake. It does not
    /// have to be a path that returns 200 — jito and nextblock 404 and that is fine, the point
    /// is to get *a* reply on a connection the server keeps open — but it has to be set.
    #[test]
    fn every_provider_declares_a_keepalive_path() {
        for p in PROVIDERS {
            // a datagram transport has no connection to keep warm and would not be answered
            if p.body == "udp_raw" {
                assert!(
                    p.health_path.is_empty(),
                    "{}: a UDP endpoint cannot be probed",
                    p.name
                );
                continue;
            }
            assert!(
                !p.health_path.is_empty(),
                "{}: no health_path, its connections will go cold",
                p.name
            );
            assert!(
                p.health_path.starts_with('/'),
                "{}: health_path {} is not a path",
                p.name,
                p.health_path
            );
        }
    }

    /// Providers we deliberately removed. Re-adding one is a real decision (its tip list and
    /// endpoints have to be re-verified), not something to do by pasting a key into `.env`.
    #[test]
    fn dropped_providers_stay_dropped() {
        for gone in ["lucum", "lunarlander"] {
            assert!(
                !PROVIDERS.iter().any(|p| p.name == gone),
                "{gone} was dropped on 2026-08-26"
            );
        }
    }

    /// Falcon takes the transaction as the whole body on `/binary`, so the key has nowhere to
    /// live except the query string — there is no header form. It also caps a transaction at
    /// 1,232 bytes; ours is ~1,043, but a template change could close that gap silently.
    #[test]
    fn falcon_submits_binary_with_the_key_in_the_query() {
        let f = PROVIDERS
            .iter()
            .find(|p| p.name == "falcon")
            .expect("falcon missing from the catalogue");

        assert_eq!(f.body, "binary");
        assert_eq!(f.path, "/binary?api-key={KEY}");
        assert_eq!(f.auth, Auth::Query);
        assert_eq!(f.health_path, "/health");
        assert_eq!(f.min_tip, 1_000_000, "falcon rejects a tip under 0.001 SOL");
        assert_eq!(f.hosts.len(), 9);
        assert_eq!(f.tips.len(), 10);
        for (region, host) in f.hosts {
            assert!(
                host.ends_with(".falcon.wtf"),
                "{region}: {host} is not a falcon endpoint"
            );
        }
    }

    /// The failure this pins is the one that cost us a whole provider silently: a hostname
    /// that looks regional but is a CDN edge. Four of 0slot's five published names
    /// (`ny`, `ams`, `jp`, `la`) resolve to one Cloudflare anycast pair, which is a proxy hop
    /// in front of the submission host — the same trap as nozomi's digit-less aliases and
    /// bloxroute's retired `la`. Where 0slot runs a direct host we must use it.
    #[test]
    fn zeroslot_prefers_its_direct_hosts() {
        let z = PROVIDERS
            .iter()
            .find(|p| p.name == "0slot")
            .expect("0slot missing from the catalogue");

        for direct in ["ny2.0slot.trade", "de2.0slot.trade"] {
            assert!(
                z.hosts.iter().any(|(_, h)| *h == direct),
                "{direct} is a direct 0slot host and must be in the fan-out"
            );
        }
        // `de` is the Cloudflare-fronted Frankfurt name; `de2` is the bare metal one
        assert!(
            !z.hosts.iter().any(|(_, h)| *h == "de.0slot.trade"),
            "de.0slot.trade is proxied - de2 is the direct host"
        );
        // Binary-Tx: raw transaction bytes, no base64, no JSON envelope
        assert_eq!(z.path, "/txb?api-key={KEY}");
        assert_eq!(z.body, "binary");
        // and the probe must NOT carry the key: 0slot rate-limits at 5 TPS and an
        // authenticated keep-alive spends that budget, while `/health` explicitly does not
        assert_eq!(z.health_path, "/health");
        assert!(
            !z.health_path.contains("{KEY}"),
            "an authenticated probe counts against 0slot's 5 TPS limit"
        );
        assert_eq!(z.tips.len(), 21, "0slot publishes 21 tip accounts");
    }

    /// Nozomi document three submission routes and rank them themselves: "Use Batch Send over
    /// a direct `http://` endpoint … the fastest option even for a single transaction". We
    /// were on the slowest of the three (JSON-RPC + base64) for no reason.
    #[test]
    fn nozomi_uses_the_batch_route_they_call_fastest() {
        let n = PROVIDERS.iter().find(|p| p.name == "nozomi").unwrap();
        assert_eq!(n.path, "/api/sendBatch?c={KEY}");
        assert_eq!(n.body, "len_prefixed_binary");
        // plain HTTP on a direct host is the other half of their recommendation
        assert_eq!((n.port, n.tls), (80, false));
    }

    /// Astralane's `/irisb` takes the raw transaction; `/iris` wraps base64 in JSON-RPC. The
    /// `method` query parameter replaces the JSON envelope and must survive edits to the path.
    /// `mev-protect` and `swqos-only` stay off: both trade landing speed for protection we do
    /// not want on a create-block snipe.
    #[test]
    fn astralane_submits_binary_and_asks_for_no_protection() {
        let a = PROVIDERS.iter().find(|p| p.name == "astralane").unwrap();
        assert_eq!(a.body, "binary");
        assert!(a.path.starts_with("/irisb?"), "{}", a.path);
        assert!(a.path.contains("method=sendTransaction"), "{}", a.path);
        assert!(!a.path.contains("mev-protect"), "{}", a.path);
        assert!(!a.path.contains("swqos-only"), "{}", a.path);
        // The probe is a GET, and a GET to `/iris` answers 400 with `Connection: close` —
        // it destroyed the connection it was meant to keep warm. The binary route answers
        // 405 with `Connection: keep-alive`. Verify with `ping_providers.py --reuse-after`;
        // plain reachability cannot see this, because closing a connection is not an error.
        assert_eq!(a.health_path, "/irisb?api-key={KEY}&method=getHealth");
        assert!(
            !a.health_path.starts_with("/iris?"),
            "a GET to /iris replies Connection: close"
        );
        assert_eq!(a.hosts.len(), 10);
        // Cloudflare again: their docs recommend broadcasting to `edge`, but it resolves to
        // Cloudflare rather than to astralane, so it is a proxy hop and not a race entry.
        assert!(
            !a.hosts.iter().any(|(_, h)| h.starts_with("edge.")),
            "edge.astralane.io is Cloudflare-fronted"
        );
        for (region, host) in a.hosts {
            assert!(
                host.ends_with(".gateway.astralane.io"),
                "{region}: {host} is not an astralane gateway"
            );
        }
    }

    /// falcon-udp can never confirm a send — the provider answers nothing on `:9000` — so a
    /// misconfiguration there is invisible. It must therefore ship off, and the HTTP falcon
    /// entry it shadows must stay on.
    #[test]
    fn falcon_udp_is_opt_in_and_does_not_replace_the_http_route() {
        let udp = PROVIDERS.iter().find(|p| p.name == "falcon-udp").unwrap();
        assert_eq!(
            udp.default_regions, "",
            "an unverifiable transport must not default to enabled"
        );
        assert_eq!(udp.body, "udp_raw");
        assert_eq!(udp.port, 9000);
        assert!(udp.path.is_empty(), "a datagram carries no path");
        assert!(!udp.tls);
        // it reuses the HTTP entry's key, and that entry must still exist and still be on
        let http = PROVIDERS.iter().find(|p| p.name == "falcon").unwrap();
        assert_eq!(udp.env_key, http.env_key);
        assert_eq!(http.default_regions, "all");
        assert_eq!(http.body, "binary");
    }

    /// Everything except the unverifiable UDP transport ships enabled.
    #[test]
    fn only_unverified_transports_default_to_disabled() {
        for p in PROVIDERS {
            let expected = if p.body == "udp_raw" { "" } else { "all" };
            assert_eq!(
                p.default_regions, expected,
                "{}: unexpected default_regions",
                p.name
            );
        }
    }

    #[test]
    fn provider_names_are_unique() {
        let mut names = PROVIDERS.iter().map(|p| p.name).collect::<Vec<_>>();
        names.sort_unstable();
        let before = names.len();
        names.dedup();
        assert_eq!(before, names.len(), "duplicate provider name");
    }
}
