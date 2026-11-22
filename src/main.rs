use std::time::Duration;

use clap::Parser;
use zmk_tui::simulator::SimulatedBoard;
use zmk_tui::tui::StartOptions;
use zmk_tui::tui::app::VialStart;

#[derive(Debug, Parser)]
#[command(
    name = "zmk-tui",
    version,
    about = "Terminal UI for editing ZMK keymaps over USB or Bluetooth",
    long_about = "A terminal keymap editor for ZMK Studio-enabled keyboards.\n\n\
                  Connect over USB CDC-ACM serial or Bluetooth LE, browse the physical\n\
                  keymap, edit bindings, manage layers, and save changes to the keyboard."
)]
struct Cli {
    /// Connect to this USB serial port on startup (e.g. /dev/cu.usbmodem1234)
    #[arg(short, long, value_name = "PORT")]
    port: Option<String>,

    /// Connect to the first Bluetooth keyboard whose name contains this text
    #[arg(short, long, value_name = "NAME", conflicts_with = "port")]
    ble: Option<String>,

    /// List USB and Bluetooth Studio keyboards, then exit
    #[arg(long)]
    list: bool,

    /// Start with a built-in simulated keyboard instead of hardware
    #[arg(
        long,
        value_name = "BOARD",
        value_enum,
        num_args = 0..=1,
        default_missing_value = "corne"
    )]
    simulate: Option<SimulatedBoard>,

    /// Edit the simulated Vial keyboard instead of a ZMK Studio device
    #[arg(long, conflicts_with_all = ["port", "ble", "simulate", "vial"])]
    simulate_vial: bool,

    /// Edit the first Vial keyboard over USB raw HID
    #[arg(long, conflicts_with_all = ["port", "ble", "simulate", "simulate_vial"])]
    vial: bool,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    if cli.list {
        list_devices().await?;
        return Ok(());
    }

    let vial = if cli.simulate_vial {
        Some(VialStart::Simulate)
    } else if cli.vial {
        Some(VialStart::Usb)
    } else {
        None
    };

    zmk_tui::tui::run(StartOptions {
        port: cli.port,
        ble: cli.ble,
        simulate: cli.simulate,
        vial,
        scan_vial: true,
        auto_connect: true,
    })
    .await
}

async fn list_devices() -> anyhow::Result<()> {
    let usb = zmk_tui::transport::serial::list_usb_devices();
    println!("USB serial devices ({}):", usb.len());
    if usb.is_empty() {
        println!("  (none)");
    }
    for device in &usb {
        println!("  {}", device.label());
        if let Some(manufacturer) = &device.manufacturer {
            println!("      manufacturer: {manufacturer}");
        }
        if let Some(serial) = &device.serial_number {
            println!("      serial: {serial}");
        }
    }

    println!();
    println!("Raw HID interfaces on usage page 0xFF60 / usage 0x61:");
    match zmk_tui::vial::transport::list() {
        Ok(devices) if devices.is_empty() => println!("  (none)"),
        Ok(devices) => {
            for device in devices {
                println!(
                    "  {:04X}:{:04X} {} vial_serial={} serial={:?} path={}",
                    device.vendor_id,
                    device.product_id,
                    device.product,
                    device.is_vial,
                    device.serial,
                    device.path
                );
            }
        }
        Err(error) => println!("  HID enumeration failed: {error}"),
    }

    println!();
    println!("Detected Vial keyboards:");
    let vial = zmk_tui::vial::transport::list_vial_devices();
    if vial.is_empty() {
        println!("  (none)");
        println!("  If this is a 2.4G receiver, try a wired USB connection.");
    }
    for device in vial {
        println!(
            "  {:04X}:{:04X} {}  serial={:?} path={}",
            device.vendor_id, device.product_id, device.name, device.serial, device.path
        );
    }

    println!();
    println!("Bluetooth LE Studio keyboards:");
    match zmk_tui::transport::ble::scan(Duration::from_secs(3)).await {
        Ok(devices) if devices.is_empty() => {
            println!("  (none)");
            println!();
            println!(
                "Bluetooth keyboards must be connected to this computer, and the firmware\n\
                 must be built with CONFIG_ZMK_STUDIO=y."
            );
        }
        Ok(devices) => {
            for device in &devices {
                println!("  {}", device.label());
            }
        }
        Err(error) => {
            println!("  scan failed: {error:#}");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn simulate_alone_selects_corne() {
        let cli = Cli::try_parse_from(["zmk-tui", "--simulate"]).expect("parse");
        assert_eq!(cli.simulate, Some(SimulatedBoard::Corne));
    }

    #[test]
    fn simulate_accepts_sofle() {
        let cli = Cli::try_parse_from(["zmk-tui", "--simulate", "sofle"]).expect("parse");
        assert_eq!(cli.simulate, Some(SimulatedBoard::Sofle));
    }

    #[test]
    fn no_simulate_flag_is_none() {
        let cli = Cli::try_parse_from(["zmk-tui"]).expect("parse");
        assert_eq!(cli.simulate, None);
    }
}
