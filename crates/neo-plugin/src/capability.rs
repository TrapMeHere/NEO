//! 能力白名单。运行时放行的是「清单声明 ∩ 用户授权」。

use crate::manifest::ManifestCaps;

/// 一项能力。`Read` 是隐式的，不出现在清单里。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Capability {
    /// 只读快照。
    Read,
    /// 提交编辑命令。
    Command,
    /// 订阅事件。
    Events,
    /// 注册面板。
    Ui,
    /// 出站 HTTPS。
    Network,
    /// 读插件目录里的文件。
    Filesystem,
    /// 登记影像源。
    Imagery,
}

/// 实际放行的能力。
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct CapabilitySet {
    /// 允许 `submit-command`。
    pub command: bool,
    /// 允许注册面板。
    pub ui: bool,
    /// 允许的事件主题。`*` 表示清单里声明的全部主题。
    pub events: Vec<String>,
    /// 允许的 HTTPS 主机名（不含协议）。
    pub network_hosts: Vec<String>,
    /// 允许读插件目录。
    pub filesystem: bool,
    /// 允许影像命令。
    pub imagery: bool,
}

impl CapabilitySet {
    /// 测试和开发者模式用的全开授权。网络主机仍要显式列出。
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

    /// 声明与授权求交。
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

    /// 这个主题是否可以派发。
    pub fn allows_event(&self, topic: &str) -> bool {
        self.events.iter().any(|item| item == "*" || item == topic)
    }

    /// 只允许 HTTPS，并且主机名在白名单里。
    pub fn allows_url(&self, url: &str) -> bool {
        let Some(rest) = url.strip_prefix("https://") else {
            return false;
        };
        let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
        // `https://allowed.com@evil.com/` 的 authority 是 `allowed.com@evil.com`，
        // 旧实现按 ':' 截断后会错误放行。userinfo 一律拒绝。
        if authority.is_empty() || authority.contains('@') {
            return false;
        }
        let host = if let Some(stripped) = authority.strip_prefix('[') {
            // IPv6：[::1]:443
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

/// 把 `https://host/path` 或 `host` 收成主机名。
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
