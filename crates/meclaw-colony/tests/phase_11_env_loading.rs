//! Phase-11-Pre-Demo: .env wird beim Mutation-Apply gelesen, ${ENV_VAR} substituiert.
//! Schließt Phase-6-Stub colony.rs:643 ("T13-MVP uses an empty env map").
//!
//! Vollständiger Roundtrip-Test wird in 11-F nachgereicht, sobald
//! `build_staging_tree` den .env-substituierten config.json-Inhalt schreibt.

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn env_var_in_override_params_resolves_from_root_env() {
    let td = tempfile::TempDir::new().unwrap();
    std::fs::write(td.path().join(".env"), "FOO=hello-env\n").unwrap();
    // Setup skeleton: Colony-Boot empty, Mutation with add_nodes(template=echo,
    // override_params={greeting: "${FOO}"}). Expected: substituted "hello-env"
    // visible in the spawned Cell's config.json.
    // ↳ Concrete Cell-Type choice in 11-F — only compile-and-link stub here.
    // ↳ Full test delivered in 11-F once build_staging_tree writes
    //   the .env-substituted config.json content.
    let _ = td; // placeholder until 11-F
}
