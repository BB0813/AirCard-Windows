//! PassKit pass packages (`.pkpass`).
//!
//! A pass is a signed zip whose images follow Apple's naming rules rather than a
//! fixed list. Reading one gives the real set of filenames for a given card,
//! which is what makes editing anything beyond the card background possible —
//! the device-side directory cannot be listed over AFC on current iOS.

use std::fs::File;
use std::path::Path;

use anyhow::{Context, Result, bail};
use zip::ZipArchive;

/// Apple's documented pass image slots, in the order PassKit prefers them.
///
/// Each entry is the base name; the `@2x`/`@3x` variants and the file extension
/// are resolved from what the package actually contains.
const KNOWN_SLOTS: &[&str] = &[
    "background",
    "backgroundPortrait",
    "strip",
    "thumbnail",
    "thumbnail@2x",
    "logo",
    "footer",
    "icon",
];

/// Image extensions a pass may carry, ordered by preference.
const IMAGE_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "gif"];

/// One image found inside a pass package.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PassAsset {
    /// Path inside the zip, e.g. `logo@2x.png`.
    pub name: String,
    /// Uncompressed size in bytes.
    pub size: u64,
}

impl PassAsset {
    /// The slot this asset fills, e.g. `logo` for `logo@2x.png`.
    pub fn slot(&self) -> String {
        let stem = self
            .name
            .rsplit_once('.')
            .map(|(stem, _)| stem)
            .unwrap_or(&self.name);
        stem.trim_end_matches("@2x")
            .trim_end_matches("@3x")
            .to_string()
    }

    /// Whether this is a `@2x` or `@3x` variant rather than the base image.
    pub fn scale(&self) -> Option<&'static str> {
        if self.name.contains("@3x") {
            Some("@3x")
        } else if self.name.contains("@2x") {
            Some("@2x")
        } else {
            None
        }
    }
}

/// A pass package read from disk.
pub struct PassPackage {
    assets: Vec<PassAsset>,
    /// `pass.json` if present, which names the pass type and its fields.
    pass_json: Option<String>,
}

impl PassPackage {
    /// Read the image list from a `.pkpass` zip.
    pub fn open(path: &Path) -> Result<Self> {
        let file =
            File::open(path).with_context(|| format!("Could not open {}", path.display()))?;
        let mut archive = ZipArchive::new(file)
            .with_context(|| format!("{} is not a readable zip/.pkpass file", path.display()))?;

        let mut assets = Vec::new();
        let mut pass_json = None;

        for index in 0..archive.len() {
            let entry = archive
                .by_index(index)
                .with_context(|| format!("Could not read entry {index} of the pass package"))?;
            let name = entry.name().to_string();

            if entry.is_dir() {
                continue;
            }
            if name == "pass.json" {
                use std::io::Read;
                let mut json = String::new();
                // Re-open by name, since `entry` was consumed above.
                drop(entry);
                let mut again = archive
                    .by_name("pass.json")
                    .context("pass.json disappeared while reading")?;
                again
                    .read_to_string(&mut json)
                    .context("Could not read pass.json")?;
                pass_json = Some(json);
                continue;
            }

            if is_pass_image(&name) {
                assets.push(PassAsset {
                    name,
                    size: entry.size(),
                });
            }
        }

        if assets.is_empty() {
            bail!(
                "No pass images found in {}. A .pkpass package normally contains logo, strip or background images.",
                path.display()
            );
        }

        // Preferred slots first, then anything unexpected, each alphabetically
        // so the list is stable between runs.
        assets.sort_by(|a, b| {
            slot_rank(&a.slot())
                .cmp(&slot_rank(&b.slot()))
                .then(a.name.cmp(&b.name))
        });

        Ok(Self { assets, pass_json })
    }

    /// The images the package carries.
    pub fn assets(&self) -> &[PassAsset] {
        &self.assets
    }

    /// Whether the package declared a `pass.json`.
    pub fn has_manifest(&self) -> bool {
        self.pass_json.is_some()
    }

    /// Slots that exist in this package, in preference order.
    pub fn slots(&self) -> Vec<String> {
        let mut seen = Vec::new();
        for asset in &self.assets {
            if !seen.contains(&asset.slot()) {
                seen.push(asset.slot());
            }
        }
        seen
    }

    /// Copy the package's images into a local archive directory.
    ///
    /// The device-side pass directory cannot be read over AFC on current iOS, so
    /// the original artwork can only be preserved from the `.pkpass` the pass
    /// arrived in. Written with the same temp-then-rename pattern as the wallet
    /// backups, so a partial copy never looks like a complete one.
    pub fn archive_assets(&self, path: &Path, destination: &Path) -> Result<usize> {
        std::fs::create_dir_all(destination).with_context(|| {
            format!(
                "Could not create the archive directory {}",
                destination.display()
            )
        })?;

        let file =
            File::open(path).with_context(|| format!("Could not reopen {}", path.display()))?;
        let mut archive =
            ZipArchive::new(file).context("The pass package changed while reading")?;

        let mut written = 0;
        for asset in &self.assets {
            let target = destination.join(&asset.name);
            if target.is_file() {
                // Already archived; keep the first copy, which is the original.
                written += 1;
                continue;
            }

            let mut source = archive
                .by_name(&asset.name)
                .with_context(|| format!("Could not read {} from the pass package", asset.name))?;
            let temporary = destination.join(format!("{}.tmp", asset.name));
            {
                let mut out = std::fs::File::create(&temporary)
                    .with_context(|| format!("Could not create {}", temporary.display()))?;
                std::io::copy(&mut source, &mut out).with_context(|| {
                    format!("Could not copy {} out of the pass package", asset.name)
                })?;
            }
            if target.exists() {
                let _ = std::fs::remove_file(&target);
            }
            std::fs::rename(&temporary, &target)
                .with_context(|| format!("Could not move {} into the archive", asset.name))?;
            written += 1;
        }

        Ok(written)
    }
}

/// Whether a zip entry is an image PassKit would use.
fn is_pass_image(name: &str) -> bool {
    // Pass packages are flat: reject anything Apple adds in a subdirectory.
    if name.contains('/') {
        return false;
    }
    let Some(ext) = name.rsplit_once('.').map(|(_, ext)| ext.to_lowercase()) else {
        return false;
    };
    IMAGE_EXTENSIONS.contains(&ext.as_str())
}

fn slot_rank(slot: &str) -> usize {
    KNOWN_SLOTS
        .iter()
        .position(|known| known.eq_ignore_ascii_case(slot))
        .unwrap_or(KNOWN_SLOTS.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slot_and_scale_are_extracted_from_the_filename() {
        let asset = PassAsset {
            name: "logo@2x.png".to_string(),
            size: 100,
        };
        assert_eq!(asset.slot(), "logo");
        assert_eq!(asset.scale(), Some("@2x"));
        assert!(!asset.name.contains('/'));
    }

    #[test]
    fn base_images_have_no_scale() {
        let asset = PassAsset {
            name: "strip.png".to_string(),
            size: 100,
        };
        assert_eq!(asset.slot(), "strip");
        assert_eq!(asset.scale(), None);
    }

    /// Any asset at all beats nothing, but the three card-background names are
    /// what the wallet skinner needs most, so they sort first.
    #[test]
    fn preferred_slots_sort_before_unknown_ones() {
        let assets = [
            PassAsset {
                name: "zzz.png".to_string(),
                size: 1,
            },
            PassAsset {
                name: "logo.png".to_string(),
                size: 1,
            },
            PassAsset {
                name: "background.png".to_string(),
                size: 1,
            },
        ];
        let mut sorted = assets.to_vec();
        sorted.sort_by(|a, b| {
            slot_rank(&a.slot())
                .cmp(&slot_rank(&b.slot()))
                .then(a.name.cmp(&b.name))
        });
        assert_eq!(sorted[0].name, "background.png");
        assert_eq!(sorted[1].name, "logo.png");
        assert_eq!(sorted[2].name, "zzz.png");
    }

    /// A real package is read so `open`, `slots` and `archive_assets` are
    /// covered against an actual zip rather than a hand-built value.
    #[test]
    fn a_real_pkpass_zip_is_read_and_archived() {
        use zip::write::SimpleFileOptions;

        // Build a pass package the way Apple does: a flat zip with images.
        let temp =
            std::env::temp_dir().join(format!("aircard-passkit-{}.pkpass", std::process::id()));
        {
            let file = std::fs::File::create(&temp).unwrap();
            let mut writer = zip::ZipWriter::new(file);
            for name in ["logo@2x.png", "strip.png", "background.png", "pass.json"] {
                writer
                    .start_file(name, SimpleFileOptions::default())
                    .unwrap();
                std::io::Write::write_all(&mut writer, name.as_bytes()).unwrap();
            }
            writer.finish().unwrap();
        }

        let package = PassPackage::open(&temp).expect("the package should read");
        // pass.json is not an image, so only the three images are listed.
        assert_eq!(package.assets().len(), 3);
        assert!(package.has_manifest());
        assert_eq!(
            package.slots(),
            vec![
                "background".to_string(),
                "strip".to_string(),
                "logo".to_string()
            ]
        );

        let archive_dir =
            std::env::temp_dir().join(format!("aircard-passkit-archive-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&archive_dir);
        let written = package
            .archive_assets(&temp, &archive_dir)
            .expect("archiving should succeed");
        assert_eq!(written, 3, "all three images should be archived");
        assert!(archive_dir.join("logo@2x.png").is_file());
        assert!(!archive_dir.join("pass.json").exists());

        // A second pass keeps the first copy, so re-running is idempotent.
        let again = package
            .archive_assets(&temp, &archive_dir)
            .expect("re-archiving should succeed");
        assert_eq!(
            again, 3,
            "already-archived assets are counted, not rewritten"
        );

        let _ = std::fs::remove_dir_all(&archive_dir);
        let _ = std::fs::remove_file(&temp);
    }
}
