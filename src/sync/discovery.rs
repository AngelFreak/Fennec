//! Lets phones find Fennec on the network (mDNS, `_fennec._tcp`) when its
//! address has changed since pairing.

use mdns_sd::{ServiceDaemon, ServiceInfo};

const SERVICE: &str = "_fennec._tcp.local.";

pub struct Discovery {
    daemon: ServiceDaemon,
    fullname: String,
}

impl Discovery {
    /// Announces Fennec until dropped. `id` tells phones which Fennec it is.
    pub fn advertise(name: &str, port: u16, id: &str) -> Result<Self, String> {
        let daemon = ServiceDaemon::new().map_err(|e| e.to_string())?;
        let host: String = name
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
            .collect();
        let props = [("v", "1"), ("id", id), ("name", name)];
        let info = ServiceInfo::new(SERVICE, name, &format!("{host}.local."), "", port, &props[..])
            .map_err(|e| e.to_string())?
            .enable_addr_auto();
        let fullname = info.get_fullname().to_string();
        daemon.register(info).map_err(|e| e.to_string())?;
        Ok(Self { daemon, fullname })
    }
}

impl Drop for Discovery {
    fn drop(&mut self) {
        let _ = self.daemon.unregister(&self.fullname);
        let _ = self.daemon.shutdown();
    }
}
