//! BlueZ (§13.2, §13.3): battery via HFP, "connect as audio", discovery.

#[derive(Clone, Debug, Default, PartialEq)]
pub struct BatteryInfo {
    /// `org.bluez.Battery1.Percentage`, if present
    pub percent: Option<u8>,
    /// `MediaControl1.Connected` or a `MediaTransport1` object under the device
    pub audio_connected: bool,
    /// the device with this MAC is known to BlueZ
    pub found: bool,
}

pub async fn poll(mac: String) -> BatteryInfo {
    #[cfg(target_os = "linux")]
    {
        match linux::system().await {
            Ok(conn) => linux::managed_objects(&conn).await.map(|o| battery_from(&o, &mac)).unwrap_or_default(),
            Err(_) => BatteryInfo::default(),
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = mac;
        BatteryInfo::default()
    }
}

/// `Device1.Connect()` with a 30 s timeout.
pub async fn connect_audio(mac: String) -> Result<(), String> {
    #[cfg(target_os = "linux")]
    {
        linux::connect_audio(&mac).await
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = mac;
        Err(tr!("bluez.unsupported").into())
    }
}

/// Classic discovery for `secs` seconds: `(name, MAC)`, devices with "Divoom" in the name first.
pub async fn discover(secs: u64) -> Result<Vec<(String, String)>, String> {
    #[cfg(target_os = "linux")]
    {
        linux::discover(secs).await
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = secs;
        Err(tr!("bluez.unsupported").into())
    }
}

#[cfg(target_os = "linux")]
use linux::Objects;

#[cfg(target_os = "linux")]
fn str_prop<'a>(props: &'a std::collections::HashMap<String, zbus::zvariant::OwnedValue>, key: &str) -> Option<&'a str> {
    props.get(key).and_then(|v| v.downcast_ref::<&str>().ok())
}

#[cfg(target_os = "linux")]
fn device_path<'a>(objects: &'a Objects, mac: &str) -> Option<&'a str> {
    objects.iter().find_map(|(path, ifaces)| {
        let addr = str_prop(ifaces.get("org.bluez.Device1")?, "Address")?;
        addr.eq_ignore_ascii_case(mac.trim()).then_some(path.as_str())
    })
}

#[cfg(target_os = "linux")]
fn battery_from(objects: &Objects, mac: &str) -> BatteryInfo {
    let Some(dev) = device_path(objects, mac) else { return BatteryInfo::default() };
    let percent = objects
        .iter()
        .find(|(p, _)| p.as_str() == dev)
        .and_then(|(_, ifaces)| ifaces.get("org.bluez.Battery1")?.get("Percentage")?.downcast_ref::<u8>().ok());
    let prefix = format!("{dev}/");
    let audio_connected = objects.iter().any(|(path, ifaces)| {
        let p = path.as_str();
        (p == dev || p.starts_with(&prefix))
            && (ifaces.contains_key("org.bluez.MediaTransport1")
                || ifaces
                    .get("org.bluez.MediaControl1")
                    .and_then(|mc| mc.get("Connected")?.downcast_ref::<bool>().ok())
                    .unwrap_or(false))
    });
    BatteryInfo { percent, audio_connected, found: true }
}

/// Devices of `adapter` seen by the current discovery (they have `RSSI`) or connected now,
/// "Divoom" first, then by signal strength.
#[cfg(target_os = "linux")]
fn discovered(objects: &Objects, adapter: &str) -> Vec<(String, String)> {
    let prefix = format!("{adapter}/");
    let mut found: Vec<(bool, i16, String, String)> = objects
        .iter()
        .filter(|(path, _)| path.as_str().starts_with(&prefix))
        .filter_map(|(_, ifaces)| {
            let dev = ifaces.get("org.bluez.Device1")?;
            let addr = str_prop(dev, "Address")?.to_uppercase();
            let rssi = dev.get("RSSI").and_then(|v| v.downcast_ref::<i16>().ok());
            let connected = dev.get("Connected").and_then(|v| v.downcast_ref::<bool>().ok()).unwrap_or(false);
            if rssi.is_none() && !connected {
                return None;
            }
            let name = str_prop(dev, "Name").or_else(|| str_prop(dev, "Alias")).unwrap_or(&addr).to_string();
            let divoom = name.to_lowercase().contains("divoom");
            Some((divoom, rssi.unwrap_or(0), name, addr))
        })
        .collect();
    found.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.cmp(&a.1)).then_with(|| a.2.cmp(&b.2)));
    found.dedup_by(|a, b| a.3 == b.3);
    found.into_iter().map(|(_, _, name, addr)| (name, addr)).collect()
}

#[cfg(target_os = "linux")]
mod linux {
    use std::collections::HashMap;
    use std::time::Duration;
    use zbus::Connection;
    use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};

    pub type Objects = HashMap<OwnedObjectPath, HashMap<String, HashMap<String, OwnedValue>>>;

    const BLUEZ: &str = "org.bluez";
    const CALL_TIMEOUT: Duration = Duration::from_secs(10);

    pub async fn system() -> Result<Connection, String> {
        tokio::time::timeout(CALL_TIMEOUT, Connection::system())
            .await
            .map_err(|_| tr!("bluez.bus_timeout").to_string())?
            .map_err(|e| tr!("bluez.bus_unavailable", error = e))
    }

    fn describe(e: zbus::Error) -> String {
        match e {
            zbus::Error::MethodError(name, msg, _) => {
                let name = name.as_str();
                if name == "org.freedesktop.DBus.Error.ServiceUnknown" || name == "org.freedesktop.DBus.Error.NameHasNoOwner" {
                    return tr!("bluez.not_running").into();
                }
                let short = name.rsplit('.').next().unwrap_or(name);
                match msg {
                    Some(m) if !m.is_empty() => format!("{short}: {m}"),
                    _ => short.to_string(),
                }
            }
            e => e.to_string(),
        }
    }

    async fn call<B>(conn: &Connection, path: &str, iface: &str, member: &str, body: &B, timeout: Duration) -> Result<zbus::Message, String>
    where
        B: serde::Serialize + zbus::zvariant::DynamicType,
    {
        tokio::time::timeout(timeout, conn.call_method(Some(BLUEZ), path, Some(iface), member, body))
            .await
            .map_err(|_| tr!("bluez.call_timeout", call = member, secs = timeout.as_secs()))?
            .map_err(describe)
    }

    pub async fn managed_objects(conn: &Connection) -> Result<Objects, String> {
        let reply = call(conn, "/", "org.freedesktop.DBus.ObjectManager", "GetManagedObjects", &(), CALL_TIMEOUT).await?;
        reply.body().deserialize::<Objects>().map_err(|e| tr!("bluez.bad_reply", error = e))
    }

    pub async fn connect_audio(mac: &str) -> Result<(), String> {
        let conn = system().await?;
        let objects = managed_objects(&conn).await?;
        let path = super::device_path(&objects, mac)
            .ok_or_else(|| tr!("bluez.device_not_found", mac = mac))?
            .to_string();
        call(&conn, &path, "org.bluez.Device1", "Connect", &(), Duration::from_secs(30))
            .await
            .map(drop)
            .map_err(|e| tr!("bluez.connect_failed", error = e))
    }

    fn adapter(objects: &Objects) -> Result<String, String> {
        let mut adapters: Vec<(&str, bool)> = objects
            .iter()
            .filter_map(|(path, ifaces)| {
                let a = ifaces.get("org.bluez.Adapter1")?;
                let powered = a.get("Powered").and_then(|v| v.downcast_ref::<bool>().ok()).unwrap_or(false);
                Some((path.as_str(), powered))
            })
            .collect();
        adapters.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
        match adapters.first() {
            None => Err(tr!("bluez.no_adapter").into()),
            Some((_, false)) => Err(tr!("bluez.powered_off").into()),
            Some((path, true)) => Ok(path.to_string()),
        }
    }

    pub async fn discover(secs: u64) -> Result<Vec<(String, String)>, String> {
        let conn = system().await?;
        let adapter = adapter(&managed_objects(&conn).await?)?;
        const IFACE: &str = "org.bluez.Adapter1";
        let filter = HashMap::from([("Transport", Value::from("bredr"))]);
        if let Err(e) = call(&conn, &adapter, IFACE, "SetDiscoveryFilter", &(filter,), CALL_TIMEOUT).await {
            log::debug!("SetDiscoveryFilter: {e}");
        }
        call(&conn, &adapter, IFACE, "StartDiscovery", &(), CALL_TIMEOUT)
            .await
            .map_err(|e| tr!("bluez.discovery_failed", error = e))?;
        tokio::time::sleep(Duration::from_secs(secs)).await;
        let found = managed_objects(&conn).await.map(|o| super::discovered(&o, &adapter));
        let stopped = call(&conn, &adapter, IFACE, "StopDiscovery", &(), CALL_TIMEOUT).await;
        if let Err(e) = stopped {
            log::debug!("StopDiscovery: {e}");
        }
        found
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};

    fn v<'a>(x: impl Into<Value<'a>>) -> OwnedValue {
        OwnedValue::try_from(x.into()).unwrap()
    }

    fn obj(objects: &mut Objects, path: &str, iface: &str, props: Vec<(&str, OwnedValue)>) {
        objects
            .entry(OwnedObjectPath::try_from(path).unwrap())
            .or_default()
            .insert(iface.into(), props.into_iter().map(|(k, v)| (k.to_string(), v)).collect::<HashMap<_, _>>());
    }

    const DEV: &str = "/org/bluez/hci0/dev_B1_21_81_05_E2_65";

    fn base() -> Objects {
        let mut o = Objects::new();
        obj(&mut o, "/org/bluez/hci0", "org.bluez.Adapter1", vec![("Powered", v(true))]);
        obj(&mut o, DEV, "org.bluez.Device1", vec![("Address", v("B1:21:81:05:E2:65")), ("Name", v("Divoom MiniToo-Audio")), ("Connected", v(true))]);
        obj(&mut o, "/org/bluez/hci0/dev_AA_BB_CC_DD_EE_FF", "org.bluez.Device1", vec![("Address", v("AA:BB:CC:DD:EE:FF")), ("Alias", v("Phone")), ("RSSI", v(-40i16))]);
        obj(&mut o, "/org/bluez/hci0/dev_11_22_33_44_55_66", "org.bluez.Device1", vec![("Address", v("11:22:33:44:55:66")), ("Name", v("Far away"))]);
        o
    }

    #[test]
    fn battery_and_audio() {
        let mut o = base();
        assert_eq!(battery_from(&o, "b1:21:81:05:e2:65"), BatteryInfo { percent: None, audio_connected: false, found: true });
        assert_eq!(battery_from(&o, "00:00:00:00:00:00"), BatteryInfo::default());

        obj(&mut o, DEV, "org.bluez.Battery1", vec![("Percentage", v(70u8))]);
        obj(&mut o, &format!("{DEV}/player0"), "org.bluez.MediaControl1", vec![("Connected", v(false))]);
        assert_eq!(battery_from(&o, "B1:21:81:05:E2:65"), BatteryInfo { percent: Some(70), audio_connected: false, found: true });

        obj(&mut o, &format!("{DEV}/sep1/fd0"), "org.bluez.MediaTransport1", vec![("State", v("idle"))]);
        assert!(battery_from(&o, "B1:21:81:05:E2:65").audio_connected);

        // a transport of another device does not count
        let mut o = base();
        obj(&mut o, "/org/bluez/hci0/dev_AA_BB_CC_DD_EE_FF/sep1/fd0", "org.bluez.MediaTransport1", vec![]);
        obj(&mut o, &format!("{DEV}_X/player0"), "org.bluez.MediaControl1", vec![("Connected", v(true))]);
        assert!(!battery_from(&o, "B1:21:81:05:E2:65").audio_connected);
        obj(&mut o, &format!("{DEV}/player0"), "org.bluez.MediaControl1", vec![("Connected", v(true))]);
        assert!(battery_from(&o, "B1:21:81:05:E2:65").audio_connected);
    }

    #[test]
    fn discovery_order() {
        let found = discovered(&base(), "/org/bluez/hci0");
        assert_eq!(
            found,
            vec![
                ("Divoom MiniToo-Audio".to_string(), "B1:21:81:05:E2:65".to_string()),
                ("Phone".to_string(), "AA:BB:CC:DD:EE:FF".to_string()),
            ]
        );
        assert!(discovered(&base(), "/org/bluez/hci1").is_empty());
    }
}
