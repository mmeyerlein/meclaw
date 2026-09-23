//! `meclaw --env-report` (GH #826): which `.env` keys does this colony's tree
//! substitute, and which `${…}` references does the `.env` not answer?
//!
//! The `.env` of a root is a substitution source and nothing else
//! (`meclaw_colony::env_file::load_env`; it never reaches the process
//! environment), and the boot substitutes the WHOLE parsed `config.json` of
//! every cell it walks — `params`, `script_inline`, `default`, `api_key`, all of
//! it. So the report asks the same question over the same span, with the
//! substrate's own scanner (`collect_env_keys`, GH #465) and the substrate's own
//! walk (`walk_cell_directories`), never with a pattern of its own.
//!
//! It reads files and nothing else: no root lease, no `colony.db`, no log — so
//! it answers for a colony that is running, which is the case it was measured
//! on. Names only: the parsed map is reduced to its keys the moment it is read,
//! so no value is held past `load_keys`.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use anyhow::Context as _;
use meclaw_colony::mutation::substitute::collect_env_keys;
use meclaw_colony::templates::{TemplateEntry, TemplatesRegistry, scan_templates_dir};

/// What `--env-report` found. Names and paths, never a value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnvReport {
    /// The `.env` file that was read (it may not exist: then it has no keys).
    pub env_path: PathBuf,
    /// How many keys that file carries.
    pub env_key_count: usize,
    /// How many distinct names the tree substitutes, defaulted ones included.
    pub used_count: usize,
    /// Keys in the `.env` that no `${…}` anywhere in the tree substitutes.
    pub unused: BTreeSet<String>,
    /// Strict `${KEY}` references (no default) whose key the `.env` does not
    /// carry, each with the cell directories that ask for it (root-relative
    /// where they lie under the root).
    pub missing: BTreeMap<String, BTreeSet<PathBuf>>,
    /// Things the report could not look into, e.g. a marker whose template the
    /// registry does not carry.
    pub notes: Vec<String>,
}

/// Read the `.env` and reduce it to its key set at once.
fn load_keys(path: &Path) -> anyhow::Result<BTreeSet<String>> {
    // `EnvFileError::Parse` names the line, never the text of it.
    let map = meclaw_colony::env_file::load_env(path)
        .with_context(|| format!("env report: {}", path.display()))?;
    Ok(map.into_keys().collect())
}

/// Is `k` shaped like a key (`[A-Za-z_][A-Za-z0-9_]*`)? A line of the `.env`
/// that is not `KEY=…` — the continuation of a multi-line value, say — still
/// splits at its first `=`, and what stands left of it is a fragment of a
/// VALUE. Such a "key" is counted, never printed (T7 review M1).
fn is_key_shaped(k: &str) -> bool {
    let mut chars = k.chars();
    matches!(chars.next(), Some(c) if c == '_' || c.is_ascii_alphabetic())
        && chars.all(|c| c == '_' || c.is_ascii_alphanumeric())
}

/// The first string under `v` with a `${` that never closes, as a JSON pointer
/// and the character position of that `${` in the string.
fn first_unterminated(v: &serde_json::Value, ptr: &str) -> Option<(String, usize)> {
    match v {
        serde_json::Value::String(s) => unterminated_at(s).map(|p| (ptr.to_string(), p)),
        serde_json::Value::Array(a) => a
            .iter()
            .enumerate()
            .find_map(|(i, x)| first_unterminated(x, &format!("{ptr}/{i}"))),
        serde_json::Value::Object(o) => o.iter().find_map(|(k, x)| {
            first_unterminated(
                x,
                &format!("{ptr}/{}", k.replace('~', "~0").replace('/', "~1")),
            )
        }),
        _ => None,
    }
}

fn unterminated_at(s: &str) -> Option<usize> {
    let mut from = 0;
    while let Some(i) = s[from..].find("${") {
        let at = from + i;
        match s[at..].find('}') {
            Some(j) => from = at + j + 1,
            None => return Some(s[..at].chars().count()),
        }
    }
    None
}

/// Accumulates what the walk sees.
#[derive(Default)]
struct Seen {
    used: BTreeSet<String>,
    strict: BTreeMap<String, BTreeSet<PathBuf>>,
    notes: Vec<String>,
}

impl Seen {
    fn read_config(&mut self, root: &Path, dir: &Path) -> anyhow::Result<Option<String>> {
        let file = dir.join("config.json");
        let raw = std::fs::read_to_string(&file)
            .with_context(|| format!("env report: read {}", file.display()))?;
        let value: serde_json::Value = serde_json::from_str(&raw)
            .with_context(|| format!("env report: parse {}", file.display()))?;
        // The substituter's error quotes the WHOLE string, which may be a
        // `script_inline` of many kilobytes. The report names where instead:
        // file, JSON pointer, position (T7 review M2).
        let names = collect_env_keys(&value).map_err(|_| match first_unterminated(&value, "") {
            Some((ptr, pos)) => anyhow::anyhow!(
                "env report: {}: {ptr}: the `${{` at character {pos} has no closing `}}`",
                file.display()
            ),
            None => anyhow::anyhow!(
                "env report: {}: a substitution token could not be read",
                file.display()
            ),
        })?;
        let shown = dir.strip_prefix(root).unwrap_or(dir).to_path_buf();
        for (name, defaulted) in names {
            self.used.insert(name.clone());
            if !defaulted {
                self.strict.entry(name).or_default().insert(shown.clone());
            }
        }
        // A `ref` marker grows a template at first boot; what it will
        // substitute is that template's tree, not the three lines of the marker.
        let is_ref = value
            .get("cell")
            .and_then(|c| c.get("type"))
            .and_then(|t| t.as_str())
            == Some("ref");
        Ok(if is_ref {
            value
                .get("cell")
                .and_then(|c| c.get("template"))
                .and_then(|t| t.as_str())
                .map(str::to_string)
        } else {
            None
        })
    }

    /// Every `config.json` of one directory tree (the directory itself
    /// included), following the markers it holds through `registry`.
    fn walk_tree(
        &mut self,
        root: &Path,
        top: &Path,
        registry: &TemplatesRegistry,
        visited: &mut BTreeSet<PathBuf>,
    ) -> anyhow::Result<()> {
        let mut dirs = Vec::new();
        if top.join("config.json").is_file() {
            dirs.push(top.to_path_buf());
        }
        dirs.extend(meclaw_colony::bootstrap::walk_cell_directories(top));
        for dir in dirs {
            if let Some(reference) = self.read_config(root, &dir)? {
                match registry.resolve(&reference) {
                    Ok(entry) => {
                        let path = entry.filesystem_path.clone();
                        if visited.insert(path.clone()) {
                            self.walk_tree(root, &path, registry, visited)?;
                        }
                    }
                    Err(e) => self.notes.push(format!(
                        "marker {} names {reference}, which the templates directory does not \
                         carry ({e}); its keys are not counted",
                        dir.strip_prefix(root).unwrap_or(&dir).display()
                    )),
                }
            }
        }
        Ok(())
    }
}

/// Build the report for the colony under `root`.
///
/// `env` defaults to `<root>/.env` and `templates` to `<root>/templates`, the
/// same defaults the boot uses.
pub fn build(
    root: &Path,
    env: Option<&Path>,
    templates: Option<&Path>,
) -> anyhow::Result<EnvReport> {
    let env_path = env
        .map(Path::to_path_buf)
        .unwrap_or_else(|| root.join(".env"));
    let (keys, unshaped): (BTreeSet<String>, BTreeSet<String>) = load_keys(&env_path)?
        .into_iter()
        .partition(|k| is_key_shaped(k));

    let templates_root = templates
        .map(Path::to_path_buf)
        .unwrap_or_else(|| root.join("templates"));
    let scanned = scan_templates_dir(&templates_root)
        .with_context(|| format!("env report: scan {}", templates_root.display()))?;
    let registry = TemplatesRegistry::from_entries(
        scanned
            .into_iter()
            .map(|t| TemplateEntry {
                template_id: String::new(),
                name: t.name,
                version: t.version,
                filesystem_path: t.filesystem_path,
            })
            .collect(),
    );

    let root_dir = meclaw_colony::bootstrap::assert_single_root_dir(root)
        .map_err(|e| anyhow::anyhow!("env report: {e}"))?;
    let mut seen = Seen::default();
    let mut visited = BTreeSet::new();
    seen.walk_tree(root, &root_dir, &registry, &mut visited)?;

    let unused = keys.difference(&seen.used).cloned().collect();
    let missing = seen
        .strict
        .into_iter()
        .filter(|(name, _)| !keys.contains(name))
        .collect();
    let mut notes = seen.notes;
    if !unshaped.is_empty() {
        notes.push(format!(
            "{} line(s) of {} do not start with a KEY= name; not read as keys and not shown",
            unshaped.len(),
            env_path.display()
        ));
    }
    Ok(EnvReport {
        env_path,
        env_key_count: keys.len(),
        used_count: seen.used.len(),
        unused,
        missing,
        notes,
    })
}

impl EnvReport {
    /// The report as the operator reads it: a header, then one line per
    /// finding (`unused <KEY>`, `missing <KEY> <dir>…`), or `clean`.
    pub fn render(&self) -> String {
        let mut out = format!(
            "env report: {} keys in {}, {} names substituted in the tree\n",
            self.env_key_count,
            self.env_path.display(),
            self.used_count
        );
        for name in &self.unused {
            out.push_str(&format!("  unused   {name}\n"));
        }
        for (name, dirs) in &self.missing {
            let dirs: Vec<String> = dirs.iter().map(|d| d.display().to_string()).collect();
            out.push_str(&format!("  missing  {name}   {}\n", dirs.join(", ")));
        }
        for note in &self.notes {
            out.push_str(&format!("  note     {note}\n"));
        }
        if self.unused.is_empty() && self.missing.is_empty() && self.notes.is_empty() {
            out.push_str("  clean\n");
        }
        out
    }
}
