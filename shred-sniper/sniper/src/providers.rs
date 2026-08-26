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
    pub auth: Auth,
    pub port: u16,
    pub tls: bool,
    /// `{KEY}` is replaced with the API key
    pub path: &'static str,
    pub health_path: &'static str,
    /// one of json_rpc | wrapped | plain_tx | batch
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

pub const ZEROSLOT_TIPS: &[&str] = &[
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
        auth: Auth::Query,
        port: 80,
        tls: false,
        path: "/?api-key={KEY}",
        health_path: "/?api-key={KEY}",
        body: "json_rpc",
        min_tip: 1_000_000,
        signup: "https://0slot.trade/",
        hosts: &[
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
        auth: Auth::Query,
        port: 80,
        tls: false,
        path: "/iris?api-key={KEY}",
        health_path: "/iris?api-key={KEY}",
        body: "json_rpc",
        min_tip: 10_000,
        signup: "https://astralane.gitbook.io/docs (portal.astralane.io)",
        hosts: &[
            ("ny", "ny.gateway.astralane.io"),
            ("fra", "fr.gateway.astralane.io"),
            ("ams", "ams.gateway.astralane.io"),
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
        ],
        tips: NEXTBLOCK_TIPS,
    },
    ProviderSpec {
        name: "nozomi",
        env_key: "NOZOMI_KEY",
        env_regions: "NOZOMI_REGIONS",
        auth: Auth::Query,
        port: 80,
        tls: false,
        path: "/?c={KEY}",
        // nozomi closes an idle connection after 65s and publishes a lightweight GET /ping
        // for exactly this. Without it the sender's 50s probe skips nozomi and every launch
        // pays a fresh TCP handshake.
        health_path: "/ping",
        body: "json_rpc",
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
        // for up to four slots. See BodyFormat::WrappedBlox.
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
                    "json_rpc" | "wrapped" | "wrapped_blox" | "plain_tx" | "batch" | "binary"
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

    #[test]
    fn provider_names_are_unique() {
        let mut names = PROVIDERS.iter().map(|p| p.name).collect::<Vec<_>>();
        names.sort_unstable();
        let before = names.len();
        names.dedup();
        assert_eq!(before, names.len(), "duplicate provider name");
    }
}
