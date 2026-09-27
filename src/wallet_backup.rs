use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use crate::afc::AfcClient;
use crate::device::{ActiveDeviceSession, ConnectionMode};

/// Preferred asset names, in the order Wallet is most likely to want them.
/// Used to order and prioritise, never to exclude: the real set is discovered
/// from the device so an unexpected filename still gets backed up.
pub const CARD_ARTWORK_ASSETS: [&str; 3] = [
    "cardBackgroundCombined@3x.png",
    "cardBackgroundCombined@2x.png",
    "cardBackgroundCombined.pdf",
];

const METADATA_FILE: &str = "metadata.json";
const BACKUPABLE_EXTENSIONS: [&str; 2] = ["png", "pdf"];

fn backup_root() -> PathBuf {
    local_app_data().join("AirCard").join("wallet-backups")
}

/// Exposed for `aircard.exe probe`, which reports what backups exist locally.
pub fn backup_root_for_probe() -> PathBuf {
    backup_root()
}

fn local_app_data() -> PathBuf {
    std::env::var("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(r"C:\Users\Default\AppData\Local"))
}

fn backup_dir(udid: &str, card_hash: &str) -> PathBuf {
    backup_root().join(format!(
        "{}-{}",
        safe_component(udid),
        stable_hash(card_hash)
    ))
}

fn safe_component(value: &str) -> String {
    let component: String = value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.') {
                ch
            } else {
                '_'
            }
        })
        .collect();
    if component.is_empty() {
        "unknown".to_string()
    } else {
        component
    }
}

fn stable_hash(value: &str) -> String {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in value.as_bytes() {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}")
}

/// True when there is something to restore.
///
/// Deliberately not keyed to `CARD_ARTWORK_ASSETS`: a backup is usable as long
/// as it holds any artwork file, whatever it is named. An empty directory - the
/// state left behind by a failed capture - is not a backup.
pub fn backup_exists(udid: &str, card_hash: &str) -> bool {
    !local_backup_assets(&backup_dir(udid, card_hash)).is_empty()
}

/// How many artwork files the backup holds, for display beside the restore
/// button. `None` when there is no backup at all.
pub fn backup_asset_count(udid: &str, card_hash: &str) -> Option<usize> {
    let count = local_backup_assets(&backup_dir(udid, card_hash)).len();
    (count > 0).then_some(count)
}

/// Artwork files present in a local backup directory.
fn local_backup_assets(dir: &Path) -> Vec<String> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .filter_map(|entry| entry.ok())
        .filter(|entry| is_backupable(&entry.path()))
        .map(|entry| entry.file_name().to_string_lossy().to_string())
        .collect();
    // Preferred names first, so @3x is restored before @2x.
    names.sort_by_key(|name| asset_rank(name));
    names
}

fn is_backupable(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
        return false;
    };
    if name == METADATA_FILE || name.ends_with(".tmp") || !path.is_file() {
        return false;
    }
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| BACKUPABLE_EXTENSIONS.contains(&ext.to_lowercase().as_str()))
}

/// Where a name sits in the restore order; unknown names sort last.
fn asset_rank(name: &str) -> usize {
    CARD_ARTWORK_ASSETS
        .iter()
        .position(|known| known.eq_ignore_ascii_case(name))
        .unwrap_or(CARD_ARTWORK_ASSETS.len())
}

fn card_hash_candidates(card_hash: &str) -> Vec<String> {
    let trimmed = card_hash.trim_end_matches('=');
    let mut candidates = vec![card_hash.to_string(), trimmed.to_string()];
    if !trimmed.is_empty() {
        candidates.push(format!("{trimmed}="));
        candidates.push(format!("{trimmed}=="));
    }
    candidates.dedup();
    candidates
}

/// Directories on device that hold this card, in preference order.
///
/// Guessing the path directly fails whenever the hash differs by padding, so
/// the Cards directory is listed and matched against every candidate instead.
/// `.pkpass` (the card bundle) wins over the `.cache`/`.pkcache` siblings.
fn discover_card_dirs(afc: &AfcClient, card_hash: &str) -> Vec<String> {
    let entries = match afc.list_directory("/var/mobile/Library/Passes/Cards") {
        Ok(entries) => entries,
        Err(_) => return Vec::new(),
    };

    let candidates = card_hash_candidates(card_hash);
    let mut found: Vec<String> = entries
        .iter()
        .filter(|entry| {
            let stem = entry
                .rsplit_once('.')
                .map(|(stem, _)| stem)
                .unwrap_or(entry.as_str());
            candidates
                .iter()
                .any(|candidate| candidate.eq_ignore_ascii_case(stem))
        })
        .map(|entry| format!("/var/mobile/Library/Passes/Cards/{entry}"))
        .collect();

    found.sort_by_key(|dir| {
        if dir.ends_with(".pkpass") {
            0
        } else if dir.ends_with(".pkcache") {
            2
        } else {
            1
        }
    });
    found
}

/// Artwork files in a device card directory, under their real names.
fn list_device_assets(afc: &AfcClient, card_dir: &str) -> Vec<String> {
    let Ok(entries) = afc.list_directory(card_dir) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .into_iter()
        .filter(|name| {
            Path::new(name)
                .extension()
                .and_then(|ext| ext.to_str())
                .is_some_and(|ext| BACKUPABLE_EXTENSIONS.contains(&ext.to_lowercase().as_str()))
        })
        .collect();
    names.sort_by_key(|name| asset_rank(name));
    names
}

pub fn capture_original_card<L>(
    udid: &str,
    connection_mode: ConnectionMode,
    card_hash: &str,
    mut log: L,
) -> Result<Option<String>>
where
    L: FnMut(&str),
{
    let dir = backup_dir(udid, card_hash);
    fs::create_dir_all(&dir)
        .with_context(|| format!("Could not create backup directory {}", dir.display()))?;

    if backup_exists(udid, card_hash) {
        log("Checking for an existing original Wallet card face backup... already present.");
        return Ok(Some(card_hash.to_string()));
    }

    log("Checking for an existing original Wallet card face backup...");
    let session = match ActiveDeviceSession::open(Some(udid), connection_mode) {
        Ok(session) => session,
        Err(err) => {
            log(&format!(
                "Could not open the device to back up the original card face: {:#}",
                err
            ));
            return Ok(None);
        }
    };
    let afc = match AfcClient::new(&session) {
        Ok(afc) => afc,
        Err(err) => {
            log(&format!(
                "Could not open AFC to back up the original card face: {:#}",
                err
            ));
            return Ok(None);
        }
    };

    // Modern iOS answers AFC access to /var with error 8, so the card bundle is
    // unreachable even though AirTraffic can write into it. Report that
    // specifically: "cannot read /var" and "card not found" look alike in the
    // log but mean different things.
    let card_dirs = discover_card_dirs(&afc, card_hash);
    if card_dirs.is_empty() {
        if afc_sandbox_is_closed(&afc) {
            log(
                "This iOS build does not let AFC read /var, so the original card artwork cannot be copied off the device.",
            );
            log("Restoring the original face is therefore unavailable for this device.");
            return Ok(None);
        }
        log(&format!(
            "Wallet card directory not found for hash {} among the device card entries; continuing without an original backup.",
            card_hash
        ));
        return Ok(None);
    }

    let card_dir = card_dirs[0].clone();
    // The directory name minus its extension is the hash form Wallet uses.
    let resolved_hash = card_dir
        .rsplit('/')
        .next()
        .and_then(|leaf| leaf.rsplit_once('.'))
        .map(|(stem, _)| stem)
        .unwrap_or(card_hash)
        .to_string();

    // Whatever is actually in the directory is what gets backed up. The
    // preferred names are only a priority hint, never a filter.
    let device_assets = list_device_assets(&afc, &card_dir);
    if device_assets.is_empty() {
        log(&format!(
            "Wallet card directory {} exists but holds no readable artwork; continuing without an original backup.",
            card_dir
        ));
        return Ok(Some(resolved_hash));
    }

    let mut backed_up = 0;
    for asset in &device_assets {
        let backup_path = dir.join(asset);
        if backup_path.is_file() {
            backed_up += 1;
            log(&format!("Original backup already contains {}", asset));
            continue;
        }

        match afc.read_file(&format!("{card_dir}/{asset}")) {
            Ok(data) => {
                write_backup_file(&backup_path, &data)
                    .with_context(|| format!("Failed to save original card asset {}", asset))?;
                backed_up += 1;
                log(&format!(
                    "Backed up original card asset: {} ({} bytes)",
                    asset,
                    data.len()
                ));
            }
            Err(err) => {
                // One unreadable asset must not abort the whole capture: the
                // remaining files still make the backup worth having.
                log(&format!(
                    "Could not read original card asset {}: {:#}",
                    asset, err
                ));
            }
        }
    }

    if backed_up == 0 {
        log("No original card artwork could be backed up; restore will stay unavailable.");
        return Ok(Some(resolved_hash));
    }

    write_metadata(&dir, udid, card_hash, &resolved_hash);
    log(&format!(
        "Original card face backed up: {} of {} asset(s).",
        backed_up,
        device_assets.len()
    ));
    Ok(Some(resolved_hash))
}

/// True when AFC works but `/var` is outside its sandbox, so the card bundle
/// cannot be reached.
///
/// `com.apple.afc` exposes only the Media sandbox, whose entries are bare names
/// (`/DCIM`, `/Photos`, `/Books`, `/Downloads`) and which answers error 8 for
/// anything under `/var/mobile` — exactly where Wallet keeps its card bundles.
/// The AFC2 services that offer the full filesystem are refused by current iOS.
///
/// This is a property of the device rather than a fault, so the check compares
/// against a path the Media sandbox must always allow: if `/DCIM` lists but
/// `/var/mobile` does not, AFC is healthy and the card bundle is simply out of
/// reach.
fn afc_sandbox_is_closed(afc: &AfcClient) -> bool {
    if !afc.list_directory("/DCIM").is_ok() {
        // AFC itself is not working; that is a different failure.
        return false;
    }
    afc.list_directory("/var/mobile").is_err()
}

/// Restore the backed-up artwork, under the names it was captured with.
///
/// Reads whatever is in the backup directory, in the preferred order, rather
/// than assuming the three well-known files were all present.
pub fn load_original_assets(udid: &str, card_hash: &str) -> Result<Vec<(String, Vec<u8>)>> {
    let assets = scan_backup_assets(&backup_dir(udid, card_hash));
    if assets.is_empty() {
        bail!(
            "Original card face backup not found for card hash {}",
            card_hash
        );
    }
    Ok(assets)
}

/// Every artwork file in a local backup, in restore order.
fn scan_backup_assets(dir: &Path) -> Vec<(String, Vec<u8>)> {
    local_backup_assets(dir)
        .into_iter()
        .filter_map(|name| {
            let path = dir.join(&name);
            fs::read(&path).ok().map(|data| (name, data))
        })
        .collect()
}

/// Record what was captured, so a restore knows the real asset names rather
/// than assuming the three preferred ones were all present.
fn write_metadata(dir: &Path, udid: &str, card_hash: &str, resolved_hash: &str) {
    let assets: Vec<String> = local_backup_assets(dir);
    let json = serde_json::json!({
        "udid": udid,
        "card_hash": card_hash,
        "resolved_hash": resolved_hash,
        "assets": assets,
    });
    if let Ok(text) = serde_json::to_string_pretty(&json) {
        let _ = write_backup_file(&dir.join(METADATA_FILE), text.as_bytes());
    }
}

fn write_backup_file(path: &Path, data: &[u8]) -> Result<()> {
    let temporary = path.with_extension("tmp");
    fs::write(&temporary, data)?;
    if path.exists() {
        fs::remove_file(path)?;
    }
    fs::rename(temporary, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every padding variant of a hash, which is what makes directory discovery
    /// work when the device renders the hash with a different trailing `=`.
    #[test]
    fn card_hash_candidates_cover_every_padding_form() {
        let candidates = card_hash_candidates("ZG+pLeL8u+lORdfO1J481gIgnPk=");
        assert!(candidates.contains(&"ZG+pLeL8u+lORdfO1J481gIgnPk=".to_string()));
        assert!(candidates.contains(&"ZG+pLeL8u+lORdfO1J481gIgnPk".to_string()));
        // A hash that already ends in `=` must not gain a bogus `===`.
        assert!(!candidates.contains(&"ZG+pLeL8u+lORdfO1J481gIgnPk===".to_string()));
    }

    #[test]
    fn card_hash_candidates_pad_an_unpadded_hash() {
        let candidates = card_hash_candidates("ZG+pLeL8u+lORdfO1J481gIgnPk");
        assert!(candidates.contains(&"ZG+pLeL8u+lORdfO1J481gIgnPk=".to_string()));
        assert!(candidates.contains(&"ZG+pLeL8u+lORdfO1J481gIgnPk==".to_string()));
    }

    /// The empty directory left behind by a failed capture is not a backup.
    #[test]
    fn backup_exists_is_false_for_an_empty_directory() {
        let dir = temp_dir("empty");
        fs::create_dir_all(&dir).unwrap();
        assert!(!backup_exists_in(&dir));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn backup_exists_is_false_for_metadata_only() {
        let dir = temp_dir("meta-only");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(METADATA_FILE), b"{}").unwrap();
        assert!(!backup_exists_in(&dir));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn backup_exists_is_true_for_a_single_artwork_file() {
        let dir = temp_dir("one-asset");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("cardBackgroundCombined@3x.png"), b"png").unwrap();
        assert!(backup_exists_in(&dir));
        assert_eq!(local_backup_assets(&dir).len(), 1);
        let _ = fs::remove_dir_all(&dir);
    }

    /// A name outside the preferred three is still a backup: the device decides
    /// what the card bundle contains, not this app.
    #[test]
    fn backup_exists_accepts_unexpected_asset_names() {
        let dir = temp_dir("odd-name");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("artwork@2x.png"), b"png").unwrap();
        assert!(backup_exists_in(&dir));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn backup_ignores_temporary_files() {
        let dir = temp_dir("tmp-only");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("cardBackgroundCombined@3x.png.tmp"), b"partial").unwrap();
        assert!(!backup_exists_in(&dir));
        let _ = fs::remove_dir_all(&dir);
    }

    /// The preferred order decides which file is restored first, so @3x is put
    /// back before @2x.
    #[test]
    fn backup_assets_are_ordered_by_preference() {
        let dir = temp_dir("ordering");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("cardBackgroundCombined.pdf"), b"pdf").unwrap();
        fs::write(dir.join("cardBackgroundCombined@2x.png"), b"2x").unwrap();
        fs::write(dir.join("cardBackgroundCombined@3x.png"), b"3x").unwrap();
        fs::write(dir.join("stripes.png"), b"other").unwrap();

        let order = local_backup_assets(&dir);
        assert_eq!(
            order,
            vec![
                "cardBackgroundCombined@3x.png".to_string(),
                "cardBackgroundCombined@2x.png".to_string(),
                "cardBackgroundCombined.pdf".to_string(),
                "stripes.png".to_string(),
            ]
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn scan_backup_assets_reads_real_contents() {
        let dir = temp_dir("contents");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("cardBackgroundCombined@3x.png"), b"3x-bytes").unwrap();

        let assets = scan_backup_assets(&dir);
        assert_eq!(assets.len(), 1);
        assert_eq!(assets[0].0, "cardBackgroundCombined@3x.png");
        assert_eq!(assets[0].1, b"3x-bytes");
        let _ = fs::remove_dir_all(&dir);
    }

    /// `backup_exists` is defined in terms of the on-disk directory; this keeps
    /// the test independent of `LOCALAPPDATA`.
    fn backup_exists_in(dir: &Path) -> bool {
        !local_backup_assets(dir).is_empty()
    }

    fn temp_dir(label: &str) -> PathBuf {
        let unique = std::process::id();
        std::env::temp_dir().join(format!("aircard-test-{unique}-{label}"))
    }
}
