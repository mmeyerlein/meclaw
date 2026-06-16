//! Opaque path identifier used as the key in Colony's actor registry,
//! plus pure-string resolution primitives (`is_absolute`, `parent`,
//! `starts_with`, `resolve`) per `docs/meclaw-overview.md`
//! § Pfad-Adressierung.

use std::sync::Arc;

#[derive(Debug, Clone, Hash, Eq, PartialEq)]
pub struct Path(Arc<str>);

impl Path {
    pub fn new(s: &str) -> Self {
        Path(Arc::from(s))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Returns true if this path starts with `/` (i.e., is absolute).
    pub fn is_absolute(&self) -> bool {
        self.0.starts_with('/')
    }

    /// Returns true if this path's string form starts with `prefix`. Plain
    /// string-prefix check (mirrors `str::starts_with`). Segment-awareness, where
    /// needed, lives at the call site — see `colony::route()` for the
    /// `/colony` vs `/colonial` disambiguation.
    pub fn starts_with(&self, prefix: &str) -> bool {
        self.0.starts_with(prefix)
    }

    /// Resolve a target expression against a sender path. Pure string normalisation.
    ///
    /// Always returns a `Path`, never an error: invalid-looking inputs (empty target,
    /// `../` past root, bare name) normalise to a well-defined path per the
    /// `docs/meclaw-overview.md` § "Pfad-Resolution: Edge-Cases" table. Whether the
    /// resulting path is registered is a separate question answered by the registry
    /// lookup downstream.
    pub fn resolve(sender: &Path, target: &str) -> Path {
        if target.starts_with('/') {
            return normalize_segments(target);
        }
        // Strip leading "./" if present; bare name keeps no prefix.
        let trimmed = target.strip_prefix("./").unwrap_or(target);
        // "." alone or empty trailing means "stay at sender".
        if trimmed.is_empty() || trimmed == "." {
            return normalize_segments(sender.as_str());
        }
        let joined = if sender.as_str() == "/" {
            format!("/{trimmed}")
        } else {
            format!("{}/{}", sender.as_str(), trimmed)
        };
        normalize_segments(&joined)
    }

    /// Returns the parent path. `/a/b/c` → `/a/b`. `/` (root) stays `/` (Linux convention).
    pub fn parent(&self) -> Path {
        let s = self.0.as_ref();
        if s == "/" {
            return self.clone();
        }
        match s.rfind('/') {
            Some(0) => Path::new("/"),
            Some(idx) => Path::new(&s[..idx]),
            None => self.clone(),
        }
    }
}

/// Walk segments of an absolute path and produce a canonical form:
/// drop empty and `.` segments (collapsing multi-slashes and trailing slashes),
/// and pop on `..` with root-clamp (popping past `/` stays `/`, since
/// `Vec::pop` on an empty stack is a no-op). Always returns an absolute path.
fn normalize_segments(raw: &str) -> Path {
    let mut stack: Vec<&str> = Vec::new();
    for seg in raw.split('/') {
        match seg {
            "" | "." => continue,
            ".." => {
                // pop on empty stack = no-op = root-clamp per spec
                // § Pfad-Resolution: Edge-Cases (clamp on `/`).
                stack.pop();
            }
            other => stack.push(other),
        }
    }
    if stack.is_empty() {
        Path::new("/")
    } else {
        let mut out = String::new();
        for seg in &stack {
            out.push('/');
            out.push_str(seg);
        }
        Path::new(&out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn path_round_trips_through_hashmap() {
        let mut map: HashMap<Path, &'static str> = HashMap::new();
        map.insert(Path::new("/a/b"), "value");
        assert_eq!(map.get(&Path::new("/a/b")), Some(&"value"));
        assert_eq!(map.get(&Path::new("/a")), None);
    }

    #[test]
    fn path_clone_shares_inner_arc() {
        let p = Path::new("/x/y/z");
        let q = p.clone();
        assert_eq!(p.as_str(), q.as_str());
    }

    #[test]
    fn is_absolute_distinguishes_leading_slash() {
        assert!(Path::new("/").is_absolute());
        assert!(Path::new("/a").is_absolute());
        assert!(Path::new("/a/b/c").is_absolute());
        assert!(!Path::new("a").is_absolute());
        assert!(!Path::new("./a").is_absolute());
        assert!(!Path::new("../a").is_absolute());
        assert!(!Path::new("").is_absolute());
    }

    #[test]
    fn parent_returns_path_above_last_segment() {
        assert_eq!(Path::new("/a/b/c").parent().as_str(), "/a/b");
        assert_eq!(Path::new("/a").parent().as_str(), "/");
        // Clamp: parent of "/" stays "/".
        assert_eq!(Path::new("/").parent().as_str(), "/");
    }

    #[test]
    fn starts_with_checks_string_prefix() {
        assert!(Path::new("/colony/dead_letters").starts_with("/colony"));
        assert!(Path::new("/colony").starts_with("/colony"));
        assert!(!Path::new("/colonial/x").starts_with("/colony/"));
        assert!(!Path::new("/a/b").starts_with("/colony"));
    }

    #[test]
    fn resolve_returns_target_when_target_is_absolute() {
        let sender = Path::new("/main/agent");
        assert_eq!(
            Path::resolve(&sender, "/other/cell").as_str(),
            "/other/cell"
        );
        assert_eq!(
            Path::resolve(&sender, "/colony/dead_letters").as_str(),
            "/colony/dead_letters"
        );
        assert_eq!(Path::resolve(&sender, "/").as_str(), "/");
    }

    #[test]
    fn resolve_prepends_sender_for_relative_target_without_dotdot() {
        let sender = Path::new("/main/agent");
        // ./X form
        assert_eq!(
            Path::resolve(&sender, "./tool").as_str(),
            "/main/agent/tool"
        );
        // Bare name (no prefix) — treated as ./X per spec § Edge-Cases.
        assert_eq!(Path::resolve(&sender, "tool").as_str(), "/main/agent/tool");
        // "." alone → sender_path.
        assert_eq!(Path::resolve(&sender, ".").as_str(), "/main/agent");
        // "./" alone → sender_path.
        assert_eq!(Path::resolve(&sender, "./").as_str(), "/main/agent");
        // Multi-segment relative
        assert_eq!(Path::resolve(&sender, "./a/b").as_str(), "/main/agent/a/b");
        // Sender at root
        assert_eq!(Path::resolve(&Path::new("/"), "./cell").as_str(), "/cell");
        assert_eq!(Path::resolve(&Path::new("/"), "cell").as_str(), "/cell");
    }

    #[test]
    fn resolve_handles_dotdot_with_root_clamp() {
        let s = Path::new("/main/agent");
        // Single ..
        assert_eq!(
            Path::resolve(&s, "../collector").as_str(),
            "/main/collector"
        );
        // Multiple ..
        assert_eq!(Path::resolve(&s, "../../x").as_str(), "/x");
        // .. past root → clamp to /
        assert_eq!(Path::resolve(&Path::new("/a"), "../../x").as_str(), "/x");
        assert_eq!(Path::resolve(&Path::new("/"), "../x").as_str(), "/x");
        // .. that lands exactly at /
        assert_eq!(Path::resolve(&Path::new("/a/b"), "../..").as_str(), "/");
        // Mixed segments after ..
        assert_eq!(
            Path::resolve(&s, "../sibling/leaf").as_str(),
            "/main/sibling/leaf"
        );
    }

    #[test]
    fn resolve_normalises_trailing_slash_empty_and_multi_slash() {
        // Trailing slash → normalised out
        assert_eq!(Path::resolve(&Path::new("/main"), "/a/b/").as_str(), "/a/b");
        assert_eq!(
            Path::resolve(&Path::new("/main"), "./tool/").as_str(),
            "/main/tool"
        );
        // Multi-slash → collapsed
        assert_eq!(Path::resolve(&Path::new("/main"), "/a//b").as_str(), "/a/b");
        assert_eq!(Path::resolve(&Path::new("/main"), "//x").as_str(), "/x");
        // Empty target → sender (identical to ".")
        assert_eq!(
            Path::resolve(&Path::new("/main/agent"), "").as_str(),
            "/main/agent"
        );
        // Empty target with root sender stays at root
        assert_eq!(Path::resolve(&Path::new("/"), "").as_str(), "/");
    }
}
