use super::Snapshot;
use serde_json::Value;

pub fn number(value: &Value) -> Option<f64> {
    value
        .as_f64()
        .or_else(|| value.as_str()?.trim().parse().ok())
        .filter(|v| v.is_finite())
}
pub fn text(value: &Value) -> String {
    match value {
        Value::String(s) if !s.is_empty() => s.clone(),
        Value::Number(n) => n.to_string(),
        Value::Bool(b) => if *b {
            crate::i18n::tr("开启")
        } else {
            crate::i18n::tr("关闭")
        }
        .into(),
        _ => "—".into(),
    }
}
pub fn bytes(value: f64) -> String {
    if value >= 1e9 {
        format!("{:.2} GB", value / 1e9)
    } else if value >= 1e6 {
        format!("{:.1} MB", value / 1e6)
    } else {
        format!("{:.1} KB", value / 1e3)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Power {
    Unknown,
    Battery,
    Charging,
    External,
    Bypass,
    Full,
}
impl Power {
    pub fn read(battery: &Value, charger: &Value) -> Self {
        let flag = |value: &Value| match value {
            Value::Bool(v) => Some(*v),
            Value::Number(v) if v.as_u64() == Some(1) => Some(true),
            Value::Number(v) if v.as_u64() == Some(0) => Some(false),
            Value::String(v) => match v.as_str() {
                "1" | "true" | "enable" => Some(true),
                "0" | "false" | "disable" => Some(false),
                _ => None,
            },
            _ => None,
        };
        let connected = flag(&charger["charger_connect"]);
        let bypass = flag(&charger["direct_power_supply_mode"]) == Some(true);
        let status = battery["status"].as_str().unwrap_or("");
        if connected == Some(false) || flag(&charger["otg_powerbank_state"]) == Some(true) {
            return Self::Battery;
        }
        if connected == Some(true) {
            // In bypass mode sysfs can still report Discharging or Charging.
            if bypass {
                return Self::Bypass;
            }
            if status == "Full" {
                return Self::Full;
            }
            return match number(&charger["charge_status"]) {
                Some(1.) => Self::Charging,
                Some(_) => Self::External,
                None if status == "Charging" => Self::Charging,
                None => Self::External,
            };
        }
        // A missing charger sample cannot turn a configured bypass mode into
        // proof of external power. Fall back only to explicit battery states.
        match status {
            "Charging" | "Full" if bypass => Self::Bypass,
            "Charging" => Self::Charging,
            "Full" => Self::Full,
            "Discharging" => Self::Battery,
            _ => Self::Unknown,
        }
    }
    pub fn external(self) -> bool {
        matches!(
            self,
            Self::Charging | Self::External | Self::Bypass | Self::Full
        )
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::Unknown => crate::i18n::tr("电源状态未知"),
            Self::Battery => crate::i18n::tr("电池供电"),
            Self::Charging => crate::i18n::tr("充电中"),
            Self::External => crate::i18n::tr("外接电源"),
            Self::Bypass => crate::i18n::tr("直供电模式"),
            Self::Full => crate::i18n::tr("已充满"),
        }
    }
}
pub struct Dashboard {
    pub battery: String,
    pub battery_level: i32,
    pub power: Power,
    pub network: String,
    pub carrier: String,
    pub network_badge: String,
    /// -1 = unavailable, 0 = no service, 1–4 = measured signal strength.
    pub signal_level: i32,
    pub radio: String,
    pub rsrp: String,
    /// SINR of the serving cell (NR when NR has an RSRP, else LTE).
    pub sinr: String,
    /// 0 = unknown, 1 = weak, 2 = fair, 3 = good.
    pub rsrp_level: i32,
    pub sinr_level: i32,
    /// The worse of the known RSRP and SINR levels, 0 when neither is known.
    pub quality: i32,
    /// Bands of every component carrier in use, primary first ("n41 + n1").
    pub bands: String,
    /// Number of component carriers in use, "—" when unknown.
    pub carriers: String,
    pub online: bool,
    pub download: String,
    pub upload: String,
    pub total: String,
}

fn rsrp_level(dbm: f64) -> i32 {
    if dbm >= -90. {
        3
    } else if dbm >= -105. {
        2
    } else {
        1
    }
}

fn sinr_level(db: f64) -> i32 {
    if db >= 13. {
        3
    } else if db >= 0. {
        2
    } else {
        1
    }
}

/// A band number from firmware text ("n78", "78", "NR5G BAND 78", "B3").
fn band_number(raw: &str) -> Option<u32> {
    const PREFIXES: [&str; 8] = ["NR5G", "LTE", "BAND", "NR", "5G", "4G", "B", "N"];
    let sep = |c: char| c.is_whitespace() || matches!(c, ':' | '_' | '-');
    let upper = raw.to_ascii_uppercase();
    let mut v = upper.trim_matches(sep);
    while let Some(p) = PREFIXES.iter().find(|p| v.starts_with(**p)) {
        v = v[p.len()..].trim_matches(sep);
    }
    if v.is_empty() || !v.bytes().all(|c| c.is_ascii_digit()) {
        return None;
    }
    v.parse().ok().filter(|n| *n > 0)
}

/// Bands of the distinct component carriers (PCI, band, channel) of one RAT:
/// the serving cell first, then the firmware's CA list ("PCI,Band,Index,
/// Channel,BW;…"), skipping placeholder rows and repeats. Same rules as the
/// agent's signal page.
fn component_bands(signal: &Value, nr: bool) -> Vec<String> {
    let (list, pci, band, channel, prefix) = if nr {
        ("nrca", "nr5g_pci", "nr5g_action_band", "nr5g_action_channel", "n")
    } else {
        ("lteca", "lte_pci", "wan_active_band", "wan_active_channel", "B")
    };
    let max_pci = if nr { 1007 } else { 503 };
    let key = |p: &str, b: &str, c: &str| {
        let p = p.trim().parse::<u32>().ok().filter(|p| *p <= max_pci)?;
        Some((p, band_number(b)?, c.trim().parse::<u32>().ok()?))
    };
    let mut seen = Vec::new();
    let primary = key(
        text(&signal[pci]).as_str(),
        text(&signal[band]).as_str(),
        text(&signal[channel]).as_str(),
    );
    seen.extend(primary);
    if nr || text(&signal["lteca_state"]) != "0" {
        for entry in text(&signal[list]).split(';') {
            let p: Vec<&str> = entry.split(',').map(str::trim).collect();
            if p.len() < 5 || (p[0] == "0" && p[3] == "0") {
                continue;
            }
            if let Some(k) = key(p[0], p[1], p[3]).filter(|k| !seen.contains(k)) {
                seen.push(k);
            }
        }
    }
    if seen.is_empty() {
        return band_number(&text(&signal[band]))
            .map(|b| vec![format!("{prefix}{b}")])
            .unwrap_or_default();
    }
    seen.into_iter().map(|(_, b, _)| format!("{prefix}{b}")).collect()
}
impl Dashboard {
    pub fn read(state: &Snapshot) -> Self {
        let null = Value::Null;
        let radio = state
            .get("operating")
            .and_then(|v| v["airplane"]["enabled"].as_bool());
        let unavailable = state.values.contains_key("operating") && radio != Some(false);
        let signal = if unavailable {
            &null
        } else {
            state.get("signal").unwrap_or(&null)
        };
        let traffic = state.get("traffic").unwrap_or(&null);
        let battery = state.get("battery").unwrap_or(&null);
        let charger = state.get("charger").unwrap_or(&null);
        let rsrp = |key: &str| number(&signal[key]).filter(|n| (-160.0..=-20.0).contains(n));
        let nr = rsrp("nr5g_rsrp");
        let lte = rsrp("lte_rsrp");
        let mode = signal["network_type"].as_str().unwrap_or("");
        let no_service = matches!(
            mode,
            "NO_SERVICE" | "OFFLINE" | "DISCONNECTED" | "SEARCH" | "SEARCHING"
        );
        let measured = if no_service { None } else { nr.or(lte) };
        let signal_level = if no_service {
            0
        } else {
            measured.map_or(-1, |v| {
                if v >= -80. {
                    4
                } else if v >= -95. {
                    3
                } else if v >= -110. {
                    2
                } else {
                    1
                }
            })
        };
        let network = match mode {
            "SA" | "NR5G-SA" | "5G_SA" => "5G SA",
            "ENDC" | "LTE-NSA" | "NSA" => "5G NSA",
            "LTE" => "4G",
            "NO_SERVICE" => crate::i18n::tr("无服务"),
            "LIMITED_SERVICE_LTE" | "LIMITED_SERVICE_SA" => crate::i18n::tr("有限服务"),
            "" => "—",
            other => other,
        }
        .to_string();
        let network_badge = if no_service {
            ""
        } else if mode.contains("5G") || mode.contains("SA") || matches!(mode, "ENDC" | "LTE-NSA") {
            "5G"
        } else if mode.contains("LTE") || mode == "4G" {
            "4G"
        } else if mode.contains("WCDMA") || mode.contains("HSPA") || mode == "UMTS" {
            "3G"
        } else if matches!(mode, "GSM" | "GPRS" | "EDGE") {
            "2G"
        } else {
            "—"
        };
        let band = if nr.is_some() {
            text(&signal["nr5g_action_band"])
        } else {
            text(&signal["wan_active_band"])
        };
        let radio = if band == "—" {
            network.clone()
        } else {
            format!("{network} · {band}")
        };
        let rate = |key: &str| {
            if unavailable {
                return "—".to_string();
            }
            number(&traffic[key])
                .filter(|v| *v >= 0.)
                .map(|v| format!("{:.1}", v * 8. / 1e6))
                .unwrap_or("—".into())
        };
        let sinr = measured.and_then(|_| {
            number(&signal[if nr.is_some() { "nr5g_snr" } else { "lte_snr" }])
                .filter(|v| (-30.0..=60.0).contains(v))
        });
        let rsrp_lv = measured.map_or(0, rsrp_level);
        let sinr_lv = sinr.map_or(0, sinr_level);
        let mut bands = Vec::new();
        if measured.is_some() {
            // EN-DC: the LTE anchor's carriers, then NR's. SA: NR only.
            if lte.is_some() && network != "5G SA" {
                bands.extend(component_bands(signal, false));
            }
            if nr.is_some() {
                bands.extend(component_bands(signal, true));
            }
        }
        let capacity = number(&battery["capacity"]).filter(|v| (0.0..=100.0).contains(v));
        Self {
            sinr: sinr.map(|v| format!("{v:.0}")).unwrap_or("—".into()),
            rsrp_level: rsrp_lv,
            sinr_level: sinr_lv,
            quality: [rsrp_lv, sinr_lv].into_iter().filter(|l| *l > 0).min().unwrap_or(0),
            carriers: if bands.is_empty() { "—".into() } else { bands.len().to_string() },
            bands: bands.join(" + "),
            battery: capacity.map(|v| format!("{v:.0}%")).unwrap_or("—".into()),
            battery_level: capacity.map_or(-1, |v| v.round() as i32),
            power: Power::read(battery, charger),
            network_badge: network_badge.into(),
            carrier: if unavailable
                && state
                    .get("operating")
                    .is_some_and(|v| v["airplane"]["enabled"] == true)
            {
                crate::i18n::tr("飞行模式").into()
            } else if no_service {
                crate::i18n::tr("无服务").into()
            } else {
                signal["network_provider"]
                    .as_str()
                    .map(str::trim)
                    .filter(|name| !name.is_empty() && !matches!(*name, "—" | "--"))
                    .map(crate::carrier::label)
                    .unwrap_or(crate::i18n::tr("运营商未提供"))
                    .into()
            },
            signal_level,
            online: measured.is_some(),
            rsrp: measured.map(|v| format!("{v:.0}")).unwrap_or("—".into()),
            network: if unavailable {
                if state
                    .get("operating")
                    .is_some_and(|v| v["airplane"]["enabled"] == true)
                {
                    crate::i18n::tr("飞行模式")
                } else {
                    crate::i18n::tr("状态未知")
                }
                .into()
            } else {
                network
            },
            radio,
            download: rate("real_rx_speed"),
            upload: rate("real_tx_speed"),
            total: number(&traffic["real_rx_bytes"])
                .zip(number(&traffic["real_tx_bytes"]))
                .map(|(rx, tx)| bytes(rx + tx))
                .unwrap_or("—".into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::{Sample, Status};
    use serde_json::json;
    #[test]
    fn flight_and_unknown_radio_hide_stale_signal_and_speed() {
        for mode in [json!(true), Value::Null] {
            let state = Snapshot::live_fixture(&[
                (
                    "signal",
                    json!({"network_type":"SA","nr5g_rsrp":-72,"network_provider":"UNICOM"}),
                ),
                (
                    "traffic",
                    json!({"real_rx_speed":1_000_000,"real_tx_speed":100_000}),
                ),
                ("operating", json!({"airplane":{"enabled":mode}})),
            ]);
            let d = Dashboard::read(&state);
            assert!(!d.online);
            assert_eq!(d.signal_level, -1);
            assert_ne!(d.network_badge, "5G");
            assert_eq!(d.download, "—");
        }
    }
    #[test]
    fn power_distinguishes_charging_bypass_full_unplugged_and_unknown() {
        for (status, charger, expected) in [
            ("Charging", json!({}), Power::Charging),
            (
                "Full",
                json!({"charger_connect":1,"charge_status":1}),
                Power::Full,
            ),
            (
                "Charging",
                json!({"charger_connect":0,"charge_status":1}),
                Power::Battery,
            ),
            (
                "Discharging",
                json!({"charger_connect":"1","direct_power_supply_mode":"enable","charge_status":1}),
                Power::Bypass,
            ),
            (
                "Charging",
                json!({"charger_connect":1,"direct_power_supply_mode":"enable"}),
                Power::Bypass,
            ),
            (
                "Discharging",
                json!({"charger_connect":1,"direct_power_supply_mode":"disable","charge_status":"1"}),
                Power::Charging,
            ),
            (
                "Not charging",
                json!({"charger_connect":true,"charge_status":0}),
                Power::External,
            ),
            (
                "Charging",
                json!({"charger_connect":1,"otg_powerbank_state":1}),
                Power::Battery,
            ),
            (
                "Discharging",
                json!({"direct_power_supply_mode":"enable"}),
                Power::Battery,
            ),
            (
                "Unknown",
                json!({"direct_power_supply_mode":"enable"}),
                Power::Unknown,
            ),
            ("", Value::Null, Power::Unknown),
        ] {
            assert_eq!(
                Power::read(&json!({"status":status}), &charger),
                expected,
                "{status}: {charger}"
            );
        }
    }
    #[test]
    fn signal_bars_follow_measurements_and_disconnected_samples_clear_indicators() {
        let mut state = Snapshot {
            status: Status::Live,
            packet_at: Some(std::time::Instant::now()),
            ..Default::default()
        };
        for (dbm, level) in [
            (-80, 4),
            (-81, 3),
            (-95, 3),
            (-96, 2),
            (-110, 2),
            (-111, 1),
            (0, -1),
            (9999, -1),
        ] {
            state.values.insert(
                "signal".into(),
                Sample {
                    data: json!({"network_type":"SA","nr5g_rsrp":dbm}),
                    age_ms: 0,
                    seq: 1,
                    failed: false,
                },
            );
            let model = Dashboard::read(&state);
            assert_eq!(model.signal_level, level);
            assert_eq!(model.network_badge, "5G");
        }
        state.values.get_mut("signal").unwrap().data =
            json!({"network_type":"NO_SERVICE","nr5g_rsrp":-65});
        let model = Dashboard::read(&state);
        assert_eq!(
            model.signal_level, 0,
            "no service overrides retained measurements"
        );
        assert!(!model.online);
        assert_eq!(model.rsrp, "—");
        for (topic, data) in [
            ("battery", json!({"capacity":0,"status":"Discharging"})),
            (
                "charger",
                json!({"charger_connect":1,"direct_power_supply_mode":"enable"}),
            ),
        ] {
            state.values.insert(
                topic.into(),
                Sample {
                    data,
                    age_ms: 0,
                    seq: 1,
                    failed: false,
                },
            );
        }
        assert_eq!(Dashboard::read(&state).battery_level, 0);
        assert_eq!(Dashboard::read(&state).power, Power::Bypass);
        state.values.get_mut("charger").unwrap().age_ms = 10_001;
        assert_eq!(
            Dashboard::read(&state).power,
            Power::Battery,
            "stale charger data cannot claim external power"
        );
        state.status = Status::Reconnecting;
        let model = Dashboard::read(&state);
        assert_eq!(model.signal_level, -1);
        assert_eq!(model.battery_level, -1);
        assert_eq!(model.power, Power::Unknown);
    }
    #[test]
    fn signal_card_counts_carriers_and_grades_the_worse_reading() {
        let mut state = Snapshot {
            status: Status::Live,
            packet_at: Some(std::time::Instant::now()),
            ..Default::default()
        };
        let mut read = |data: Value| {
            state.values.insert(
                "signal".into(),
                Sample { data, age_ms: 0, seq: 1, failed: false },
            );
            Dashboard::read(&state)
        };
        // SA with one NR secondary; the primary repeated in the list and a
        // placeholder row do not count.
        let m = read(json!({"network_type":"SA","nr5g_rsrp":-86,"nr5g_snr":"-23",
            "nr5g_pci":"512","nr5g_action_band":"n41","nr5g_action_channel":"513000",
            "nrca":"512,41,0,513000,100;300,1,1,428000,20;0,0,0,0,0;",
            "lte_rsrp":-90,"lteca":"1,3,0,1650,20"}));
        assert_eq!((m.bands.as_str(), m.carriers.as_str()), ("n41 + n1", "2"));
        assert_eq!((m.sinr.as_str(), m.rsrp_level, m.sinr_level, m.quality), ("-23", 3, 1, 1));
        // EN-DC: LTE anchor (CA off) then NR.
        let m = read(json!({"network_type":"ENDC","lte_rsrp":-100,"lte_snr":5,
            "lte_pci":"10","wan_active_band":"B3","wan_active_channel":"1650",
            "lteca_state":"0","lteca":"10,3,0,1650,20;11,1,1,300,10",
            "nr5g_rsrp":-95,"nr5g_snr":15,"nr5g_action_band":"78"}));
        assert_eq!((m.bands.as_str(), m.carriers.as_str()), ("B3 + n78", "2"));
        assert_eq!((m.rsrp_level, m.sinr_level, m.quality), (2, 3, 2));
        // No service hides everything.
        let m = read(json!({"network_type":"NO_SERVICE","nr5g_rsrp":-80,"nr5g_snr":20}));
        assert_eq!((m.bands.as_str(), m.carriers.as_str(), m.sinr.as_str(), m.quality), ("", "—", "—", 0));
    }
    #[test]
    fn live_values_handle_units_unknowns_failed_reads_and_stale_snapshots() {
        let mut state = Snapshot {
            status: Status::Live,
            packet_at: Some(std::time::Instant::now()),
            ..Default::default()
        };
        for (topic, data) in [
            ("battery", json!({"capacity":"0"})),
            (
                "signal",
                json!({"network_type":"LTE-NSA", "network_provider":"  Example Mobile  ", "nr5g_rsrp":"-87", "nr5g_action_band":"n78"}),
            ),
            (
                "traffic",
                json!({"real_rx_speed":"1250000", "real_tx_speed":0, "real_rx_bytes":1000000, "real_tx_bytes":2000000}),
            ),
        ] {
            state.values.insert(
                topic.into(),
                Sample {
                    data,
                    age_ms: 0,
                    seq: 1,
                    failed: false,
                },
            );
        }
        let model = Dashboard::read(&state);
        assert_eq!(
            (
                model.battery.as_str(),
                model.download.as_str(),
                model.upload.as_str()
            ),
            ("0%", "10.0", "0.0")
        );
        assert_eq!(model.radio, "5G NSA · n78");
        assert_eq!(model.carrier, "Example Mobile");
        state.values.get_mut("signal").unwrap().data["network_type"] = json!("NO_SERVICE");
        assert_eq!(Dashboard::read(&state).carrier, "无服务");
        state.values.get_mut("signal").unwrap().data["network_type"] = json!("LTE-NSA");
        state.values.get_mut("signal").unwrap().data["nr5g_rsrp"] = json!(9999);
        assert_eq!(Dashboard::read(&state).rsrp, "—");
        state.values.get_mut("battery").unwrap().failed = true;
        assert_eq!(Dashboard::read(&state).battery, "—");
        state.values.get_mut("traffic").unwrap().age_ms = 10_000;
        assert_eq!(Dashboard::read(&state).download, "—");
        state.status = Status::Reconnecting;
        assert_eq!(Dashboard::read(&state).network, "—");
        assert_eq!(Dashboard::read(&state).carrier, "运营商未提供");
    }
}
