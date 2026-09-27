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
    // `aircard.exe probe --roundtrip` writes one small marker into the Media
    // sandbox to find out whether AFC can read its own writes back.
    let roundtrip = std::env::args().any(|arg| arg == "--roundtrip");
    let result = if roundtrip {
        afc_roundtrip()
    } else {
        inspect(udid)
    };

    match result {
        Ok(()) => {}
        Err(err) => {
            println!("PROBE FAILED: {err:#}");
            std::process::exit(1);
        }
    }
}

/// Whether AFC can create a file in the Media sandbox and read it back.
///
/// Settles the open question behind the restore limitation: AirTraffic writes
/// into the pass bundle but lands somewhere AFC cannot read, and AFC reads only
/// the sandbox. If AFC round-trips its own files here, the sandbox is a usable
/// staging area for preserving the originals.
///
/// Writes one small marker into a directory it creates, then removes both. The
/// pass bundle is never touched.
fn afc_roundtrip() -> Result<()> {
    println!("=== AFC sandbox round-trip ===\n");

    let session = ActiveDeviceSession::open(None, ConnectionMode::Auto)
        .context("Could not open a device session")?;
    println!(
        "[ok] session over {} ({})",
        session.transport.label(),
        session.udid
    );

    let afc = AfcClient::new(&session).context("Could not open AFC")?;
    println!("[ok] AFC open (encrypted: {})", afc.is_encrypted());

    let marker = b"aircard-sandbox-roundtrip-v1";
    for dir in ["/AirCardProbe", "/iTunes_Control/AirCardProbe"] {
        println!("\n--- {dir} ---");

        if let Err(e) = afc.make_directory_recursive(dir) {
            println!("  create : FAILED ({e:#})");
            continue;
        }
        println!("  create : ok");

        let file = format!("{dir}/probe.bin");
        if let Err(e) = afc.write_file(&file, marker) {
            println!("  write  : FAILED ({e:#})");
            let _ = afc.remove_path(dir);
            continue;
        }
        println!("  write  : ok ({} bytes)", marker.len());

        match afc.read_file(&file) {
            Ok(read_back) if read_back == marker => {
                println!("  read   : MATCH - AFC round-trips its own writes here");
            }
            Ok(read_back) => {
                println!("  read   : MISMATCH ({} bytes back)", read_back.len());
            }
            Err(e) => println!("  read   : FAILED ({e:#})"),
        }

        // Leave the device as it was found.
        let _ = afc.remove_path(&file);
        let _ = afc.remove_path(dir);
        println!("  cleanup: done");
    }

    // The directory that would carry pass artwork.
    println!("\n--- /iTunes_Control/iTunes/Artwork/Originals ---");
    // Descend one level: the sharded layout is what tells us whether these are
    // artwork directories at all.
    let originals = "/iTunes_Control/iTunes/Artwork/Originals";
    match afc.list_directory(originals) {
        Ok(entries) => {
            println!(
                "  readable: {} entr{}",
                entries.len(),
                if entries.len() == 1 { "y" } else { "ies" }
            );
            for name in entries.iter().take(4) {
                println!("      {name}");
                let child = format!("{originals}/{name}");
                if let Ok(inner) = afc.list_directory(&child) {
                    println!(
                        "        {} entr{}",
                        inner.len(),
                        if inner.len() == 1 { "y" } else { "ies" }
                    );
                    for f in inner.iter().take(6) {
                        println!("            {f}");
                    }
                }
            }
        }
        Err(e) => println!("  denied  : {e:#}"),
    }

    // Can AirTraffic land a file where AFC reads it back?
    //
    // An earlier probe searched the wrong path and wrongly concluded it could
    // not: the write lands under `airlift-link-<token>/`, not under the requested
    // directory. That mistake cost the restore feature its best route, so the
    // real destination is searched for here.
    println!("\n--- AirTraffic write, AFC read back ---");
    match airtraffic_write_then_read() {
        Ok(msg) => println!("  {msg}"),
        Err(e) => println!("  FAILED: {e:#}"),
    }

    println!("\n=== round-trip complete ===");
    Ok(())
}

/// Write one marker through the same path the skin transfer uses, then find it
/// over AFC.
///
/// The target is inside the Media sandbox, so this measures the channel rather
/// than the escape. The pass bundle is never involved.
fn airtraffic_write_then_read() -> Result<String> {
    let marker = b"aircard-airtraffic-probe";

    // `write_system_file` deletes its staging copies the moment it returns, so
    // the file can only be seen from inside the transfer. It already opens an
    // AFC session for staging, and it logs each step - so the write is driven to
    // completion and the target is read back from a second session opened
    // immediately afterwards, while the device still has the file in place.
    //
    // Note the target is a sandbox path, not the pass bundle: this measures the
    // channel and touches no card.
    #[allow(unused_variables)]
    let mut steps = Vec::new();
    let write_ok = crate::flasher::write_system_file_keep_staging(
        "00008140-00184D21222A801C",
        ConnectionMode::Auto,
        "AirCardProbe",
        "probe.bin",
        marker,
        |msg| steps.push(msg.to_string()),
    );
    if let Err(e) = &write_ok {
        return Ok(format!("write FAILED: {e:#}"));
    }

    // The symlink target the exploit builds points at /AirCardProbe, three levels
    // up from the staging directory - which is inside the sandbox. So the file
    // should be readable straight from there.
    let session = ActiveDeviceSession::open(None, ConnectionMode::Auto)
        .context("could not open a session to verify")?;
    let afc = AfcClient::new(&session).context("could not open AFC to verify")?;

    let mut found = Vec::new();
    for path in [
        "/AirCardProbe/probe.bin".to_string(),
        "/iTunes_Control/AirCardProbe/probe.bin".to_string(),
    ] {
        match afc.read_file(&path) {
            Ok(bytes) if bytes == marker => found.push(format!("{path} (MATCH)")),
            Ok(bytes) => found.push(format!("{path} ({} bytes)", bytes.len())),
            Err(_) => {}
        }
    }

    // The real landing place: the requested directory is created *inside* the
    // staging directory, because the symlink target `../../../<tail>` is resolved
    // relative to `airlift-src-<token>/p0/p1/p2` - which lands back in the
    // staging directory, not at the sandbox root.
    for prefix in ["airlift-src-", "airlift-link-"] {
        for hit in afc
            .list_directory("/")
            .unwrap_or_default()
            .iter()
            .filter(|name| name.starts_with(prefix))
            .cloned()
            .collect::<Vec<_>>()
        {
            // Walk the staging directory to the leaf, rather than assuming the
            // shape: the symlink's `../../../<tail>` resolves relative to the
            // staging directory, so the target may sit at any depth.
            let mut stack = vec![format!("/{hit}")];
            while let Some(dir) = stack.pop() {
                let Ok(inner) = afc.list_directory(&dir) else {
                    continue;
                };
                for name in inner {
                    let path = format!("{dir}/{name}");
                    if let Ok(children) = afc.list_directory(&path) {
                        if !children.is_empty() {
                            stack.push(path);
                            continue;
                        }
                    }
                    // Report every leaf, so the tree walk shows what is there
                    // rather than only what matched.
                    match afc.read_file(&path) {
                        Ok(bytes) if bytes == marker => found.push(format!("{path} (MATCH)")),
                        Ok(bytes) => found.push(format!("{path} ({} bytes)", bytes.len())),
                        Err(_) => found.push(format!("{path} (unreadable)")),
                    }
                }
            }
        }
    }

    if found.is_empty() {
        // Nothing matched the expected names. Dump the sandbox root so the real
        // landing place is visible instead of guessed at.
        let root = afc.list_directory("/").unwrap_or_default();
        let detail = root
            .iter()
            .filter(|n| {
                !matches!(
                    n.as_str(),
                    "DCIM" | "Books" | "Downloads" | "Photos" | "Music"
                )
            })
            .map(|name| {
                let path = format!("/{name}");
                match afc.list_directory(&path) {
                    Ok(inner) => format!("{name}/[{}]", inner.join(",")),
                    Err(_) => name.clone(),
                }
            })
            .collect::<Vec<_>>();
        return Ok(format!("not found. sandbox root: {}", detail.join("  ")));
    }
    Ok(found.join(", "))
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
            println!("       The original card artwork cannot be copied off the device, so");
            println!("       \"Restore Original\" is unavailable. Applying a skin still works:");
            println!("       that path goes through AirTraffic, which can write the bundle.");
            println!();
            println!("       Note: AirTraffic's writes *can* be read back over AFC - see");
            println!("       `probe --roundtrip`. The blocker is that no message lets the device");
            println!("       send the card bundle outward in the first place.");

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
                    "/iTunes_Control",
                    "/iTunes_Control/iTunes",
                    "/iTunes_Control/iTunes/Artwork",
                    "/PublicStaging",
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
                            // One level into the shared area, to see whether it
                            // could carry a copy of pass artwork.
                            if probe.starts_with("/iTunes_Control") {
                                for name in entries.iter().take(12) {
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

    // Where a copy of card artwork might survive. The exploit stages through
    // Books, and Books is one of the few trees AFC can read - so an overwritten
    // card should not be assumed gone without looking here.
    println!("\n--- Possible card-artwork remnants in Books ---");
    if let Ok(client) = AfcClient::new(&session) {
        for dir in [
            "/Books/Sync/Artwork",
            "/Books/Sync",
            "/Books/Purchases",
            "/Books/Managed",
        ] {
            match client.list_directory(dir) {
                Ok(entries) => {
                    println!(
                        "  readable: {dir} ({} entr{})",
                        entries.len(),
                        if entries.len() == 1 { "y" } else { "ies" }
                    );
                    for name in entries.iter().take(12) {
                        let path = format!("{dir}/{name}");
                        let size = client.file_size(&path);
                        println!(
                            "      {name}{}",
                            size.map(|s| format!("  ({s} bytes)")).unwrap_or_default()
                        );
                    }
                }
                Err(err) => println!("  denied  : {dir} ({err:#})"),
            }
        }
    }

    println!("\n=== probe complete ===");
    Ok(())
}
