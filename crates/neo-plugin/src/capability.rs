//! Capability allowlist. What is allowed at runtime is "manifest declaration ∩ user grant".

use crate::manifest::ManifestCaps;

/// A capability. `Read` is implicit and does not appear in the manifest.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Capability {
    /// Read-only snapshot.
    Read,
    /// Submits edit commands.
    Command,
    /// Subscribes to events.
    Events,
    /// Registers panels.
    Ui,
    /// Outbound HTTPS.
    Network,
    /// Reads files in the plugin directory.
    Filesystem,
    /// Registers imagery sources.
    Imagery,
}

/// Capabilities actually allowed.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct CapabilitySet {
    /// Allows `submit-command`.
    pub command: bool,
    /// Allows registering panels.
    pub ui: bool,
    /// Allowed event topics. `*` means all topics declared in the manifest.
    pub events: Vec<String>,
    /// Allowed HTTPS host names (without scheme).
    pub network_hosts: Vec<String>,
    /// Allows reading the plugin directory.
    pub filesystem: bool,
    /// Allows imagery commands.
    pub imagery: bool,
}

impl CapabilitySet {
    /// Fully-open grant for tests and developer mode. Network hosts still must be listed explicitly.
    pub fn grant_all(network_hosts: &[&str]) -> Self {
        Self {
            command: true,
            ui: true,
            events: vec!["*".to_owned()],
            network_hosts: network_hosts.iter().map(|h| normalize_host(h)).collect(),
            filesystem: true,
            imagery: true,
        }
    }

    /// Intersects the declaration with the grant.
    pub fn intersect(&self, declared: &ManifestCaps) -> Self {
        let events = if self.events.iter().any(|topic| topic == "*") {
            declared.events.clone()
        } else {
            declared
                .events
                .iter()
                .filter(|topic| self.events.iter().any(|granted| granted == *topic))
                .cloned()
                .collect()
        };
        let network_hosts = declared
            .network
            .iter()
            .map(|entry| normalize_host(entry))
            .filter(|host| {
                self.network_hosts
                    .iter()
                    .any(|granted| granted == host || granted == "*")
            })
            .collect();
        Self {
            command: self.command && declared.command,
            ui: self.ui && declared.ui,
            events,
            network_hosts,
            filesystem: self.filesystem && declared.filesystem,
            imagery: self.imagery && declared.imagery,
        }
    }

    /// Whether this topic can be dispatched.
    pub fn allows_event(&self, topic: &str) -> bool {
        self.events.iter().any(|item| item == "*" || item == topic)
    }

    /// Only HTTPS is allowed, and the host name must be on the allowlist.
    pub fn allows_url(&self, url: &str) -> bool {
        let Some(rest) = url.strip_prefix("https://") else {
            return false;
        };
        let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
        // The authority of `https://allowed.com@evil.com/` is `allowed.com@evil.com`;
        // the old implementation truncated at ':' and wrongly allowed it. userinfo is always rejected.
        if authority.is_empty() || authority.contains('@') {
            return false;
        }
        let host = if let Some(stripped) = authority.strip_prefix('[') {
            // IPv6: [::1]:443
            stripped.split(']').next().unwrap_or("")
        } else {
            authority.split(':').next().unwrap_or(authority)
        };
        !host.is_empty()
            && self
                .network_hosts
                .iter()
                .any(|item| item.eq_ignore_ascii_case(host))
    }
}

/// Reduces `https://host/path` or `host` to a host name.
pub fn normalize_host(entry: &str) -> String {
    let trimmed = entry.trim().trim_matches('"');
    let trimmed = trimmed.strip_prefix("https://").unwrap_or(trimmed);
    let trimmed = trimmed.strip_prefix("http://").unwrap_or(trimmed);
    trimmed
        .split(['/', '?', '#', ':'])
        .next()
        .unwrap_or(trimmed)
        .to_owned()
}
