// The GUI build is a windows application, but `aircard.exe probe` prints a
// diagnostic report and needs a console to write it to. Attach to the parent's
// console when launched from a terminal, and fall back to allocating one.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod afc;
mod airlift;
mod airtraffic;
mod app;
mod apple;
mod corefp;
mod device;
mod flasher;
mod i18n;
mod image_skin;
mod passkit;
mod passthm;
mod probe;
mod probe_pass;
mod scanner;
mod wallet_backup;

#[cfg(windows)]
fn attach_console() {
    unsafe extern "system" {
        fn AttachConsole(process_id: u32) -> i32;
    }
    // ATTACH_PARENT_PROCESS
    unsafe {
        if AttachConsole(0xFFFF_FFFF) == 0 {
            return;
        }
    }
}

fn main() -> eframe::Result<()> {
    // `aircard.exe probe` runs a read-only device inspection and exits. It is a
    // diagnostic aid for reports where a backup or transfer fails: it prints
    // what the device actually exposes instead of requiring a screenshot.
    if std::env::args().nth(1).as_deref() == Some("probe-pass") {
        #[cfg(windows)]
        attach_console();
        probe_pass::run();
        return Ok(());
    }
    if std::env::args().nth(1).as_deref() == Some("probe") {
        #[cfg(windows)]
        attach_console();
        let udid = std::env::args().nth(2);
        probe::run(udid.as_deref());
        return Ok(());
    }

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1000.0, 680.0])
            .with_min_inner_size([900.0, 600.0])
            .with_title("AirCard v1.4.0"),
        ..Default::default()
    };

    eframe::run_native(
        "AirCard v1.4.0",
        options,
        Box::new(|cc| Ok(Box::new(app::AirCardApp::new(cc)))),
    )
}
