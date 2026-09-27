use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Context, Result};
use regex::Regex;

use crate::device::{ActiveDeviceSession, ConnectionMode};

#[cfg(windows)]
unsafe extern "system" {
    fn setsockopt(s: usize, level: i32, optname: i32, optval: *const i8, optlen: i32) -> i32;
}

#[cfg(windows)]
const SOL_SOCKET: i32 = 0xffff;
#[cfg(windows)]
const SO_RCVTIMEO: i32 = 0x1006;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SavedCard {
    pub hash: String,
    pub name: String,
    /// Display name the user gave this card, if any. Scan-derived names are
    /// often "Unknown" or a generic label, so a user-set name always wins.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Filename of the skin last applied to this card, for the history list.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_skin: Option<String>,
}

impl SavedCard {
    /// The name to show in lists: the user's label if set, else the scan name.
    pub fn display_name(&self) -> &str {
        self.label
            .as_deref()
            .filter(|l| !l.trim().is_empty())
            .unwrap_or(&self.name)
    }
}

pub fn get_cards_storage_path() -> PathBuf {
    let local_app_data = std::env::var("LOCALAPPDATA")
        .unwrap_or_else(|_| r"C:\Users\Default\AppData\Local".to_string());
    let dir = PathBuf::from(local_app_data).join("AirCard");
    let _ = fs::create_dir_all(&dir);
    dir.join("cards.json")
}

/// Decides whether a string seen in syslog is a real Wallet card hash.
///
/// The structural test comes first: a Wallet card hash is base64 that decodes to
/// exactly 20 or 32 bytes with cryptographic-hash entropy. Noise from iOS
/// subsystems - bundle IDs, asset names, log tags - almost always fails one of
/// those outright, so it never reaches the keyword list.
///
/// The keyword list is a last-resort net for noise that happens to be shaped
/// like a hash. It used to be the primary filter, which meant every iOS release
/// that introduced new subsystem names required editing it by hand.
pub fn is_valid_card_hash(h: &str) -> bool {
    let Some(decoded) = decode_card_hash(h) else {
        return false;
    };

    // A cryptographic hash is uniformly distributed. These two checks separate
    // that from an identifier that merely happens to be 20 or 32 bytes long.
    let has_high = decoded.iter().any(|&b| b >= 128);
    let has_low = decoded.iter().any(|&b| b < 128);
    if !has_high || !has_low {
        return false;
    }

    let unique_bytes: std::collections::HashSet<u8> = decoded.iter().copied().collect();
    // 20 bytes of SHA-1 concentrate well above 12 distinct values; a
    // human-readable identifier does not.
    let min_distinct = (decoded.len() / 2).max(8);
    if unique_bytes.len() < min_distinct {
        return false;
    }

    if decoded.iter().all(|&b| b == decoded[0]) {
        return false;
    }

    let trimmed = h
        .trim_matches(['\'', '"'])
        .trim_end_matches(['.', ','])
        .trim();
    !is_known_noise(trimmed)
}

/// Decode a candidate into the 20 or 32 bytes a card hash must contain.
fn decode_card_hash(h: &str) -> Option<Vec<u8>> {
    let trimmed = h
        .trim_matches(['\'', '"'])
        .trim_end_matches(['.', ','])
        .trim();
    let len = trimmed.len();
    // SHA-1 (27-28 chars) or SHA-256 (43-44 chars) in base64.
    if len != 27 && len != 28 && len != 43 && len != 44 {
        return None;
    }

    if !trimmed.chars().all(|c| {
        c.is_ascii_alphanumeric() || c == '+' || c == '/' || c == '-' || c == '_' || c == '='
    }) {
        return None;
    }

    // '=' is only ever padding, so it cannot appear mid-string.
    if let Some(pos) = trimmed.find('=') {
        if pos < len - 2 {
            return None;
        }
    }

    // A hash has no word structure: it carries no separators. Subsystem names
    // and bundle IDs are built from them (`com_apple_MobileAsset_...`), and one
    // of those happens to be exactly 43 characters - the SHA-256 base64 length -
    // so length and entropy alone let it through.
    if trimmed.chars().filter(|&c| c == '_').count() > 1
        || trimmed.chars().filter(|&c| c == '-').count() > 2
    {
        return None;
    }

    // Accept both alphabets; Wallet hashes appear in either.
    let mut b64 = trimmed.replace('-', "+").replace('_', "/");
    while b64.len() % 4 != 0 {
        b64.push('=');
    }

    use base64::Engine;
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(&b64)
        .or_else(|_| base64::engine::general_purpose::URL_SAFE.decode(&b64))
        .ok()?;

    (decoded.len() == 20 || decoded.len() == 32).then_some(decoded)
}

/// Strings that pass the structural test but are not card hashes.
///
/// Kept deliberately short. Anything listed here is a case the entropy and
/// length rules let through - if a new noise source needs adding, the real fix
/// is a better structural rule, not another keyword.
fn is_known_noise(trimmed: &str) -> bool {
    // Wallet's own placeholder hashes, which it logs on paths that carry no card.
    if DUMMY_HASHES.contains(&trimmed)
        || DUMMY_HASHES
            .iter()
            .any(|d| d.trim_end_matches('=') == trimmed)
    {
        return true;
    }

    // Bundle IDs and asset names: separators no card hash uses.
    let lower = trimmed.to_lowercase();
    lower.contains("com.") || lower.contains("apple.")
}

pub fn load_saved_cards() -> Vec<SavedCard> {
    let path = get_cards_storage_path();
    if let Ok(content) = fs::read_to_string(&path) {
        if let Ok(cards) = serde_json::from_str::<Vec<SavedCard>>(&content) {
            let valid_cards: Vec<SavedCard> = cards
                .into_iter()
                .filter(|c| is_valid_card_hash(&c.hash))
                .collect();
            // Automatically purge corrupted or garbage entries from disk
            save_saved_cards(&valid_cards);
            return valid_cards;
        }
    }
    Vec::new()
}

pub fn save_saved_cards(cards: &[SavedCard]) {
    let path = get_cards_storage_path();
    let mut unique = Vec::new();
    let mut seen = HashSet::new();
    for c in cards {
        if is_valid_card_hash(&c.hash) && seen.insert(c.hash.clone()) {
            unique.push(c.clone());
        }
    }
    if let Ok(json) = serde_json::to_string_pretty(&unique) {
        let _ = fs::write(path, json);
    }
}

pub fn add_or_update_card(hash: &str, name: &str) {
    if !is_valid_card_hash(hash) {
        return;
    }
    let mut cards = load_saved_cards();
    if let Some(existing) = cards.iter_mut().find(|c| c.hash == hash) {
        // Only fill in a real name; a user-set label is never overwritten,
        // because `display_name` prefers it.
        if !name.is_empty() && (existing.name.is_empty() || existing.name.starts_with("Card ")) {
            existing.name = name.to_string();
        }
    } else {
        cards.push(SavedCard {
            hash: hash.to_string(),
            name: if name.is_empty() {
                format!("Card {}", cards.len() + 1)
            } else {
                name.to_string()
            },
            label: None,
            last_skin: None,
        });
    }
    save_saved_cards(&cards);
}

/// Rename a saved card. An empty label clears it, falling back to the scan name.
pub fn rename_saved_card(hash: &str, label: Option<String>) {
    let mut cards = load_saved_cards();
    if let Some(card) = cards.iter_mut().find(|c| c.hash == hash) {
        card.label = label
            .map(|l| l.trim().to_string())
            .filter(|l| !l.is_empty());
        save_saved_cards(&cards);
    }
}

/// Record which skin was last applied to a card.
pub fn set_last_skin(hash: &str, skin: Option<String>) {
    let mut cards = load_saved_cards();
    if let Some(card) = cards.iter_mut().find(|c| c.hash == hash) {
        card.last_skin = skin;
        save_saved_cards(&cards);
    }
}

/// Drop a card from the saved list.
pub fn remove_saved_card(hash: &str) {
    let mut cards = load_saved_cards();
    cards.retain(|c| c.hash != hash);
    save_saved_cards(&cards);
}

const WALLET_KEYWORDS: &[&str] = &[
    "passd",
    "passbook",
    "passkit",
    "stockholm",
    "nanopassd",
    "npkcompanion",
    "wallet",
    "/cards/",
    "/passes/",
];

const DUMMY_HASHES: &[&str] = &[
    "M6nDwZrkYbFlsodLgCbvyFZQ1cc=",
    "kJL-D0rr-SZhbj2c8nK-OQ9hCMY=",
    "hwAtAmHKYwsQrJbT5cTNDsaxVME=",
];

use std::sync::LazyLock;

static DESC_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"(?i)(?:description|localizedDescription|passName|title)\s*[:=]\s*['"]([^'"]+)['"]"#,
    )
    .unwrap()
});

static CARD_REGEXES: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    vec![
        Regex::new(r"/(?:Cards|Passes/Cards)/([A-Za-z0-9+/_-]{27,44})(?:\.pkpass|\.cache|\.pkcache|/|\s|\x22|'|\)|,|$)").unwrap(),
        Regex::new(r"/([A-Za-z0-9+/_-]{27,44})\.(?:pkpass|cache|pkcache)").unwrap(),
        Regex::new(r"(?:^|[^A-Za-z0-9+/_-])([A-Za-z0-9+/_-]{27}=)(?:$|[^A-Za-z0-9+/_-])").unwrap(),
        Regex::new(r"(?i)(?:card[_\s]?(?:hash|id)|pass[_\s]?(?:hash|id)|unique[_\s]?id)\s*[:=]\s*['\x22]?([A-Za-z0-9+=_-]{27,44})").unwrap(),
    ]
});

pub fn extract_card_name_from_line(line: &str) -> Option<String> {
    if let Some(caps) = DESC_RE.captures(line) {
        if let Some(m) = caps.get(1) {
            let name = m.as_str().trim();
            if name.len() > 1 && !name.to_lowercase().contains("<private>") {
                return Some(name.to_string());
            }
        }
    }
    None
}

pub fn extract_card_hash_from_line(line: &str) -> Option<String> {
    let lower = line.to_lowercase();
    let has_wallet = WALLET_KEYWORDS.iter().any(|k| lower.contains(k));
    if !has_wallet {
        return None;
    }

    for r in CARD_REGEXES.iter() {
        if let Some(caps) = r.captures(line) {
            if let Some(m) = caps.get(1) {
                let h = m
                    .as_str()
                    .trim()
                    .trim_matches(['\'', '"'])
                    .trim_end_matches(['.', ',']);
                if is_valid_card_hash(h) {
                    let mut norm = h.to_string();
                    if norm.len() == 27 {
                        norm.push('=');
                    }
                    return Some(norm);
                }
            }
        }
    }

    None
}

pub fn scan_syslog_for_cards<F, L>(
    udid: Option<&str>,
    connection_mode: ConnectionMode,
    stop_flag: Arc<AtomicBool>,
    mut on_card_found: F,
    mut log: L,
) -> Result<()>
where
    F: FnMut(String, String),
    L: FnMut(String),
{
    log("Connecting to device session for syslog monitoring...".to_string());
    let session = ActiveDeviceSession::open(udid, connection_mode)
        .context("Failed to connect to device for syslog scanning")?;
    log(format!(
        "Connected to {} over {}.",
        session.udid,
        session.transport.label()
    ));
    let libs = &session.libs;
    log("Starting com.apple.syslog_relay service on device...".to_string());
    let service_conn = session
        .start_service("com.apple.syslog_relay")
        .context("Failed to start com.apple.syslog_relay service")?;

    let raw_socket = unsafe { (libs.amd_service_connection_get_socket)(service_conn) };
    if raw_socket <= 0 {
        unsafe { (libs.amd_service_connection_invalidate)(service_conn) };
        anyhow::bail!("Invalid syslog socket");
    }

    // Set socket receive timeout
    #[cfg(windows)]
    unsafe {
        let timeout_ms: u32 = 500;
        setsockopt(
            raw_socket as usize,
            SOL_SOCKET,
            SO_RCVTIMEO,
            &timeout_ms as *const u32 as *const i8,
            std::mem::size_of::<u32>() as i32,
        );
    }

    log("Syslog relay established. Listening for Wallet & PassKit events...".to_string());
    log("Tip: Open Apple Wallet on your iPhone or tap your card to trigger events.".to_string());

    let mut buffer = [0u8; 8192];
    let mut line_acc = Vec::with_capacity(1024);

    while !stop_flag.load(Ordering::Relaxed) {
        let bytes_read = unsafe {
            (libs.amd_service_connection_receive)(service_conn, buffer.as_mut_ptr(), buffer.len())
        };

        if bytes_read > 0 {
            let slice = &buffer[..bytes_read as usize];
            for &b in slice {
                if b == b'\n' || b == b'\0' {
                    if !line_acc.is_empty() {
                        let line = String::from_utf8_lossy(&line_acc);
                        if let Some(hash) = extract_card_hash_from_line(&line) {
                            let name = extract_card_name_from_line(&line).unwrap_or_default();
                            log(format!(
                                "Found card pass! Name: '{}', Hash: {}",
                                if name.is_empty() { "Unknown" } else { &name },
                                hash
                            ));
                            add_or_update_card(&hash, &name);
                            on_card_found(hash, name);
                        }
                        line_acc.clear();
                    }
                } else if b != b'\r' {
                    line_acc.push(b);
                }
            }
        } else if bytes_read == 0 {
            log("Syslog socket closed by device.".to_string());
            break; // Socket closed
        } else {
            // Timeout or transient: sleep briefly to avoid pegging CPU
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    }

    unsafe {
        (libs.amd_service_connection_invalidate)(service_conn);
    }
    log("Syslog scan stopped.".to_string());

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_card_hash() {
        let line1 = "passd[123]: Card hash: 'OM6NYhwXMZrAw0sRUjR62wmF4ZQ=' loaded";
        assert_eq!(
            extract_card_hash_from_line(line1),
            Some("OM6NYhwXMZrAw0sRUjR62wmF4ZQ=".to_string())
        );

        let line2 = "nanopassd: Accessing /var/mobile/Library/Passes/Cards/d64fKk0kyHWP11IWV2GRLud4XQk.pkpass";
        assert_eq!(
            extract_card_hash_from_line(line2),
            Some("d64fKk0kyHWP11IWV2GRLud4XQk=".to_string())
        );

        // Dummy/unrelated lines should be ignored
        let dummy = "passd: Using dummy hash hwAtAmHKYwsQrJbT5cTNDsaxVME=";
        assert_eq!(extract_card_hash_from_line(dummy), None);
    }

    #[test]
    fn test_extract_card_name() {
        let line = "passd[456]: Pass with localizedDescription = 'Apple Card' updated";
        assert_eq!(
            extract_card_name_from_line(line),
            Some("Apple Card".to_string())
        );
    }

    #[test]
    fn test_is_valid_card_hash() {
        // Real card hashes
        assert!(is_valid_card_hash("OM6NYhwXMZrAw0sRUjR62wmF4ZQ="));
        assert!(is_valid_card_hash("d64fKk0kyHWP11IWV2GRLud4XQk="));
        assert!(is_valid_card_hash("d64fKk0kyHWP11IWV2GRLud4XQk"));

        // System garbage strings that must be rejected
        let garbage = [
            "PresentationBinderIndirectAccessHosting-",
            "SB-systemApertureCurtain",
            "com_apple_MobileAsset_UAF_Translation_Assets",
            "com_apple_MobileAsset_UAF_FM_Visual",
            "com_apple_MobileAsset_UAF_DeviceCheck",
            "com_apple_MobileAsset_UAF_Siri_TextToSpeech",
            "com_apple_MobileAsset_UAF_Siri_DialogAssets",
            "com_apple_MobileAsset_UAF_IF_Planner",
            "SubscriptionOptimizerTimingModels",
            "com_apple_MobileAsset_UAF_LinguisticData",
            "com_apple_MobileAsset_UAF_FM_Overrides",
            "com_apple_MobileAsset_UAF_Siri_Understanding",
            "com_apple_MobileAsset_UAF_MotionAnomalyFM",
            "com_apple_MobileAsset_UAF_TKModelMessages",
            "com_apple_MobileAsset_UAF_Search_ODLA",
        ];

        for g in garbage {
            assert!(
                !is_valid_card_hash(g),
                "Expected {} to be rejected as card hash",
                g
            );
        }
    }

    /// Length and entropy alone are not enough.
    ///
    /// `com_apple_MobileAsset_UAF_Siri_DialogAssets` is 43 characters - exactly
    /// the SHA-256 base64 length - and decodes to 32 bytes with plausible
    /// entropy, so it passes every structural check except the separator rule.
    /// Without that rule it is captured as a card. This test exists so the rule
    /// is not dropped again on the assumption that entropy covers it.
    #[test]
    fn noise_that_looks_structurally_valid_is_still_rejected() {
        let structurally_valid_noise = [
            "com_apple_MobileAsset_UAF_Siri_DialogAssets",
            "com_apple_MobileAsset_UAF_VisualIntelligence",
            "com_apple_MobileAsset_UAF_Siri_TextToSpeech",
        ];
        for sample in structurally_valid_noise {
            // Either base64 width for 32 bytes counts as SHA-256 shaped.
            assert!(
                sample.len() == 43 || sample.len() == 44,
                "sample is meant to be SHA-256 shaped"
            );
            assert!(
                !is_valid_card_hash(sample),
                "Expected {sample} to be rejected - it is a bundle name, not a hash"
            );
        }
    }

    /// A real card hash carries no separators at all, in either alphabet.
    #[test]
    fn real_hashes_have_no_separators() {
        for hash in [
            "d64fKk0kyHWP11IWV2GRLud4XQk=",
            "OM6NYhwXMZrAw0sRUjR62wmF4ZQ=",
            "ZG+pLeL8u+lORdfO1J481gIgnPk=",
        ] {
            assert!(
                !hash.contains('_') && !hash.contains('-'),
                "{hash} was assumed to be separator-free"
            );
            assert!(is_valid_card_hash(hash), "{hash} should be accepted");
        }
    }

    #[test]
    fn test_saved_cards_purging() {
        let loaded = load_saved_cards();
        for card in &loaded {
            assert!(
                is_valid_card_hash(&card.hash),
                "Invalid hash was not purged: {}",
                card.hash
            );
        }
    }

    #[test]
    fn test_syslog_service_receive() {
        let session = match ActiveDeviceSession::open(None, ConnectionMode::Auto) {
            Ok(s) => s,
            Err(e) => {
                println!("No device connected: {:?}", e);
                return;
            }
        };
        let libs = &session.libs;
        let conn = session
            .start_service("com.apple.syslog_relay")
            .expect("start syslog_relay");
        let raw_socket = unsafe { (libs.amd_service_connection_get_socket)(conn) };
        unsafe {
            let timeout_ms: u32 = 500;
            setsockopt(
                raw_socket as usize,
                SOL_SOCKET,
                SO_RCVTIMEO,
                &timeout_ms as *const u32 as *const i8,
                std::mem::size_of::<u32>() as i32,
            );
        }
        let mut buf = [0u8; 4096];
        let start = std::time::Instant::now();
        let n = unsafe { (libs.amd_service_connection_receive)(conn, buf.as_mut_ptr(), buf.len()) };
        println!(
            "AMDServiceConnectionReceive returned: {} in {:?}",
            n,
            start.elapsed()
        );
        unsafe { (libs.amd_service_connection_invalidate)(conn) };
    }
}
