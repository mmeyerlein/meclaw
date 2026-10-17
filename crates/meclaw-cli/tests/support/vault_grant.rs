//! GH #1235 (T6-22): a secret param takes no `${VAR}` -- the colony's
//! environment is readable by every cell it spawns. A test colony whose cells
//! spend a secret therefore grows a real `access` hive in its own tree and
//! names a credential grant per cell; the vault delivers the secret sealed to
//! exactly that cell. The pattern of `crates/meclaw/tests/gh1061_*`, in
//! process: one `--apply` grows `access` (vault opening itself from a 0600
//! plain file) with the ask/answer lane of every granted cell and its grant
//! row, the secrets go in over the vault's user channel while no colony runs,
//! and the test boots the colony afterwards. Include with
//! `// harness_fold: #[path = "support/vault_grant.rs"]`.

use meclaw_cli::{Cli, run_with_hooks};
use serde_json::{Value, json};
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};

/// The vault cell's colony path (the user channel's address).
pub const VAULT: &str = "/main/access/vault";
/// The subject of every grant row.
const SUBJECT: &str = "colony:test";
/// A stub passphrase; the file holding it is the vault's key source.
const PASSPHRASE: &str = "stub-vault-passphrase-1235";
/// What a cell logs once its sealed box opened (`proxy/meclaw`, `proxy/webhook`).
const OPENED: &[u8] = b"received sealed and opened in RAM";

/// One granted cell: its name under `main/`, its grant, the vault's name of
/// the credential, and the stub value the vault holds.
pub struct Granted {
    pub cell: &'static str,
    pub grant: &'static str,
    pub cred_ref: &'static str,
    pub secret: &'static str,
}

fn requester(cell: &str) -> String {
    format!("agent:test/{cell}")
}

fn copy_tree(src: &Path, dst: &Path) {
    std::fs::create_dir_all(dst).expect("mkdir");
    for entry in std::fs::read_dir(src).expect("read_dir").flatten() {
        let to = dst.join(entry.file_name());
        if entry.file_type().expect("file_type").is_dir() {
            copy_tree(&entry.path(), &to);
        } else {
            std::fs::copy(entry.path(), &to).expect("copy");
        }
    }
}

/// Grows `access` in the tree at `root` (the colony `cli` names, not running)
/// and stores every secret of `granted` under its `cred_ref`.
pub async fn grow_vault(root: &Path, mut cli: Cli, granted: &[Granted]) {
    copy_tree(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("../../templates/access"),
        &root.join("templates/access"),
    );
    let key = root.join("vault_key");
    std::fs::write(&key, format!("{PASSPHRASE}\n")).expect("write key");
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&key, std::fs::Permissions::from_mode(0o600)).expect("chmod");
    }
    let (mut edges, mut grants, mut events) = (Vec::new(), Vec::new(), Vec::new());
    for g in granted {
        let at = format!("./{}", g.cell);
        edges.push(json!({
            "from": at, "to": "./access",
            "condition": "has(hop.route) && hop.route == 'credential_request'",
            "modifier": {"set_hop": {"route": "'in_invoke'"},
                         "set_context": {"requester": format!("'{}'", requester(g.cell))}}
        }));
        edges.push(json!({
            "from": "./access", "to": at,
            "condition": format!(
                "has(hop.route) && hop.route == 'ack' && has(hop.operation) && \
                 hop.operation == 'vault.deliver' && has(hop.grant_id) && \
                 hop.grant_id == '{}'", g.grant),
            "modifier": {"set_hop": {"route": "'in_sealed'"}}
        }));
        grants.push(json!({
            "grant_id": g.grant, "requester": requester(g.cell),
            "capability": "credential.read", "subject": SUBJECT,
            "scope": {"actions": ["vault.deliver"]}, "cred_ref": g.cred_ref,
            "purpose": format!("authenticate /{} in a test colony", g.cell),
            "issued_at": "2026-10-16T00:00:00.000000Z",
            "expires_at": "2099-01-01T00:00:00.000000Z",
            "rule_id": "credential-read", "constraints": {"rate_per_min": 60}
        }));
        events.push(json!({
            "id": format!("ev-test-{}", g.cell), "grant_id": g.grant,
            "event": "granted", "at": "2026-10-16T00:00:00.000000Z",
            "actor": "operator", "reason_code": "",
            "detail": {"why": "seeded with the test colony (#1235)"}
        }));
    }
    let manifest: Value = json!({"manifest": [
        {"scope": "/", "diff": {"add_nodes": [{
            "name": "access", "template": "access",
            "override_params": {"vault": {
                "key_source": "plainfile", "key_file": key.display().to_string()}}
        }]}},
        {"scope": "/", "diff": {
            "add_edges": edges,
            "seed_rows": [
                {"target": "./access/store", "table": "grants", "rows": grants},
                {"target": "./access/store", "table": "grant_events", "rows": events}
            ]
        }}
    ]});
    let path = root.join("grow-access.json");
    std::fs::write(&path, serde_json::to_string_pretty(&manifest).expect("ser"))
        .expect("write manifest");
    cli.api = None;
    cli.apply = Some(path);
    tokio::time::timeout(
        meclaw_testing::deadline::APPLY_BOOT_HANG,
        run_with_hooks(cli, None, None),
    )
    .await
    .expect("hang mark: the one-shot --apply ends")
    .expect("the manifest grows access");
    for g in granted {
        let (root, cred_ref, secret) = (root.to_path_buf(), g.cred_ref, g.secret);
        tokio::task::spawn_blocking(move || {
            meclaw_cells::vault::user_channel::add(
                &root,
                VAULT,
                cred_ref,
                secret.as_bytes(),
                PASSPHRASE.as_bytes(),
            )
        })
        .await
        .expect("join")
        .expect("the vault stores the secret");
    }
}

/// Woken on every line a test's log capture takes: its writer calls
/// `notify_waiters` on it, so [`wait_opened`] waits on the event, not a clock.
pub fn log_written() -> Arc<tokio::sync::Notify> {
    static WROTE: OnceLock<Arc<tokio::sync::Notify>> = OnceLock::new();
    WROTE
        .get_or_init(|| Arc::new(tokio::sync::Notify::new()))
        .clone()
}

/// A granted cell asks the vault in `on_start`; until its box opened it holds
/// its traffic (`credential_pending`), which is the #1105/#1106 contract and
/// not what the tests measure. Ready = `n` opened boxes in the capture. The
/// test is a client outside the substrate; the deadline is the boot hang mark
/// (the vault derives its key at first use), never a budget.
pub async fn wait_opened(logs: &Arc<Mutex<Vec<u8>>>, n: usize) {
    let deadline = tokio::time::Instant::now() + meclaw_testing::deadline::BOOT_HANG;
    let woke = log_written();
    loop {
        // Armed before the look, so a line written in between still wakes us.
        let wrote = woke.notified();
        tokio::pin!(wrote);
        wrote.as_mut().enable();
        let text = logs.lock().expect("logs").clone();
        let opened = text.windows(OPENED.len()).filter(|w| *w == OPENED).count();
        if opened >= n {
            return;
        }
        if tokio::time::timeout_at(deadline, wrote).await.is_err() {
            let text = String::from_utf8_lossy(&text);
            let trail: Vec<&str> = text
                .lines()
                .filter(|l| l.contains("vault") || l.contains("grant") || l.contains("credential"))
                .collect();
            panic!("hang mark: {opened} of {n} sealed boxes opened -- vault lines: {trail:#?}");
        }
    }
}
