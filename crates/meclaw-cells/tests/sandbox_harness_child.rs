//! GH #85: the `harness` cell's stdio child carries the same sandbox profile
//! as `code` and `bash`.
//!
//! The 0.1.8 acceptance pass noted that `harness` brings its own shell, file
//! access and network and is measurably contained by nothing but `env_clear`,
//! an env passthrough allow-list and the canonicalized cwd clamp. Phase 1 left
//! it out because the spawn site owns process-group and reaping behaviour of
//! its own; this file is the proof that the second wiring pass happened and
//! that it did not disturb either.
//!
//! Two levels, because they prove two different things:
//!
//! - `HarnessParams` carries `params.sandbox` into the `ChildSpec` it builds
//!   (the wiring), and refuses to let a runtime params message move it;
//! - `StdioChild::spawn` really applies it to a real process (the enforcement),
//!   with a control, skip-guarded exactly like the phase-1 isolation tests.

use meclaw_cells::stdio_child::{ChildSpec, Frame, StdioChild};
use meclaw_core::serde_json::json;

// ---- the wiring -----------------------------------------------------------

/// A `harness` params block, plus whatever `extra` adds.
fn harness_params(root: &std::path::Path, extra: meclaw_core::JsonValue) -> meclaw_core::JsonValue {
    let mut v = json!({
        "adapter": "claude-code",
        "emit_to": "/out",
        "workspace_root": root.to_str().unwrap(),
    });
    if let Some(obj) = extra.as_object() {
        for (k, val) in obj {
            v.as_object_mut().unwrap().insert(k.clone(), val.clone());
        }
    }
    v
}

#[test]
fn harness_params_carry_the_sandbox_block() {
    let root = tempfile::TempDir::new().unwrap();
    let p = meclaw_cells::harness::HarnessParams::parse(&harness_params(
        root.path(),
        json!({"sandbox": {"trust": "restricted", "filesystem": {"read": ["/usr"]}}}),
    ))
    .expect("params parse");
    assert!(
        p.sandbox.is_some(),
        "a harness that declares a sandbox must carry it into the cell"
    );
}

#[test]
fn a_harness_without_a_sandbox_block_still_parses() {
    // The prospective cut (GH #85) applies at INSTANTIATION, not here: a
    // hand-written config that predates it keeps running unchanged.
    let root = tempfile::TempDir::new().unwrap();
    let p = meclaw_cells::harness::HarnessParams::parse(&harness_params(root.path(), json!({})))
        .expect("params parse");
    assert!(p.sandbox.is_none());
}

#[test]
fn a_broken_sandbox_block_is_a_boot_error_not_a_spawn_surprise() {
    let root = tempfile::TempDir::new().unwrap();
    let e = meclaw_cells::harness::HarnessParams::parse(&harness_params(
        root.path(),
        json!({"sandbox": {"trust": "restricted"}}),
    ))
    .unwrap_err();
    assert!(e.contains("params.sandbox.filesystem"), "{e}");
}

#[test]
fn sandbox_is_immutable_under_a_runtime_params_message() {
    // A security boundary a message can move is not a boundary. `harness` is
    // the first sandbox consumer with a runtime overlay at all, so this is
    // where the rule stops being structural and has to be stated.
    use meclaw_cells::params_overlay::OverlayParams;
    assert!(
        meclaw_cells::harness::HarnessOverlay::IMMUTABLE_KEYS.contains(&"sandbox"),
        "sandbox must be listed immutable, otherwise an update reads as merely unknown"
    );
    assert!(meclaw_cells::harness::HarnessOverlay::KNOWN_KEYS.contains(&"sandbox"));
}

// ---- the enforcement ------------------------------------------------------

/// True when this kernel can enforce a filesystem allowlist.
fn have_landlock(test: &str) -> bool {
    if meclaw_cells::sandbox::landlock_abi().is_some() {
        true
    } else {
        eprintln!("[{test}] SKIPPED: no Landlock on this kernel (needs Linux 5.13+)");
        false
    }
}

/// A child that prints whether it could read `path`, as one line-JSON frame.
fn read_probe_spec(path: &std::path::Path, sandbox: Option<meclaw_core::JsonValue>) -> ChildSpec {
    let script = format!(
        "import sys, json\n\
         try:\n    open({p}).read()\n    r = 'READ_OK'\n\
         except Exception as e:\n    r = 'READ_DENIED:' + type(e).__name__\n\
         sys.stdout.write(json.dumps({{'r': r}}) + '\\n')\n\
         sys.stdout.flush()\n",
        p = json!(path.to_str().unwrap())
    );
    ChildSpec {
        program: "python3".into(),
        args: vec!["-c".into(), script],
        kill_grace_ms: 2_000,
        // The harness's own settings, kept exactly as the cell sets them: this
        // test must exercise the sandbox NEXT TO them, not instead of them.
        process_group: true,
        env_clear: true,
        sandbox: sandbox.map(|v| {
            Box::new(
                meclaw_cells::sandbox::SandboxProfile::parse(&json!({"sandbox": v}))
                    .expect("profile parse")
                    .expect("a profile"),
            )
        }),
        ..ChildSpec::default()
    }
}

/// Read the child's single frame, verbatim.
///
/// The 30 s budget is a failure marker, not a timing discriminator: the child
/// answers in milliseconds, and the width is there so a loaded parallel cargo
/// run cannot turn a pass into a flake.
async fn first_frame(child: &mut StdioChild) -> String {
    let frame = tokio::time::timeout(std::time::Duration::from_secs(30), child.read_frame())
        .await
        .expect("the child answers well inside the failure-marker budget")
        .expect("a readable frame")
        .expect("not EOF");
    match frame {
        Frame::Json(v) => v.to_string(),
        Frame::Malformed(s) => s,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_stdio_child_cannot_read_outside_the_allowed_paths() {
    const T: &str = "the_stdio_child_cannot_read_outside_the_allowed_paths";
    if !have_landlock(T) {
        return;
    }
    let allowed = tempfile::TempDir::new().unwrap();
    let forbidden = tempfile::TempDir::new().unwrap();
    std::fs::write(allowed.path().join("visible"), b"visible").unwrap();
    std::fs::write(forbidden.path().join("secret"), b"secret").unwrap();
    let sb = json!({
        "trust": "restricted",
        "network": "allow",
        "filesystem": {"read": [allowed.path().to_str().unwrap()]}
    });

    // The proof: a file in a directory the profile never named.
    let mut denied = StdioChild::spawn(&read_probe_spec(
        &forbidden.path().join("secret"),
        Some(sb.clone()),
    ))
    .expect("spawn");
    let line = first_frame(&mut denied).await;
    assert!(
        line.contains("READ_DENIED:PermissionError"),
        "the harness child must be refused outside its allowlist, got {line:?}"
    );
    denied.terminate(std::time::Duration::from_secs(2)).await;

    // The control: same profile, a file inside the allowlist.
    let mut ok = StdioChild::spawn(&read_probe_spec(&allowed.path().join("visible"), Some(sb)))
        .expect("spawn");
    let line = first_frame(&mut ok).await;
    assert!(
        line.contains("READ_OK"),
        "and it must keep the access it was granted, got {line:?}"
    );
    ok.terminate(std::time::Duration::from_secs(2)).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unsandboxed_stdio_child_is_unchanged() {
    // The counter-control for the whole wiring: without a profile the spawn
    // site behaves exactly as it did before, which is what keeps `mcp` and
    // `subcolony` out of this change.
    let forbidden = tempfile::TempDir::new().unwrap();
    std::fs::write(forbidden.path().join("secret"), b"secret").unwrap();
    let mut child =
        StdioChild::spawn(&read_probe_spec(&forbidden.path().join("secret"), None)).expect("spawn");
    let line = first_frame(&mut child).await;
    assert!(line.contains("READ_OK"), "got {line:?}");
    child.terminate(std::time::Duration::from_secs(2)).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_profile_that_cannot_be_applied_fails_the_spawn() {
    // Fail-closed at this spawn site too: the harness child never falls back
    // to running unsandboxed.
    let spec = read_probe_spec(
        std::path::Path::new("/etc/hostname"),
        Some(json!({
            "trust": "restricted",
            "filesystem": {"read": ["/nonexistent-gh85-harness-path"]}
        })),
    );
    let msg = match StdioChild::spawn(&spec) {
        Ok(_) => panic!("the spawn must fail rather than run the harness unsandboxed"),
        Err(e) => e.detail(),
    };
    assert!(
        msg.contains("sandbox not applied") && msg.contains("/nonexistent-gh85-harness-path"),
        "the error must say which boundary could not be built, got {msg:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_sandbox_leaves_the_process_group_alone() {
    // The reaping semantics `stdio_child` owns must survive the second wiring
    // pass: a sandboxed child still leads its own group, which is what lets
    // `terminate` sweep its descendants.
    const T: &str = "the_sandbox_leaves_the_process_group_alone";
    if !have_landlock(T) {
        return;
    }
    let work = tempfile::TempDir::new().unwrap();
    let mut spec = read_probe_spec(
        &work.path().join("nope"),
        Some(json!({
            "trust": "restricted", "network": "allow",
            "filesystem": {"read": [work.path().to_str().unwrap()]}
        })),
    );
    // Report the child's own pid and process group instead of a file read.
    spec.args = vec![
        "-c".into(),
        "import sys, json, os\n\
         sys.stdout.write(json.dumps({'pid': os.getpid(), 'pgid': os.getpgid(0)}) + '\\n')\n\
         sys.stdout.flush()\n"
            .into(),
    ];
    let mut child = StdioChild::spawn(&spec).expect("spawn");
    let line = first_frame(&mut child).await;
    let v: meclaw_core::JsonValue = meclaw_core::serde_json::from_str(&line).expect("json");
    assert_eq!(
        v["pid"], v["pgid"],
        "a sandboxed harness child must still lead its own process group, got {line:?}"
    );
    child.terminate(std::time::Duration::from_secs(2)).await;
}
