//! Where the master key comes from — the layer ssh keeps separate from the
//! agent itself, kept separate here too.
//!
//! Every backend feeds the same vault cell and the same derivation. What
//! differs is only the origin of the passphrase material: a user typing it, a
//! credential systemd hands to the service at start, or a file protected by
//! its permissions. Whatever a backend returns is treated as a *passphrase*,
//! never as a raw key — that keeps one derivation path for all sources and
//! lets an operator put anything in a systemd credential without having to
//! produce exactly 32 bytes.

use super::params::{KeySource, VaultParams};
use std::path::{Path as FsPath, PathBuf};

/// What a key source produced.
#[derive(Debug)]
pub enum KeyMaterial {
    /// Passphrase material, ready for derivation.
    Passphrase(Vec<u8>),
    /// This source cannot produce anything on its own — the passphrase has to
    /// arrive over the user channel. `prompt`, and `auto` outside systemd.
    AwaitUserChannel,
}

/// Resolve `auto` into the concrete source, the way ssh resolves "agent if
/// `SSH_AUTH_SOCK`, prompt otherwise".
pub fn effective_source(params: &VaultParams) -> KeySource {
    match params.key_source {
        KeySource::Auto => {
            if credentials_dir().is_some() {
                KeySource::SystemdCred
            } else {
                KeySource::Prompt
            }
        }
        ref explicit => explicit.clone(),
    }
}

/// `$CREDENTIALS_DIRECTORY`, if the process runs under systemd's credential
/// contract.
fn credentials_dir() -> Option<PathBuf> {
    std::env::var_os("CREDENTIALS_DIRECTORY").map(PathBuf::from)
}

/// Read whatever the effective source can offer without a user present.
///
/// Runs with an operation timeout (rule 12/A) because it touches the
/// filesystem. Trailing newlines are stripped: a credential written with
/// `echo` would otherwise derive a different key than the same secret typed
/// at a prompt, and that failure mode is unbearably confusing.
pub async fn load(params: &VaultParams) -> Result<KeyMaterial, String> {
    let source = effective_source(params);
    let path = match source {
        KeySource::Prompt | KeySource::Auto => return Ok(KeyMaterial::AwaitUserChannel),
        KeySource::SystemdCred => {
            let dir = credentials_dir()
                .ok_or("vault: key_source \"systemd-cred\" but $CREDENTIALS_DIRECTORY is unset")?;
            dir.join(&params.credential_name)
        }
        KeySource::PlainFile => PathBuf::from(
            params
                .key_file
                .as_ref()
                .ok_or("vault: key_source \"plainfile\" without key_file")?,
        ),
    };

    let timeout = std::time::Duration::from_millis(params.external_timeout_ms);
    let read = tokio::time::timeout(timeout, read_key_file(path.clone()))
        .await
        .map_err(|_| {
            format!(
                "vault: reading key material from {} timed out",
                path.display()
            )
        })??;

    Ok(KeyMaterial::Passphrase(read))
}

/// Read a key file and refuse it if it is readable by anyone else — the same
/// answer ssh gives for a loose `~/.ssh/id_*`.
async fn read_key_file(path: PathBuf) -> Result<Vec<u8>, String> {
    refuse_loose_permissions(&path).await?;
    let bytes = tokio::fs::read(&path).await.map_err(|e| {
        format!(
            "vault: cannot read key material from {}: {e}",
            path.display()
        )
    })?;
    let trimmed = trim_trailing_newline(&bytes);
    if trimmed.is_empty() {
        return Err(format!(
            "vault: key material at {} is empty",
            path.display()
        ));
    }
    Ok(trimmed)
}

/// Strip one trailing `\n` or `\r\n`, and nothing else — leading and interior
/// bytes are part of the passphrase.
fn trim_trailing_newline(bytes: &[u8]) -> Vec<u8> {
    let mut end = bytes.len();
    if end > 0 && bytes[end - 1] == b'\n' {
        end -= 1;
        if end > 0 && bytes[end - 1] == b'\r' {
            end -= 1;
        }
    }
    bytes[..end].to_vec()
}

/// Refuse a key file that group or others can read.
///
/// systemd's credentials directory is already 0400/0500 and owned by the
/// service user, so this passes there without a special case; a hand-placed
/// `plainfile` is where it bites, which is exactly the point.
#[cfg(unix)]
async fn refuse_loose_permissions(path: &FsPath) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    let meta = tokio::fs::metadata(path)
        .await
        .map_err(|e| format!("vault: cannot stat key material at {}: {e}", path.display()))?;
    let mode = meta.permissions().mode() & 0o077;
    if mode != 0 {
        return Err(format!(
            "vault: key material at {} is readable by group or others (mode {:o}); \
             tighten it to 0600 — the same refusal ssh gives for a loose private key",
            path.display(),
            meta.permissions().mode() & 0o777
        ));
    }
    Ok(())
}

#[cfg(not(unix))]
async fn refuse_loose_permissions(_path: &FsPath) -> Result<(), String> {
    // No portable equivalent of the unix mode bits; the platform's own ACLs
    // are the boundary there. Stated rather than silently skipped.
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use meclaw_core::serde_json::json;

    fn params(extra: meclaw_core::JsonValue) -> VaultParams {
        let mut o = json!({"broker": "/main/access/broker"});
        for (k, v) in extra.as_object().unwrap() {
            o.as_object_mut().unwrap().insert(k.clone(), v.clone());
        }
        VaultParams::parse(&o).unwrap()
    }

    #[tokio::test]
    async fn prompt_produces_nothing_on_its_own() {
        let p = params(json!({"key_source": "prompt"}));
        assert!(matches!(
            load(&p).await.unwrap(),
            KeyMaterial::AwaitUserChannel
        ));
    }

    #[tokio::test]
    async fn a_plainfile_key_is_read_and_its_trailing_newline_stripped() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("key");
        tokio::fs::write(&f, b"hunter2\n").await.unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            tokio::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o600))
                .await
                .unwrap();
        }
        let p = params(json!({"key_source": "plainfile", "key_file": f.to_str().unwrap()}));
        match load(&p).await.unwrap() {
            KeyMaterial::Passphrase(b) => assert_eq!(b, b"hunter2"),
            _ => panic!("expected passphrase material"),
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_world_readable_key_file_is_refused_like_ssh_refuses_one() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("key");
        tokio::fs::write(&f, b"hunter2").await.unwrap();
        tokio::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o644))
            .await
            .unwrap();
        let p = params(json!({"key_source": "plainfile", "key_file": f.to_str().unwrap()}));
        let err = load(&p).await.unwrap_err();
        assert!(err.contains("readable by group or others"), "{err}");
    }

    #[tokio::test]
    async fn an_empty_key_file_is_a_named_error_not_an_empty_passphrase() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("key");
        tokio::fs::write(&f, b"\n").await.unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            tokio::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o600))
                .await
                .unwrap();
        }
        let p = params(json!({"key_source": "plainfile", "key_file": f.to_str().unwrap()}));
        assert!(load(&p).await.unwrap_err().contains("is empty"));
    }

    #[test]
    fn auto_resolves_to_prompt_without_a_credentials_directory() {
        // The test process is not run by systemd with credentials.
        if std::env::var_os("CREDENTIALS_DIRECTORY").is_none() {
            assert_eq!(effective_source(&params(json!({}))), KeySource::Prompt);
        }
    }

    #[test]
    fn only_one_trailing_newline_is_stripped() {
        assert_eq!(trim_trailing_newline(b"a\n\n"), b"a\n");
        assert_eq!(trim_trailing_newline(b"a\r\n"), b"a");
        assert_eq!(trim_trailing_newline(b"a"), b"a");
        assert_eq!(trim_trailing_newline(b" a "), b" a ");
    }
}
