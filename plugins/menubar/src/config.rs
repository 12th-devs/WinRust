use std::path::PathBuf;

#[derive(Clone)]
pub struct Config {
    pub seconds: bool,
    pub auto_hide: bool,
    pub primary_only: bool,
    pub battery: bool,
    pub wifi: bool,
    pub bluetooth: bool,
    pub acrylic: bool,
}
impl Default for Config {
    fn default() -> Self {
        Self {
            seconds: false,
            auto_hide: false,
            primary_only: false,
            battery: true,
            wifi: true,
            bluetooth: true,
            acrylic: false,
        }
    }
}
impl Config {
    pub fn path() -> PathBuf {
        PathBuf::from(std::env::var_os("LOCALAPPDATA").unwrap_or_default())
            .join("WinRust/menubar.cfg")
    }
    pub fn load() -> Self {
        let mut value = Self::default();
        for line in std::fs::read_to_string(Self::path())
            .unwrap_or_default()
            .lines()
        {
            if let Some((key, v)) = line.split_once('=') {
                match key {
                    "seconds" => value.seconds = v == "1",
                    "auto_hide" => value.auto_hide = v == "1",
                    "primary_only" => value.primary_only = v == "1",
                    "battery" => value.battery = v == "1",
                    "wifi" => value.wifi = v == "1",
                    "bluetooth" => value.bluetooth = v == "1",
                    "material" => value.acrylic = v == "acrylic",
                    _ => {}
                }
            }
        }
        value
    }
    pub fn toggle(&mut self, key: &str) {
        match key {
            "seconds" => self.seconds = !self.seconds,
            "auto_hide" => self.auto_hide = !self.auto_hide,
            "battery" => self.battery = !self.battery,
            "wifi" => self.wifi = !self.wifi,
            "bluetooth" => self.bluetooth = !self.bluetooth,
            "mica" => self.acrylic = false,
            "acrylic" => self.acrylic = true,
            _ => return,
        }
        let path = Self::path();
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(
            path,
            format!(
                "seconds={}\nauto_hide={}\nprimary_only={}\nbattery={}\nwifi={}\nbluetooth={}\nmaterial={}\n",
                self.seconds as u8,
                self.auto_hide as u8,
                self.primary_only as u8,
                self.battery as u8,
                self.wifi as u8,
                self.bluetooth as u8,
                if self.acrylic { "acrylic" } else { "mica" }
            ),
        );
    }
}
