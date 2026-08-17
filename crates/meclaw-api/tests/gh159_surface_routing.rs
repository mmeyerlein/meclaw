//! GH #159 — one wildcard route, one parser, two reserved words.
//!
//! `@` is ours. `live` is the phoenix client's: it appends exactly "/websocket"
//! to the URL it is handed (assets/js/phoenix/socket.js:191), so the suffix is
//! the bundle's and the prefix is ours. Both are reserved, so a cell named
//! `state` stays addressable and a cell named `live` cannot shadow a transport.

use meclaw_api::surface::{Target, parse_target};

#[test]
fn a_bare_path_is_the_page() {
    assert_eq!(
        parse_target("org/acme/canvy/render"),
        Some(Target::Page {
            cell: "org/acme/canvy/render".into()
        })
    );
}

#[test]
fn the_socket_sits_under_the_surface_prefix() {
    assert_eq!(
        parse_target("org/acme/canvy/render/live/websocket"),
        Some(Target::Socket {
            cell: "org/acme/canvy/render".into()
        })
    );
}

#[test]
fn a_surface_serves_its_own_files_under_its_own_prefix() {
    assert_eq!(
        parse_target("org/acme/canvy/render/@asset/canvy.js"),
        Some(Target::Asset {
            cell: "org/acme/canvy/render".into(),
            file: "canvy.js".into()
        })
    );
}

#[test]
fn the_bundles_have_their_own_reserved_prefix() {
    assert_eq!(
        parse_target("@client/phoenix.min.js"),
        Some(Target::Client {
            file: "phoenix.min.js".into()
        })
    );
}

/// One file name, no directories. A path that reaches a filesystem is the oldest
/// bug in web serving, so the shape is refused before any join.
#[test]
fn an_asset_is_one_plain_file_name() {
    for bad in [
        "org/acme/render/@asset/../../../etc/passwd",
        "org/acme/render/@asset/sub/dir.js",
        "org/acme/render/@asset/",
        "org/acme/render/@asset",
        "org/acme/render/@asset/.",
        "org/acme/render/@asset/..",
        "@client/../../etc/passwd",
        "@client/sub/dir.js",
        "@client/",
        "@client",
    ] {
        assert_eq!(parse_target(bad), None, "{bad:?}");
    }
}

#[test]
fn an_unknown_verb_is_a_miss() {
    for bad in [
        "org/acme/render/@state",
        "org/acme/render/@",
        "org/acme/render/@asse/x.js",
        "@state",
        "org/acme/render/live",
        "org/acme/render/live/socket",
        "org/acme/render/live/websocket/more",
    ] {
        assert_eq!(parse_target(bad), None, "{bad:?}");
    }
}

/// A cell named `state` keeps working, which is what reserving `@` buys.
#[test]
fn a_cell_named_state_is_still_reachable() {
    assert_eq!(
        parse_target("org/acme/state"),
        Some(Target::Page {
            cell: "org/acme/state".into()
        })
    );
}

/// A cell named `live` cannot shadow the transport: the suffix wins. `locate`
/// refuses a `live` segment as well, so the page 404s and the ambiguity has
/// exactly one resolution.
#[test]
fn a_cell_named_live_cannot_shadow_the_transport() {
    assert_eq!(
        parse_target("org/acme/live/websocket"),
        Some(Target::Socket {
            cell: "org/acme".into()
        })
    );
}

#[test]
fn an_empty_or_verb_only_path_is_a_miss() {
    for bad in ["", "/", "@", "//", "live/websocket"] {
        assert_eq!(parse_target(bad), None, "{bad:?}");
    }
}

/// A single-segment surface is legal: a colony may serve something that hangs
/// directly off the root, and `locate` decides whether it exists.
#[test]
fn a_single_segment_path_is_a_page() {
    assert_eq!(
        parse_target("dashboard"),
        Some(Target::Page {
            cell: "dashboard".into()
        })
    );
}
