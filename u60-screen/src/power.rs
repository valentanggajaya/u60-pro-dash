//! Shut down / restart from the long-press power menu.
//!
//! Done through ZTE's master control the way the vendor UI's own menu does it
//! (seen in `/logfs/key.log` on MU5250 B28): `zwrt_mc.device.manager
//! device_poweroff` switches the firmware to `mode_power_off`, every module
//! that subscribed with `poweroff_subscribe` shuts its part down and answers
//! `poweroff_compelte` (sic), and only then is the device switched off. The
//! vendor UI (`zte_topsw_devui`) subscribes at every start and stays
//! registered while this screen replaces it, so the screen answers in its
//! name; otherwise the firmware would keep waiting for it. With fast boot
//! (`quicken_power_on=1`) the firmware asks for `poweroff_quicken_compelte`.
//! Restart is `device_reboot`, which has no vendor-UI subscriber (verified:
//! the router rebooted and came back to this screen on its own).

#[derive(Clone, Copy, Debug)]
pub enum Action {
    Shutdown,
    Restart,
}

#[cfg(feature = "device")]
pub fn request(action: Action) {
    use std::{
        process::{Command, ExitStatus, Stdio},
        thread,
        time::Duration,
    };
    const MANAGER: &str = "zwrt_mc.device.manager";
    const VENDOR_UI: &str = r#"{"moduleName":"zte_topsw_devui"}"#;
    fn ubus(method: &str) -> std::io::Result<ExitStatus> {
        Command::new("/bin/ubus")
            .args(["call", MANAGER, method, VENDOR_UI])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
    }
    // Never block the UI thread on the firmware.
    thread::spawn(move || match action {
        Action::Restart => {
            eprintln!("power: restart requested");
            let status = ubus("device_reboot");
            eprintln!("power: device_reboot -> {status:?}");
        }
        Action::Shutdown => {
            eprintln!("power: shutdown requested");
            // The firmware may keep this call open until every subscriber is
            // done, including the answer below, so it runs on its own thread.
            thread::spawn(|| {
                let status = ubus("device_poweroff");
                eprintln!("power: device_poweroff -> {status:?}");
            });
            thread::sleep(Duration::from_secs(2));
            let quicken = Command::new("/sbin/uci")
                .args(["-q", "get", "zwrt_zte_mc.power.quicken_power_on"])
                .stdin(Stdio::null())
                .output()
                .is_ok_and(|out| String::from_utf8_lossy(&out.stdout).trim() == "1");
            let method = if quicken {
                "poweroff_quicken_compelte"
            } else {
                "poweroff_compelte"
            };
            let status = ubus(method);
            eprintln!("power: {method} for the vendor UI -> {status:?}");
        }
    });
}

#[cfg(not(feature = "device"))]
pub fn request(action: Action) {
    eprintln!("power: {action:?} is only carried out on the device");
}
