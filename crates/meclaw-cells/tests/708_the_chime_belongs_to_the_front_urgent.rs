//! GH #708 -- the sound belongs to the front urgent (§ 6.13), and the client
//! remembers nothing across mounts (§ 3.2, § 4.33).
//!
//! "The sound … not in the model (client): ringing in the tile is visual only;
//! a sound the client plays only for the front urgent (`front_urgent`, step 7):
//! on its appearance on level 3, then every two seconds, at most one minute;
//! the sound does not hang on `inputs` … and a browser that forbids sound
//! before the first gesture stays silent" (§ 6.13).
//!
//! Two defects this file closes. The hook collected EVERY window with the rung
//! `urgent` and rang for each -- two ringing timers rang twice, although only
//! one of them stands on level 3 (§ 4.18: there is one front urgent, and the
//! curator says which). And what had rung was kept on `window`, across mounts:
//! a wall screen that reconnects all day carried a memory that decided whether
//! something rings. § 3.2 allows exactly one browser state with meaning -- the
//! dock -- and R-23-4 struck the "seen" memory by name (§ 4.33: linger, no seen
//! memory). The ring's own beat (appearance, every two seconds, one minute) is
//! rendering and stays in the client, but it starts afresh with the mount.
//!
//! The browser proof is B-24 (counted `start()` calls: the front urgent rings,
//! the second urgent counts zero), strand H5.
//!
//! Skips when the templates do not ship (R2b).

fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

const COMPOSE: &str = "templates/display/compose/compose.py";

fn library_ships() -> bool {
    repo("templates/display/template.json").is_file()
}

fn read(rel: &str) -> String {
    std::fs::read_to_string(repo(rel)).unwrap_or_else(|e| panic!("{rel}: {e}"))
}

fn block(name: &str) -> String {
    let src = read(COMPOSE);
    let open = format!("{name} = (");
    let start = src.find(&open).unwrap_or_else(|| panic!("{name} is gone"));
    let end = src[start..]
        .find("\n)")
        .unwrap_or_else(|| panic!("{name} is not one parenthesised block"));
    src[start..start + end].to_string()
}

#[test]
fn only_the_front_urgent_rings() {
    if !library_ships() {
        return;
    }
    let scene = block("SCENE_CLIENT_JS");
    assert!(
        scene.contains("querySelector('[data-region] [id][data-front=\\\"1\\\"]')"),
        "the ring does not ask for the FRONT urgent -- § 6.13 and § 4.18: one \
         window stands on level 3, and the curator says which one. A selector \
         over every urgent rings twice for two timers."
    );
    assert!(
        !scene.contains("data-state"),
        "the scene hook still reads the struck curator word `state` (§ 2: the \
         step is `rung`, where it is drawn is `level`)"
    );
    for beat in ["CHIME_EVERY_MS = 2000", "CHIME_MAX_MS = 60000"] {
        assert!(
            scene.contains(beat),
            "the beat changed: § 6.13 says `{beat}` in as many words (on its \
             appearance, then every two seconds, at most one minute)"
        );
    }
}

#[test]
fn nothing_about_the_ring_survives_a_mount() {
    if !library_ships() {
        return;
    }
    let scene = block("SCENE_CLIENT_JS");
    assert!(
        !scene.contains("__displaySceneSeen"),
        "the ring's memory still lives on `window`, across mounts -- § 3.2: no \
         state in the browser carries meaning but the dock, and § 4.33 has no \
         seen memory at all"
    );
    assert!(
        scene.contains("var seen = { id: \\\"\\\", since: 0, last: 0 }"),
        "the beat's bookkeeping is not a fresh object of this mount: it belongs \
         to the hook (`this.__scene`), where a reconnect starts it again"
    );
    assert!(
        scene.contains("seen: seen"),
        "the hook does not carry `seen` in `this.__scene`"
    );
}
