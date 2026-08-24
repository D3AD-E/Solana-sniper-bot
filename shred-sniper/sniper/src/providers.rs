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

pub const BLOX_TIPS: &[&str] = &[
    "HWEoBxYs7ssKuudEjzjmpfJVX7Dvi7wescFsVx2L5yoY",
    "95cfoy472fcQHaw4tPGBTKpn6ZQnfEPfBgDQx6gcRmRg",
    "3UQUKjhMKaY2S6bjcQD6yHB7utcZt5bfarRCmctpRtUd",
    "FogxVNs6Mm2w9rnGL1vkARSwJxvLE8mujTv3LK8RnUhF",
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

pub const LUCUM_TIPS: &[&str] = &[
    "Lucum3sDVsPmHnQVaRKGpLXVPQLhcUqJqmcN5Tn9xuR",
    "Lucum2REE14nX1xBJee9RR24gMaM878icjigfuvWy7H",
    "Lucum2g9HQeHdXEaENapK66C9bgprAMADsg1XijoW2m",
    "Lucum3TJzgBRMZV5CgkmsH6jnE9YKa9ceAQDrUQQuA6",
    "Lucum3TosrLyi8nwP9L9E6s9HWRTg8Y8kv67MjWpkKk",
    "Lucum3yhZeqqXxW3yeTRheBRqwwXnr285HzTiyWKrgm",
    "Lucum4XaQeeARcS4EwmJsGpjNWUNH75hAD2k7jsxSKD",
    "Lucum4r22CCf5M5Zsj4PvhxYJ8CGz4QQMUCrL89Rupz",
    "Lucum5FeurZkc7qrKadaWmzsZ6L1ig79EHGXJU65rPn",
    "Lucum6s8rtKN5n7oWMm1h2Afm18DxWuA8Fgmraikxa3",
];

pub const LUNARLANDER_TIPS: &[&str] = &[
    "moon17L6BgxXRX5uHKudAmqVF96xia9h8ygcmG2sL3F",
    "moon26Sek222Md7ZydcAGxoKG832DK36CkLrS3PQY4c",
    "moon7fwyajcVstMoBnVy7UBcTx87SBtNoGGAaH2Cb8V",
    "moonBtH9HvLHjLqi9ivyrMVKgFUsSfrz9BwQ9khhn1u",
    "moonCJg8476LNFLptX1qrK8PdRsA1HD1R6XWyu9MB93",
    "moonF2sz7qwAtdETnrgxNbjonnhGGjd6r4W4UC9284s",
    "moonKfftMiGSak3cezvhEqvkPSzwrmQxQHXuspC96yj",
    "moonQBUKBpkifLcTd78bfxxt4PYLwmJ5admLW6cBBs8",
    "moonXwpKwoVkMegt5Bc776cSW793X1irL5hHV1vJ3JA",
    "moonZ6u9E2fgk6eWd82621eLPHt9zuJuYECXAYjMY1C",
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
        health_path: "",
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
        health_path: "",
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
        health_path: "",
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
        health_path: "",
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
        auth: Auth::Header("Authorization"),
        port: 443,
        tls: true,
        path: "/api/v2/submit",
        health_path: "",
        body: "wrapped",
        min_tip: 1_000_000,
        signup: "https://bloxroute.com/products/solana-trader-api/",
        hosts: &[
            ("ny", "ny.solana.dex.blxrbdn.com"),
            ("fra", "germany.solana.dex.blxrbdn.com"),
            ("ams", "amsterdam.solana.dex.blxrbdn.com"),
            ("lon", "uk.solana.dex.blxrbdn.com"),
            ("la", "la.solana.dex.blxrbdn.com"),
            ("tyo", "tokyo.solana.dex.blxrbdn.com"),
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
        health_path: "",
        body: "batch",
        min_tip: 1_000_000,
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
        auth: Auth::Header("apikey"),
        port: 443,
        tls: false,
        path: "/sendTransaction",
        health_path: "",
        body: "plain_tx",
        min_tip: 1_000_000,
        signup: "https://blockrazor.io/ (docs.blockrazor.io)",
        hosts: &[
            ("ny", "newyork.solana.blockrazor.xyz"),
            ("fra", "frankfurt.solana.blockrazor.xyz"),
            ("ams", "amsterdam.solana.blockrazor.xyz"),
            ("lon", "london.solana.blockrazor.xyz"),
            ("tyo", "tokyo.solana.blockrazor.xyz"),
            ("sgp", "singapore.solana.blockrazor.xyz"),
            ("la", "losangeles.solana.blockrazor.xyz"),
        ],
        tips: BLOCKRAZOR_TIPS,
    },
    ProviderSpec {
        name: "lucum",
        env_key: "LUCUM_KEY",
        env_regions: "LUCUM_REGIONS",
        auth: Auth::Query,
        port: 80,
        tls: false,
        path: "/?api-key={KEY}",
        health_path: "",
        body: "plain_tx",
        min_tip: 1_000_000,
        signup: "https://lucum.io/docs/",
        hosts: &[
            ("ny", "ny.lucum.io"),
            ("fra", "fra.lucum.io"),
            ("ams", "ams.lucum.io"),
            ("lon", "lon.lucum.io"),
        ],
        tips: LUCUM_TIPS,
    },
    ProviderSpec {
        name: "lunarlander",
        env_key: "HELLOMOON_KEY",
        env_regions: "HELLOMOON_REGIONS",
        auth: Auth::Header("x-api-key"),
        port: 80,
        tls: false,
        path: "/send",
        health_path: "/ping",
        body: "json_rpc",
        min_tip: 1_000_000,
        signup: "https://docs.hellomoon.io/reference/lunar-lander",
        hosts: &[
            ("ny", "nyc.lunar-lander.hellomoon.io"),
            ("fra", "fra.lunar-lander.hellomoon.io"),
            ("ams", "ams.lunar-lander.hellomoon.io"),
            ("lon", "lon.lunar-lander.hellomoon.io"),
            ("la", "lax.lunar-lander.hellomoon.io"),
            ("tyo", "tyo.lunar-lander.hellomoon.io"),
            ("sgp", "sgp.lunar-lander.hellomoon.io"),
            ("ash", "ash.lunar-lander.hellomoon.io"),
            ("chi", "chi.lunar-lander.hellomoon.io"),
        ],
        tips: LUNARLANDER_TIPS,
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
                matches!(p.body, "json_rpc" | "wrapped" | "plain_tx" | "batch"),
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

    #[test]
    fn provider_names_are_unique() {
        let mut names = PROVIDERS.iter().map(|p| p.name).collect::<Vec<_>>();
        names.sort_unstable();
        let before = names.len();
        names.dedup();
        assert_eq!(before, names.len(), "duplicate provider name");
    }
}
