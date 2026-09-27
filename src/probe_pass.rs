// Where might an original .pkpass be reachable?
//
// The pass bundle itself is under /var, which AFC cannot read and no AirTraffic
// message asks the device to send outward. But a pass can arrive on the device
// through other routes that leave a copy somewhere AFC *can* read, and those are
// worth checking before concluding nothing can be recovered.
//
// Read-only: nothing is written, nothing is moved.

use anyhow::{Context, Result};

use crate::afc::AfcClient;
use crate::device::{ActiveDeviceSession, ConnectionMode};

pub fn run() {
    match scan() {
        Ok(()) => {}
        Err(err) => {
            println!("PROBE FAILED: {err:#}");
            std::process::exit(1);
        }
    }
}

fn scan() -> Result<()> {
    println!("=== .pkpass reachability scan ===\n");

    let session = ActiveDeviceSession::open(None, ConnectionMode::Auto)
        .context("Could not open a device session")?;
    println!("[ok] session over {} ({})", session.transport.label(), session.udid);

    let afc = AfcClient::new(&session).context("Could not open AFC")?;

    // Places a pass could be sitting that are outside /var.
    //
    // Downloads and iTunes File Sharing are the two that matter: a pass received
    // by mail or a browser lands in Downloads, and an app that declares file
    // sharing exposes its container under iTunes_Control.
    let candidates = [
        // Safari / mail downloads.
        "/Downloads",
        // iTunes file sharing containers, per-app.
        "/iTunes_Control/iTunes",
        // Books, where a pass can be attached as an asset.
        "/Books",
        "/Books/Purchases",
        "/Books/Managed",
        // Photo libraries sometimes hold screenshots of a pass.
        "/DCIM",
        "/PublicStaging",
        // AirDrop staging.
        "/AirFair",
        "/Airlock",
    ];

    let mut hits = Vec::new();

    for root in candidates {
        println!("\n--- {root} ---");
        walk(&afc, root, 0, &mut hits);
    }

    println!("\n=== Files whose name suggests a pass ===");
    if hits.is_empty() {
        println!("  (nothing found)");
    } else {
        for hit in &hits {
            println!("  {hit}");
        }
    }

    // Also report anything pass-shaped regardless of name: a .pkpass is a zip,
    // but AFC exposes no content type, so the name is all there is to go on.
    println!("\n=== scan complete ===");
    Ok(())
}

/// Depth-first listing, recording anything that looks like a pass package.
fn walk(afc: &AfcClient, dir: &str, depth: usize, hits: &mut Vec<String>) {
    // Deep enough: these trees are wide, and a pass is never buried past this.
    if depth > 4 {
        return;
    }

    let Ok(entries) = afc.list_directory(dir) else {
        println!("  {}denied  {dir}", "  ".repeat(depth));
        return;
    };

    println!(
        "  {}{dir} ({} entr{})",
        "  ".repeat(depth),
        entries.len(),
        if entries.len() == 1 { "y" } else { "ies" }
    );

    for name in &entries {
        let path = format!("{dir}/{name}");
        if name.to_lowercase().ends_with(".pkpass")
            || name.to_lowercase().ends_with(".pass")
            || name.to_lowercase().contains("passbook")
        {
            let size = afc.file_size(&path);
            hits.push(format!(
                "{path}{}",
                size.map(|s| format!("  ({s} bytes)")).unwrap_or_default()
            ));
        }

        // Descend into directories only. AFC has no stat, so a directory is
        // whatever lists successfully.
        if afc.list_directory(&path).is_ok() {
            walk(afc, &path, depth + 1, hits);
        }
    }
}
