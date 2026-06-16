//! Slice 11-D: Boot-Wiring. Leere Templates-Registry → auto-scan.
//! Populiertes templates/-Verzeichnis → Einträge nach Boot in DB.

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
        "Vorbedingung: leer"
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
    // Filesystem ändert sich nicht — zweiter Boot ohne force darf NICHT scannen
    // (Beweis indirekt: scanned_at bleibt 100).
    meclaw_colony::templates::boot_load_or_scan(&td.path().join("templates"), &db, false, 200)
        .await
        .unwrap();
    assert_eq!(db.read_templates().unwrap()[0].scanned_at, 100);
    // Force-Rescan überschreibt scanned_at auf 300.
    meclaw_colony::templates::boot_load_or_scan(&td.path().join("templates"), &db, true, 300)
        .await
        .unwrap();
    assert_eq!(db.read_templates().unwrap()[0].scanned_at, 300);
}
