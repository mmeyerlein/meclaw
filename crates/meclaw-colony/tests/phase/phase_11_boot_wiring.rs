//! Slice 11-D: boot wiring. An empty templates registry → auto-scan.
//! A populated templates/ directory → entries in the DB after boot.

use meclaw_colony::ColonyDb;
use tempfile::TempDir;

fn make_template(td: &TempDir, dir: &str, body: &str) {
    let p = td.path().join("templates").join(dir);
    std::fs::create_dir_all(&p).unwrap();
    std::fs::write(p.join("template.json"), body).unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn auto_scan_on_empty_registry() {
    let td = TempDir::new().unwrap();
    make_template(&td, "echo", r#"{"name":"echo"}"#);
    let db = ColonyDb::open(&td.path().join("c.db")).unwrap();
    assert!(
        db.read_templates().unwrap().is_empty(),
        "precondition: empty"
    );
    // Boot-Wiring-Funktion.
    meclaw_colony::templates::boot_load_or_scan(
        &td.path().join("templates"),
        &db,
        /*force_rescan=*/ false,
        1234,
    )
    .await
    .unwrap();
    assert_eq!(db.read_templates().unwrap().len(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn populated_registry_is_not_re_scanned_unless_forced() {
    let td = TempDir::new().unwrap();
    make_template(&td, "echo", r#"{"name":"echo"}"#);
    let db = ColonyDb::open(&td.path().join("c.db")).unwrap();
    // Vorab: einmal scannen.
    meclaw_colony::templates::boot_load_or_scan(&td.path().join("templates"), &db, false, 100)
        .await
        .unwrap();
    // The filesystem does not change — a second boot without force must NOT scan
    // (indirect proof: scanned_at stays 100).
    meclaw_colony::templates::boot_load_or_scan(&td.path().join("templates"), &db, false, 200)
        .await
        .unwrap();
    assert_eq!(db.read_templates().unwrap()[0].scanned_at, 100);
    // A forced rescan overwrites scanned_at with 300.
    meclaw_colony::templates::boot_load_or_scan(&td.path().join("templates"), &db, true, 300)
        .await
        .unwrap();
    assert_eq!(db.read_templates().unwrap()[0].scanned_at, 300);
}
