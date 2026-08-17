//! The CLI half of the vault's user channel.
//!
//! It decides only where bytes come from — the secret off stdin, the
//! passphrase off the terminal or a key source — and hands them to
//! `meclaw_cells::vault::user_channel`, which does the sealing. Nothing here
//! builds a message or boots a colony: a credential that never becomes a
//! message cannot be read out of one.
//!
//! The passphrase never arrives as an argument. A key on a command line lands
//! in `ps` output and in shell history, which is why `--vault-key-source`
//! names a *source*.

use anyhow::{Context, anyhow, bail};
use meclaw_cells::vault::user_channel;
use std::io::{IsTerminal, Read};
use std::path::{Path, PathBuf};

/// What the operator asked for.
pub enum VaultCommand {
    /// Store (or rotate) a secret. The value is read from stdin.
    Add(String),
    /// Report what the vault holds — names and versions, never content.
    Status,
    /// Revoke every active version of a name.
    Revoke(String),
}

/// Run a vault command against the vault cell at `cell_path` under `root`.
pub fn run(
    root: &Path,
    cell_path: &str,
    command: VaultCommand,
    key_source: &str,
    key_file: Option<&Path>,
) -> anyhow::Result<()> {
    // GH #160: single writer, by construction. These modes deliberately do not
    // boot a colony and therefore never take the root lease (`lib.rs` returns
    // above the lease block, on purpose — a mode that spawned cells to store a
    // credential would hand it to a running tree). That left `--vault-add`
    // writing the vault's `cell.db` while the live vault cell owned it: WAL makes
    // that survivable rather than correct, because the cell has no idea its store
    // grew a version and its in-memory view is stale with nothing announcing it.
    //
    // So the channel asks who holds the root, before the first
    // `Connection::open`, and refuses to WRITE into a live one.
    //
    // `Status` is read-only and stays available — one connection, one snapshot.
    // `Revoke` gets the documented exemption from the same issue: being locked
    // out of a vault must never be what stops somebody killing a leaked
    // credential. It proceeds, loudly, and says what the operator still has to do
    // for the running cell to see it.
    let holder = crate::lease::current_holder(root);
    if let Some(h) = holder {
        match &command {
            VaultCommand::Add(_) => bail!(
                "a meclaw colony is running on this root (pid {}, start_id {}) and the vault \
                 cell owns its own cell.db — refusing to write a second version into it from \
                 outside. Stop pid {} and retry, or add the secret through the running vault.",
                h.pid,
                h.start_id,
                h.pid
            ),
            VaultCommand::Revoke(name) => eprintln!(
                "warning: a meclaw colony is running on this root (pid {}) — revoking `{name}` \
                 anyway, because a leaked credential must never wait for a maintenance window. \
                 The running vault cell keeps its current view until it is restarted; restart \
                 pid {} to make the revocation take effect inside the colony.",
                h.pid, h.pid
            ),
            VaultCommand::Status => {}
        }
    }
    match command {
        VaultCommand::Status => {
            let held = user_channel::status(root, cell_path).map_err(|e| anyhow!(e))?;
            if held.is_empty() {
                println!("{cell_path}: no secrets");
            } else {
                println!("{cell_path}:");
                for (name, version) in held {
                    println!("  {name}  (v{version})");
                }
            }
        }
        VaultCommand::Revoke(name) => {
            let changed = user_channel::revoke(root, cell_path, &name).map_err(|e| anyhow!(e))?;
            if changed == 0 {
                println!("{name}: nothing active to revoke");
            } else {
                println!("{name}: {changed} version(s) revoked");
            }
        }
        VaultCommand::Add(name) => {
            // Order matters: the secret is consumed off stdin first, so the
            // passphrase prompt below can own the terminal without competing
            // for the same stream.
            let secret = read_secret_from_stdin()?;
            let passphrase = passphrase_from(key_source, key_file)?;
            let version = user_channel::add(root, cell_path, &name, &secret, &passphrase)
                .map_err(|e| anyhow!(e))?;
            println!("{name}: stored as v{version}");
        }
    }
    Ok(())
}

/// The secret itself: whatever stdin carries, minus one trailing newline.
///
/// stdin rather than an argument, for the `ps`/history reason; and the whole
/// of stdin rather than a line, because a secret may legitimately contain
/// newlines (a PEM key does).
fn read_secret_from_stdin() -> anyhow::Result<Vec<u8>> {
    if std::io::stdin().is_terminal() {
        eprintln!("reading the secret from stdin — paste it, then press Ctrl-D");
    }
    let mut buf = Vec::new();
    std::io::stdin()
        .read_to_end(&mut buf)
        .context("reading the secret from stdin")?;
    Ok(trim_one_newline(buf))
}

/// Strip one trailing `\n` or `\r\n`, and nothing else.
fn trim_one_newline(mut bytes: Vec<u8>) -> Vec<u8> {
    if bytes.last() == Some(&b'\n') {
        bytes.pop();
        if bytes.last() == Some(&b'\r') {
            bytes.pop();
        }
    }
    bytes
}

/// Resolve the passphrase from the named source.
///
/// `prompt` reads from the terminal, not from stdin: stdin is carrying the
/// secret. Without a terminal there is nothing to prompt, and the honest
/// answer is to name the source to use instead rather than to invent one.
fn passphrase_from(key_source: &str, key_file: Option<&Path>) -> anyhow::Result<Vec<u8>> {
    let resolved = match key_source {
        "auto" => {
            if std::env::var_os("CREDENTIALS_DIRECTORY").is_some() {
                "systemd-cred"
            } else {
                "prompt"
            }
        }
        other => other,
    };
    match resolved {
        "prompt" => read_passphrase_from_tty(),
        "systemd-cred" => {
            let dir = std::env::var_os("CREDENTIALS_DIRECTORY").ok_or_else(|| {
                anyhow!("--vault-key-source systemd-cred but $CREDENTIALS_DIRECTORY is unset")
            })?;
            read_key_file(&PathBuf::from(dir).join("vault_key"))
        }
        "plainfile" => {
            let f = key_file.ok_or_else(|| {
                anyhow!("--vault-key-source plainfile needs --vault-key-file <PATH>")
            })?;
            read_key_file(f)
        }
        other => bail!("unknown --vault-key-source {other:?} (auto|prompt|systemd-cred|plainfile)"),
    }
}

/// Read key material from a file, refusing it if group or others can read it —
/// the same answer ssh gives for a loose private key.
fn read_key_file(path: &Path) -> anyhow::Result<Vec<u8>> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let meta = std::fs::metadata(path).with_context(|| format!("stat {}", path.display()))?;
        if meta.permissions().mode() & 0o077 != 0 {
            bail!(
                "{} is readable by group or others — tighten it to 0600",
                path.display()
            );
        }
    }
    let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    let bytes = trim_one_newline(bytes);
    if bytes.is_empty() {
        bail!("{} is empty", path.display());
    }
    Ok(bytes)
}

/// Read the passphrase from `/dev/tty` with echo off — the same place and the
/// same manners `ssh-keygen` uses.
///
/// Echo is turned off through `stty` rather than a termios binding: this crate
/// has no libc dependency, and `stty` exists on every unix that has a
/// `/dev/tty`. If echo cannot be turned off the passphrase is NOT read —
/// printing it on screen is not a graceful degradation.
#[cfg(unix)]
fn read_passphrase_from_tty() -> anyhow::Result<Vec<u8>> {
    use std::io::{BufRead, BufReader, Write};
    let tty = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/tty")
        .map_err(|e| {
            anyhow!(
                "no terminal to ask for the passphrase ({e}). stdin is carrying the secret, so \
                 the passphrase cannot come from there — use --vault-key-source plainfile \
                 --vault-key-file <PATH> (or systemd-cred) for unattended use."
            )
        })?;
    let mut out = tty.try_clone()?;
    stty(&tty, "-echo").map_err(|e| {
        anyhow!(
            "cannot turn terminal echo off ({e}) — refusing to read a \
                              passphrase in the clear"
        )
    })?;
    write!(out, "vault passphrase: ")?;
    out.flush()?;
    let mut line = String::new();
    let read = BufReader::new(tty.try_clone()?).read_line(&mut line);
    let _ = stty(&tty, "echo");
    writeln!(out)?;
    read.context("reading the passphrase")?;
    let pass = line.trim_end_matches(['\n', '\r']).as_bytes().to_vec();
    if pass.is_empty() {
        bail!("empty passphrase — nothing was stored");
    }
    Ok(pass)
}

/// Run `stty <arg>` against an open terminal.
#[cfg(unix)]
fn stty(tty: &std::fs::File, arg: &str) -> anyhow::Result<()> {
    let status = std::process::Command::new("stty")
        .arg(arg)
        .stdin(tty.try_clone()?)
        .status()
        .context("running stty")?;
    if !status.success() {
        bail!("stty {arg} exited with {status}");
    }
    Ok(())
}

#[cfg(not(unix))]
fn read_passphrase_from_tty() -> anyhow::Result<Vec<u8>> {
    bail!(
        "interactive passphrase entry is unix-only here; use --vault-key-source plainfile \
         --vault-key-file <PATH>"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unknown_key_source_is_named_rather_than_guessed() {
        let err = passphrase_from("magic", None).unwrap_err().to_string();
        assert!(err.contains("unknown --vault-key-source"), "{err}");
    }

    #[test]
    fn plainfile_without_a_file_says_which_flag_is_missing() {
        let err = passphrase_from("plainfile", None).unwrap_err().to_string();
        assert!(err.contains("--vault-key-file"), "{err}");
    }

    #[cfg(unix)]
    #[test]
    fn a_loose_key_file_is_refused() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("key");
        std::fs::write(&f, b"pw").unwrap();
        std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o644)).unwrap();
        let err = read_key_file(&f).unwrap_err().to_string();
        assert!(err.contains("readable by group or others"), "{err}");
    }

    #[cfg(unix)]
    #[test]
    fn a_tight_key_file_is_read_and_its_newline_stripped() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("key");
        std::fs::write(&f, b"hunter2\n").unwrap();
        std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(read_key_file(&f).unwrap(), b"hunter2");
    }

    #[test]
    fn only_one_trailing_newline_is_stripped_from_a_secret() {
        assert_eq!(trim_one_newline(b"tok\n".to_vec()), b"tok");
        assert_eq!(trim_one_newline(b"tok\r\n".to_vec()), b"tok");
        assert_eq!(trim_one_newline(b"tok\n\n".to_vec()), b"tok\n");
        assert_eq!(
            trim_one_newline(b"-----BEGIN\nkey\n-----END".to_vec()),
            b"-----BEGIN\nkey\n-----END"
        );
    }
}
