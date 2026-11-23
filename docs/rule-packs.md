# Rule packs

A rule pack is a TOML file of extra checks. Packs let anyone add checks for a platform
(Shopify, WordPress), a kind of site (local business, online shop) or an agency's own
standards, without writing Rust. A rule's findings show up exactly like the built-in checks:
in the Issues panel, the URL inspector, the HTML report, `issues.csv`, the CLI summary, the
health score, `--fail-on` and `--fail-on-check`.

Starter packs are in the [`rules/`](../rules) folder: `shopify.toml`, `wordpress.toml`,
`local-seo.toml` and `ecommerce.toml`.

- [Using packs](#using-packs)
- [Write your first rule in 5 minutes](#write-your-first-rule-in-5-minutes)
- [Format reference](#format-reference)
- [Conditions](#conditions)
- [Testing a pack](#testing-a-pack)
- [Sharing a pack](#sharing-a-pack)

## Using packs

**Command line.** Add `--rules` to `crawl` or `list`, once per file or folder (a folder loads
every `*.toml` file in it):

```text
shouting-toad crawl https://example.com --rules rules/wordpress.toml
shouting-toad crawl https://example.com --rules rules/ --rules ~/agency-rules.toml
shouting-toad crawl https://staging.example.com --rules rules/ --fail-on-check wordpress/default-tagline
shouting-toad rules list rules/            # every rule with its id
shouting-toad rules validate my-pack.toml  # check a pack without crawling
```

An invalid pack stops the crawl with the list of problems (exit code 1), so a broken pack
never silently passes a CI gate.

**Desktop app.** Open **Configuration → Rule packs…**, then **Add pack file…** or
**Add folder…**. Each pack shows its rule count or its errors, and has a checkbox to switch
it on or off. The list of paths is saved with your settings. Packs apply to the next crawl;
the current results are re-audited straight away (rules that use `selector` or `html` tests
need a new crawl, see [how rules run](#how-rules-run)).

## Write your first rule in 5 minutes

Say your client's pages must never link to the old shop at `shop.example.com`, and every
page needs the cookie banner (`<div id="cookie-banner">`).

1. Create `my-agency.toml`:

   ```toml
   [pack]
   name = "My Agency"
   version = "1.0.0"
   author = "My Agency Ltd"
   homepage = "https://my-agency.example"

   [[rule]]
   id = "old-shop-links"
   name = "Links to the old shop"
   kind = "issue"
   priority = "high"
   description = "The page links to shop.example.com, which closes in March."
   fix = "Link to /store/ instead."
   when = { selector = 'a[href*="shop.example.com"]' }

   [[rule]]
   id = "no-cookie-banner"
   name = "Cookie banner missing"
   description = "Every page must load the consent banner."
   fix = "Add the banner include to the page template."
   when = { selector = '#cookie-banner', exists = false }
   ```

2. Check it: `shouting-toad rules validate my-agency.toml`.
3. Run it: `shouting-toad crawl https://example.com --rules my-agency.toml`.

The rules appear as `my-agency/old-shop-links` and `my-agency/no-cookie-banner` (the pack id
comes from the file name), under a category named after the pack.

## Format reference

A pack is one `[pack]` table and one or more `[[rule]]` tables. Unknown keys are errors, so
typos are caught when the pack loads.

### `[pack]`

| Key | Required | Meaning |
| --- | --- | --- |
| `name` | yes | Display name, e.g. `"WordPress"`. Also the default category of its rules. |
| `id` | no | Prefix of the rule ids. Lower-case letters, digits, `-`, `_`. Defaults to the file name. |
| `version` | no | Your pack's version, e.g. `"1.2.0"`. |
| `author` | no | Who maintains it. Shown in the app and in `rules list`. |
| `homepage` | no | Where the pack lives. The app links to it. |
| `description` | no | One or two sentences about what the pack checks. |
| `min_version` | no | Oldest ShoutingToad version the pack works with, e.g. `"1.1.0"`. Older versions refuse the pack with a clear message. |

### `[[rule]]`

| Key | Required | Meaning |
| --- | --- | --- |
| `id` | yes | Unique within the pack. Lower-case letters, digits, `-`, `_`. The full id is `pack-id/rule-id`, which is what `--fail-on-check` and the CSV exports use. |
| `name` | yes | Short title, e.g. `"Default tagline is still set"`. |
| `when` | yes | The [condition](#conditions) a URL must meet to be reported. |
| `category` | no | A built-in category (`"Security"`, `"Content"`, `"Structured Data"`, … matched case-insensitively) or a new one. Defaults to the pack name. |
| `kind` | no | `"issue"`, `"warning"` (default) or `"opportunity"`. |
| `priority` | no | `"high"`, `"medium"` (default) or `"low"`. Issues at or above the `--fail-on` level fail CI. |
| `description` | no | What the finding means and why it matters. |
| `fix` | no | How to fix it. |
| `scope` | no | Which URLs the rule looks at, see below. Default `"html"`. |

`kind` and `priority` also weigh the rule in the health score, exactly like built-in checks.

### Scopes

| Scope | URLs |
| --- | --- |
| `html` (default) | Internal HTML pages that returned 200. |
| `indexable` | Internal HTML pages that are indexable: 200, no noindex, canonical to themselves, not blocked. |
| `internal` | Every internal URL: pages, redirects, errors, images, CSS, JavaScript, PDFs. |
| `external` | External URLs (link checks only fetch their status and headers). |
| `all` | Internal and external URLs. |
| `images`, `css`, `js` | URLs of that type, internal or external. |

robots.txt and sitemap files are never in scope.

## Conditions

`when` is a table with exactly one test, or a list of tables that must all match:

```toml
when = { url = '/products/' }                          # one test
when = [ { url = '/products/' }, { word_count = '< 150' } ]   # all must match
```

Write regular expressions and selectors in single-quoted TOML strings (`'...'`): nothing
is escaped inside them, so `'\d+'` means what it says.

### Combining tests

| Test | Matches when |
| --- | --- |
| `all = [ {…}, {…} ]` | every condition in the list matches |
| `any = [ {…}, {…} ]` | at least one matches |
| `not = {…}` | the condition does not match |

They nest: `{ all = [ { url = '/blog/' }, { not = { any = [ {…}, {…} ] } } ] }`.

### Page content (CSS selectors and HTML)

| Test | Extra keys | Matches when |
| --- | --- | --- |
| `selector = 'css'` | | at least one element matches the CSS selector |
| | `exists = false` | no element matches |
| | `count = '>= 3'` | the number of matching elements passes the comparison |
| | `attr = 'name'` | only elements that have this attribute count |
| | `matches = 'regex'` | only elements whose text (or, with `attr`, attribute value) matches count |
| `html = 'regex'` | `exists`, `count` | the raw HTML source matches; `count` compares the number of matches |

Examples:

```toml
{ selector = 'h1', count = '> 1' }                       # more than one H1
{ selector = 'img:not([alt])' }                          # an image without alt
{ selector = 'meta[name="generator" i]', attr = 'content', matches = '^WordPress \d' }
{ selector = 'script[type="application/ld+json"]', matches = '"@type"\s*:\s*"Product"', exists = false }
{ html = '<!--\s*TODO' }                                 # a TODO comment in the source
```

Selectors support CSS Level 4 as implemented by the `selectors` crate (attribute operators
and the `i` flag, `:not()`, `:nth-child()`, combinators). `html` sees the page exactly as
served, including comments and scripts, which the DOM-based tests do not.

These tests only run on HTML pages. On any other URL (an image, a PDF, a redirect) they do
not match, whatever `exists` says.

### Response

| Test | Extra keys | Matches when |
| --- | --- | --- |
| `url = 'regex'` | | the full URL matches |
| `content_type = 'regex'` | | the Content-Type header matches |
| `header = 'name'` | | the response has this header (name is case-insensitive) |
| | `matches = 'regex'` | a header of that name has a matching value |
| | `exists = false` | the header is missing (with `matches`: no value matches) |

Regexes are [Rust regex syntax](https://docs.rs/regex/latest/regex/#syntax). They are case
sensitive; start with `(?i)` to ignore case. They match anywhere in the text unless you
anchor them with `^` and `$`. Look-around and back-references are not supported, which is
also why a regex can never hang a crawl.

### Numbers

Compare a number with `field = 300` (equal) or a string such as `'>= 2'`, `'< 300'`,
`'!= 0'`, `'<= 1.5'`.

| Field | Value |
| --- | --- |
| `status` | HTTP status code (0 when there was no response) |
| `response_ms` | response time in milliseconds |
| `size` | body size in bytes after decompression |
| `transfer_size` | bytes on the wire |
| `depth` | clicks from the start page |
| `inlinks` | internal links pointing at the URL |
| `word_count` | words of visible text (HTML only) |
| `title_length` | characters in the first `<title>` (HTML only, 0 when missing) |
| `description_length` | characters in the first meta description (HTML only, 0 when missing) |
| `h1_count` | number of H1 headings (HTML only) |
| `outlinks` | number of `<a href>` links on the page (HTML only) |
| `images` | number of images on the page (HTML only) |

Fields marked "HTML only" do not match on URLs that are not HTML pages.

### How rules run

Tests that need the page source (`selector` and `html`) run in the crawler while it parses
each page, and only their match counts are kept (two bytes per test per page), so they add
little memory even at 50,000 pages. Identical tests in different rules or packs are only
evaluated once. Every other test runs in the audit on data the crawl already stores.

So after you add or change a pack, `selector` and `html` rules need a new crawl. Reopening a
saved crawl, or re-running the audit, applies URL, header and number rules at once; rules
with page-source tests that the crawl did not evaluate stay silent rather than guess.

## Testing a pack

1. `shouting-toad rules validate my-pack.toml` checks the syntax and every regex and
   selector, and reports problems as `file:line: rule 'id': message`:

   ```text
   error: my-pack.toml:14: rule 'old-shop-links': when: invalid CSS selector "a[href": …
   error: my-pack.toml:21: rule 'slow': when: response_ms: 'slow' is not a comparison: use a number or e.g. ">= 2", "< 300", "!= 0"
   ```

2. Crawl a site where you know the answer, small and slow:
   `shouting-toad crawl https://example.com --rules my-pack.toml --max-urls 100 --threads 4 --delay 150`.
   Open `issue_urls.csv` and check each URL a rule reports with your browser or `curl`.
   Look for false positives as much as for hits.
3. Turn a rule's finding into a CI gate with `--fail-on-check my-pack/rule-id`.

Before you share a pack, check that each rule:

- reports something a site owner can act on, with a `fix` that says how;
- is true on real sites: test it on a few, including ones that do it right;
- has the right `kind`: an Issue is broken, a Warning is likely a problem, an Opportunity is
  an improvement;
- uses the narrowest `scope` that works (`indexable` for duplicate-content rules).

## Sharing a pack

- **Publish it on GitHub** in a repository with the topic
  [`shoutingtoad-rules`](https://github.com/topics/shoutingtoad-rules), so others can find it.
  Say in the README which ShoutingToad version you tested with, set `homepage` to the
  repository and bump `version` when rules change.
- **Send it upstream.** Packs that are useful to many sites are welcome as pull requests to the
  [`rules/`](../rules) folder. Run `shouting-toad rules validate rules/` and include in the pull
  request the sites you tested each rule on.
- **Keep it in your project.** A pack checked into a site's repository can gate every deploy:
  `--rules .seo/rules.toml --fail-on high`.

Rule packs are data, not code: they cannot run programs, read files or make requests, so
loading a pack from someone else is safe. A pack can only report findings.
