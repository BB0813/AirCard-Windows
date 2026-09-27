//! Read-only device inspection, used to diagnose why a backup or transfer
//! failed. Invoked as `aircard.exe probe [udid]`.
//!
//! The questions it answers are the ones that are otherwise invisible: can
//! FairPlay load, does AFC reach the Wallet card directory, what artwork files
//! does the device actually have, and is there a local backup for the card.

use anyhow::{Context, Result};

use crate::afc::AfcClient;
use crate::apple::get_apple_libraries;
use crate::device::{ActiveDeviceSession, ConnectionMode};

pub fn run(udid: Option<&str>) {
    match inspect(udid) {
        Ok(()) => {}
        Err(err) => {
            println!("PROBE FAILED: {err:#}");
            std::process::exit(1);
        }
    }
}

fn inspect(udid: Option<&str>) -> Result<()> {
    println!("=== AirCard device probe ===\n");

    // 1. FairPlay: the layer that silently blocks the handshake.
    match crate::corefp::probe_core_fp() {
        crate::corefp::CoreFpHealth::Available { path } => {
            println!("[ok]   FairPlay CoreFP.dll: {path}");
        }
        crate::corefp::CoreFpHealth::MissingFile { expected } => {
            println!("[fail] FairPlay CoreFP.dll: registry points at {expected}, which is missing");
            println!("       {}", crate::corefp::remediation_hint());
        }
        crate::corefp::CoreFpHealth::Unresolved => {
            println!("[fail] FairPlay CoreFP.dll: not found on this PC");
            println!("       {}", crate::corefp::remediation_hint());
        }
    }

    // 2. Apple support libraries, which every other step depends on.
    get_apple_libraries().context("Apple Mobile Device Support is not usable")?;
    println!("[ok]   Apple Mobile Device Support loaded");

    // 3. Device session over AFC.
    println!(
        "\nOpening device session{}...",
        udid.map(|u| format!(" for {u}")).unwrap_or_default()
    );
    let session = ActiveDeviceSession::open(udid, ConnectionMode::Auto).context(
        "Could not open a device session. Is the iPhone connected, unlocked and trusted?",
    )?;
    println!(
        "[ok]   Connected over {} ({})",
        session.transport.label(),
        session.udid
    );

    // 3. Which AFC service can actually reach the card bundle.
    //
    // `com.apple.afc` serves the Media sandbox and, on recent iOS, answers
    // error 8 for every path under /var. Full access is offered under a
    // different service name, so try the known ones and keep whichever works.
    println!("\n--- AFC service probing ---");
    let mut afc: Option<AfcClient> = None;
    for name in crate::afc::AFC_SERVICE_CANDIDATES {
        match AfcClient::new_on_service(&session, name) {
            Ok(client) => match client.list_directory(crate::afc::CARDS_DIR) {
                Ok(entries) => {
                    println!(
                        "[ok]   {name}: reaches {}, {} entr{}",
                        crate::afc::CARDS_DIR,
                        entries.len(),
                        if entries.len() == 1 { "y" } else { "ies" }
                    );
                    if afc.is_none() {
                        afc = Some(client);
                    }
                }
                Err(err) => {
                    println!("[----] {name}: started, but cannot read the card directory ({err:#})")
                }
            },
            Err(err) => println!("[----] {name}: {err}"),
        }
    }

    // Whether the connection negotiated a secure IO context. A null context is
    // normal — the macOS reference implementation passes NULL options too — and
    // does not affect which paths AFC can reach. Reported only so it is not
    // mistaken for the cause of a path being refused.
    if let Ok(client) = AfcClient::new(&session) {
        println!(
            "\n  AFC secure IO context: {}",
            if client.is_encrypted() {
                "established (encrypted)"
            } else {
                "not established (unencrypted, but functional)"
            }
        );
    }

    // 4. Detail the card bundles through whichever service reached them.
    println!("\n--- Wallet card bundles ---");
    match &afc {
        Some(afc) => {
            let entries = afc
                .list_directory(crate::afc::CARDS_DIR)
                .unwrap_or_default();
            for entry in entries.iter().filter(|e| e.contains('.')) {
                let bundle = format!("{}/{entry}", crate::afc::CARDS_DIR);
                println!("  {bundle}");
                match afc.list_directory(&bundle) {
                    Ok(files) => {
                        for file in &files {
                            let path = format!("{bundle}/{file}");
                            let size = afc.file_size(&path);
                            println!(
                                "      {file}{}",
                                size.map(|s| format!("  ({s} bytes)")).unwrap_or_default()
                            );
                        }
                    }
                    Err(err) => println!("      (could not list: {err:#})"),
                }
            }
        }
        None => {
            println!("[fail] No AFC service could read the card directory.");
            println!("       The original card face cannot be backed up, so Restore Original");
            println!("       stays unavailable. Applying a skin still works through AirTraffic.");

            // Show where AFC *is* allowed to look, so the boundary is visible
            // rather than implied. Listing the entries at "/" also proves AFC
            // itself works — a dead connection fails there too.
            println!("\n--- AFC sandbox boundary ---");
            if let Ok(client) = AfcClient::new(&session) {
                for probe in [
                    "/",
                    "/DCIM",
                    "/Books",
                    "/Downloads",
                    "/var",
                    "/var/mobile",
                    "/var/mobile/Library",
                    "/var/mobile/Library/Passes",
                ] {
                    match client.list_directory(probe) {
                        Ok(entries) => {
                            println!(
                                "  readable: {probe} ({} entr{})",
                                entries.len(),
                                if entries.len() == 1 { "y" } else { "ies" }
                            );
                            // Listing a few names makes the sandbox's shape
                            // obvious rather than implied by the count.
                            if probe == "/DCIM" {
                                for name in entries.iter().take(10) {
                                    println!("             {name}");
                                }
                            }
                            // The Books tree matters because it is the one the
                            // transfer writes into and AFC can still read, which
                            // is what makes a read-back of card artwork viable.
                            if probe == "/Books" {
                                for name in entries.iter().take(10) {
                                    println!("             {name}");
                                }
                                for sub in ["Sync", "MetadataStore"] {
                                    let path = format!("/Books/{sub}");
                                    if let Ok(inner) = client.list_directory(&path) {
                                        println!(
                                            "        {path}: {} entr{}",
                                            inner.len(),
                                            if inner.len() == 1 { "y" } else { "ies" }
                                        );
                                    }
                                }
                            }
                        }
                        Err(err) => println!("  denied  : {probe} ({err})"),
                    }
                }
            }
        }
    }

    // 5. Local backups, so the restore button's state can be explained.
    println!("\n--- Local card backups ---");
    let root = crate::wallet_backup::backup_root_for_probe();
    match std::fs::read_dir(&root) {
        Ok(entries) => {
            let mut any = false;
            for entry in entries.filter_map(|e| e.ok()) {
                any = true;
                let dir = entry.path();
                let files: Vec<String> = std::fs::read_dir(&dir)
                    .map(|rd| {
                        rd.filter_map(|f| f.ok())
                            .map(|f| f.file_name().to_string_lossy().to_string())
                            .collect()
                    })
                    .unwrap_or_default();
                println!(
                    "    {}{}: {} file(s)",
                    dir.display(),
                    if files.is_empty() { " (EMPTY)" } else { "" },
                    files.len()
                );
                for file in &files {
                    println!("        {file}");
                }
            }
            if !any {
                println!("    (no backups yet)");
            }
        }
        Err(err) => println!("    (backup directory missing: {err})"),
    }

    println!("\n=== probe complete ===");
    Ok(())
}
