//! GH #1209 (OR-T12-7, OR-T12-12) -- the timer warms on the compositor and looks as the
//! colour animation it replaced looked.
//!
//! The bar and the digits used to animate `background-color` and `color` over the
//! timer's whole length -- a repaint every frame. Since M12i each is a crossfade of two
//! layers (calm fades out, warm fades in, `plus-lighter` in an isolated group), which
//! only the compositor moves. This pin draws the timer at 50 %, 90 %, 97 % and done,
//! once on the sheet that ships and once with [`REFERENCE`] beside it -- the colour
//! animation of `cc3e03a92`, restored -- and compares the two pictures pixel by pixel
//! in Chromium AND WebKit, in time frozen on the point
//! (`workshop/tools/display-timer-warmth-browser.mjs`).
//!
//! **The measure is the product's page** (owner ruling OR-T12-12): the card stands in
//! `.display-columns`, the container the product draws every card in, above the
//! drifting light. A card straight in `<body>` paints below that layer and Chromium
//! rasters its text with LCD subpixels there -- a page the product never shows
//! (tv-bn M12i, `bare`: up to 116 levels from the text raster alone).
//!
//! **Why a tolerance and not equality** ([`MAX_DELTA`]): the crossfade is the colour
//! mix in exact arithmetic, but not bit for bit. The compositor blends the two layers
//! through an 8-bit intermediate surface (the isolated group), which quantises each
//! step -- measured ±1-2 at the resting points; and Skia weights a glyph edge's
//! coverage by the luminance of the text colour, so two layers in two colours
//! antialias a partly covered pixel slightly differently from one glyph in their mix
//! -- measured up to 7 in Chromium at 90 % on the digits, up to 3 in WebKit (tv-bn
//! M12i, three runs, noise of a page against itself 0). Neither is reachable from CSS
//! without painting the text again on every frame, which is what the crossfade
//! removed. 8 is the bound that holds both and stays far below every real fault:
//! a warm layer only laid OVER the calm text (the M12h form) differs by 51-53, a
//! wrong colour by tens to hundreds.
//!
//! **One named exception: the drain's edge column.** The bar drains by `scaleX`; where
//! its right edge falls inside a pixel, the crossfade group (composited layers) covers
//! that pixel partly while the single-colour fill of the reference is snapped to it --
//! measured on the product's container at 90 %: ONE column, x = 45 at a box edge of
//! 45.x, up to 30 levels (tv-bn M12j). That is where the edge stands, not what colour
//! the bar has, and it moves with every frame of the drain. A pixel above the bound is
//! therefore allowed only in ONE pixel column -- the one where the two pictures' drawn
//! right edges disagree most (measured from the pixels by the driver, `edge` and
//! `reference_edge`; never a fixed column) -- and inside the bar's height (OR-T12-24).
//! The exception is only honest if the edge stands where it should: the pin holds both
//! drawn edges within [`MAX_EDGE_SHIFT`] of each other AND of the drain's own geometry
//! (track left + width x (1 - progress)), so a drain whose edge moved by two pixels is
//! red, wherever its colour is.
//!
//! **Why SKIP and not RED** -- the tool guard every browser proof in this tree uses:
//! without `playwright`, `node` or `python3` the driver says `SKIP`; under
//! `MECLAW_REQUIRE_BROWSERS=1` (the gate booked the engine) a SKIP is red.

use std::process::Command;

use meclaw_core::serde_json::Value;

fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

const COMPOSE: &str = "templates/display/compose/compose.py";
const DRIVER: &str = "workshop/tools/display-timer-warmth-browser.mjs";
/// The laboratory WebKit runs in on this host, sourced for every run except a Chromium
/// one (the same guard as every two-engine browser proof here).
const WKENV: &str = "workshop/tools/wkenv.sh";
const WITH_LAB: &str =
    "case \" $* \" in *\" --engine chromium \"*) ;; *) . \"$1\" ;; esac; shift; exec node \"$@\"";

/// The largest channel difference a pixel of the shipped timer may have against the
/// reference, per channel 0-255 (see the module doc: 8-bit intermediate surface of the
/// crossfade group, colour-weighted glyph antialiasing; measured max 7).
const MAX_DELTA: u64 = 8;

/// How far the drawn right edge of the draining bar may stand from the reference's, in
/// pixels: the reference snaps its edge to the pixel grid, the crossfade covers the edge
/// pixel partly (the named exception above) -- less than one pixel apart by construction.
const MAX_EDGE_SHIFT: f64 = 1.0;

/// The colour animation of `cc3e03a92`, restored on top of the shipped sheet: the
/// layers are gone, the bar animates `background-color` and the digits `color` with
/// the old keyframes. The variables (`--accent`, `--accent-soft`, `--fg-primary`) are
/// the shipped ones, so the pin compares the mechanism, not the palette.
const REFERENCE: &str = r#"
.display-timer-fill {
  isolation: auto;
  animation:
    display-timer-drain calc(var(--total, 0) * 1ms) linear calc(var(--elapsed, 0) * -1ms) both,
    display-timer-ref-heat calc(var(--total, 0) * 1ms) linear calc(var(--elapsed, 0) * -1ms) both;
}
.display-timer-remaining {
  position: static;
  isolation: auto;
  -webkit-text-fill-color: currentColor;
  animation: display-timer-ref-alarm calc(var(--total, 0) * 1ms) linear
    calc(var(--elapsed, 0) * -1ms) both;
}
:is(.display-timer-fill, .display-timer-remaining)::before,
:is(.display-timer-fill, .display-timer-remaining)::after { display: none; animation: none; }
@keyframes display-timer-ref-heat {
  0%, 82% { background-color: var(--accent-soft); }
  92%, 100% { background-color: var(--accent); }
}
@keyframes display-timer-ref-alarm {
  0%, 88% { color: var(--fg-primary); }
  96%, 100% { color: var(--accent); }
}
.display-timer--done .display-timer-fill { animation: none; background-color: var(--accent); }
.display-timer--done .display-timer-remaining { animation: none; color: var(--accent); }
"#;

/// The sheet and the timer at each point as `compose.py` ships them; `None` without python3.
fn write_parts(dir: &std::path::Path) -> Option<()> {
    let out = Command::new("python3")
        .arg("-c")
        .arg(
            "import importlib.util, re, sys\n\
             spec = importlib.util.spec_from_file_location('compose', sys.argv[1])\n\
             m = importlib.util.module_from_spec(spec)\n\
             spec.loader.exec_module(m)\n\
             d = sys.argv[2]\n\
             open(d + '/sheet.css', 'w').write(m.LAYOUT_RULES + m.KIT_CSS)\n\
             def fill(t, **v):\n\
             \x20   t = re.sub(r'\\{\\{#if (\\w+)\\}\\}(.*?)\\{\\{/if\\}\\}', lambda g: g.group(2) if v.get(g.group(1)) else '', t)\n\
             \x20   return re.sub(r'\\{\\{(\\w+)\\}\\}', lambda g: str(v.get(g.group(1), '')), t)\n\
             end, total = 10**13, 7200000\n\
             for name, frac, done in [('p50', .5, 0), ('p90', .9, 0), ('p97', .97, 0), ('done', 1, 1)]:\n\
             \x20   html = fill(m.TIMER_TEMPLATE, end_at=end, total_ms=total, now=int(end - total + frac * total), remaining='01:23', label='a timer', done=done)\n\
             \x20   open(d + '/timer-' + name + '.html', 'w').write(html)\n",
        )
        .arg(repo(COMPOSE))
        .arg(dir)
        .output()
        .ok()?;
    assert!(
        out.status.success(),
        "compose.py did not answer:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    std::fs::write(dir.join("reference.css"), REFERENCE).expect("reference.css");
    Some(())
}

fn skip(why: &str) {
    println!("SKIP {why}");
    assert_ne!(
        std::env::var("MECLAW_REQUIRE_BROWSERS").as_deref(),
        Ok("1"),
        "SKIP under the switch: {why}"
    );
}

/// The progress a frozen animation of `name` reports on `on`, from the driver's timing list.
fn progress(timing: &Value, name: &str, on: &str) -> Option<f64> {
    timing
        .as_array()?
        .iter()
        .find(|a| a["name"] == name && a["on"] == on)?["progress"]
        .as_f64()
}

fn the_timer_warms_as_the_colour_did_in(engine: &str) {
    if !repo("templates/display/template.json").is_file() {
        return skip("the template library does not ship in this tree");
    }
    if !repo(DRIVER).is_file() || !repo(WKENV).is_file() {
        return skip("the browser driver does not ship in this tree");
    }
    let td = tempfile::TempDir::new().expect("tempdir");
    if write_parts(td.path()).is_none() {
        return skip("no python3 on this host");
    }
    let out = match Command::new("sh")
        .arg("-c")
        .arg(WITH_LAB)
        .arg("sh")
        .arg(repo(WKENV))
        .arg(repo(DRIVER))
        .arg(td.path())
        .arg("--engine")
        .arg(engine)
        .arg("--bound")
        .arg(MAX_DELTA.to_string())
        .output()
    {
        Ok(out) => out,
        Err(e) => {
            return skip(&format!(
                "neither node nor a shell for it on this host: {e}"
            ));
        }
    };
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    if out.status.code() == Some(3) || stderr.lines().any(|l| l.starts_with("SKIP ")) {
        // the switch is read in this branch, not only in `skip`: the lock
        // (`test_browser_required`) reads each exit-3 branch up to its `return`
        let why = format!("{engine}: {}", stderr.trim());
        assert_ne!(
            std::env::var("MECLAW_REQUIRE_BROWSERS").as_deref(),
            Ok("1"),
            "SKIP under MECLAW_REQUIRE_BROWSERS=1 is red: {why}"
        );
        return skip(&why);
    }
    let line = stdout
        .lines()
        .find(|l| l.starts_with('{'))
        .unwrap_or_else(|| panic!("{engine}: the driver printed no report:\n{stdout}\n{stderr}"));
    let report: Value = meclaw_core::serde_json::from_str(line).expect("the report is JSON");

    // Every point in every tone a window's catalogue writes (§ M12k, M3): the colour
    // animation held the digits at `--fg-primary` over `accent` and `muted` alike, so a
    // tinted timer has to look as it did, not only the plain one.
    let keys = |base: &str| -> Vec<String> {
        ["", "accent-", "muted-"]
            .iter()
            .map(|t| format!("{t}{base}"))
            .collect()
    };
    // Time stood on the point: the warm layers report the moment the reference does.
    for (base, at) in [("p50", 0.5), ("p90", 0.9), ("p97", 0.97)] {
        for point in keys(base) {
            let point = point.as_str();
            let p = &report["points"][point];
            for (name, on) in [
                ("display-timer-heat", "display-timer-fill::after"),
                ("display-timer-alarm", "display-timer-remaining::after"),
                ("display-timer-ref-heat", "display-timer-fill"),
                ("display-timer-ref-alarm", "display-timer-remaining"),
            ] {
                // Before the shot and after it: `animations: "disabled"` finishes every
                // finite animation it does not find at rate 0 (the driver's head).
                let side = if name.contains("-ref-") {
                    "reference_timing"
                } else {
                    "timing"
                };
                for key in [side.to_string(), format!("{side}_after_shot")] {
                    let got = progress(&p[key.as_str()], name, on).unwrap_or_else(|| {
                        panic!("{engine} {point}: no frozen `{name}` on `{on}` in `{key}`:\n{p:#}")
                    });
                    assert!(
                        (got - at).abs() < 1e-6,
                        "{engine} {point} ({key}): `{name}` on `{on}` stands at {got}, not {at} -- time is not frozen on the point"
                    );
                }
            }
        }
    }
    // The comparison has teeth: the reference itself warms between 50 % and 97 %/done.
    for point in ["p97", "done"] {
        let w = &report["warms"][point];
        assert!(
            w["max"].as_u64().unwrap_or(0) > 4 * MAX_DELTA,
            "{engine}: the reference at {point} hardly differs from 50 % ({w}) -- nothing warms, the pin proves nothing"
        );
    }
    let mut over = Vec::new();
    for point in ["p50", "p90", "p97", "done"].into_iter().flat_map(keys) {
        let point = point.as_str();
        let p = &report["points"][point];
        assert!(
            p.get("size").is_none(),
            "{engine} {point}: the two pictures differ in size: {p}"
        );
        let max = p["max"].as_u64().expect("max");
        let fill = &p["fill"];
        let measured = |key: &str| {
            p[key]["edge"].as_f64().unwrap_or_else(|| {
                panic!(
                    "{engine} {point}: the drawn edge of the bar is not measurable in `{key}` -- no edge, no exception: {}",
                    p[key]
                )
            })
        };
        let (edge, reference_edge) = (measured("edge"), measured("reference_edge"));
        assert!(
            (edge - reference_edge).abs() <= MAX_EDGE_SHIFT,
            "{engine} {point}: the bar ends at {edge:.2} px, the colour animation's at {reference_edge:.2} px -- the edge moved more than {MAX_EDGE_SHIFT} px"
        );
        // Where the drain puts the edge: the track's left plus its width times what is
        // left (`1 - p`, `p` the drain's progress read back; done stands full). Both
        // drawn edges are judged against it, so neither picture can carry the other.
        let p_drain = if point.ends_with("done") {
            0.0
        } else {
            progress(&p["timing"], "display-timer-drain", "display-timer-fill").unwrap_or_else(
                || panic!("{engine} {point}: the drain's progress was not read back"),
            )
        };
        let expected = fill["track_left"].as_f64().expect("track_left")
            + fill["track_width"].as_f64().expect("track_width") * (1.0 - p_drain);
        for (which, drawn) in [("shipped", edge), ("reference", reference_edge)] {
            assert!(
                (drawn - expected).abs() <= MAX_EDGE_SHIFT,
                "{engine} {point}: the {which} bar ends at {drawn:.2} px, the drain puts its edge at {expected:.2} px"
            );
        }
        // The one column the exception may cover: where the two drawn edges disagree
        // most. In one pixel that is that pixel; across a pixel boundary it is the side
        // with the larger share between them -- one column, derived from the two
        // measured edges, never a fixed one.
        let (lo, hi) = (edge.min(reference_edge), edge.max(reference_edge));
        let edge_col = if lo.floor() == hi.floor() || hi - hi.floor() >= hi.floor() - lo {
            hi.floor()
        } else {
            lo.floor()
        };
        let (top, bottom) = (
            fill["top"].as_f64().expect("top").floor(),
            fill["bottom"].as_f64().expect("bottom").ceil(),
        );
        assert_eq!(
            fill, &p["reference_fill"],
            "{engine} {point}: the bar is not drawn in the same box as the reference's"
        );
        let mut edge_column = 0;
        for px in p["over"].as_array().expect("over") {
            let (x, y, d) = (
                px[0].as_f64().unwrap(),
                px[1].as_f64().unwrap(),
                px[2].as_u64().unwrap(),
            );
            // Exactly the pixel [x, x+1) the drawn edge falls in: one column, one pixel wide.
            let at_edge = x == edge_col && top <= y && y < bottom;
            if at_edge {
                edge_column += 1;
            } else {
                over.push(format!("{point}: ({x}, {y}) differs by {d}"));
            }
        }
        if p["over"].as_array().map_or(0, Vec::len) >= 400 {
            over.push(format!("{point}: 400 or more pixels above the bound"));
        }
        println!(
            "{engine} {point}: {} px differ, max {max}, {edge_column} above the bound in the drain's edge column {edge_col} (drawn edge {edge:.2}, reference {reference_edge:.2})",
            p["px"]
        );
    }
    assert!(
        over.is_empty(),
        "{engine}: the timer does not look as the colour animation did (bound {MAX_DELTA} per channel, outside the drain's edge column): {over:?}\n{stderr}"
    );
}

#[test]
fn the_timer_warms_as_the_colour_did_in_chromium() {
    the_timer_warms_as_the_colour_did_in("chromium");
}

#[test]
fn the_timer_warms_as_the_colour_did_in_webkit() {
    the_timer_warms_as_the_colour_did_in("webkit");
}
