//! Community rule packs: extra checks written as TOML files (see docs/rule-packs.md).
//!
//! A pack is validated when it is loaded. [`install`] appends its rules to the check
//! catalogue, so their findings show up everywhere the built-in checks do. Tests that need
//! the DOM or the raw HTML (CSS selectors, `html` regexes) are "probes": the crawler counts
//! their matches while it parses each page and keeps only those counts (`HtmlData::rule_probes`);
//! every other test runs in the audit on data the crawl already stores.

use crate::audit::{self, Ctx, Kind, Priority};
use crate::checks::{self, Check, CATEGORIES};
use crate::model::{Page, Source};
use regex::{Regex, RegexBuilder};
use scraper::{ElementRef, Html, Selector};
use serde::Deserialize;
use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError, RwLock};

/// A validated rule pack.
#[derive(Clone, Debug)]
pub struct Pack {
    pub id: String,
    pub name: String,
    pub version: String,
    pub author: String,
    pub homepage: String,
    pub description: String,
    pub min_version: String,
    pub path: PathBuf,
    pub rules: Vec<Rule>,
    /// Probes the rules use; `Cond::Probe` indexes this list.
    probes: Vec<Probe>,
}

#[derive(Clone, Debug)]
pub struct Rule {
    /// `pack-id/rule-id`.
    pub id: String,
    pub name: String,
    pub category: String,
    pub kind: Kind,
    pub priority: Priority,
    pub description: String,
    pub fix: String,
    pub scope: Scope,
    pub when: Cond,
}

/// Which URLs a rule looks at.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    /// Internal HTML pages that returned 200 (the default).
    Html,
    /// Internal HTML pages that are indexable (200, no noindex, self-canonical).
    Indexable,
    /// Every internal URL: pages, redirects, errors, images, CSS, JavaScript.
    Internal,
    External,
    All,
    Images,
    Css,
    Js,
}

const SCOPES: &[(&str, Scope)] = &[
    ("html", Scope::Html),
    ("indexable", Scope::Indexable),
    ("internal", Scope::Internal),
    ("external", Scope::External),
    ("all", Scope::All),
    ("images", Scope::Images),
    ("css", Scope::Css),
    ("js", Scope::Js),
];

impl Scope {
    fn includes(self, p: &Page) -> bool {
        // robots.txt and sitemap files are recorded as URLs too; no rule is about them.
        let file =
            p.source == Some(Source::Robots) || (p.source == Some(Source::Sitemap) && p.depth == 0);
        match self {
            Scope::Html => audit::ih(p),
            Scope::Indexable => audit::indexable(p),
            Scope::Internal => p.is_internal && !file,
            Scope::External => !p.is_internal,
            Scope::All => !file,
            Scope::Images => p.kind() == "Image",
            Scope::Css => p.kind() == "CSS",
            Scope::Js => p.kind() == "JavaScript",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Op {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

/// A number comparison such as `>= 2`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Cmp {
    op: Op,
    n: f64,
}

impl Cmp {
    const EXISTS: Cmp = Cmp { op: Op::Gt, n: 0.0 };
    const NONE: Cmp = Cmp { op: Op::Eq, n: 0.0 };

    fn test(self, v: f64) -> bool {
        match self.op {
            Op::Eq => v == self.n,
            Op::Ne => v != self.n,
            Op::Lt => v < self.n,
            Op::Le => v <= self.n,
            Op::Gt => v > self.n,
            Op::Ge => v >= self.n,
        }
    }

    fn parse(s: &str) -> Result<Cmp, String> {
        let t = s.trim();
        let (op, rest) = [
            (">=", Op::Ge),
            ("<=", Op::Le),
            ("!=", Op::Ne),
            ("==", Op::Eq),
            (">", Op::Gt),
            ("<", Op::Lt),
            ("=", Op::Eq),
        ]
        .iter()
        .find_map(|(p, op)| t.strip_prefix(p).map(|r| (*op, r)))
        .unwrap_or((Op::Eq, t));
        match rest.trim().parse::<f64>() {
            Ok(n) if n.is_finite() => Ok(Cmp { op, n }),
            _ => Err(format!(
                "'{s}' is not a comparison: use a number or e.g. \">= 2\", \"< 300\", \"!= 0\""
            )),
        }
    }

    fn from_value(v: &toml::Value) -> Result<Cmp, String> {
        match v {
            toml::Value::Integer(n) => Ok(Cmp {
                op: Op::Eq,
                n: *n as f64,
            }),
            toml::Value::Float(n) => Ok(Cmp { op: Op::Eq, n: *n }),
            toml::Value::String(s) => Cmp::parse(s),
            other => Err(format!(
                "expected a number or a comparison string like \">= 2\", got {}",
                other.type_str()
            )),
        }
    }
}

/// Numbers a rule can compare.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Field {
    Status,
    WordCount,
    TitleLength,
    DescriptionLength,
    H1Count,
    ResponseMs,
    Size,
    TransferSize,
    Depth,
    Inlinks,
    Outlinks,
    Images,
}

const FIELDS: &[(&str, Field)] = &[
    ("status", Field::Status),
    ("word_count", Field::WordCount),
    ("title_length", Field::TitleLength),
    ("description_length", Field::DescriptionLength),
    ("h1_count", Field::H1Count),
    ("response_ms", Field::ResponseMs),
    ("size", Field::Size),
    ("transfer_size", Field::TransferSize),
    ("depth", Field::Depth),
    ("inlinks", Field::Inlinks),
    ("outlinks", Field::Outlinks),
    ("images", Field::Images),
];

impl Field {
    /// `None` for HTML-only fields on a URL that is not an HTML page.
    fn value(self, p: &Page, x: &Ctx) -> Option<f64> {
        let h = p.html.as_deref();
        Some(match self {
            Field::Status => p.status as f64,
            Field::ResponseMs => p.response_ms as f64,
            Field::Size => p.size as f64,
            Field::TransferSize => p.transfer_size as f64,
            Field::Depth => p.depth as f64,
            Field::Inlinks => x.inlinks_of(p).len() as f64,
            Field::WordCount => h?.word_count as f64,
            Field::TitleLength => h?.titles.first().map_or(0, |t| t.chars().count()) as f64,
            Field::DescriptionLength => h?
                .meta_descriptions
                .first()
                .map_or(0, |t| t.chars().count()) as f64,
            Field::H1Count => h?.h1.len() as f64,
            Field::Outlinks => h?.links.len() as f64,
            Field::Images => h?.images.len() as f64,
        })
    }
}

/// A test that needs the DOM or the raw HTML, evaluated by the crawler on every HTML page.
#[derive(Clone, Debug)]
pub struct Probe {
    key: String,
    kind: ProbeKind,
}

#[derive(Clone, Debug)]
enum ProbeKind {
    /// Elements matching `sel` (that have `attr`, and whose attribute value or text matches `re`).
    Select {
        sel: Selector,
        /// Tag names the selector can match (None: any element). Skips most elements
        /// without running the selector matcher.
        tags: Option<Vec<String>>,
        attr: Option<String>,
        re: Option<Regex>,
    },
    /// Matches of a regex in the raw HTML.
    Html(Regex),
}

#[derive(Clone, Debug)]
pub enum Cond {
    All(Vec<Cond>),
    Any(Vec<Cond>),
    Not(Box<Cond>),
    /// Probe index (into the pack's list until installed, then the global list) and the
    /// comparison its match count must pass.
    Probe(usize, Cmp),
    Header {
        name: String,
        re: Option<Regex>,
        exists: bool,
    },
    Url(Regex),
    ContentType(Regex),
    Num(Field, Cmp),
}

impl Cond {
    fn eval(&self, p: &Page, x: &Ctx) -> bool {
        match self {
            Cond::All(v) => v.iter().all(|c| c.eval(p, x)),
            Cond::Any(v) => v.iter().any(|c| c.eval(p, x)),
            Cond::Not(c) => !c.eval(p, x),
            Cond::Probe(i, cmp) => {
                let slot = x.rule_slots.get(*i).copied().flatten();
                match (p.html.as_deref(), slot) {
                    (Some(h), Some(s)) => h.rule_probes.get(s).is_some_and(|&n| cmp.test(n as f64)),
                    _ => false,
                }
            }
            Cond::Header { name, re, exists } => {
                let mut values = p.headers_all(name);
                match re {
                    Some(re) => values.any(|v| re.is_match(v)) == *exists,
                    None => values.next().is_some() == *exists,
                }
            }
            Cond::Url(re) => re.is_match(&p.url),
            Cond::ContentType(re) => re.is_match(&p.content_type),
            Cond::Num(f, cmp) => f.value(p, x).is_some_and(|v| cmp.test(v)),
        }
    }

    fn probes(&self, out: &mut Vec<usize>) {
        match self {
            Cond::All(v) | Cond::Any(v) => v.iter().for_each(|c| c.probes(out)),
            Cond::Not(c) => c.probes(out),
            Cond::Probe(i, _) => out.push(*i),
            _ => {}
        }
    }

    fn remap(&mut self, map: &[usize]) {
        match self {
            Cond::All(v) | Cond::Any(v) => v.iter_mut().for_each(|c| c.remap(map)),
            Cond::Not(c) => c.remap(map),
            Cond::Probe(i, _) => *i = map[*i],
            _ => {}
        }
    }
}

// ------------------------------------------------------------------ parse-time probes

/// Match counts for `probes` on one page (saturating at 65535). One walk over the DOM
/// serves every selector.
pub fn eval_probes(probes: &[Probe], doc: &Html, raw: &str) -> Box<[u16]> {
    if probes.is_empty() {
        return Box::default();
    }
    let mut out = vec![0u16; probes.len()];
    let selects: Vec<(usize, &ProbeKind)> = probes
        .iter()
        .enumerate()
        .filter(|(_, p)| matches!(p.kind, ProbeKind::Select { .. }))
        .map(|(i, p)| (i, &p.kind))
        .collect();
    if !selects.is_empty() {
        for el in doc
            .root_element()
            .descendants()
            .filter_map(ElementRef::wrap)
        {
            let name = el.value().name();
            let mut text: Option<String> = None;
            for &(i, kind) in &selects {
                let ProbeKind::Select {
                    sel,
                    tags,
                    attr,
                    re,
                } = kind
                else {
                    continue;
                };
                if tags
                    .as_ref()
                    .is_some_and(|t| !t.iter().any(|t| t.eq_ignore_ascii_case(name)))
                    || !sel.matches(&el)
                {
                    continue;
                }
                let hit = match (attr, re) {
                    (Some(a), Some(re)) => el.value().attr(a).is_some_and(|v| re.is_match(v)),
                    (Some(a), None) => el.value().attr(a).is_some(),
                    (None, Some(re)) => {
                        re.is_match(text.get_or_insert_with(|| el.text().collect()))
                    }
                    (None, None) => true,
                };
                if hit {
                    out[i] = out[i].saturating_add(1);
                }
            }
        }
    }
    for (i, p) in probes.iter().enumerate() {
        if let ProbeKind::Html(re) = &p.kind {
            out[i] = re.find_iter(raw).take(u16::MAX as usize).count() as u16;
        }
    }
    out.into_boxed_slice()
}

/// Tag names the rightmost compound of each comma-separated part of `css` requires, e.g.
/// `a[href], link.x` gives [a, link]. None when any part could match any element
/// (`.x`, `[attr]`, `*`) or the selector is too unusual to read safely.
fn rightmost_tags(css: &str) -> Option<Vec<String>> {
    if css.contains(['\\', '|']) {
        return None;
    }
    let (mut parts, mut start, mut depth, mut quote) = (Vec::new(), 0, 0i32, None);
    let mut compound = 0; // start of the current part's last compound
    for (i, c) in css.char_indices() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), _) => {}
            (None, '"' | '\'') => quote = Some(c),
            (None, '[' | '(') => depth += 1,
            (None, ']' | ')') => depth -= 1,
            (None, ',') if depth == 0 => {
                parts.push(&css[compound.max(start)..i]);
                start = i + 1;
                compound = start;
            }
            (None, ' ' | '\t' | '\n' | '>' | '+' | '~') if depth == 0 => compound = i + 1,
            _ => {}
        }
    }
    parts.push(&css[compound.max(start)..]);
    parts
        .iter()
        .map(|p| {
            let tag: String = p
                .trim()
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
                .collect();
            let first = tag.chars().next()?;
            first
                .is_ascii_alphabetic()
                .then(|| tag.to_ascii_lowercase())
        })
        .collect()
}

// ------------------------------------------------------------------ registry

/// The installed probes, in the order the crawler stores their counts.
pub struct Active {
    pub probes: Vec<Probe>,
}

impl Active {
    pub fn keys(&self) -> impl Iterator<Item = String> + '_ {
        self.probes.iter().map(|p| p.key.clone())
    }
}

static ACTIVE: RwLock<Option<Arc<Active>>> = RwLock::new(None);

/// Probes of the installed packs (None when no pack uses one). A crawl keeps the set it
/// started with.
pub fn active() -> Option<Arc<Active>> {
    ACTIVE
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .clone()
}

/// For each installed probe, its slot in a crawl that stored `crawl_keys` (None when that
/// crawl did not evaluate it: rules using it then do not fire).
pub fn slots(crawl_keys: &[String]) -> Vec<Option<usize>> {
    active().map_or_else(Vec::new, |a| {
        a.probes
            .iter()
            .map(|p| crawl_keys.iter().position(|k| *k == p.key))
            .collect()
    })
}

/// Make `packs` the active rule packs: their rules join the check catalogue (after the
/// built-in checks) and their probes run in the next crawl. An empty slice removes them.
pub fn install(packs: &[Pack]) {
    let mut probes: Vec<Probe> = Vec::new();
    let mut index: HashMap<String, usize> = HashMap::new();
    let mut extra = Vec::new();
    for pack in packs {
        let map: Vec<usize> = pack
            .probes
            .iter()
            .map(|pr| {
                *index.entry(pr.key.clone()).or_insert_with(|| {
                    probes.push(pr.clone());
                    probes.len() - 1
                })
            })
            .collect();
        for r in &pack.rules {
            let mut r = r.clone();
            r.when.remap(&map);
            extra.push(to_check(r));
        }
    }
    *ACTIVE.write().unwrap_or_else(PoisonError::into_inner) = if probes.is_empty() {
        None
    } else {
        Some(Arc::new(Active { probes }))
    };
    checks::set_extra(extra);
}

/// Category names are `&'static str` in the catalogue: built-in ones are reused, new ones
/// interned (a handful per session).
fn category(name: &str) -> &'static str {
    if let Some(c) = CATEGORIES.iter().find(|c| c.eq_ignore_ascii_case(name)) {
        return c;
    }
    static NEW: Mutex<Vec<&'static str>> = Mutex::new(Vec::new());
    let mut v = NEW.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(c) = v.iter().find(|c| c.eq_ignore_ascii_case(name)) {
        return c;
    }
    let c: &'static str = Box::leak(name.to_string().into_boxed_str());
    v.push(c);
    c
}

fn to_check(r: Rule) -> Check {
    let mut needs = Vec::new();
    r.when.probes(&mut needs);
    let r = Arc::new(r);
    Check {
        id: r.id.clone(),
        category: category(&r.category),
        name: r.name.clone(),
        kind: r.kind,
        priority: r.priority,
        description: r.description.clone(),
        fix: r.fix.clone(),
        test: Box::new(move |p, x| {
            r.scope.includes(p)
                // A crawl made without this rule's probes cannot answer it.
                && needs
                    .iter()
                    .all(|&i| x.rule_slots.get(i).is_some_and(Option::is_some))
                && r.when.eval(p, x)
        }),
    }
}

// ------------------------------------------------------------------ loading

/// A validation error, printed as `file:line: rule 'id': message`.
#[derive(Clone, Debug, PartialEq)]
pub struct Problem {
    pub file: PathBuf,
    pub line: Option<usize>,
    pub rule: Option<String>,
    pub msg: String,
}

impl fmt::Display for Problem {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}", self.file.display())?;
        if let Some(l) = self.line {
            write!(f, ":{l}")?;
        }
        if let Some(r) = &self.rule {
            write!(f, ": rule '{r}'")?;
        }
        write!(f, ": {}", self.msg)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawFile {
    pack: RawPack,
    #[serde(default)]
    rule: Vec<RawRule>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawPack {
    id: Option<String>,
    name: String,
    #[serde(default)]
    version: String,
    #[serde(default)]
    author: String,
    #[serde(default)]
    homepage: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    min_version: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawRule {
    id: toml::Spanned<String>,
    name: String,
    category: Option<String>,
    kind: Option<String>,
    priority: Option<String>,
    #[serde(default)]
    description: String,
    #[serde(default)]
    fix: String,
    scope: Option<String>,
    when: toml::Spanned<toml::Value>,
}

fn valid_id(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 64
        && s.bytes().next().is_some_and(|b| b.is_ascii_alphanumeric())
        && s.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
}

fn version_tuple(s: &str) -> Option<(u64, u64, u64)> {
    let mut it = s.trim().trim_start_matches('v').split('.');
    let mut n = || -> Option<u64> { it.next().map_or(Some(0), |p| p.parse().ok()) };
    let v = (n()?, n()?, n()?);
    it.next().is_none().then_some(v)
}

/// Load and validate one pack file.
pub fn load_file(path: &Path) -> Result<Pack, Vec<Problem>> {
    let src = std::fs::read_to_string(path).map_err(|e| {
        vec![Problem {
            file: path.into(),
            line: None,
            rule: None,
            msg: format!("could not read the file: {e}"),
        }]
    })?;
    parse_pack(&src, path)
}

/// Parse and validate a pack from its TOML text; `path` names it in errors and gives the
/// default pack id (the file name).
pub fn parse_pack(src: &str, path: &Path) -> Result<Pack, Vec<Problem>> {
    let line_of = |off: usize| src[..off.min(src.len())].matches('\n').count() + 1;
    let problem = |line: Option<usize>, rule: Option<&str>, msg: String| Problem {
        file: path.into(),
        line,
        rule: rule.map(str::to_string),
        msg,
    };
    let raw: RawFile = toml::from_str(src).map_err(|e| {
        let msg = e.message().trim().to_string();
        vec![problem(e.span().map(|s| line_of(s.start)), None, msg)]
    })?;
    let mut problems = Vec::new();
    let rp = raw.pack;
    let id = rp.id.unwrap_or_else(|| {
        path.file_stem()
            .map(|s| s.to_string_lossy().to_lowercase())
            .unwrap_or_default()
    });
    if !valid_id(&id) {
        problems.push(problem(
            None,
            None,
            format!("pack id '{id}' must be lower-case letters, digits, '-' or '_' (set `id` under [pack])"),
        ));
    }
    if rp.name.trim().is_empty() {
        problems.push(problem(None, None, "[pack] needs a name".into()));
    }
    if !rp.min_version.is_empty() {
        match version_tuple(&rp.min_version) {
            None => problems.push(problem(
                None,
                None,
                format!(
                    "min_version '{}' is not a version like \"1.1.0\"",
                    rp.min_version
                ),
            )),
            Some(need)
                if version_tuple(env!("CARGO_PKG_VERSION")).is_some_and(|have| have < need) =>
            {
                problems.push(problem(
                    None,
                    None,
                    format!(
                        "this pack needs ShoutingToad {} or newer (this is {})",
                        rp.min_version,
                        env!("CARGO_PKG_VERSION")
                    ),
                ))
            }
            _ => {}
        }
    }
    if raw.rule.is_empty() {
        problems.push(problem(
            None,
            None,
            "the pack has no [[rule]] entries".into(),
        ));
    }
    let mut probes = Vec::new();
    let mut keys: HashMap<String, usize> = HashMap::new();
    let mut rules = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for r in raw.rule {
        let line = Some(line_of(r.id.span().start));
        let when_line = Some(line_of(r.when.span().start));
        let rid = r.id.get_ref().clone();
        let mut bad = |msg: String| problems.push(problem(line, Some(&rid), msg));
        if !valid_id(&rid) {
            bad("rule id must be lower-case letters, digits, '-' or '_'".into());
        }
        if !seen.insert(rid.clone()) {
            bad("duplicate rule id in this pack".into());
        }
        if r.name.trim().is_empty() {
            bad("the rule needs a name".into());
        }
        let kind = match r
            .kind
            .as_deref()
            .unwrap_or("warning")
            .to_ascii_lowercase()
            .as_str()
        {
            "issue" => Kind::Issue,
            "warning" => Kind::Warning,
            "opportunity" => Kind::Opportunity,
            other => {
                bad(format!(
                    "kind '{other}' must be issue, warning or opportunity"
                ));
                Kind::Warning
            }
        };
        let priority = match r
            .priority
            .as_deref()
            .unwrap_or("medium")
            .to_ascii_lowercase()
            .as_str()
        {
            "high" => Priority::High,
            "medium" => Priority::Medium,
            "low" => Priority::Low,
            other => {
                bad(format!("priority '{other}' must be high, medium or low"));
                Priority::Medium
            }
        };
        let scope = match r.scope.as_deref() {
            None => Scope::Html,
            Some(s) => SCOPES
                .iter()
                .find(|(n, _)| n.eq_ignore_ascii_case(s.trim()))
                .map(|(_, sc)| *sc)
                .unwrap_or_else(|| {
                    let names: Vec<&str> = SCOPES.iter().map(|(n, _)| *n).collect();
                    bad(format!("scope '{s}' must be one of: {}", names.join(", ")));
                    Scope::Html
                }),
        };
        let category = r.category.unwrap_or_else(|| rp.name.clone());
        if category.trim().is_empty() || category.len() > 40 {
            bad("category must be 1 to 40 characters".into());
        }
        let mut probe = |p: Probe| {
            *keys.entry(p.key.clone()).or_insert_with(|| {
                probes.push(p);
                probes.len() - 1
            })
        };
        match cond(r.when.get_ref(), &mut probe) {
            Ok(when) => rules.push(Rule {
                id: format!("{id}/{rid}"),
                name: r.name,
                category: category.trim().to_string(),
                kind,
                priority,
                description: r.description.trim().to_string(),
                fix: r.fix.trim().to_string(),
                scope,
                when,
            }),
            Err(e) => problems.push(problem(when_line, Some(&rid), format!("when: {e}"))),
        }
    }
    if !problems.is_empty() {
        return Err(problems);
    }
    Ok(Pack {
        id,
        name: rp.name,
        version: rp.version,
        author: rp.author,
        homepage: rp.homepage,
        description: rp.description,
        min_version: rp.min_version,
        path: path.into(),
        rules,
        probes,
    })
}

fn regex(pattern: &str, what: &str) -> Result<Regex, String> {
    RegexBuilder::new(pattern)
        .size_limit(1 << 20)
        .dfa_size_limit(1 << 20)
        .build()
        .map_err(|e| {
            let e = e.to_string();
            let e = e.lines().last().unwrap_or("").trim().to_string();
            format!("invalid regex in `{what}` ({pattern:?}): {e}")
        })
}

fn string<'a>(v: &'a toml::Value, key: &str) -> Result<&'a str, String> {
    v.as_str()
        .ok_or_else(|| format!("`{key}` must be a string, got {}", v.type_str()))
}

/// Parse one condition: a table with one test (`url`, `selector`, `all`, …) or a list of
/// conditions that must all hold.
fn cond(v: &toml::Value, probe: &mut dyn FnMut(Probe) -> usize) -> Result<Cond, String> {
    let t = match v {
        toml::Value::Array(a) => {
            return Ok(Cond::All(
                a.iter().map(|c| cond(c, probe)).collect::<Result<_, _>>()?,
            ))
        }
        toml::Value::Table(t) => t,
        other => {
            return Err(format!(
                "a condition is a table such as {{ url = '/blog/' }} or a list of them, got {}",
                other.type_str()
            ))
        }
    };
    const TESTS: &[&str] = &[
        "all",
        "any",
        "not",
        "selector",
        "html",
        "header",
        "url",
        "content_type",
    ];
    let is_test = |k: &str| TESTS.contains(&k) || FIELDS.iter().any(|(n, _)| *n == k);
    let tests: Vec<&String> = t.keys().filter(|k| is_test(k)).collect();
    let test = match tests.as_slice() {
        [one] => one.as_str(),
        [] => {
            let names: Vec<&str> = FIELDS.iter().map(|(n, _)| *n).collect();
            return Err(format!(
                "no test in {{ {} }}: use one of {}, {}",
                t.keys().cloned().collect::<Vec<_>>().join(", "),
                TESTS.join(", "),
                names.join(", ")
            ));
        }
        many => {
            let many: Vec<&str> = many.iter().map(|s| s.as_str()).collect();
            return Err(format!(
                "one test per table, found {}: combine tests with all = [ ... ] or any = [ ... ]",
                many.join(" and ")
            ));
        }
    };
    let allowed: &[&str] = match test {
        "selector" => &["attr", "matches", "count", "exists"],
        "html" => &["count", "exists"],
        "header" => &["matches", "exists"],
        _ => &[],
    };
    if let Some(k) = t
        .keys()
        .find(|k| *k != test && !allowed.contains(&k.as_str()))
    {
        return Err(if allowed.is_empty() {
            format!("`{test}` takes no other keys (found `{k}`)")
        } else {
            format!(
                "`{k}` does not go with `{test}` (allowed: {})",
                allowed.join(", ")
            )
        });
    }
    let val = &t[test];
    let exists = match t.get("exists") {
        None => true,
        Some(toml::Value::Boolean(b)) => *b,
        Some(other) => {
            return Err(format!(
                "`exists` must be true or false, got {}",
                other.type_str()
            ))
        }
    };
    let count = || -> Result<Cmp, String> {
        match t.get("count") {
            Some(_) if t.contains_key("exists") => {
                Err("use either `count` or `exists`, not both".into())
            }
            Some(c) => Cmp::from_value(c).map_err(|e| format!("count: {e}")),
            None if exists => Ok(Cmp::EXISTS),
            None => Ok(Cmp::NONE),
        }
    };
    Ok(match test {
        "all" | "any" => {
            let list = val
                .as_array()
                .ok_or_else(|| format!("`{test}` must be a list: {test} = [ {{...}}, {{...}} ]"))?;
            if list.is_empty() {
                return Err(format!("`{test}` is empty"));
            }
            let v = list
                .iter()
                .map(|c| cond(c, probe))
                .collect::<Result<_, _>>()?;
            if test == "all" {
                Cond::All(v)
            } else {
                Cond::Any(v)
            }
        }
        "not" => Cond::Not(Box::new(cond(val, probe)?)),
        "selector" => {
            let css = string(val, "selector")?.trim();
            let sel =
                Selector::parse(css).map_err(|e| format!("invalid CSS selector {css:?}: {e}"))?;
            let attr = match t.get("attr") {
                Some(a) => Some(string(a, "attr")?.trim().to_ascii_lowercase()),
                None => None,
            };
            let pat = match t.get("matches") {
                Some(m) => Some(string(m, "matches")?),
                None => None,
            };
            let re = pat.map(|p| regex(p, "matches")).transpose()?;
            let cmp = count()?;
            let key = format!(
                "sel\u{1f}{css}\u{1f}{}\u{1f}{}",
                attr.as_deref().unwrap_or(""),
                pat.unwrap_or("")
            );
            let i = probe(Probe {
                key,
                kind: ProbeKind::Select {
                    sel,
                    tags: rightmost_tags(css),
                    attr,
                    re,
                },
            });
            Cond::Probe(i, cmp)
        }
        "html" => {
            let pat = string(val, "html")?;
            let re = regex(pat, "html")?;
            let cmp = count()?;
            let i = probe(Probe {
                key: format!("html\u{1f}{pat}"),
                kind: ProbeKind::Html(re),
            });
            Cond::Probe(i, cmp)
        }
        "header" => {
            let name = string(val, "header")?.trim().to_ascii_lowercase();
            if name.is_empty() {
                return Err("`header` needs a header name".into());
            }
            let re = match t.get("matches") {
                Some(m) => Some(regex(string(m, "matches")?, "matches")?),
                None => None,
            };
            Cond::Header { name, re, exists }
        }
        "url" => Cond::Url(regex(string(val, "url")?, "url")?),
        "content_type" => Cond::ContentType(regex(string(val, "content_type")?, "content_type")?),
        field => {
            let f = FIELDS
                .iter()
                .find(|(n, _)| *n == field)
                .map(|(_, f)| *f)
                .unwrap();
            Cond::Num(
                f,
                Cmp::from_value(val).map_err(|e| format!("{field}: {e}"))?,
            )
        }
    })
}

/// The pack files `path` stands for: itself, or the `*.toml` files of a folder (sorted).
pub fn pack_files(path: &Path) -> Result<Vec<PathBuf>, Problem> {
    if !path.is_dir() {
        return Ok(vec![path.into()]);
    }
    let rd = std::fs::read_dir(path).map_err(|e| Problem {
        file: path.into(),
        line: None,
        rule: None,
        msg: format!("could not read the folder: {e}"),
    })?;
    let mut f: Vec<PathBuf> = rd
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.is_file()
                && p.extension()
                    .is_some_and(|e| e.eq_ignore_ascii_case("toml"))
        })
        .collect();
    f.sort();
    Ok(f)
}

/// Load every pack in `paths` (files, or folders whose `*.toml` files are packs).
/// Returns the valid packs and the problems of the others.
pub fn load_paths(paths: &[PathBuf]) -> (Vec<Pack>, Vec<Problem>) {
    let mut packs: Vec<Pack> = Vec::new();
    let mut problems = Vec::new();
    for path in paths {
        let files = match pack_files(path) {
            Ok(f) => f,
            Err(e) => {
                problems.push(e);
                continue;
            }
        };
        for f in files {
            match load_file(&f) {
                Ok(p) => {
                    if let Some(other) = packs.iter().find(|o| o.id == p.id) {
                        problems.push(Problem {
                            file: f,
                            line: None,
                            rule: None,
                            msg: format!(
                                "pack id '{}' is already used by {}",
                                p.id,
                                other.path.display()
                            ),
                        });
                    } else {
                        packs.push(p);
                    }
                }
                Err(e) => problems.extend(e),
            }
        }
    }
    (packs, problems)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Crawl, HtmlData};
    use url::Url;

    fn pack(rules: &str) -> Result<Pack, Vec<Problem>> {
        let src = format!("[pack]\nname = \"Test\"\n\n{rules}");
        parse_pack(&src, Path::new("t.toml"))
    }

    fn err(rules: &str) -> String {
        pack(rules)
            .expect_err("pack should be invalid")
            .iter()
            .map(|p| p.to_string())
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Run one rule against a page built from `html` (a 200 internal HTML page),
    /// evaluating its probes the way the crawler does.
    fn fires_page(rule_when: &str, scope: &str, mut page: Page, html: Option<&str>) -> bool {
        let p = pack(&format!(
            "[[rule]]\nid = \"r\"\nname = \"R\"\nscope = \"{scope}\"\nwhen = {rule_when}\n"
        ))
        .unwrap_or_else(|e| panic!("{e:?}"));
        let url = Url::parse(&page.url).unwrap();
        if let Some(raw) = html {
            page.html = Some(Box::new(crate::parse::parse_html_probed(
                raw, &url, &p.probes,
            )));
        }
        let crawl = Crawl {
            pages: vec![page],
            site: crate::model::SiteInfo {
                rule_probes: p.probes.iter().map(|p| p.key.clone()).collect(),
                ..Default::default()
            },
            ..Default::default()
        };
        // Same check the catalogue would hold, without installing it globally
        // (other tests in this binary share the catalogue).
        let mut ctx = Ctx::new(&crawl);
        ctx.rule_slots = (0..p.probes.len()).map(Some).collect();
        let check = to_check(p.rules[0].clone());
        (check.test)(&crawl.pages[0], &ctx)
    }

    fn page(url: &str) -> Page {
        Page {
            id: 1,
            url: url.into(),
            is_internal: true,
            status: 200,
            content_type: "text/html; charset=utf-8".into(),
            ..Default::default()
        }
    }

    fn fires(when: &str, html: &str) -> bool {
        fires_page(when, "html", page("https://e.com/products/van"), Some(html))
    }

    const DOC: &str = r#"<!doctype html><html><head><title>Van – Powered by Shopify</title>
        <meta name="generator" content="WordPress 6.4.2">
        <script type="application/ld+json">{"@type":"Product","name":"Van"}</script></head>
        <body><h1>Van</h1><h1>Again</h1><a href="tel:+441234">Call</a><p class="x">Hello toad</p></body></html>"#;

    #[test]
    fn selector_conditions() {
        assert!(fires("{ selector = 'h1' }", DOC));
        assert!(!fires("{ selector = 'h2' }", DOC));
        assert!(fires("{ selector = 'h2', exists = false }", DOC));
        assert!(fires("{ selector = 'h1', count = 2 }", DOC));
        assert!(fires("{ selector = 'h1', count = '>= 2' }", DOC));
        assert!(!fires("{ selector = 'h1', count = '> 2' }", DOC));
        assert!(fires("{ selector = 'p.x', matches = 'toad$' }", DOC));
        assert!(!fires("{ selector = 'p.x', matches = '^toad' }", DOC));
        assert!(fires(
            "{ selector = 'meta[name=generator]', attr = 'content', matches = 'WordPress \\d' }",
            DOC
        ));
        assert!(fires("{ selector = 'a', attr = 'href' }", DOC));
        assert!(!fires("{ selector = 'a', attr = 'title' }", DOC));
        assert!(fires(
            "{ selector = 'script[type=\"application/ld+json\"]', matches = '\"@type\"\\s*:\\s*\"Product\"' }",
            DOC
        ));
    }

    #[test]
    fn html_regex_conditions() {
        assert!(fires("{ html = 'Powered by Shopify' }", DOC));
        assert!(fires("{ html = '<h1>', count = 2 }", DOC));
        assert!(fires("{ html = 'wp-emoji', exists = false }", DOC));
    }

    #[test]
    fn page_field_conditions() {
        let mut p = page("https://e.com/products/van?variant=12");
        p.headers = vec![
            ("X-Powered-By".into(), "PHP/8.1".into()),
            ("Server".into(), "nginx".into()),
        ];
        p.response_ms = 900;
        p.size = 2000;
        p.depth = 3;
        let f = |w: &str| fires_page(w, "html", p.clone(), Some(DOC));
        assert!(f("{ url = '[?&]variant=\\d+' }"));
        assert!(!f("{ url = '/collections/' }"));
        assert!(f("{ header = 'x-powered-by' }"));
        assert!(f("{ header = 'X-Powered-By', matches = 'PHP' }"));
        assert!(!f("{ header = 'x-powered-by', matches = 'ASP' }"));
        assert!(f(
            "{ header = 'strict-transport-security', exists = false }"
        ));
        assert!(f("{ content_type = '(?i)text/html' }"));
        assert!(f("{ status = 200 }"));
        assert!(f("{ status = '< 300' }"));
        assert!(f("{ response_ms = '> 500' }"));
        assert!(f("{ size = 2000 }"));
        assert!(f("{ depth = '>= 3' }"));
        assert!(f("{ h1_count = 2 }"));
        assert!(f("{ title_length = '> 10' }"));
        assert!(f("{ description_length = 0 }"));
        assert!(f("{ word_count = '< 50' }"));
        assert!(f("{ outlinks = 0 }"));
        assert!(f("{ images = 0 }"));
        assert!(f("{ inlinks = 0 }"));
        assert!(f("{ transfer_size = 0 }"));
    }

    #[test]
    fn combinators_and_lists() {
        assert!(fires(
            "[ { selector = 'h1' }, { url = '/products/' } ]",
            DOC
        ));
        assert!(!fires("[ { selector = 'h1' }, { url = '/blog/' } ]", DOC));
        assert!(fires(
            "{ any = [ { selector = 'h6' }, { url = '/products/' } ] }",
            DOC
        ));
        assert!(!fires(
            "{ any = [ { selector = 'h6' }, { url = '/blog/' } ] }",
            DOC
        ));
        assert!(fires("{ not = { selector = 'h6' } }", DOC));
        assert!(fires(
            "{ all = [ { url = 'van' }, { not = { any = [ { h1_count = 1 }, { html = 'nope' } ] } } ] }",
            DOC
        ));
    }

    #[test]
    fn scopes() {
        let mut img = page("https://e.com/a.jpg");
        img.content_type = "image/jpeg".into();
        assert!(fires_page(
            "{ url = 'a\\.jpg' }",
            "images",
            img.clone(),
            None
        ));
        assert!(!fires_page(
            "{ url = 'a\\.jpg' }",
            "html",
            img.clone(),
            None
        ));
        assert!(fires_page(
            "{ url = 'a\\.jpg' }",
            "internal",
            img.clone(),
            None
        ));
        assert!(!fires_page(
            "{ url = 'a\\.jpg' }",
            "external",
            img.clone(),
            None
        ));
        // HTML-only tests are false on non-HTML URLs, so `not` of them is true.
        assert!(!fires_page(
            "{ selector = 'h1', exists = false }",
            "images",
            img.clone(),
            None
        ));
        assert!(!fires_page(
            "{ word_count = 0 }",
            "images",
            img.clone(),
            None
        ));
        let mut noindex = page("https://e.com/x");
        noindex.headers = vec![("X-Robots-Tag".into(), "noindex".into())];
        assert!(fires_page(
            "{ url = 'x' }",
            "html",
            noindex.clone(),
            Some(DOC)
        ));
        assert!(!fires_page(
            "{ url = 'x' }",
            "indexable",
            noindex,
            Some(DOC)
        ));
        let mut js = page("https://cdn.e.com/a.js");
        js.is_internal = false;
        js.content_type = "application/javascript".into();
        assert!(fires_page("{ size = 0 }", "js", js.clone(), None));
        assert!(fires_page("{ size = 0 }", "external", js.clone(), None));
        assert!(fires_page("{ size = 0 }", "all", js, None));
    }

    #[test]
    fn crawl_without_probes_does_not_fire() {
        let p = pack(
            "[[rule]]\nid = \"r\"\nname = \"R\"\nwhen = { selector = 'h2', exists = false }\n",
        )
        .unwrap();
        let mut pg = page("https://e.com/");
        pg.html = Some(Box::new(HtmlData::default()));
        let crawl = Crawl {
            pages: vec![pg],
            ..Default::default()
        };
        let ctx = Ctx::new(&crawl);
        assert!(!(to_check(p.rules[0].clone()).test)(&crawl.pages[0], &ctx));
    }

    #[test]
    fn identical_tests_share_one_probe() {
        let p = pack(
            "[[rule]]\nid = \"a\"\nname = \"A\"\nwhen = { selector = 'h1' }\n\
             [[rule]]\nid = \"b\"\nname = \"B\"\nwhen = { selector = 'h1', count = 2 }\n",
        )
        .unwrap();
        assert_eq!(p.probes.len(), 1);
        assert_eq!(p.rules[0].id, "t/a");
        assert_eq!(p.rules[0].category, "Test");
        assert_eq!(p.rules[0].kind, Kind::Warning);
        assert_eq!(p.rules[0].priority, Priority::Medium);
    }

    #[test]
    fn validation_errors_name_file_line_and_rule() {
        let e = err("[[rule]]\nid = \"ok\"\nname = \"Ok\"\nwhen = { url = 'a' }\n\n[[rule]]\nid = \"bad\"\nname = \"Bad\"\nwhen = { url = '(' }\n");
        assert!(
            e.starts_with("t.toml:12: rule 'bad': when: invalid regex in `url`"),
            "{e}"
        );
        let e = err("[[rule]]\nid = \"s\"\nname = \"S\"\nwhen = { selector = 'a[' }\n");
        assert!(e.contains("rule 's': when: invalid CSS selector"), "{e}");
        assert!(
            err("[[rule]]\nid = \"s\"\nname = \"S\"\nwhen = { selecter = 'a' }\n")
                .contains("no test in { selecter }")
        );
        assert!(
            err("[[rule]]\nid = \"s\"\nname = \"S\"\nwhen = { url = 'a', selector = 'b' }\n")
                .contains("one test per table")
        );
        assert!(
            err("[[rule]]\nid = \"s\"\nname = \"S\"\nwhen = { url = 'a', matches = 'b' }\n")
                .contains("`url` takes no other keys")
        );
        assert!(err(
            "[[rule]]\nid = \"s\"\nname = \"S\"\nwhen = { selector = 'a', count = 'lots' }\n"
        )
        .contains("is not a comparison"));
        assert!(err("[[rule]]\nid = \"s\"\nname = \"S\"\nwhen = { selector = 'a', count = 1, exists = true }\n").contains("not both"));
        assert!(
            err("[[rule]]\nid = \"S S\"\nname = \"S\"\nwhen = { url = 'a' }\n")
                .contains("rule id must be")
        );
        assert!(
            err("[[rule]]\nid = \"s\"\nname = \"S\"\nkind = \"bug\"\nwhen = { url = 'a' }\n")
                .contains("kind 'bug'")
        );
        assert!(err(
            "[[rule]]\nid = \"s\"\nname = \"S\"\npriority = \"urgent\"\nwhen = { url = 'a' }\n"
        )
        .contains("priority 'urgent'"));
        assert!(err(
            "[[rule]]\nid = \"s\"\nname = \"S\"\nscope = \"pages\"\nwhen = { url = 'a' }\n"
        )
        .contains("scope 'pages'"));
        assert!(err("[[rule]]\nid = \"s\"\nname = \"S\"\nwhen = { url = 'a' }\n[[rule]]\nid = \"s\"\nname = \"S\"\nwhen = { url = 'a' }\n").contains("duplicate rule id"));
        assert!(
            err("[[rule]]\nid = \"s\"\nname = \"S\"\nwhen = { all = [] }\n")
                .contains("`all` is empty")
        );
        assert!(
            err("[[rule]]\nid = \"s\"\nname = \"S\"\nwhen = { url = 'a' }\ncolour = 1\n")
                .contains("unknown field `colour`")
        );
        assert!(err("[[rule]]\nid = \"s\"\nname = \"S\"\nwhen = 'url'\n")
            .contains("a condition is a table"));
        assert!(err("[[rule]]\nid = \"s\"\nname = \"S\"\n").contains("missing field `when`"));
        assert!(err("").contains("no [[rule]] entries"));
        let syntax =
            parse_pack("[pack]\nname = \"x\"\n[[rule]\n", Path::new("t.toml")).unwrap_err();
        assert_eq!(syntax[0].line, Some(3), "{syntax:?}");
        let future = parse_pack("[pack]\nname = \"x\"\nmin_version = \"99.0\"\n[[rule]]\nid = \"a\"\nname = \"A\"\nwhen = { url = 'a' }\n", Path::new("t.toml")).unwrap_err();
        assert!(future[0].msg.contains("needs ShoutingToad 99.0 or newer"));
    }

    #[test]
    fn rightmost_tags_are_conservative() {
        let t = |s: &str| rightmost_tags(s);
        let v = |a: &[&str]| Some(a.iter().map(|s| s.to_string()).collect::<Vec<_>>());
        assert_eq!(t("a[href*=\"x y\"]:not([rel~=\"nofollow\"])"), v(&["a"]));
        assert_eq!(
            t("link[rel=\"pingback\"], link[rel=\"EditURI\"]"),
            v(&["link", "link"])
        );
        assert_eq!(t("body.archive.author"), v(&["body"]));
        assert_eq!(t("main > ARTICLE p"), v(&["p"]));
        assert_eq!(t("div p, iframe[src*=\"a,b\"]"), v(&["p", "iframe"]));
        assert_eq!(t("meta[name=\"generator\" i]"), v(&["meta"]));
        assert_eq!(t("ul li:nth-child(2n + 1)"), v(&["li"]));
        assert_eq!(t("#x"), None);
        assert_eq!(t("a, .x"), None);
        assert_eq!(t("[itemtype]"), None);
        assert_eq!(t("div *"), None);
        assert_eq!(t("svg|a"), None);
    }

    #[test]
    fn versions() {
        assert_eq!(version_tuple("1.2"), Some((1, 2, 0)));
        assert_eq!(version_tuple("v1.2.3"), Some((1, 2, 3)));
        assert_eq!(version_tuple("1.x"), None);
        assert_eq!(version_tuple("1.2.3.4"), None);
    }

    #[test]
    fn starter_packs_are_valid() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("rules");
        let (packs, problems) = load_paths(&[dir]);
        assert!(problems.is_empty(), "{problems:#?}");
        assert!(packs.len() >= 4);
        let mut ids = std::collections::HashSet::new();
        for p in &packs {
            assert!(
                !p.homepage.is_empty() && !p.description.is_empty(),
                "{}",
                p.id
            );
            for r in &p.rules {
                assert!(ids.insert(r.id.clone()), "duplicate {}", r.id);
                assert!(
                    !r.description.is_empty() && !r.fix.is_empty(),
                    "{} lacks text",
                    r.id
                );
            }
        }
    }
}
