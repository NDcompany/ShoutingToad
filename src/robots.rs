//! robots.txt parsing and matching (RFC 9309 / Google semantics).

#[derive(Clone, Debug, Default)]
struct Group {
    agents: Vec<String>,
    rules: Vec<(bool, String)>, // (allow, pattern)
    crawl_delay: Option<f32>,
}

#[derive(Clone, Debug, Default)]
pub struct Robots {
    groups: Vec<Group>,
    pub sitemaps: Vec<String>,
    pub invalid_lines: usize,
    pub noindex_lines: usize,
}

impl Robots {
    pub fn parse(body: &str) -> Robots {
        let mut r = Robots::default();
        let mut current: Option<Group> = None;
        let mut last_was_agent = false;
        for line in body.lines() {
            let line = line.split('#').next().unwrap_or("").trim();
            if line.is_empty() {
                continue;
            }
            let Some((key, value)) = line.split_once(':') else {
                r.invalid_lines += 1;
                continue;
            };
            let key = key.trim().to_ascii_lowercase();
            let value = value.trim();
            match key.as_str() {
                "user-agent" => {
                    if !last_was_agent {
                        if let Some(g) = current.take() {
                            r.groups.push(g);
                        }
                        current = Some(Group::default());
                    }
                    if let Some(g) = current.as_mut() {
                        g.agents.push(value.to_ascii_lowercase());
                    }
                    last_was_agent = true;
                    continue;
                }
                "allow" | "disallow" => {
                    if let Some(g) = current.as_mut() {
                        if !value.is_empty() {
                            g.rules.push((key == "allow", value.to_string()));
                        }
                    } else {
                        r.invalid_lines += 1;
                    }
                }
                "crawl-delay" => {
                    if let Some(g) = current.as_mut() {
                        g.crawl_delay = value.parse().ok();
                    }
                }
                "sitemap" => r.sitemaps.push(value.to_string()),
                "noindex" | "nofollow" => r.noindex_lines += 1,
                // Unknown directives (Content-Signal, Host, Clean-param…) are ignored per RFC 9309.
                k if !k.is_empty() && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') => {}
                _ => r.invalid_lines += 1,
            }
            last_was_agent = false;
        }
        if let Some(g) = current {
            r.groups.push(g);
        }
        r
    }

    /// Group for `agent` (lower-case product token), falling back to `*`.
    fn group(&self, agent: &str) -> Option<&Group> {
        self.groups
            .iter()
            .find(|g| g.agents.iter().any(|a| a != "*" && agent.contains(a.as_str())))
            .or_else(|| self.groups.iter().find(|g| g.agents.iter().any(|a| a == "*")))
    }

    pub fn crawl_delay(&self, agent: &str) -> Option<f32> {
        self.group(agent).and_then(|g| g.crawl_delay)
    }

    /// True when `/` is disallowed for the agent.
    pub fn disallows_all(&self, agent: &str) -> bool {
        !self.is_allowed("/", agent)
    }

    /// `path` is the URL path plus query (e.g. `/a/b?c=1`).
    pub fn is_allowed(&self, path: &str, agent: &str) -> bool {
        let Some(g) = self.group(agent) else { return true };
        let mut best: Option<(usize, bool)> = None;
        for (allow, pat) in &g.rules {
            if pattern_matches(pat, path) {
                let len = pat.len();
                match best {
                    Some((l, a)) if l > len || (l == len && a) => {}
                    _ => best = Some((len, *allow)),
                }
            }
        }
        best.map(|(_, a)| a).unwrap_or(true)
    }
}

/// robots.txt pattern match: `*` = any sequence, trailing `$` = end anchor.
fn pattern_matches(pattern: &str, path: &str) -> bool {
    match pattern.strip_suffix('$') {
        Some(p) => glob_full(p.as_bytes(), path.as_bytes()),
        None => glob_full(format!("{pattern}*").as_bytes(), path.as_bytes()),
    }
}

/// Whole-string match where `*` matches any run of bytes.
fn glob_full(pat: &[u8], s: &[u8]) -> bool {
    let (mut p, mut i) = (0, 0);
    let mut star: Option<(usize, usize)> = None;
    while i < s.len() {
        if p < pat.len() && pat[p] == b'*' {
            star = Some((p, i));
            p += 1;
        } else if p < pat.len() && pat[p] == s[i] {
            p += 1;
            i += 1;
        } else if let Some((sp, si)) = star {
            p = sp + 1;
            i = si + 1;
            star = Some((sp, si + 1));
        } else {
            return false;
        }
    }
    pat[p..].iter().all(|&c| c == b'*')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn longest_match_wins_and_allow_breaks_ties() {
        let r = Robots::parse(
            "User-agent: *\nDisallow: /private\nAllow: /private/public\nDisallow: /*.pdf$\nCrawl-delay: 2\nSitemap: https://e.com/s.xml\nContent-Signal: search=yes\nbogus line\nbad key: x\n",
        );
        assert!(!r.is_allowed("/private/x", "shoutingtoad"));
        assert!(r.is_allowed("/private/public/x", "shoutingtoad"));
        assert!(!r.is_allowed("/docs/a.pdf", "shoutingtoad"));
        assert!(r.is_allowed("/docs/a.pdf?x=1", "shoutingtoad"));
        assert!(r.is_allowed("/", "shoutingtoad"));
        assert_eq!(r.crawl_delay("shoutingtoad"), Some(2.0));
        assert_eq!(r.sitemaps, vec!["https://e.com/s.xml"]);
        assert_eq!(r.invalid_lines, 2);
    }

    #[test]
    fn specific_agent_group_overrides_star() {
        let r = Robots::parse("User-agent: *\nDisallow: /\n\nUser-agent: Googlebot\nUser-agent: shoutingtoad\nDisallow:\n");
        assert!(r.is_allowed("/page", "shoutingtoad"));
        assert!(r.disallows_all("otherbot"));
    }

    #[test]
    fn empty_file_allows_everything() {
        assert!(Robots::parse("").is_allowed("/x", "shoutingtoad"));
    }
}
