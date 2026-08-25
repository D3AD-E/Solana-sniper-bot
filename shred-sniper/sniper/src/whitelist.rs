//! Launcher whitelist.
//!
//! The hot path does a single `ArcSwap::load` plus a hash lookup. Reloading happens on a
//! background thread, so no I/O, lock or await ever sits between detection and firing.

use std::{
    path::PathBuf,
    str::FromStr,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread::{Builder, JoinHandle},
    time::{Duration, SystemTime},
};

use ahash::AHashSet;
use arc_swap::ArcSwap;
use log::{info, warn};
use solana_sdk::pubkey::Pubkey;

/// A launcher set. `ahash` rather than the std hasher: this is looked up on every create,
/// before anything else, and SipHash over a 32 byte key costs ~4x what ahash does.
pub type LauncherSet = AHashSet<Pubkey>;

#[derive(Clone)]
pub struct Whitelist {
    set: Arc<ArcSwap<LauncherSet>>,
    /// when true every launcher passes; used for shadow runs
    disabled: bool,
}

impl Whitelist {
    pub fn new(disabled: bool) -> Self {
        Self {
            set: Arc::new(ArcSwap::from_pointee(LauncherSet::new())),
            disabled,
        }
    }

    #[inline(always)]
    pub fn contains(&self, key: &Pubkey) -> bool {
        self.disabled || self.set.load().contains(key)
    }

    pub fn len(&self) -> usize {
        self.set.load().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn store(&self, set: LauncherSet) {
        self.set.store(Arc::new(set));
    }

    /// Loads `path` now and reloads it whenever its mtime changes.
    pub fn spawn_refresher(&self, path: PathBuf, exit: Arc<AtomicBool>) -> JoinHandle<()> {
        let me = self.clone();
        Builder::new()
            .name("snipeWhitelist".to_string())
            .spawn(move || {
                let mut last_modified: Option<SystemTime> = None;
                while !exit.load(Ordering::Relaxed) {
                    let modified = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
                    if modified != last_modified {
                        match parse_file(&path) {
                            Ok(set) => {
                                info!("whitelist: loaded {} launchers", set.len());
                                me.store(set);
                                last_modified = modified;
                            }
                            Err(e) => warn!("whitelist: {e}"),
                        }
                    }
                    std::thread::sleep(Duration::from_millis(1000));
                }
            })
            .expect("spawn whitelist refresher")
    }
}

/// One base58 pubkey per line. Blank lines and `#` comments are ignored, and a bad line is
/// skipped rather than throwing the whole file away.
pub fn parse_file(path: &PathBuf) -> Result<LauncherSet, String> {
    let raw = std::fs::read_to_string(path).map_err(|e| format!("read {path:?}: {e}"))?;
    Ok(parse_str(&raw))
}

pub fn parse_str(raw: &str) -> LauncherSet {
    let mut set = LauncherSet::with_capacity(raw.len() / 44 + 16);
    for line in raw.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Ok(key) = Pubkey::from_str(line) {
            set.insert(key);
        }
    }
    set
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_valid_lines_and_skips_junk() {
        let set = parse_str(
            "# comment\n\
             6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P\n\
             \n\
             not-a-pubkey\n\
             TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA\n",
        );
        assert_eq!(set.len(), 2);
        assert!(set.contains(&Pubkey::from_str_const(
            "6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P"
        )));
    }

    #[test]
    fn disabled_whitelist_accepts_everything() {
        let wl = Whitelist::new(true);
        assert!(wl.contains(&Pubkey::new_from_array([9u8; 32])));
        let wl = Whitelist::new(false);
        assert!(!wl.contains(&Pubkey::new_from_array([9u8; 32])));
    }
}
