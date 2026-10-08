//! BlueZ (§13.2, §13.3): battery via HFP, "connect as audio", discovery.
//! PLACEHOLDER — replaced by the real implementation.

#[derive(Clone, Debug, Default, PartialEq)]
pub struct BatteryInfo {
    /// `org.bluez.Battery1.Percentage`, if present
    pub percent: Option<u8>,
    /// `MediaControl1.Connected` or a `MediaTransport1` object under the device
    pub audio_connected: bool,
    /// the device with this MAC is known to BlueZ
    pub found: bool,
}

pub async fn poll(_mac: String) -> BatteryInfo {
    BatteryInfo::default()
}

/// `Device1.Connect()` with a 30 s timeout.
pub async fn connect_audio(_mac: String) -> Result<(), String> {
    Err("недоступно на этой платформе".into())
}

/// Classic discovery for `secs` seconds: `(name, MAC)`, devices with "Divoom" in the name first.
pub async fn discover(_secs: u64) -> Result<Vec<(String, String)>, String> {
    Err("недоступно на этой платформе".into())
}
