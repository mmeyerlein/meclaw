//! `params` of the `vault` cell — everything that is configuration, and
//! nothing that is key material.
//!
//! The selector is called `key_source` on purpose: it names *where* the key
//! comes from, never *what* it is. A key on a command line lands in `ps` output
//! and in shell history, so the switch must not be able to carry one.

use meclaw_core::JsonValue;

/// Where the master key comes from. Same layering ssh uses: the vault is one
/// mechanism, the source of its key is a deployment property.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeySource {
    /// Decide at unlock time: a credentials directory (systemd) wins, else the
    /// user channel prompts. The default, and the one that is almost never
    /// typed out — like ssh picking the agent when `SSH_AUTH_SOCK` is set.
    Auto,
    /// The passphrase arrives over the user channel (`meclaw --vault-unlock`).
    Prompt,
    /// `$CREDENTIALS_DIRECTORY/<credential_name>`, the systemd contract. With
    /// `LoadCredentialEncrypted=` (TPM2-bound where the machine has one) the
    /// key never lies readable on disk.
    SystemdCred,
    /// A key file, host-key style: protected by file permissions alone.
    /// Explicit opt-in — this is the honest unattended-boot answer, not a
    /// recommendation.
    PlainFile,
}

impl KeySource {
    /// Parse the `key_source` param value.
    pub fn parse(s: &str) -> Result<Self, String> {
        match s {
            "auto" => Ok(Self::Auto),
            "prompt" => Ok(Self::Prompt),
            "systemd-cred" => Ok(Self::SystemdCred),
            "plainfile" => Ok(Self::PlainFile),
            other => Err(format!(
                "vault: unknown key_source {other:?} (auto|prompt|systemd-cred|plainfile)"
            )),
        }
    }

    /// The name as it appears in params and on the wire.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Prompt => "prompt",
            Self::SystemdCred => "systemd-cred",
            Self::PlainFile => "plainfile",
        }
    }
}

/// Parsed `params` of a vault cell.
#[derive(Debug, Clone)]
pub struct VaultParams {
    /// Where the master key comes from.
    pub key_source: KeySource,
    /// Credential name under `$CREDENTIALS_DIRECTORY` (`systemd-cred`).
    pub credential_name: String,
    /// Absolute path of the key file (`plainfile`). Required for that source.
    pub key_file: Option<String>,
    /// The one sender the vault answers at all. Everything else is refused
    /// before any operation is looked at — checklist point 5: the ACL is on
    /// the vault side, the grant lookup belongs to the broker.
    ///
    /// Absolute (`/main/access/invoke`) or hive-relative (`./invoke`). Relative
    /// is what makes this a template: a hive does not know where a parent will
    /// instantiate it, so it names its sibling and the cell resolves that
    /// against its own path at the first message.
    pub broker: String,
    /// Re-lock this many milliseconds after an unlock. `None` = stay unlocked
    /// until the cell sleeps or the colony stops.
    pub unlock_ttl_ms: Option<u64>,
    /// The edge neighbourhood this vault expects to find itself in — the
    /// sealed contract it attests against before it accepts key material.
    /// Empty means "attest against the broker alone".
    pub sealed_neighbors: Vec<String>,
    /// Operation timeout for filesystem reads of key material (rule 12/A).
    pub external_timeout_ms: u64,
    /// Which secrets are handed to which cell at unlock, and under which param
    /// key: `{"telegram_token": {"to": "./connector", "key": "bot_token"}}`.
    ///
    /// This is the one path on which a plaintext secret leaves the vault, and
    /// it is deliberately shaped so that no message can steer it: the map is
    /// configuration, the delivery happens once at unlock rather than per
    /// request, and a target that is not in the map cannot be named by anybody.
    pub inject_map: Vec<Injection>,
}

/// One entry of `params.inject_map`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Injection {
    /// The secret's name in the vault.
    pub name: String,
    /// The cell that receives it — absolute or hive-relative.
    pub to: String,
    /// The param key it arrives under, e.g. `api_key`.
    pub key: String,
}

/// Default operation timeout for reading key material.
const DEFAULT_EXTERNAL_TIMEOUT_MS: u64 = 5_000;

/// Resolve a possibly hive-relative path against the vault's own path.
///
/// `./invoke` next to `/main/access/vault` is `/main/access/invoke` — a
/// sibling, which is the only shape a sealed hive's interior needs. Anything
/// already absolute passes through untouched.
fn resolve(candidate: &str, vault_path: &str) -> String {
    let Some(sibling) = candidate.strip_prefix("./") else {
        return candidate.to_string();
    };
    match vault_path.rfind('/') {
        Some(0) | None => format!("/{sibling}"),
        Some(i) => format!("{}/{sibling}", &vault_path[..i]),
    }
}

impl VaultParams {
    /// Parse and validate. Shared by `validate_params` and `spawn_cell` —
    /// the parser invariant every factory in this crate follows.
    pub fn parse(raw: &JsonValue) -> Result<Self, String> {
        let obj = raw.as_object().ok_or("vault: params must be an object")?;

        for key in obj.keys() {
            const KNOWN: &[&str] = &[
                "key_source",
                "credential_name",
                "key_file",
                "broker",
                "unlock_ttl_ms",
                "sealed_neighbors",
                "external_timeout_ms",
                "inject_map",
            ];
            if !KNOWN.contains(&key.as_str()) {
                return Err(format!("vault: unknown param {key:?}"));
            }
        }

        let key_source = match obj.get("key_source") {
            None => KeySource::Auto,
            Some(v) => KeySource::parse(v.as_str().ok_or("vault: key_source must be a string")?)?,
        };

        let key_file = match obj.get("key_file") {
            None => None,
            Some(v) => Some(
                v.as_str()
                    .ok_or("vault: key_file must be a string")?
                    .to_string(),
            ),
        };
        if key_source == KeySource::PlainFile && key_file.is_none() {
            return Err("vault: key_source \"plainfile\" requires key_file".into());
        }

        let broker = obj
            .get("broker")
            .and_then(|v| v.as_str())
            .ok_or("vault: broker is mandatory — the vault answers exactly one sender")?
            .to_string();
        if !broker.starts_with('/') && !broker.starts_with("./") {
            return Err(format!(
                "vault: broker must be an absolute path (/main/access/invoke) or a \
                 hive-relative sibling (./invoke), got {broker:?}"
            ));
        }

        let credential_name = obj
            .get("credential_name")
            .and_then(|v| v.as_str())
            .unwrap_or("vault_key")
            .to_string();

        let unlock_ttl_ms = match obj.get("unlock_ttl_ms") {
            None | Some(JsonValue::Null) => None,
            Some(v) => Some(
                v.as_u64()
                    .ok_or("vault: unlock_ttl_ms must be a non-negative integer")?,
            ),
        };

        let sealed_neighbors = match obj.get("sealed_neighbors") {
            None => Vec::new(),
            Some(v) => v
                .as_array()
                .ok_or("vault: sealed_neighbors must be an array of paths")?
                .iter()
                .map(|e| {
                    e.as_str().map(|s| s.to_string()).ok_or_else(|| {
                        "vault: sealed_neighbors entries must be strings".to_string()
                    })
                })
                .collect::<Result<Vec<_>, _>>()?,
        };

        let external_timeout_ms = match obj.get("external_timeout_ms") {
            None => DEFAULT_EXTERNAL_TIMEOUT_MS,
            Some(v) => v
                .as_u64()
                .ok_or("vault: external_timeout_ms must be a non-negative integer")?,
        };

        let inject_map = match obj.get("inject_map") {
            None | Some(JsonValue::Null) => Vec::new(),
            Some(v) => {
                let m = v
                    .as_object()
                    .ok_or("vault: inject_map must be an object of name -> {to, key}")?;
                let mut out = Vec::new();
                for (name, spec) in m {
                    let o = spec.as_object().ok_or_else(|| {
                        format!("vault: inject_map[{name:?}] must be an object with to and key")
                    })?;
                    let to = o
                        .get("to")
                        .and_then(|v| v.as_str())
                        .filter(|s| !s.is_empty())
                        .ok_or_else(|| format!("vault: inject_map[{name:?}] needs \"to\""))?;
                    if !to.starts_with('/') && !to.starts_with("./") {
                        return Err(format!(
                            "vault: inject_map[{name:?}].to must be absolute or hive-relative, \
                             got {to:?}"
                        ));
                    }
                    let key = o
                        .get("key")
                        .and_then(|v| v.as_str())
                        .filter(|s| !s.is_empty())
                        .ok_or_else(|| format!("vault: inject_map[{name:?}] needs \"key\""))?;
                    out.push(Injection {
                        name: name.clone(),
                        to: to.to_string(),
                        key: key.to_string(),
                    });
                }
                out.sort_by(|a, b| a.name.cmp(&b.name));
                out
            }
        };

        Ok(Self {
            key_source,
            credential_name,
            key_file,
            broker,
            unlock_ttl_ms,
            sealed_neighbors,
            external_timeout_ms,
            inject_map,
        })
    }

    /// Every path the vault expects on its edges: the broker plus whatever the
    /// sealed contract adds, all resolved against the vault's own path.
    pub fn attested_neighbors(&self, vault_path: &str) -> Vec<String> {
        let mut v = vec![self.broker_path(vault_path)];
        for n in &self.sealed_neighbors {
            let resolved = resolve(n, vault_path);
            if !v.contains(&resolved) {
                v.push(resolved);
            }
        }
        v
    }

    /// The broker's absolute path, resolved against the vault's own.
    pub fn broker_path(&self, vault_path: &str) -> String {
        resolve(&self.broker, vault_path)
    }

    /// The injection targets, resolved against the vault's own path.
    pub fn injections(&self, vault_path: &str) -> Vec<Injection> {
        self.inject_map
            .iter()
            .map(|i| Injection {
                name: i.name.clone(),
                to: resolve(&i.to, vault_path),
                key: i.key.clone(),
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use meclaw_core::serde_json::json;

    #[test]
    fn a_vault_without_a_broker_is_refused() {
        let err = VaultParams::parse(&json!({})).unwrap_err();
        assert!(err.contains("broker"), "{err}");
    }

    #[test]
    fn the_default_key_source_is_auto() {
        let p = VaultParams::parse(&json!({"broker": "/main/access/broker"})).unwrap();
        assert_eq!(p.key_source, KeySource::Auto);
        assert_eq!(p.credential_name, "vault_key");
        assert_eq!(p.unlock_ttl_ms, None);
    }

    #[test]
    fn plainfile_without_a_file_is_refused_at_validation_not_at_unlock() {
        let err = VaultParams::parse(&json!({
            "broker": "/main/access/broker",
            "key_source": "plainfile"
        }))
        .unwrap_err();
        assert!(err.contains("key_file"), "{err}");
    }

    #[test]
    fn an_unknown_param_is_a_named_error() {
        let err = VaultParams::parse(&json!({
            "broker": "/main/access/broker",
            "passphrase": "hunter2"
        }))
        .unwrap_err();
        assert!(err.contains("passphrase"), "{err}");
    }

    #[test]
    fn the_broker_is_always_part_of_the_attested_neighbourhood() {
        let p = VaultParams::parse(&json!({
            "broker": "/main/access/broker",
            "sealed_neighbors": ["/main/access/timer"]
        }))
        .unwrap();
        assert_eq!(
            p.attested_neighbors("/main/access/vault"),
            vec!["/main/access/broker", "/main/access/timer"]
        );
    }

    /// A template does not know where it will be instantiated, so it names its
    /// sibling and the cell resolves it against its own address.
    #[test]
    fn a_hive_relative_broker_resolves_against_the_vaults_own_path() {
        let p = VaultParams::parse(&json!({
            "broker": "./invoke",
            "sealed_neighbors": ["./clock"]
        }))
        .unwrap();
        assert_eq!(p.broker_path("/main/access/vault"), "/main/access/invoke");
        assert_eq!(
            p.attested_neighbors("/main/access/vault"),
            vec!["/main/access/invoke", "/main/access/clock"]
        );
        // The same template, instantiated somewhere else entirely.
        assert_eq!(p.broker_path("/org/x/access/vault"), "/org/x/access/invoke");
    }

    #[test]
    fn a_bare_relative_broker_path_is_refused() {
        let err = VaultParams::parse(&json!({"broker": "broker"})).unwrap_err();
        assert!(err.contains("hive-relative"), "{err}");
    }
}
