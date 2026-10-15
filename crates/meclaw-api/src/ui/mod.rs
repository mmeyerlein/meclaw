//! Phase 12-D: server-rendered operator UI (`maud` HTML).
//!
//! Symmetric to the phase-12-B JSON layer: the same `ColonyMsg::Read*` inbox
//! variants, a different representation. No JS, no auto-refresh, no mutation
//! forms, no WebSocket. A read-only operator surface.
//!
//! Shared helpers (`style_block`, `layout`) live here so the dashboard and the
//! entity pages look consistent.

pub mod dashboard;
pub mod dead_letters;
pub mod errors;
pub mod graph;
pub mod message;
pub mod message_render;
pub mod messages;
pub mod registry;
pub mod templates;
pub mod trace;

use maud::{DOCTYPE, Markup, html};

/// Inline `<style>` block — ~30 lines, plain CSS, no framework.
///
/// Applies identically to every `/ui/*` page. Pragmatic: readable, functional,
/// no glossy layouts. An operator UI, not marketing.
pub(crate) fn style_block() -> Markup {
    html! {
        style {
            r#"
            body { font-family: system-ui, sans-serif; max-width: 1100px; margin: 1em auto; padding: 0 1em; color: #222; }
            h1, h2 { color: #1a4f8c; }
            nav { background: #f0f4f8; padding: 0.5em 1em; margin-bottom: 1em; }
            nav a { margin-right: 1em; color: #1a4f8c; text-decoration: none; }
            nav a:hover { text-decoration: underline; }
            table { border-collapse: collapse; width: 100%; }
            th, td { border: 1px solid #ccc; padding: 0.4em 0.6em; text-align: left; vertical-align: top; }
            th { background: #f4f7fa; }
            tr:nth-child(even) { background: #f9fafc; }
            .empty { color: #888; font-style: italic; }
            .error { color: #b00; }
            footer { color: #888; font-size: 0.9em; margin-top: 2em; border-top: 1px solid #ddd; padding-top: 0.5em; }
            form { margin: 0.5em 0; }
            input, select { padding: 0.3em 0.5em; margin-right: 0.5em; }
            ul.tree { list-style-type: none; padding-left: 1em; border-left: 1px dashed #ccc; }
            code { background: #f4f7fa; padding: 0.1em 0.3em; border-radius: 2px; font-size: 0.9em; }
            "#
        }
    }
}

/// Shared page layout: doctype + head + nav + main + footer.
///
/// `title` renders into `<title>` (with a `" — meclaw"` suffix) and as the `<h1>`.
/// `content` is the page body below the `<h1>`.
pub(crate) fn layout(title: &str, content: Markup) -> Markup {
    html! {
        (DOCTYPE)
        html {
            head {
                meta charset="utf-8";
                title { (title) " — meclaw" }
                (style_block())
            }
            body {
                nav {
                    a href="/ui/" { "Dashboard" }
                    a href="/ui/registry" { "Registry" }
                    a href="/ui/graph" { "Graph" }
                    a href="/ui/dead_letters" { "Dead Letters" }
                    a href="/ui/messages" { "Messages" }
                    a href="/ui/trace" { "Trace" }
                    a href="/ui/templates" { "Templates" }
                }
                main {
                    h1 { (title) }
                    (content)
                }
                footer {
                    "meclaw operator UI · server-rendered, no JS, no auto-refresh"
                }
            }
        }
    }
}

/// The count a heading shows for one read of `asked` rows that returned `got`: a full
/// read is a window of a longer list, never its size, so it reads `got+` ("at least").
/// One read of the dead-letter queue or the trace has no cursor to read on.
pub(crate) fn count_of(got: usize, asked: usize) -> String {
    if got >= asked {
        format!("{got}+")
    } else {
        got.to_string()
    }
}

#[cfg(test)]
mod count_tests {
    use super::count_of;

    #[test]
    fn a_full_read_is_counted_as_at_least() {
        assert_eq!(count_of(0, 100), "0");
        assert_eq!(count_of(99, 100), "99");
        assert_eq!(count_of(100, 100), "100+");
    }

    /// Headings that show the size of a whole, not of a read: the graph is one snapshot
    /// of the colony's topology, read without a limit.
    const WHOLE: &[(&str, &str)] = &[
        ("graph.rs", "reply.nodes.len()"),
        ("graph.rs", "reply.edges.len()"),
    ];

    /// The `.len()` counts a heading of `src` shows bare -- not through `count_of` -- as
    /// the expression before `.len()`.
    fn bare_counts(src: &str) -> Vec<String> {
        let mut out = Vec::new();
        for line in src.lines() {
            let t = line.trim_start();
            if !(t.starts_with("h1 {") || t.starts_with("h2 {") || t.starts_with("h3 {")) {
                continue;
            }
            for (at, _) in line.match_indices(".len()") {
                let head = &line[..at];
                let start = head.rfind('(').map_or(0, |i| i + 1);
                if head[..start].ends_with("count_of(") {
                    continue;
                }
                out.push(format!("{}.len()", &head[start..]));
            }
        }
        out
    }

    /// tv-bn M7b18 A-M3 (K8 "a reader takes one page for the whole", in the UI): every
    /// heading that counts the rows of one read goes through `count_of`, so a full read
    /// shows `N+`, never `N`. A scan of every page under `src/ui/`, not a list of them.
    #[test]
    fn no_heading_counts_one_read_as_the_whole() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/ui");
        let mut bad = Vec::new();
        let mut scanned = 0;
        for entry in std::fs::read_dir(&dir).expect("src/ui reads") {
            let path = entry.expect("an entry").path();
            if path.extension().is_none_or(|e| e != "rs") {
                continue;
            }
            scanned += 1;
            let name = path
                .file_name()
                .expect("a name")
                .to_string_lossy()
                .into_owned();
            let src = std::fs::read_to_string(&path).expect("a page reads");
            for count in bare_counts(&src) {
                if !WHOLE.iter().any(|(f, c)| *f == name && *c == count) {
                    bad.push(format!("{name}: {count}"));
                }
            }
        }
        assert!(scanned >= 8, "the scan saw the pages: {scanned}");
        bad.sort();
        assert!(
            bad.is_empty(),
            "a heading counts one read as the whole: {bad:?}"
        );
    }

    #[test]
    fn the_heading_scan_finds_a_bare_count() {
        let src = "        h2 { \"Cells (\" (reply.entries.len()) \")\" }\n        \
                   h2 { \"Messages (\" (count_of(reply.entries.len(), 100)) \")\" }\n        \
                   p { (rows.len()) }\n";
        assert_eq!(bare_counts(src), ["reply.entries.len()"]);
    }

    /// The colony handler that answers each `ColonyMsg::Read*` a counted page sends, and
    /// the expression it clamps.
    const ANSWERED_BY: &[(&str, &str, &str)] = &[
        ("ReadDeadLetters", "handle_read_dead_letters", "limit"),
        ("ReadMessages", "handle_read_messages", "filter.limit"),
        ("ReadRegistry", "handle_read_registry", "limit"),
        ("ReadTemplates", "handle_read_templates_from_rows", "limit"),
        ("ReadTrace", "handle_read_trace", "limit"),
    ];

    /// The `N` of `<expr>.clamp(1, N)` in the body of `fn <handler>(` -- the reader of
    /// `the_pages_are_the_clamps_of_the_colony` (meclaw-testing), fail-closed.
    fn clamp_of(src: &str, handler: &str, expr: &str) -> usize {
        let at = src
            .find(&format!("fn {handler}("))
            .unwrap_or_else(|| panic!("{handler} stands in colony_dispatch.rs"));
        let body = &src[at..];
        let body = &body[..body.find("\n}\n").expect("the handler ends")];
        let needle = format!("{expr}.clamp(1, ");
        let from = body
            .find(&needle)
            .unwrap_or_else(|| panic!("{handler} clamps {expr}"))
            + needle.len();
        let n: String = body[from..]
            .chars()
            .take_while(|c| c.is_ascii_digit() || *c == '_')
            .filter(|c| *c != '_')
            .collect();
        n.parse().expect("the clamp is a number")
    }

    /// The handlers of `dispatch` that clamp below the most a page can ask
    /// (`clamp_limit`'s cap): there a full read counts `N`, not `N+`.
    fn clamps_below_the_page(dispatch: &str) -> Vec<String> {
        let page = crate::handlers::clamp_limit(Some(usize::MAX));
        ANSWERED_BY
            .iter()
            .filter(|(_, h, e)| clamp_of(dispatch, h, e) < page)
            .map(|(_, h, _)| (*h).to_string())
            .collect()
    }

    fn dispatch_source() -> String {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        std::fs::read_to_string(root.join("../meclaw-colony/src/colony_dispatch.rs"))
            .expect("colony_dispatch.rs reads")
    }

    /// The second argument of each `count_of(` call in `src`, with the call's offset.
    fn asked_args(src: &str) -> Vec<(usize, String)> {
        let mut out = Vec::new();
        for (at, _) in src.match_indices("count_of(") {
            if src[..at].ends_with("fn ") {
                continue;
            }
            let args = &src[at + "count_of(".len()..];
            let (mut depth, mut comma, mut end) = (0usize, None, None);
            for (i, c) in args.char_indices() {
                match c {
                    '(' | '[' | '{' => depth += 1,
                    ')' if depth == 0 => {
                        end = Some(i);
                        break;
                    }
                    ')' | ']' | '}' => depth = depth.saturating_sub(1),
                    ',' if depth == 0 => comma = Some(i),
                    _ => {}
                }
            }
            let end = end.expect("a count_of call closes");
            let from = comma.map_or(end, |c| c + 1);
            out.push((at, args[from..end].trim().to_string()));
        }
        out
    }

    /// `expr` is one call of `clamp_limit` and nothing besides: its parenthesis closes on
    /// the last character (`clamp_limit(a) + 1` and `clamp_limit(a) * clamp_limit(b)` are not).
    fn is_one_clamp(expr: &str) -> bool {
        let Some(args) = expr.strip_prefix("clamp_limit(") else {
            return false;
        };
        let mut depth = 0usize;
        for (i, c) in args.char_indices() {
            match c {
                '(' => depth += 1,
                ')' if depth == 0 => return i + 1 == args.len(),
                ')' => depth -= 1,
                _ => {}
            }
        }
        false
    }

    /// What the `count_of` call at `at` of `src` asks, resolved in the page's own source,
    /// fail-closed: `clamp_limit(..)` asks the page, so does a name whose last `let` in the
    /// enclosing `fn` before the call binds `clamp_limit(..)`; a `const NAME: usize = N;`
    /// asks `N`. Anything else -- a literal, an unclamped `let`, a name from elsewhere -- is
    /// unknown (`None`).
    fn asked_of(src: &str, at: usize, arg: &str, page: usize) -> Option<usize> {
        if is_one_clamp(arg) {
            return Some(page);
        }
        if arg.is_empty() || !arg.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            return None;
        }
        let fn_at = src[..at].rfind("fn ").unwrap_or(0);
        let binding = format!("let {arg} = ");
        if let Some(b) = src[fn_at..at].rfind(&binding) {
            let bound = &src[fn_at + b + binding.len()..at];
            let bound = &bound[..bound.find(';')?];
            return is_one_clamp(bound.trim()).then_some(page);
        }
        let decl = format!("const {arg}: usize = ");
        let mut decls = src.match_indices(&decl);
        let (d, _) = decls.next()?;
        if decls.next().is_some() {
            return None;
        }
        let rest = &src[d + decl.len()..];
        let value = &rest[..rest.find(';')?];
        if value.is_empty() || !value.chars().all(|c| c.is_ascii_digit() || c == '_') {
            return None;
        }
        value.replace('_', "").parse().ok()
    }

    /// The counts of page `name` (source `src`) whose asked rows are unknown or above the
    /// page (`clamp_limit`'s cap, which every colony clamp holds): there a full read of
    /// the colony's most counts `N`, not `N+` (re-check N, M6).
    fn asks_beyond_the_page(name: &str, src: &str, page: usize) -> Vec<String> {
        asked_args(src)
            .into_iter()
            .filter_map(|(at, arg)| match asked_of(src, at, &arg, page) {
                Some(n) if n <= page => None,
                Some(n) => Some(format!("{name}: {arg} = {n} > {page}")),
                None => Some(format!("{name}: {arg} unknown")),
            })
            .collect()
    }

    /// tv-bn re-check bundle N M2 (OR-T11-22 in the UI): `count_of(len, clamp_limit(..))`
    /// holds only while the colony never clamps a read below `clamp_limit`'s cap. Every
    /// `ColonyMsg::Read*` a counted page sends (a scan of `src/ui/`) is answered by a
    /// handler of `ANSWERED_BY`, and each of them clamps at least the page. And every
    /// count asks at most the page (re-check N, M6): the second argument of each
    /// `count_of` resolves in the page's source -- a page constant such as
    /// `DASHBOARD_READ` or `CHILDREN_LIMIT` above the page would read "1000", not "1000+".
    #[test]
    fn every_counted_read_clamps_at_least_the_page() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/ui");
        let page = crate::handlers::clamp_limit(Some(usize::MAX));
        let (mut unknown, mut seen) = (Vec::new(), std::collections::BTreeSet::new());
        let (mut beyond, mut counts) = (Vec::new(), 0);
        for entry in std::fs::read_dir(&dir).expect("src/ui reads") {
            let path = entry.expect("an entry").path();
            let src = std::fs::read_to_string(&path).unwrap_or_default();
            if path.file_name().is_some_and(|n| n == "mod.rs") || !src.contains("count_of(") {
                continue;
            }
            let name = path.file_name().expect("a name").to_string_lossy();
            counts += asked_args(&src).len();
            beyond.extend(asks_beyond_the_page(&name, &src, page));
            // A read the page builds (`ColonyMsg::ReadX {`), not one its prose names.
            for (at, _) in src.match_indices("ColonyMsg::Read") {
                let v: String = src[at + "ColonyMsg::".len()..]
                    .chars()
                    .take_while(|c| c.is_ascii_alphanumeric())
                    .collect();
                if !src[at + "ColonyMsg::".len() + v.len()..].starts_with(" {") {
                    continue;
                }
                if ANSWERED_BY.iter().any(|(r, _, _)| *r == v) {
                    seen.insert(v);
                } else {
                    unknown.push(format!("{}: {v}", path.display()));
                }
            }
        }
        assert!(
            unknown.is_empty(),
            "a counted read with no known clamp: {unknown:?}"
        );
        assert_eq!(
            seen.len(),
            ANSWERED_BY.len(),
            "the scan saw every read: {seen:?}"
        );
        let below = clamps_below_the_page(&dispatch_source());
        assert!(below.is_empty(), "a colony clamp below the page: {below:?}");
        assert!(counts >= 9, "the scan saw the counts: {counts}");
        beyond.sort();
        assert!(
            beyond.is_empty(),
            "a count asks beyond the page: {beyond:?}"
        );
    }

    /// The bite of M6: each page constant a count asks, raised above the page in the real
    /// source, is red; so is every form of the second argument the pin cannot resolve.
    #[test]
    fn a_count_beyond_the_page_is_red() {
        let ui = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/ui");
        let page = crate::handlers::clamp_limit(Some(usize::MAX));
        for (file, constant) in [
            ("dashboard.rs", "DASHBOARD_READ"),
            ("message.rs", "CHILDREN_LIMIT"),
        ] {
            let src = std::fs::read_to_string(ui.join(file)).expect("the page reads");
            assert!(asks_beyond_the_page(file, &src, page).is_empty(), "{file}");
            let decl = format!("const {constant}: usize = ");
            let at = src.find(&decl).expect("the constant") + decl.len();
            let end = at + src[at..].find(';').expect("the constant ends");
            for raised in [(page + 1).to_string(), "2000".into(), "2_000".into()] {
                let mutant = format!("{}{raised}{}", &src[..at], &src[end..]);
                let found = asks_beyond_the_page(file, &mutant, page);
                assert!(!found.is_empty(), "{file}: {constant} = {raised}");
                assert!(found.iter().all(|f| f.contains(constant)), "{found:?}");
            }
            assert_eq!(
                asks_beyond_the_page(file, &src.replacen(&decl, "const OTHER: usize = ", 1), page)
                    .len(),
                src.matches("count_of(").count(),
                "{file}: a constant from elsewhere is unknown"
            );
        }
        let fns = |body: &str| format!("fn page() {{\n{body}\n}}\n");
        for (form, src) in [
            ("a literal", fns("h2 { (count_of(rows.len(), 2000)) }")),
            (
                "an unclamped let",
                fns("let limit = 2000;\nh2 { (count_of(rows.len(), limit)) }"),
            ),
            (
                "a let shadowed after the clamp",
                fns("let limit = clamp_limit(q.limit);\nlet limit = 2000;\n(count_of(n, limit))"),
            ),
            (
                "a clamp in another fn",
                format!(
                    "fn other() {{ let limit = clamp_limit(q.limit); }}\n{}",
                    fns("(count_of(n, limit))")
                ),
            ),
            ("a sum", fns("(count_of(n, clamp_limit(q.limit) + 1))")),
            (
                "two clamps",
                fns("(count_of(n, clamp_limit(a) * clamp_limit(b)))"),
            ),
            (
                "a let of a clamp times",
                fns("let limit = clamp_limit(q.limit) * 3;\n(count_of(n, limit))"),
            ),
            (
                "a constant declared twice",
                format!(
                    "const N: usize = 10;\nconst N: usize = 2000;\n{}",
                    fns("(count_of(n, N))")
                ),
            ),
        ] {
            assert_eq!(asks_beyond_the_page("p.rs", &src, page).len(), 1, "{form}");
        }
        let fine = format!(
            "const N: usize = 1_000;\n{}",
            fns("let limit = clamp_limit(q.limit);\n(count_of(a, limit)) (count_of(b, N))")
        );
        assert!(asks_beyond_the_page("p.rs", &fine, page).is_empty());
    }

    /// The product half of a page: everything before its test module.
    fn product(src: &str) -> &str {
        src.find("#[cfg(test)]").map_or(src, |at| &src[..at])
    }

    /// One way a page names a limit, receiver aside: `clamp_limit(self.limit)` in the
    /// query's own method is the `clamp_limit(q.limit)` its handler counts.
    fn norm(expr: &str) -> String {
        expr.trim().replace("self.", "q.")
    }

    /// One `limit` a read sends: the index of its line and the sent expression.
    type SentLimit = (usize, String);

    /// The `limit` each read of `src` sends, one entry per send, with its line: the field
    /// of a struct literal (`limit: X,`, or the shorthand `limit,`), field declarations
    /// (`pub limit:`) aside. A lookup of one row by its key -- `id: Some(..)` in the same
    /// literal -- counts nothing and comes back apart (second list).
    fn sent_limits(src: &str) -> (Vec<SentLimit>, Vec<SentLimit>) {
        let (mut sent, mut lookups, mut opened) = (Vec::new(), Vec::new(), 0usize);
        let lines: Vec<&str> = product(src).lines().collect();
        for (i, line) in lines.iter().enumerate() {
            let t = line.trim();
            if t.ends_with('{') {
                opened = i;
            }
            let expr = if t == "limit," {
                "limit".to_string()
            } else if let Some(rest) = t.strip_prefix("limit: ") {
                // Fail-closed: a send that does not end its line is kept whole and
                // matches no count.
                rest.strip_suffix(',').unwrap_or(rest).to_string()
            } else {
                continue;
            };
            let keyed = lines[opened..i]
                .iter()
                .any(|l| l.trim_start().starts_with("id: Some("));
            if keyed {
                lookups.push((i, norm(&expr)));
            } else {
                sent.push((i, norm(&expr)));
            }
        }
        (sent, lookups)
    }

    /// N-R3 (re-check N, delta N2): a count reads "N+" only when its read was asked for
    /// the `N` it counts against. Every count of page `name` asks a limit one of its
    /// reads sends, and every read it sends is one its counts ask -- written the same
    /// way, so a read that fetches fewer rows than its heading divides by is a finding.
    fn sends_other_than_it_counts(name: &str, src: &str) -> Vec<String> {
        let (sent, _) = sent_limits(src);
        let sent: Vec<String> = sent.into_iter().map(|(_, e)| e).collect();
        let asked: Vec<String> = asked_args(product(src))
            .into_iter()
            .map(|(_, a)| norm(&a))
            .collect();
        let mut found = Vec::new();
        for a in &asked {
            if !sent.contains(a) {
                found.push(format!("{name}: counts against {a}, sends {sent:?}"));
            }
        }
        for e in &sent {
            if !asked.contains(e) {
                found.push(format!("{name}: sends {e}, counts against {asked:?}"));
            }
        }
        found
    }

    /// The counted pages of `src/ui/` with their source.
    fn counted_pages() -> Vec<(String, String)> {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/ui");
        let mut pages = Vec::new();
        for entry in std::fs::read_dir(&dir).expect("src/ui reads") {
            let path = entry.expect("an entry").path();
            let src = std::fs::read_to_string(&path).unwrap_or_default();
            if path.file_name().is_some_and(|n| n == "mod.rs") || !src.contains("count_of(") {
                continue;
            }
            let name = path
                .file_name()
                .expect("a name")
                .to_string_lossy()
                .to_string();
            pages.push((name, src));
        }
        pages.sort();
        pages
    }

    /// N-R3: what a page counts against is what it sent (`asked == limit`).
    #[test]
    fn every_count_asks_what_its_page_sends() {
        let pages = counted_pages();
        assert!(
            pages.len() >= 7,
            "the scan saw the counted pages: {}",
            pages.len()
        );
        let (mut found, mut sends, mut lookups) = (Vec::new(), 0, 0);
        for (name, src) in &pages {
            let (sent, keyed) = sent_limits(src);
            sends += sent.len();
            lookups += keyed.len();
            found.extend(sends_other_than_it_counts(name, src));
        }
        assert!(
            found.is_empty(),
            "a count is not what its read sent: {found:?}"
        );
        // dashboard 3, dead_letters/messages/registry/templates/trace 1, message 1 + its lookup.
        assert!(sends >= 9, "the scan saw the sends: {sends}");
        assert_eq!(
            lookups, 1,
            "only message.rs's read of one row by its key is a lookup"
        );
    }

    /// The bite of N-R3: each send of each counted page, set to another limit in the real
    /// source, is red -- and so is a count that asks another limit than the one sent.
    #[test]
    fn a_read_that_sends_another_limit_is_red() {
        let mut bitten = 0;
        for (name, src) in counted_pages() {
            let (sent, _) = sent_limits(&src);
            let lines: Vec<&str> = src.lines().collect();
            for (i, expr) in &sent {
                let line = lines[*i];
                let indent = &line[..line.len() - line.trim_start().len()];
                let mut mutant: Vec<String> = lines.iter().map(|l| l.to_string()).collect();
                mutant[*i] = format!("{indent}limit: 50,");
                let found = sends_other_than_it_counts(&name, &mutant.join("\n"));
                assert!(
                    !found.is_empty(),
                    "{name}:{}: limit {expr} sent as 50",
                    i + 1
                );
                bitten += 1;
            }
            // The count's side: its asked limit swapped for another one.
            let (at, arg) = asked_args(&src).into_iter().next().expect("a count");
            let call = &src[at..];
            let off = at + call.find(arg.as_str()).expect("the argument");
            let mutant = format!("{}DASHBOARD_ELSE{}", &src[..off], &src[off + arg.len()..]);
            assert!(
                !sends_other_than_it_counts(&name, &mutant).is_empty(),
                "{name}: a count against another limit than the one sent"
            );
        }
        assert!(bitten >= 9, "every send was bitten: {bitten}");
    }

    /// The bite: each handler's clamp lowered by one in the real source is red.
    #[test]
    fn a_lowered_clamp_is_red() {
        let src = dispatch_source();
        let page = crate::handlers::clamp_limit(Some(usize::MAX));
        for (_, handler, expr) in ANSWERED_BY {
            let at = src.find(&format!("fn {handler}(")).expect("the handler");
            let clamp = format!("{expr}.clamp(1, {page})");
            let off = at + src[at..].find(&clamp).expect("the clamp");
            let lowered = format!("{expr}.clamp(1, {})", page - 1);
            let mutant = format!("{}{lowered}{}", &src[..off], &src[off + clamp.len()..]);
            assert_eq!(clamps_below_the_page(&mutant), [*handler], "{handler}");
        }
    }
}
