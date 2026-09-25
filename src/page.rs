//! The HTML for GitHoot's own pull-request pages.
//!
//! Pure functions only: every one of them takes what it needs and returns a `String`, so the whole
//! page is assertable without a socket, a browser or a clock. `serve` is the I/O half and holds no
//! rendering; this module holds no I/O. That split is what lets the security-critical parts — the
//! escaping and the URL guard — be tested directly rather than inferred from a screenshot.
//!
//! The page exists because GitHub's own search pages cannot express what two of the three bars count,
//! and because opening one browser tab per pull request was the alternative.

use crate::portal::types::{CheckRollup, PrEntry, ReviewState, Reviewer};
use crate::portal::{AuthStatus, AuthStyle, PortalInfo, PortalStatus, SignInPrompt};

/// Everything the settings page shows about portals, and the two markers a redirect back from a
/// button carries: the id whose sign-in was just asked for, and the id just signed out of. Both
/// exist because the poll thread may not have published the outcome by the time the browser follows
/// the redirect, so the page says what was asked and reloads once to catch up.
#[derive(Clone, Copy)]
pub struct PortalsView<'a> {
    pub portals: &'a [PortalStatus],
    pub signin_started: Option<&'a str>,
    pub signed_out: Option<&'a str>,
    pub now_unix: u64,
    /// Names the copy button's script in the CSP. Empty means no script is emitted.
    pub nonce: &'a str,
}

/// Which of GitHoot's own pages is showing. They share one line of links at the top.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tab {
    Settings,
    Accounts,
    Muted,
    Integrations,
}

/// The nav line, shared by the four pages so each can be single-purpose and still one click from
/// the others.
///
/// **Why four pages and not one.** They used to share the settings page, and two of them fought the
/// settings form: a sign-in's auto-refresh reloaded the page under a half-edited form, and the
/// dispatcher's own buttons reloaded it too. Anything that reloads now lives on a page with no form
/// of yours on it. The Integrations tab and each integration's page count as one tab.
#[derive(Clone, Copy, Debug)]
pub struct Nav {
    pub current: Tab,
    /// How many pull requests are muted, shown on the tab when there are any.
    pub muted: usize,
}

fn nav(token: &str, n: &Nav) -> String {
    let muted = if n.muted > 0 { format!("Muted ({})", n.muted) } else { "Muted".to_string() };
    let tabs = vec![
        (Tab::Settings, "settings", "Settings".to_string()),
        (Tab::Accounts, "accounts", "Accounts".to_string()),
        (Tab::Muted, "muted", muted),
        (Tab::Integrations, "integrations", "Integrations".to_string()),
    ];
    let links: Vec<String> = tabs
        .into_iter()
        .map(|(tab, path, label)| {
            if tab == n.current {
                format!("<span class=\"on\" aria-current=\"page\">{}</span>", esc(&label))
            } else {
                format!("<a href=\"/{}/{path}\">{}</a>", esc(token), esc(&label))
            }
        })
        .collect();
    format!("<nav class=\"tabs\">{}</nav>\n", links.join(""))
}

/// One integration as the Integrations tab lists it.
pub struct IntegrationRow<'a> {
    pub id: &'a str,
    pub name: &'a str,
    pub summary: &'a str,
    pub status: String,
}

/// Everything the generic part of an integration's page shows, as plain data.
pub struct IntegrationView<'a> {
    pub id: &'a str,
    pub name: &'a str,
    pub summary: &'a str,
    pub installed: bool,
    /// Why this build cannot run it. No Install button then, and the reason instead.
    pub unsupported: Option<&'a str>,
    /// Tools it needs that will not run. Only ever asked while installed, because spawning processes
    /// to draw a card for something nobody installed is waste.
    pub missing: &'a [&'a str],
    /// The outcome of the last button press, shown once.
    pub flash: Option<&'a str>,
    /// What the last Dry run said, in the order a pass produced it. Empty until one is asked for.
    pub dry_run: &'a [String],
    /// Its declared settings with their current values, in declaration order.
    pub settings: Vec<SettingRow<'a>>,
    /// What the integration adds below. Already HTML, escaped by the integration.
    pub body: String,
}

/// One declared setting as its form shows it.
pub struct SettingRow<'a> {
    pub key: &'a str,
    pub label: &'a str,
    /// The text as the file has it. Unused for a flag.
    pub value: String,
    /// What an empty text value means. Unused for a flag.
    pub placeholder: &'a str,
    /// `Some(on)` for a flag, drawn as a checkbox; `None` for text.
    pub flag: Option<bool>,
}

/// "Installed", "Not installed", and the two states that must not read as either.
pub fn integration_status(installed: bool, unsupported: Option<&str>, missing: &[&str]) -> String {
    match (unsupported, installed, missing.is_empty()) {
        (Some(_), _, _) => "Not available here".to_string(),
        (None, false, _) => "Not installed".to_string(),
        (None, true, true) => "Installed".to_string(),
        // Installed but unable to do anything is not "installed" in any sense that matters to the reader.
        (None, true, false) => format!("Installed, but idle: missing {}", missing.join(", ")),
    }
}

/// What a PR page needs to offer mute links: where to post them, and which pull requests are muted
/// right now and until when. `None` wherever a page is rendered without them (most tests), which
/// renders exactly what the page rendered before mutes existed.
#[derive(Clone, Copy)]
pub struct Mutes<'a> {
    pub token: &'a str,
    pub axis: PrAxis,
    pub now_unix: u64,
    pub until: &'a dyn Fn(&str) -> Option<u64>,
}

impl Mutes<'_> {
    fn of(&self, e: &PrEntry) -> Option<u64> {
        (self.until)(e.key())
    }
}

/// One portal's share of an axis page: who it is, and what it last confirmed.
///
/// `entries` carries the distinction the whole page is built around: `None` is "no confirmed list" —
/// the axis is off, has never answered, or the track gave up — and `Some(&[])` is a confirmed empty.
/// Rendering the first as "0 pull requests" would assert something GitHoot does not know, which is
/// the one thing this codebase refuses to do.
///
/// With one group the page renders exactly as it did before portals existed: no heading, the
/// group's own inbox in the empty states and the footer. With several, each group gets a heading
/// and its own empty state, because "nothing here" on GitLab says nothing about GitHub.
pub struct PortalGroup<'a> {
    pub info: &'a PortalInfo,
    pub entries: Option<&'a [PrEntry]>,
}

/// The groups for one axis, from what the poll loop last published.
pub fn groups(from: &[(PortalInfo, Option<Vec<PrEntry>>)]) -> Vec<PortalGroup<'_>> {
    from.iter().map(|(info, entries)| PortalGroup { info, entries: entries.as_deref() }).collect()
}
use crate::icons;
use crate::state::PrAxis;
use std::time::Duration;

/// Escapes text for HTML, in both element content and double-quoted attribute values.
///
/// `&` first, or an already-escaped `&lt;` would come back as `&amp;amp;lt;`. `'` is escaped as
/// `&#39;` rather than `&apos;`, which is not an HTML4 entity. Every attribute this page writes is
/// double-quoted, so escaping `"` is what matters there; `'` is escaped anyway, so a later change of
/// quoting style cannot silently open a hole.
pub fn esc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}

/// The URL as an `href`, or `None` when it must not become a link.
///
/// **`esc` is not enough for an `href`.** `javascript:alert(1)` contains not one escapable character
/// and would survive untouched into a link the page invites the user to click. So this is an
/// allowlist of exactly one prefix, not a denylist of schemes: `url` comes from the portal's own
/// `url` field on a pull request, which on GitHub is always `https://github.com/owner/repo/pull/N`,
/// and anything else is either a payload the portal could not have sent or one we should not be
/// following. The caller renders a rejected URL as escaped text, so nothing is hidden — it just is
/// not clickable.
///
/// `prefix` is the portal's `PortalInfo::link_prefix`, so a GitLab pull request under the GitLab
/// group is a link and the same URL under the GitHub group is text. The trailing slash in the prefix
/// is load-bearing: without it `https://github.com.evil.com/` passes.
pub fn safe_url<'a>(url: &'a str, prefix: &str) -> Option<&'a str> {
    url.starts_with(prefix).then_some(url)
}

/// GitHub's `updatedAt` as a short relative age.
///
/// Parsed by hand from the fixed-width `YYYY-MM-DDTHH:MM:SSZ` GitHub always sends. A date crate for
/// one subtitle would be a new dependency in a crate where every one is argued for, and `log.rs`
/// already does the same arithmetic in the other direction for its timestamps.
///
/// Anything unparseable falls back to the raw text: honest about what GitHub said, and it cannot
/// panic. A timestamp in the future — a local clock a few seconds behind GitHub's — reads "just now"
/// rather than a negative age.
pub fn age(iso: &str, now_unix: u64) -> String {
    let Some(then) = unix_from_iso(iso) else { return iso.to_string() };
    let elapsed = now_unix.saturating_sub(then);
    match elapsed {
        0..=59 => "just now".to_string(),
        60..=3599 => format!("{} m", elapsed / 60),
        3600..=86_399 => format!("{} h", elapsed / 3600),
        _ => format!("{} d", elapsed / 86_400),
    }
}

/// `YYYY-MM-DDTHH:MM:SSZ` to a unix timestamp, or `None` if it is not that shape.
fn unix_from_iso(iso: &str) -> Option<u64> {
    let b = iso.as_bytes();
    if b.len() < 19 || b[4] != b'-' || b[7] != b'-' || b[10] != b'T' || b[13] != b':' || b[16] != b':'
    {
        return None;
    }
    let num = |r: std::ops::Range<usize>| iso.get(r)?.parse::<i64>().ok();
    let (y, mo, d) = (num(0..4)?, num(5..7)?, num(8..10)?);
    let (h, mi, sec) = (num(11..13)?, num(14..16)?, num(17..19)?);
    if !(1..=12).contains(&mo) || !(1..=31).contains(&d) {
        return None;
    }
    // Howard Hinnant's days-from-civil: exact for every proleptic Gregorian date, no tables, no
    // leap-year special cases beyond the ones already folded into the arithmetic.
    let y = if mo <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let doy = (153 * (if mo > 2 { mo - 3 } else { mo + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    u64::try_from(days * 86_400 + h * 3600 + mi * 60 + sec).ok()
}

/// The accent for `axis`, taken from the tray icon rather than written out again here.
fn accent(axis: PrAxis) -> String {
    icons::css_hex(match axis {
        PrAxis::ReviewRequested => icons::REVIEW_DOT_COLOR,
        PrAxis::ReadyToMerge => icons::MERGE_DOT_COLOR,
        PrAxis::ChangesRequested => icons::CHANGES_DOT_COLOR,
    })
}

/// The page's own heading for `axis` — what the bar means, not what the menu entry does.
fn heading(axis: PrAxis) -> &'static str {
    match axis {
        PrAxis::ReviewRequested => "Awaiting your review",
        PrAxis::ReadyToMerge => "Approved",
        PrAxis::ChangesRequested => "Work required",
    }
}

/// The checks pill: a CSS class and the words beside it.
///
/// `Unknown` is deliberately neutral. GitHub answers `null` for a repository with no checks at all,
/// and a refused field degrades to the same value, so red here would be a claim neither case supports.
fn checks_pill(checks: CheckRollup) -> (&'static str, &'static str) {
    match checks {
        CheckRollup::Success => ("checks-success", "Checks passing"),
        CheckRollup::Pending => ("checks-pending", "Checks running"),
        CheckRollup::Failure => ("checks-failure", "Checks failing"),
        CheckRollup::Error => ("checks-failure", "Checks errored"),
        CheckRollup::Expected => ("checks-pending", "Checks expected"),
        CheckRollup::Unknown => ("checks-unknown", "Checks unknown"),
    }
}

/// Everything the page needs that never changes: one `&'static str`, zero interpolation.
///
/// The accent is the only colour that varies, and it arrives as a `--accent` custom property that
/// `axis_page` appends **after** this sheet. This sheet therefore does not define `--accent` at all:
/// it did once, as a grey default written before the axis colour, and later-wins meant every page
/// rendered grey while the test that checked for the right hex still passed, because the hex was
/// present — just overridden. One definition, and it is the axis's.
///
/// Keeping the sheet itself constant is what makes `style-src 'unsafe-inline'` safe in the CSP:
/// nothing user-controlled can reach it.
///
/// **The accent is ornament, never text.** `#1AC94A` on white is about 2:1 contrast, unreadable as
/// prose. It is used for borders, dots and filled badges with white text — never for a text colour on
/// the page background.
const STYLESHEET: &str = "\
:root{--bg:#f6f7f9;--card:#fff;--ink:#1c1f23;--dim:#5b6570;--line:#e3e6ea;\
--ok:#1A7F3C;--warn:#8a5a00;--bad:#b02525}\
@media(prefers-color-scheme:dark){:root{--bg:#14171a;--card:#1c2024;--ink:#e8eaed;--dim:#9aa4ae;\
--line:#2b3136;--ok:#4ad07a;--warn:#e0a44a;--bad:#f07070}}\
*{box-sizing:border-box}\
body{margin:0;background:var(--bg);color:var(--ink);\
font:15px/1.5 system-ui,-apple-system,Segoe UI,Roboto,sans-serif}\
main{max-width:56rem;margin:0 auto;padding:1.5rem 1rem 3rem}\
header{display:flex;align-items:center;gap:.75rem;margin-bottom:.25rem}\
header img{width:36px;height:36px}\
h1{font-size:1.4rem;margin:0;font-weight:650}\
.rule{height:4px;border-radius:2px;background:var(--accent);margin:.6rem 0 1.25rem}\
.sub{color:var(--dim);font-size:.85rem;margin:0}\
.card{background:var(--card);border:1px solid var(--line);border-left:4px solid var(--accent);\
border-radius:8px;padding:.85rem 1rem;margin-bottom:.6rem}\
.card h2{font-size:1rem;margin:0 0 .3rem;font-weight:600}\
.card a{color:inherit;text-decoration:none}\
.card a:hover{text-decoration:underline}\
.meta{color:var(--dim);font-size:.83rem;display:flex;flex-wrap:wrap;gap:.5rem;align-items:center}\
.pill{font-size:.72rem;padding:.12rem .5rem;border-radius:999px;border:1px solid var(--line);\
white-space:nowrap}\
.draft{background:var(--accent);color:#fff;border-color:transparent}\
.checks-success{color:var(--ok)}.checks-pending{color:var(--warn)}\
.checks-failure{color:var(--bad)}.checks-unknown{color:var(--dim)}\
.who{margin:.45rem 0 0;padding:0;list-style:none;font-size:.83rem;color:var(--dim)}\
.who li{display:inline-block;margin-right:.8rem}\
.dot{display:inline-block;width:.5rem;height:.5rem;border-radius:50%;margin-right:.3rem;\
vertical-align:baseline}\
.dot-ok{background:var(--ok)}.dot-no{background:var(--bad)}.dot-wait{background:var(--dim)}\
.empty{background:var(--card);border:1px dashed var(--line);border-radius:8px;padding:1.5rem;\
text-align:center;color:var(--dim)}\
footer{margin-top:2rem;color:var(--dim);font-size:.78rem;text-align:center}\
footer a,.empty a{color:var(--dim)}\
.section{font-size:.85rem;text-transform:uppercase;letter-spacing:.04em;color:var(--dim);\
margin:1.5rem 0 .5rem;font-weight:600}\
.section a{font-weight:400;color:var(--dim);text-decoration:underline;text-underline-offset:.25em}\
.section a:hover{color:var(--ink)}\
.row{display:flex;align-items:center;gap:.6rem;padding:.35rem 0;cursor:pointer}\
.row input{width:1rem;height:1rem;accent-color:var(--accent);flex:none}\
button{background:var(--accent);color:#fff;border:0;border-radius:8px;padding:.6rem 1.4rem;\
font:inherit;font-weight:600;cursor:pointer}\
code{background:var(--bg);padding:.1rem .3rem;border-radius:4px}\
.actions{display:flex;flex-wrap:wrap;gap:.6rem;align-items:center;margin-top:.6rem}\
.tabs{display:flex;flex-wrap:wrap;gap:1.2rem;margin:.2rem 0 1.2rem;padding-bottom:.4rem;border-bottom:1px solid var(--line)}\
.tabs a{color:var(--dim);text-decoration:none}\
.tabs a:hover{color:var(--ink)}\
.tabs .on{color:var(--ink);font-weight:600;box-shadow:0 .45rem 0 -.25rem var(--accent)}\
.mute{display:flex;flex-wrap:wrap;gap:.35rem;align-items:center;margin-top:.5rem;font-size:.85rem;color:var(--dim)}\
.mute form{display:inline;margin:0}\
button.link{background:none;border:0;padding:0;color:var(--dim);font:inherit;font-weight:400;\
text-decoration:underline;cursor:pointer}\
button.link:hover{color:var(--ink)}\
.muted-head{margin-top:1.6rem}\
.path{display:flex;flex-wrap:wrap;gap:.5rem;align-items:center;margin:.5rem 0 0;font-size:.9rem}\
.path span{color:var(--dim);flex:0 0 auto}\
.path input{flex:1 1 16rem;min-width:0;font:inherit;padding:.35rem .5rem;border:1px solid var(--line);\
border-radius:6px;background:var(--bg);color:inherit}\
.dry{margin:.6rem 0 0;padding:.6rem .7rem;background:var(--bg);border:1px solid var(--line);\
border-radius:8px;font:.82rem/1.5 ui-monospace,SFMono-Regular,Menlo,Consolas,monospace;\
white-space:pre-wrap;overflow-wrap:anywhere;color:var(--dim)}\
.actions form{margin:0}\
.small{padding:.4rem 1rem;font-size:.9rem}\
textarea{display:block;box-sizing:border-box;width:100%;min-height:14rem;margin:.3rem 0 1rem;\
padding:.6rem .7rem;border:1px solid var(--line);border-radius:8px;background:var(--bg);color:inherit;\
font:.9rem/1.45 ui-monospace,SFMono-Regular,Menlo,Consolas,monospace;resize:vertical}\
.code-row{display:flex;gap:.6rem;align-items:stretch;margin:.6rem 0}\
.code-row input{flex:0 0 auto;width:11ch;box-sizing:content-box;text-align:center;font:inherit;\
font-size:1.5rem;font-weight:700;letter-spacing:.15em;padding:.55rem .6rem;border:1px solid var(--line);\
border-radius:8px;background:var(--bg);color:inherit}\
.portal{font-size:.85rem;font-weight:600;letter-spacing:.04em;opacity:.7;margin:1.4rem 0 .5rem}";

/// One axis's whole page.
///
/// `entries` carries the distinction the whole page is built around: `None` is "no confirmed list" —
/// the axis is off, has never answered, or the track gave up — and `Some(&[])` is a confirmed empty.
/// Rendering the first as "0 pull requests" would assert something GitHoot does not know, which is
/// the one thing this codebase refuses to do.
///
/// `now_unix` is injected rather than read from the clock, the same trick `github::classify_with`
/// uses, so every age string on the page is assertable.
/// How often the page asks whether anything has changed, in milliseconds.
///
/// Short, because asking is nearly free: the request is conditional, so between polls the answer is
/// a bodyless `304`. It never reaches GitHub — it reads what the poll loop last stored — so the only
/// cost of asking often is a few bytes over loopback.
const REFRESH_MS: u32 = 5_000;

/// The one script on the page.
///
/// Deliberately small enough to read in full, because it is the only executable thing GitHoot serves
/// and it runs under a CSP that names it by nonce. It fetches, compares, and swaps — no framework,
/// no dependencies, no state beyond the last items it saw.
///
/// Three things in it are not obvious:
///
/// - **The age is computed here, not sent.** That is what makes a real `304` honest: "as of 47 s
///   ago" changes every second, so a response carrying it could never be unchanged, and the server
///   would have to resend the whole list every few seconds to keep one line current. Instead the
///   server sends how old the data was *when it answered*, the page anchors a local clock to that,
///   and the age ticks on with no request at all.
/// - **The list is replaced only when the server actually sends a new one.** Rebuilding it would
///   drop hover, focus and any text selection inside, for cards that usually have not moved.
/// - **It stops after a run of failures.** When GitHoot quits, this tab would otherwise poll a dead
///   port for as long as it stays open.
/// - **It pauses while the tab is hidden**, and catches up the moment it is shown again, so a
///   forgotten tab costs nothing.
const REFRESH_SCRIPT: &str = "(function(){var asof=document.getElementById('asof'),count=document.getElementById('count'),items=document.getElementById('items');var url=location.pathname.replace(/\\/$/,'')+'/items',tag=null,base=null,fails=0,timer;function ago(s){return s<60?s+' s':s<3600?((s/60)|0)+' m':s<86400?((s/3600)|0)+' h':((s/86400)|0)+' d';}function paint(){asof.textContent=base===null?'not polled yet':'as of '+ago(Math.max(0,(Date.now()-base)/1000|0))+' ago';}function tick(){if(document.hidden)return;fetch(url,{cache:'no-store',headers:tag?{'If-None-Match':tag}:{}}).then(function(r){if(r.status===304){paint();return null;}if(!r.ok)throw 0;tag=r.headers.get('ETag');return r.json();}).then(function(j){fails=0;if(!j)return;base=j.age===null?null:Date.now()-j.age*1000;count.textContent=j.count;items.innerHTML=j.items;paint();}).catch(function(){if(++fails>=5){clearInterval(timer);asof.textContent='GitHoot is not running';}});}timer=setInterval(tick,REFRESH_MS);document.addEventListener('visibilitychange',function(){if(!document.hidden)tick();});tick();})();";

/// The count half of the summary line, as plain text — the script sets it with `textContent`.
///
/// Separate from the age because the two change on different clocks: the count only when a poll
/// publishes, the age every second. Keeping them apart is what lets the age tick locally while the
/// server answers `304`.
fn count_text(groups: &[PortalGroup], mutes: Option<&Mutes>) -> String {
    // Only confirmed lists are counted, and a page with none confirmed says nothing rather than "0".
    // Muted pull requests are listed but not counted, the same as on the icon.
    let confirmed: Vec<usize> = groups
        .iter()
        .filter_map(|g| g.entries.map(|list| list.iter().filter(|e| mutes.and_then(|m| m.of(e)).is_none()).count()))
        .collect();
    if confirmed.is_empty() {
        String::new()
    } else {
        format!("{} pull request(s) · ", confirmed.iter().sum::<usize>())
    }
}

/// The age half, rendered server-side for the first paint. The script takes over after that.
fn age_text(polled: Option<Duration>) -> String {
    match polled {
        Some(d) => format!("as of {} ago", age_of(d)),
        None => "not polled yet".to_string(),
    }
}

/// The cards, or whichever empty state applies. Shared by the page and the refresh fragment, so the
/// two can never render the list differently.
///
/// One group renders bare, as the page always did. Several get a heading each, so a card can be
/// placed, and each its own empty state, so "nothing here" is said per portal rather than once for
/// all of them. No groups at all — nothing configured, or nothing published yet — is the "not
/// known" state with nowhere to send anyone.
fn items(groups: &[PortalGroup], now_unix: u64, mutes: Option<&Mutes>) -> String {
    match groups {
        [] => "<div class=\"empty\"><p>This list is <strong>not known</strong> right now — GitHoot has \
               no answer it still stands behind for this bar.</p></div>\n"
            .to_string(),
        [one] => group_items(one, now_unix, mutes),
        many => many
            .iter()
            .map(|g| {
                format!(
                    "<h2 class=\"portal\">{}</h2>\n{}",
                    esc(&g.info.display_name),
                    group_items(g, now_unix, mutes)
                )
            })
            .collect(),
    }
}

/// One portal's cards, or its empty state, with its own inbox as the way out.
fn group_items(g: &PortalGroup, now_unix: u64, mutes: Option<&Mutes>) -> String {
    let inbox = || {
        portal_link(
            &g.info.link_prefix,
            &g.info.inbox_url,
            &format!("Open your pull requests on {}", g.info.display_name),
        )
    };
    match g.entries {
        // No confirmed list. Say that, and offer somewhere to go rather than a dead end.
        None => format!(
            "<div class=\"empty\"><p>This list is <strong>not known</strong> right now — GitHoot has \
             no answer it still stands behind for this bar.</p><p>{}</p></div>\n",
            inbox()
        ),
        Some(list) => {
            let (held, active): (Vec<&PrEntry>, Vec<&PrEntry>) =
                newest_first(list).into_iter().partition(|e| mutes.and_then(|m| m.of(e)).is_some());
            let mut h = if active.is_empty() {
                format!("<div class=\"empty\"><p>Nothing here right now.</p><p>{}</p></div>\n", inbox())
            } else {
                active.iter().map(|e| card(e, &g.info.link_prefix, now_unix, &mute_links(mutes, e))).collect()
            };
            // At the bottom and under a heading of its own: still on the page, so nothing is hidden,
            // but out of the way of what actually needs you.
            if !held.is_empty() {
                h.push_str(&format!(
                    "<h2 class=\"section muted-head\">Muted · <a href=\"/{}/muted\">all muted</a></h2>\n",
                    esc(mutes.map_or("", |m| m.token))
                ));
                for e in held {
                    h.push_str(&card(e, &g.info.link_prefix, now_unix, &mute_links(mutes, e)));
                }
            }
            h
        }
    }
}

/// "Mute for 3 days · 7 days · 30 days", or "Muted, back in 5 days · Unmute".
///
/// Forms styled as links, not links: a mute changes state, so it is a `POST` behind the same `Origin`
/// check as every other write, and a `GET` that a prefetcher could follow must never mute anything.
fn mute_links(mutes: Option<&Mutes>, e: &PrEntry) -> String {
    let Some(m) = mutes else { return String::new() };
    let form = |days: u64, label: &str| {
        format!(
            "<form method=\"post\" action=\"/{}/{}/mute\"><input type=\"hidden\" name=\"key\" value=\"{}\">\
             <input type=\"hidden\" name=\"days\" value=\"{days}\"><button class=\"link\" type=\"submit\">{label}</button></form>",
            esc(m.token),
            m.axis.slug(),
            esc(e.key()),
        )
    };
    match m.of(e) {
        None => format!(
            "<div class=\"mute\">Mute for {} · {} · {}</div>",
            form(3, "3 days"),
            form(7, "7 days"),
            form(30, "30 days")
        ),
        Some(until) => format!(
            "<div class=\"mute\">Muted, back in {} · {}</div>",
            esc(&crate::mute::remaining(until, m.now_unix)),
            form(0, "Unmute")
        ),
    }
}

/// What the refresh fetches: the age line and the list, kept apart so only what changed is replaced.
///
/// JSON rather than a bare HTML fragment precisely so the two can travel separately. `serde_json`
/// does the escaping, which is what makes it safe to build by hand from strings that already contain
/// markup.
pub fn items_json(groups: &[PortalGroup], polled: Option<Duration>, now_unix: u64, mutes: Option<&Mutes>) -> String {
    serde_json::json!({
        // How old the data was *at this instant*, not a rendered age. The page anchors its own clock
        // to it, so the line keeps counting between polls without asking again.
        "age": polled.map(|d| d.as_secs()),
        "count": count_text(groups, mutes),
        "items": items(groups, now_unix, mutes),
    })
    .to_string()
}

pub fn axis_page(
    axis: PrAxis,
    groups: &[PortalGroup],
    polled: Option<Duration>,
    token: &str,
    now_unix: u64,
    nonce: &str,
    mutes: Option<&Mutes>,
) -> String {
    let mut h = String::with_capacity(4096);
    let title = heading(axis);

    h.push_str("<!doctype html>\n<html lang=\"en\">\n<head>\n");
    h.push_str("<meta charset=\"utf-8\">\n");
    h.push_str("<meta name=\"viewport\" content=\"width=device-width,initial-scale=1\">\n");
    // Belt and braces with the `Referrer-Policy` header: without either, the first click through to
    // github.com hands GitHub this page's URL, token included.
    // `same-origin`, not `no-referrer`: under `no-referrer` a form POST carries `Origin: null` and the
    // mute links would be refused by the CSRF check. Nothing reaches github.com either way, because
    // `same-origin` sends nothing cross-origin and every outbound link carries `rel="noreferrer"`.
    h.push_str("<meta name=\"referrer\" content=\"same-origin\">\n");
    h.push_str(&format!("<title>{} — GitHoot</title>\n", esc(title)));
    h.push_str(&format!("<link rel=\"icon\" href=\"/{}/owl.png\">\n", esc(token)));
    // The accent goes last: later wins, and this is the only definition of `--accent` there is.
    h.push_str(&format!("<style>{STYLESHEET}\n:root{{--accent:{}}}</style>\n", accent(axis)));
    h.push_str("</head>\n<body>\n<main>\n");

    h.push_str(&format!(
        "<header><img src=\"/{}/owl.png\" alt=\"\" width=\"36\" height=\"36\"><h1>{}</h1></header>\n",
        esc(token),
        esc(title)
    ));

    h.push_str(&format!(
        "<p class=\"sub\"><span id=\"count\">{}</span><span id=\"asof\">{}</span></p>\n",
        esc(&count_text(groups, mutes)),
        esc(&age_text(polled))
    ));
    h.push_str("<div class=\"rule\"></div>\n");
    h.push_str(&format!("<div id=\"items\">{}</div>\n", items(groups, now_unix, mutes)));

    // One link per portal, the first portal's first. With none there is nowhere to send anyone.
    let inboxes: Vec<String> = groups
        .iter()
        .map(|g| {
            portal_link(
                &g.info.link_prefix,
                &g.info.inbox_url,
                &format!("Your pull requests on {}", g.info.display_name),
            )
        })
        .collect();
    h.push_str("<footer>Rendered locally by GitHoot from its last poll, and kept current without a reload.");
    for inbox in inboxes {
        h.push_str(&format!(" · {inbox}"));
    }
    h.push_str("</footer>\n");
    h.push_str(&format!(
        "<script nonce=\"{}\">{}</script>\n",
        esc(nonce),
        REFRESH_SCRIPT.replace("REFRESH_MS", &REFRESH_MS.to_string())
    ));
    h.push_str("</main>\n</body>\n</html>\n");
    h
}

/// The entries in the order the page shows them: most recently updated first.
///
/// Sorted here rather than trusted from the poll, because GitHub's search answers in **"best match"
/// relevance** order whenever the query names no sort — and none of the three does. For queries this
/// narrow that ordering is effectively arbitrary, so the same pull requests could come back in a
/// different order on the next poll and the list would shuffle under the reader between two reloads.
/// A list you are meant to scan has to sit still.
///
/// Done on the rendered copy, not on what the poll stored: the order is a presentation decision, and
/// `scheduler`'s snapshot stays exactly what GitHub said.
///
/// An entry with no `updatedAt` sorts **last**. Treating a missing date as the epoch would be the
/// other obvious choice and it is the wrong one: it puts the entry GitHoot knows least about at the
/// bottom either way, but only this way round does it stay out of the top of the list, which is the
/// part anyone actually reads.
fn newest_first(list: &[PrEntry]) -> Vec<&PrEntry> {
    let mut sorted: Vec<&PrEntry> = list.iter().collect();
    // `sort_by_key` cannot borrow from the element, so this is `sort_by` with the same comparison.
    // Stable, so pull requests sharing a timestamp keep GitHub's relative order rather than swapping.
    sorted.sort_by(|a, b| {
        let key = |e: &PrEntry| e.updated_at.as_deref().and_then(unix_from_iso);
        match (key(a), key(b)) {
            (Some(x), Some(y)) => y.cmp(&x),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => std::cmp::Ordering::Equal,
        }
    });
    sorted
}

/// The settings form.
///
/// A plain HTML form and nothing else — no script, so the page's CSP stays `default-src 'none'` and
/// only `form-action` has to open up. Every control posts its own key name, so what the browser sends
/// and what lands in `config.txt` are the same words, and `Config::from_form` is the only thing that
/// turns one into the other.
///
/// `saved` is how many keys the last submission actually changed, for the banner. `None` means the
/// page was opened rather than submitted.
///
/// `portals` is every configured portal with how its sign-in stands, and `signin_started` is the id
/// of the portal whose sign-in the previous request just asked for, from the redirect back. That
/// banner exists because the poll thread may not have published "in progress" yet by the time the
/// browser follows the redirect, and a page that still offered the button would invite a second
/// click while the first device code dialog is on its way up.
pub fn settings_page(cfg: &crate::config::Config, token: &str, restarts: &[&str], tabs: &Nav) -> String {
    let mut h = shell("Settings", crate::icons::css_hex(crate::icons::MERGE_DOT_COLOR), token, None);
    h.push_str(&nav(token, tabs));
    if !restarts.is_empty() {
        h.push_str(&format!(
            "<div class=\"empty\"><strong>Saved.</strong> {} need{} a restart to take effect: {}.</div>\n",
            if restarts.len() == 1 { "One setting" } else { "Some settings" },
            if restarts.len() == 1 { "s" } else { "" },
            esc(&restarts.join(", "))
        ));
    }
    h.push_str(&format!("<form method=\"post\" action=\"/{}/settings\">\n", esc(token)));

    h.push_str("<h2 class=\"section\">Pull request signals</h2><div class=\"card\">");
    for (axis, what) in [
        (PrAxis::ReviewRequested, "Somebody wants your review"),
        (PrAxis::ReadyToMerge, "Your pull request was approved"),
        (PrAxis::ChangesRequested, "Your pull request needs work"),
    ] {
        h.push_str(&checkbox(crate::config::pr_key(axis), what, cfg.pr_enabled(axis)));
    }
    h.push_str("</div>\n");

    h.push_str("<h2 class=\"section\">Behaviour</h2><div class=\"card\">");
    h.push_str(&checkbox("copilotReviews", "Count Copilot's unresolved comments as work", cfg.copilot_reviews));
    h.push_str(&checkbox("sound", "Hoot when a pull request needs you", cfg.sound));
    h.push_str(&checkbox("updateCheck", "Check for a newer release", cfg.update_check));
    // The label says what it opens, not just what it offers. An EDR or firewall prompt at the next
    // launch has to be traceable to a box somebody deliberately ticked.
    h.push_str(&checkbox(
        "localApi",
        "Serve the lists as JSON to local scripts (opens the local port at startup)",
        cfg.local_api,
    ));
    h.push_str("</div>\n");

    h.push_str("<h2 class=\"section\">Log detail</h2><div class=\"card\">");
    for (value, what) in [
        ("error", "Failures only"),
        ("info", "Add lifecycle detail, for diagnosing"),
    ] {
        let on = matches!(cfg.log_level, crate::log::Level::Info) == (value == "info");
        h.push_str(&format!(
            "<label class=\"row\"><input type=\"radio\" name=\"logLevel\" value=\"{value}\"{}> {what}</label>",
            if on { " checked" } else { "" }
        ));
    }
    h.push_str("</div>\n");

    h.push_str(
        "<h2 class=\"section\">Which parts of GitHub count as an outage</h2>\n\
         <p class=\"sub\">GitHub's page-wide verdict says \"degraded\" whenever any single component \
         is, including the ones a pull-request tray never touches. Tick nothing to watch the whole \
         page.</p><div class=\"card\">",
    );
    for name in crate::config::all_components() {
        let on = cfg.status_components.iter().any(|c| c == name);
        h.push_str(&format!(
            "<label class=\"row\"><input type=\"checkbox\" name=\"statusComponents\" value=\"{}\"{}> {}</label>",
            esc(name),
            if on { " checked" } else { "" },
            esc(name)
        ));
    }
    h.push_str("</div>\n");

    h.push_str("<p><button type=\"submit\">Save</button></p>\n</form>\n");
    h.push_str(
        "<footer>Written to <code>config.txt</code>, one line per changed setting — your comments \
         and any keys this version has never heard of are left alone.</footer>\n",
    );
    h.push_str("</main>\n</body>\n</html>\n");
    h
}

/// The portals and their sign-ins. Its own page because a sign-in reloads the page every few seconds
/// until it lands, and on the settings page that reload threw away whatever you were editing.
pub fn accounts_page(token: &str, view: &PortalsView, tabs: &Nav) -> String {
    let PortalsView { portals, signin_started, signed_out, now_unix, nonce } = *view;
    // Reload every few seconds while a sign-in is in flight, or has just been asked for, so the
    // code appears without a click and the card turns to "Signed in" on its own.
    // A redirect marker means the poll thread was just asked for something and the page should
    // catch up as soon as it plausibly has: one second. A running flow reloads at the slower pace.
    let refresh = if signin_started.is_some() || signed_out.is_some() {
        Some(CATCH_UP_REFRESH_SECS)
    } else if portals.iter().any(|p| matches!(p.auth, AuthStatus::SigningIn(_))) {
        Some(SIGN_IN_REFRESH_SECS)
    } else {
        None
    };
    let mut h = shell("Accounts", crate::icons::css_hex(crate::icons::MERGE_DOT_COLOR), token, refresh.map(|secs| (secs, "accounts")));
    h.push_str(&nav(token, tabs));
    if let Some(started) = signin_started.and_then(|id| portals.iter().find(|p| p.info.id.0 == id)) {
        h.push_str(&format!(
            "<div class=\"empty\"><strong>Sign-in to {} started.</strong> This page refreshes itself; \
             what to do appears below in a moment.</div>\n",
            esc(&started.info.display_name)
        ));
    }

    if let Some(gone) = signed_out.and_then(|id| portals.iter().find(|p| p.info.id.0 == id)) {
        h.push_str(&format!(
            "<div class=\"empty\"><strong>Signed out of {}.</strong> The saved credential was \
             deleted; sign in again whenever you like.</div>\n",
            esc(&gone.info.display_name)
        ));
    }

    // Its own forms, outside the settings form below: a form cannot nest, and a sign-in is an
    // action rather than a setting to save.
    h.push_str("<h2 class=\"section\">Portals</h2>\n");
    let mut code_on_screen = false;
    for portal in portals {
        h.push_str(&portal_card(portal, token, now_unix));
        code_on_screen |= matches!(portal.auth, AuthStatus::SigningIn(Some(_)));
    }

    // The one script this page ever carries, and only while there is a code to copy. Named by
    // nonce like the PR page's refresh script, so the CSP stays `default-src 'none'` otherwise.
    if code_on_screen && !nonce.is_empty() {
        h.push_str(&format!("<script nonce=\"{}\">{COPY_SCRIPT}</script>\n", esc(nonce)));
    }
    h.push_str("</main>\n</body>\n</html>\n");
    h
}

/// The Integrations tab: every integration this build knows, installed or not.
pub fn integrations_page(token: &str, rows: &[IntegrationRow], tabs: &Nav) -> String {
    let mut h = shell("Integrations", crate::icons::css_hex(crate::icons::MERGE_DOT_COLOR), token, None);
    h.push_str(&nav(token, tabs));
    h.push_str(
        "<p class=\"sub\">What GitHoot may do with the pull requests it finds, beyond showing them. \
         Each is off until you install it, and removing one keeps its settings and files.</p>\n",
    );
    for row in rows {
        h.push_str(&format!(
            "<div class=\"card\"><div class=\"row\"><strong><a href=\"/{}/integrations/{}\">{}</a></strong> · \
             <span class=\"portal-status\">{}</span></div><p class=\"sub\">{}</p></div>\n",
            esc(token),
            esc(row.id),
            esc(row.name),
            esc(&row.status),
            esc(row.summary),
        ));
    }
    h.push_str("</main>\n</body>\n</html>\n");
    h
}

/// One integration's page: the generic card, its settings, then whatever it adds.
pub fn integration_page(token: &str, v: &IntegrationView, tabs: &Nav) -> String {
    let mut h = shell(v.name, crate::icons::css_hex(crate::icons::MERGE_DOT_COLOR), token, None);
    h.push_str(&nav(token, tabs));
    h.push_str(&integration_card(token, v));
    h.push_str(&v.body);
    h.push_str("</main>\n</body>\n</html>\n");
    h
}

/// Copies the device code and opens the sign-in address in a new tab when the button is clicked,
/// then says so on the button for a moment.
///
/// `navigator.clipboard` needs a secure context, which `*.localhost` is in every current browser;
/// where it is refused anyway the code is selected instead, one keystroke from copied. The address
/// comes from the button's own `data-url`, which the page sets only for an allowlisted URL, and the
/// tab is opened inside the click handler so popup blockers let it through. No dependencies, no
/// state, nothing that runs before a click.
const COPY_SCRIPT: &str = "(function(){var b=document.getElementById('copy-code'),i=document.getElementById('device-code');if(!b||!i)return;var url=b.getAttribute('data-url');function done(){b.textContent='Copied';setTimeout(function(){b.textContent='Copy and open';},1500);if(url){window.open(url,'_blank','noopener,noreferrer');}}function copy(){if(navigator.clipboard&&navigator.clipboard.writeText){return navigator.clipboard.writeText(i.value).catch(function(){i.select();});}i.select();try{document.execCommand('copy');}catch(e){}return Promise.resolve();}b.addEventListener('click',function(){copy().then(done,done);});})();";

/// How often the settings page reloads while a sign-in runs. Short enough that the code shows
/// within a moment of the click and the card flips to "Signed in" soon after the browser finishes;
/// long enough not to fight the user reading the code.
const SIGN_IN_REFRESH_SECS: u32 = 3;

/// How soon the page reloads after a button's redirect, to show what the poll thread made of the
/// click. The click is handled within milliseconds; the second is the browser's round trip.
const CATCH_UP_REFRESH_SECS: u32 = 1;

/// One portal on the settings page: its name, how its sign-in stands, and one button. Sign in while
/// not signed in; Sign out while signed in, which deletes the saved credential; Cancel while a
/// sign-in runs. Only a dead end the portal declared (nothing installed, switched off in the file)
/// gets no button, because a sign-in would change nothing there.
fn portal_card(portal: &PortalStatus, token: &str, now_unix: u64) -> String {
    let name = esc(&portal.info.display_name);
    let action = |field: &str, label: &str| {
        format!(
            "<form method=\"post\" action=\"/{}/settings/authenticate\"><input type=\"hidden\" \
             name=\"portal\" value=\"{}\">{}<button type=\"submit\">{label}</button></form>",
            esc(token),
            esc(&portal.info.id.0),
            if field.is_empty() {
                String::new()
            } else {
                format!("<input type=\"hidden\" name=\"{field}\" value=\"1\">")
            }
        )
    };
    let (status, form) = match &portal.auth {
        AuthStatus::SignedIn => ("Signed in".to_string(), action("signout", "Sign out")),
        AuthStatus::NotSignedIn => (
            format!("Not signed in. <span class=\"sub\">{}</span>", sign_in_hint(&portal.info)),
            action("", &format!("Sign in to {name}")),
        ),
        AuthStatus::SigningIn(None) => ("Starting sign-in…".to_string(), action("cancel", "Cancel")),
        AuthStatus::SigningIn(Some(prompt)) => {
            (sign_in_step(&portal.info, prompt, now_unix), action("cancel", "Cancel"))
        }
        AuthStatus::Off(reason) => (esc(reason), String::new()),
    };
    // A `div`, not a `p`: the running sign-in puts a block (the code row) inside the status.
    format!(
        "<div class=\"card\"><div class=\"row\"><strong>{name}</strong> · <span class=\"portal-status\">{status}</span></div>{form}</div>\n"
    )
}

/// What the user has to do right now: the code, where to enter it, and how long they have. The
/// link opens in a new tab on purpose, the one place this app does that: this page has to stay open
/// to show the outcome, and the code is also on the clipboard.
///
/// The address came over the network, so it is a link only under the portal's own prefix, like
/// every other URL on these pages; anything else is shown as text.
fn sign_in_step(info: &PortalInfo, prompt: &SignInPrompt, now_unix: u64) -> String {
    let left = prompt.expires_at.saturating_sub(now_unix);
    let deadline = if left == 0 {
        "The code has expired; cancel and start again.".to_string()
    } else {
        format!("Expires in {} min.", left.div_ceil(60))
    };
    // The button opens the address only when it passed the allowlist; otherwise it only copies,
    // and the address is shown as text for the user to judge.
    let safe = safe_url(&prompt.url, &info.link_prefix);
    let where_ = match safe {
        Some(url) => format!(
            "<a href=\"{}\" target=\"_blank\" rel=\"noreferrer noopener\">{}</a>",
            esc(url),
            esc(url)
        ),
        None => esc(&prompt.url),
    };
    let open = safe.map(|url| format!(" data-url=\"{}\"", esc(url))).unwrap_or_default();
    format!(
        "Enter this code at {where_}. {deadline}<div class=\"code-row\"><input id=\"device-code\" \
         class=\"device-code\" type=\"text\" readonly value=\"{}\" aria-label=\"Device code\">\
         <button type=\"button\" id=\"copy-code\"{open}>Copy and open</button></div><span class=\"sub\">\
         This page updates itself when you are done.</span>",
        esc(&prompt.code)
    )
}

/// What the sign-in will look like, so the button is not a surprise.
fn sign_in_hint(info: &PortalInfo) -> String {
    match info.capabilities.auth_style {
        AuthStyle::DeviceFlow => format!(
            "A code and a link to {} appear here; enter the code there to finish.",
            esc(&info.display_name)
        ),
        AuthStyle::PastedToken => {
            format!("You will be asked for a token you created on {}.", esc(&info.display_name))
        }
    }
}

/// What the settings route says when `serve::install` was never called.
pub fn settings_unavailable(token: &str) -> String {
    let mut h = shell("Settings", crate::icons::css_hex(crate::icons::REVIEW_DOT_COLOR), token, None);
    h.push_str(
        "<div class=\"empty\">Settings are not available in this run.</div>\n</main>\n</body>\n</html>\n",
    );
    h
}

/// One row of the muted page: a live mute, and the pull request it names if a bar still holds it.
pub struct MutedRow<'a> {
    pub key: &'a str,
    pub until: u64,
    /// The bar it was found in, the entry, and that portal's link prefix. `None` when no bar holds it
    /// any more, typically because it was merged or closed while muted.
    pub found: Option<(PrAxis, &'a PrEntry, &'a str)>,
}

/// Every muted pull request in one place, across all three bars, each with Unmute.
///
/// This exists because a muted pull request can otherwise be unreachable: an empty bar hides its
/// menu entry, so a bar whose only pull request is muted has no page to unmute it from. Reached from
/// the settings page, and from the muted section of any bar.
pub fn muted_page(rows: &[MutedRow], token: &str, now_unix: u64, tabs: &Nav) -> String {
    let mut h = shell("Muted pull requests", crate::icons::css_hex(crate::icons::MERGE_DOT_COLOR), token, None);
    h.push_str(&nav(token, tabs));
    h.push_str("<p class=\"sub\">A muted pull request stays out of its bar until the mute ends, then comes back as new.</p>\n");
    if rows.is_empty() {
        h.push_str("<div class=\"empty\"><p>Nothing is muted.</p></div>\n");
    }
    let unmute = |key: &str, until: u64| {
        format!(
            "<div class=\"mute\">Muted, back in {} · <form method=\"post\" action=\"/{}/muted\">\
             <input type=\"hidden\" name=\"key\" value=\"{}\"><button class=\"link\" type=\"submit\">Unmute</button></form></div>",
            esc(&crate::mute::remaining(until, now_unix)),
            esc(token),
            esc(key)
        )
    };
    for axis in PrAxis::ALL {
        let here: Vec<_> = rows.iter().filter(|r| r.found.is_some_and(|(a, _, _)| a == axis)).collect();
        if here.is_empty() {
            continue;
        }
        h.push_str(&format!("<h2 class=\"section\">{}</h2>\n", esc(heading(axis))));
        for r in here {
            let (_, e, prefix) = r.found.expect("filtered to found rows");
            h.push_str(&card(e, prefix, now_unix, &unmute(r.key, r.until)));
        }
    }
    let gone: Vec<_> = rows.iter().filter(|r| r.found.is_none()).collect();
    if !gone.is_empty() {
        h.push_str("<h2 class=\"section\">No longer in any bar</h2>\n");
        h.push_str("<p class=\"sub\">Merged, closed, or simply not waiting on you right now. The mute ends by itself; \
                    unmute it to have it count as new the next time it turns up.</p>\n");
        for r in gone {
            h.push_str(&format!("<div class=\"card\"><h2><code>{}</code></h2>{}</div>\n", esc(r.key), unmute(r.key, r.until)));
        }
    }
    h.push_str("</main>\n</body>\n</html>\n");
    h
}

/// Whether it is installed, what it is missing, the switch, the rehearsal, and its settings.
///
/// The switch sits on the page that explains the integration, because that is the right page to turn
/// it on from. Dry run is offered installed or not, and that is the point: the only safe way to learn
/// what installing it would do is to ask first.
fn integration_card(token: &str, v: &IntegrationView) -> String {
    let status = integration_status(v.installed, v.unsupported, v.missing);
    let action = format!("/{}/integrations/{}", esc(token), esc(v.id));
    let button = |name: &str, label: &str| {
        format!(
            "<form method=\"post\" action=\"{action}\"><input type=\"hidden\" name=\"action\" value=\"{name}\">\
             <button class=\"small\" type=\"submit\">{label}</button></form>"
        )
    };

    let mut notes = String::new();
    if let Some(why) = v.unsupported {
        notes.push_str(&format!("<p class=\"sub\">{}</p>", esc(why)));
    } else if v.installed && !v.missing.is_empty() {
        notes.push_str(&format!(
            "<p class=\"sub\">Put <code>{}</code> on your PATH. Until then it does nothing.</p>",
            v.missing.iter().map(|m| esc(m)).collect::<Vec<_>>().join("</code>, <code>")
        ));
    }
    let flash = v.flash.map(|m| format!("<p class=\"sub\"><strong>{}</strong></p>", esc(m))).unwrap_or_default();
    let switch = match (v.unsupported, v.installed) {
        (Some(_), true) => button("remove", "Remove"),
        (Some(_), false) => String::new(),
        (None, true) => button("remove", "Remove"),
        (None, false) => button("install", "Install"),
    };
    let dry = if v.unsupported.is_some() { String::new() } else { button("dryrun", "Dry run") };
    let said = if v.dry_run.is_empty() {
        String::new()
    } else {
        format!("<pre class=\"dry\">{}</pre>", v.dry_run.iter().map(|l| esc(l)).collect::<Vec<_>>().join("\n"))
    };

    let mut h = format!(
        "<h2 class=\"section\">{name}</h2>\n<div class=\"card\"><div class=\"row\"><strong>{name}</strong> · \
         <span class=\"portal-status\">{status}</span></div><p class=\"sub\">{summary}</p>{notes}{flash}\
         <div class=\"actions\">{switch}{dry}</div>{said}</div>\n",
        name = esc(v.name),
        status = esc(&status),
        summary = esc(v.summary),
    );
    if !v.settings.is_empty() {
        h.push_str(&format!(
            "<h2 class=\"section\">Settings</h2>\n<div class=\"card\"><form method=\"post\" action=\"{action}\">\
             <input type=\"hidden\" name=\"action\" value=\"settings\">"
        ));
        for row in &v.settings {
            match row.flag {
                Some(on) => h.push_str(&checkbox(row.key, row.label, on)),
                None => h.push_str(&format!(
                    "<label class=\"path\"><span>{}</span><input type=\"text\" name=\"{}\" value=\"{}\" placeholder=\"{}\" spellcheck=\"false\"></label>",
                    esc(row.label),
                    esc(row.key),
                    esc(&row.value),
                    esc(row.placeholder)
                )),
            }
        }
        h.push_str("<button class=\"small\" type=\"submit\">Save settings</button></form></div>\n");
    }
    h
}

fn checkbox(name: &str, label: &str, on: bool) -> String {
    format!(
        "<label class=\"row\"><input type=\"checkbox\" name=\"{}\" value=\"on\"{}> {}</label>",
        esc(name),
        if on { " checked" } else { "" },
        esc(label)
    )
}

/// Everything both kinds of page share: head, stylesheet, owl and heading.
/// `refresh_secs` makes the page reload itself, the one way a page with no script can follow
/// something changing on the poll thread. Used only while a sign-in is running.
fn shell(title: &str, accent: String, token: &str, refresh: Option<(u32, &str)>) -> String {
    let mut h = String::with_capacity(4096);
    h.push_str("<!doctype html>\n<html lang=\"en\">\n<head>\n");
    h.push_str("<meta charset=\"utf-8\">\n");
    if let Some((secs, path)) = refresh {
        // To the plain address, so a `?signin=` marker from a redirect is carried exactly once and
        // the page does not reload forever on the strength of a click long finished.
        h.push_str(&format!(
            "<meta http-equiv=\"refresh\" content=\"{secs};url=/{}/{path}\">\n",
            esc(token)
        ));
    }
    h.push_str("<meta name=\"viewport\" content=\"width=device-width,initial-scale=1\">\n");
    // `same-origin`, not `no-referrer`: the latter also nulls the `Origin` header on this page's own
    // form POST, which is the CSRF check's only evidence. See `serve::Referrer`.
    h.push_str("<meta name=\"referrer\" content=\"same-origin\">\n");
    h.push_str(&format!("<title>{} — GitHoot</title>\n", esc(title)));
    h.push_str(&format!("<link rel=\"icon\" href=\"/{}/owl.png\">\n", esc(token)));
    h.push_str(&format!("<style>{STYLESHEET}\n:root{{--accent:{accent}}}</style>\n"));
    h.push_str("</head>\n<body>\n<main>\n");
    h.push_str(&format!(
        "<header><img src=\"/{}/owl.png\" alt=\"\" width=\"36\" height=\"36\"><h1>{}</h1></header>\n",
        esc(token),
        esc(title)
    ));
    h.push_str("<div class=\"rule\"></div>\n");
    h
}

/// A `Duration` as the same short form `age` produces, for the "as of" line.
fn age_of(d: Duration) -> String {
    let s = d.as_secs();
    match s {
        0..=59 => format!("{s} s"),
        60..=3599 => format!("{} m", s / 60),
        3600..=86_399 => format!("{} h", s / 3600),
        _ => format!("{} d", s / 86_400),
    }
}

/// An outbound link. Every one on this page goes through here, so `noreferrer` cannot be forgotten on
/// one of them — and it is the load-bearing half: without it the click hands GitHub this page's URL,
/// token included, in the `Referer` header.
///
/// No `target="_blank"`. Links replace the page, because opening a tab per click is the habit this
/// page exists to get away from, and the back button is a better way back to the list than a pile of
/// tabs. `noopener` goes with `_blank`: it exists to deny the opened tab a handle back to this one,
/// and a same-window navigation has no opened tab to deny.
fn portal_link(prefix: &str, url: &str, text: &str) -> String {
    match safe_url(url, prefix) {
        Some(safe) => {
            format!("<a href=\"{}\" rel=\"noreferrer\">{}</a>", esc(safe), esc(text))
        }
        None => esc(text),
    }
}

/// One pull request.
fn card(e: &PrEntry, link_prefix: &str, now_unix: u64, actions: &str) -> String {
    let title = e.title.as_deref().unwrap_or("(untitled)");
    // A URL that is not the portal's own is shown but not offered as a link — see `safe_url`.
    let headline = match safe_url(&e.url, link_prefix) {
        Some(url) => format!("<a href=\"{}\" rel=\"noreferrer\">{}</a>", esc(url), esc(title)),
        None => esc(title),
    };

    let mut meta = String::new();
    let where_ = match (e.repo.as_deref(), e.number) {
        (Some(repo), Some(n)) => format!("{repo} #{n}"),
        (Some(repo), None) => repo.to_string(),
        (None, Some(n)) => format!("#{n}"),
        (None, None) => String::new(),
    };
    if !where_.is_empty() {
        meta.push_str(&format!("<span>{}</span>", esc(&where_)));
    }
    if let Some(author) = &e.author {
        meta.push_str(&format!("<span>{}</span>", esc(author)));
    }
    if let Some(updated) = &e.updated_at {
        meta.push_str(&format!("<span>updated {}</span>", esc(&age(updated, now_unix))));
    }
    if e.is_draft {
        meta.push_str("<span class=\"pill draft\">Draft</span>");
    }
    // Load-bearing, not decoration: a conflict and Copilot's comments are two of the three reasons a
    // pull request is on the work-required page at all, and neither leaves a review verdict behind.
    // Without these the list shows rows it cannot explain.
    if e.conflicting {
        meta.push_str("<span class=\"pill checks-failure\">Merge conflict</span>");
    }
    if let Some(bot) = &e.bot_review {
        let plural = if bot.unresolved == 1 { "" } else { "s" };
        meta.push_str(&format!(
            "<span class=\"pill checks-pending\">{} unresolved {} comment{plural}</span>",
            bot.unresolved,
            esc(&bot.name)
        ));
    }
    let (class, words) = checks_pill(e.checks);
    meta.push_str(&format!("<span class=\"pill {class}\">{words}</span>"));

    let mut who = String::new();
    for v in &e.verdicts {
        let (dot, what) = match v.state {
            ReviewState::Approved => ("dot-ok", "approved"),
            ReviewState::ChangesRequested => ("dot-no", "requested changes"),
        };
        who.push_str(&format!(
            "<li><span class=\"dot {dot}\"></span>{} {what}</li>",
            esc(&v.login)
        ));
    }
    for r in &e.pending {
        let name = match r {
            Reviewer::User(login) => login.clone(),
            // A team is named by its slug; it has no login, which is why `still_on_you` cannot match
            // one. Saying so beats a row that looks like nobody is being waited on.
            Reviewer::Team(slug) => format!("team {slug}"),
        };
        who.push_str(&format!(
            "<li><span class=\"dot dot-wait\"></span>{} re-review pending</li>",
            esc(&name)
        ));
    }
    let who = if who.is_empty() { String::new() } else { format!("<ul class=\"who\">{who}</ul>") };

    format!("<div class=\"card\"><h2>{headline}</h2><div class=\"meta\">{meta}</div>{who}{actions}</div>\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    const NAV: Nav = Nav { current: Tab::Settings, muted: 0 };

    const NOW: u64 = 1_789_000_000;

    fn entry(url: &str) -> PrEntry {
        PrEntry {
            id: None,
            url: url.to_string(),
            title: Some("Fix the bed-exit debounce".to_string()),
            repo: Some("qumea/care-api".to_string()),
            number: Some(2204),
            author: Some("octocat".to_string()),
            updated_at: Some("2026-09-15T09:12:33Z".to_string()),
            is_draft: false,
            conflicting: false,
            bot_review: None,
            checks: CheckRollup::Success,
            verdicts: vec![],
            pending: vec![],
        }
    }

    /// The one portal every existing install has, as the page sees it.
    static GITHUB: std::sync::LazyLock<PortalInfo> = std::sync::LazyLock::new(|| PortalInfo {
        id: crate::portal::PortalId("github".to_string()),
        kind: crate::portal::PortalKind::GitHub,
        display_name: "GitHub".to_string(),
        link_prefix: "https://github.com/".to_string(),
        inbox_url: "https://github.com/pulls/inbox".to_string(),
        status_page: None,
        capabilities: crate::portal::Capabilities {
            auth_style: crate::portal::AuthStyle::DeviceFlow,
            conflict_state: true,
            rereview_pending: true,
            team_reviewers: true,
            bot_reviewer: Some("Copilot"),
        },
        min_poll_interval: Duration::from_secs(60),
    });

    /// A second portal, so the grouping has something to group.
    static GITLAB: std::sync::LazyLock<PortalInfo> = std::sync::LazyLock::new(|| PortalInfo {
        id: crate::portal::PortalId("gitlab".to_string()),
        display_name: "GitLab".to_string(),
        link_prefix: "https://gitlab.example/".to_string(),
        inbox_url: "https://gitlab.example/dashboard/merge_requests".to_string(),
        ..GITHUB.clone()
    });

    fn group(entries: Option<&[PrEntry]>) -> PortalGroup<'_> {
        PortalGroup { info: &GITHUB, entries }
    }

    fn page(entries: Option<&[PrEntry]>) -> String {
        axis_page(PrAxis::ChangesRequested, &[group(entries)], Some(Duration::from_secs(47)), "deadbeef", NOW, "cafebabe", None)
    }

    // ── Escaping: the security surface ────────────────────────────────────────

    /// Ampersand first, or `&lt;` would come back as `&amp;amp;lt;`.
    #[test]
    fn escaping_covers_every_dangerous_character() {
        assert_eq!(esc("&"), "&amp;");
        assert_eq!(esc("&lt;"), "&amp;lt;");
        assert_eq!(esc("<b>"), "&lt;b&gt;");
        assert_eq!(esc("a \"b\" 'c'"), "a &quot;b&quot; &#39;c&#39;");
        assert_eq!(esc("plain"), "plain");
    }

    /// A PR title is text GitHub hands us from whoever opened the pull request. It reaches a page in
    /// the user's browser, so it is the one input on this page that has to be treated as hostile.
    #[test]
    fn a_title_that_is_an_injection_attempt_is_inert() {
        let mut e = entry("https://github.com/o/r/pull/1");
        e.title = Some(r#""><img src=x onerror=alert(1)>"#.to_string());
        let html = page(Some(&[e]));
        assert!(!html.contains("<img src=x"), "the tag must not survive as markup");
        assert!(html.contains("&lt;img src=x"), "it should be visible as text instead");
    }

    /// Escaping `"` is what stops a title closing an attribute and opening an event handler.
    #[test]
    fn an_attribute_break_out_is_escaped() {
        let mut e = entry("https://github.com/o/r/pull/1");
        e.title = Some(r#"a" onmouseover="evil()"#.to_string());
        let html = page(Some(&[e]));
        assert!(!html.contains(r#"onmouseover="evil()"#));
    }

    /// `esc` cannot save an `href`: `javascript:alert(1)` has not one escapable character in it, and
    /// would survive untouched into a link the user is invited to click. Only GitHub's own https URLs
    /// become links; anything else is rendered as text.
    #[test]
    fn only_github_https_urls_become_links() {
        assert_eq!(safe_url("https://github.com/o/r/pull/1", &GITHUB.link_prefix), Some("https://github.com/o/r/pull/1"));
        for hostile in [
            "javascript:alert(1)",
            "JavaScript:alert(1)",
            "data:text/html,<script>alert(1)</script>",
            "http://github.com/o/r/pull/1",
            "https://evil.com/",
            "https://github.com.evil.com/",
            "",
        ] {
            assert_eq!(safe_url(hostile, &GITHUB.link_prefix), None, "{hostile} must not become a link");
        }
    }

    #[test]
    fn a_rejected_url_renders_as_text_not_a_link() {
        let mut e = entry("javascript:alert(1)");
        e.title = Some("Sneaky".to_string());
        let html = page(Some(&[e]));
        assert!(!html.contains("href=\"javascript:"));
        assert!(!html.contains("href=\"j"));
    }

    /// **Exactly one script, and it is ours.**
    ///
    /// The page ran nothing at all until the live refresh arrived. It now runs one small script, so
    /// the rule changed from "none" to "one, named by nonce" — and that is the property worth
    /// holding: a second `<script>` appearing here would be one nobody wrote on purpose, and the CSP
    /// would refuse it anyway for want of the nonce.
    #[test]
    fn exactly_one_script_is_emitted_and_it_carries_the_nonce() {
        for entries in [None, Some(&[][..]), Some(&[entry("https://github.com/o/r/pull/1")][..])] {
            let html = page(entries);
            assert_eq!(html.matches("<script").count(), 1, "got {html}");
            assert_eq!(html.matches(r#"<script nonce="cafebabe">"#).count(), 1);
        }
    }

    /// And hostile content still cannot become one. Escaping is the rule; the nonce is the backstop.
    #[test]
    fn a_title_cannot_introduce_a_script() {
        let mut e = entry("https://github.com/o/r/pull/1");
        e.title = Some("</script><script>alert(1)</script>".to_string());
        let html = page(Some(&[e]));
        assert_eq!(html.matches("<script").count(), 1, "only ours survives");
        assert!(html.contains("&lt;/script&gt;"));
    }

    /// Without this the first click through to github.com hands GitHub the page's own URL, token and
    /// all, in the `Referer` header.
    /// The footer used to offer "the same query on GitHub", built from the axis's own search string.
    /// It was not the same query — two of the three bars narrow their hits client-side, so no search
    /// page can match them — and the URL did not work either. The inbox is honest and always does.
    #[test]
    fn every_outbound_link_goes_to_the_pr_inbox() {
        for entries in [None, Some(&[][..])] {
            let html = page(entries);
            assert!(html.contains(&GITHUB.inbox_url), "got {html}");
            assert!(!html.contains("github.com/pulls?q="), "no hand-built search URL: {html}");
            assert!(!html.contains("same query"), "and no claim to be one: {html}");
        }
    }

    #[test]
    fn every_outbound_link_is_noreferrer() {
        let html = page(Some(&[entry("https://github.com/o/r/pull/1")]));
        let links = html.matches("<a ").count();
        assert!(links > 0, "the page should link somewhere");
        assert_eq!(html.matches("noreferrer").count(), links, "every link, not most of them");
    }

    /// Links replace the page rather than piling up tabs. Opening a new tab per click is the habit
    /// this whole page was built to get away from, and the back button is the way back to the list.
    #[test]
    fn no_link_opens_a_new_tab() {
        for entries in [None, Some(&[][..]), Some(&[entry("https://github.com/o/r/pull/1")][..])] {
            let html = page(entries);
            assert!(!html.contains("target="), "got: {html}");
        }
    }

    // ── What the page says ────────────────────────────────────────────────────

    /// `None` is "we do not know", and it must never be dressed up as a zero. Same refusal
    /// `PollState::pr_menu_label` makes about showing a count it no longer stands behind.
    #[test]
    fn an_unconfirmed_list_says_so_and_never_shows_a_zero() {
        let html = page(None);
        assert!(html.contains("not known") || html.contains("no confirmed list"), "got: {html}");
        assert!(!html.contains(">0<"), "an unknown list is not an empty one");
    }

    #[test]
    fn a_confirmed_empty_list_reads_as_nothing_here() {
        let html = page(Some(&[]));
        assert!(html.to_lowercase().contains("nothing"));
    }

    /// GitHub's search answers in "best match" relevance order when the query names no sort, which
    /// for these three queries is effectively arbitrary — the same poll twice can hand back the same
    /// pull requests in a different order. A list you are meant to scan has to sit still.
    #[test]
    fn the_list_is_newest_first_whatever_order_github_sent() {
        let at = |n: u64, iso: &str| {
            let mut e = entry(&format!("https://github.com/o/r/pull/{n}"));
            e.number = Some(n);
            e.updated_at = Some(iso.to_string());
            e
        };
        let scrambled = [
            at(2, "2026-09-10T00:00:00Z"),
            at(3, "2026-09-01T00:00:00Z"),
            at(1, "2026-09-14T00:00:00Z"),
        ];
        let html = page(Some(&scrambled));
        // The full span text, not "#1": the stylesheet is full of hex colours like `#1c1f23`.
        let seen: Vec<_> = [1, 2, 3]
            .map(|n| {
                let needle = format!("<span>qumea/care-api #{n}</span>");
                html.find(&needle).unwrap_or_else(|| panic!("missing {needle} in {html}"))
            })
            .into_iter()
            .collect();
        assert!(seen[0] < seen[1] && seen[1] < seen[2], "newest first, got {seen:?}");
    }

    /// An entry GitHub gave no `updatedAt` cannot be placed by date. It goes last rather than
    /// sorting as the epoch and displacing something real from the top of the list.
    #[test]
    fn an_entry_without_a_date_sorts_last_rather_than_oldest() {
        let mut undated = entry("https://github.com/o/r/pull/9");
        undated.number = Some(9);
        undated.updated_at = None;
        let mut old = entry("https://github.com/o/r/pull/8");
        old.number = Some(8);
        old.updated_at = Some("2020-01-01T00:00:00Z".to_string());
        let html = page(Some(&[undated, old]));
        let at = |n: u32| html.find(&format!("<span>qumea/care-api #{n}</span>")).expect("a card");
        assert!(at(8) < at(9), "the dated one comes first");
    }

    #[test]
    fn each_pr_shows_repo_number_author_and_age() {
        let html = page(Some(&[entry("https://github.com/o/r/pull/1")]));
        assert!(html.contains("qumea/care-api"));
        assert!(html.contains("#2204"));
        assert!(html.contains("octocat"));
        assert!(html.contains("Fix the bed-exit debounce"));
    }

    /// Copilot never approves and never requests changes, so a pull request can be on the
    /// work-required page for a reason with no verdict behind it. Saying so is what keeps the page
    /// from listing a PR it cannot explain.
    #[test]
    fn unresolved_copilot_comments_are_named_on_the_card() {
        let mut e = entry("https://github.com/o/r/pull/1");
        e.bot_review = Some(crate::portal::types::BotReview { name: "Copilot".into(), unresolved: 3 });
        let html = page(Some(&[e]));
        assert!(html.contains("3 unresolved Copilot comments"), "got: {html}");
    }

    /// Singular, because "1 unresolved Copilot comments" is the kind of thing people notice.
    #[test]
    fn one_copilot_comment_reads_as_one() {
        let mut e = entry("https://github.com/o/r/pull/1");
        e.bot_review = Some(crate::portal::types::BotReview { name: "Copilot".into(), unresolved: 1 });
        assert!(page(Some(&[e])).contains("1 unresolved Copilot comment<"));
    }

    #[test]
    fn no_copilot_comments_says_nothing() {
        assert!(!page(Some(&[entry("https://github.com/o/r/pull/1")])).contains("Copilot"));
    }

    /// The settings page's section headings are small uppercase labels. That style was added as a
    /// bare `h2` rule, and a PR title is also an `h2` — so every title on the PR pages rendered in
    /// capitals, from 2.0.0 to 2.0.3. The rule is class-scoped now, and this pins the selector.
    #[test]
    fn only_settings_section_headings_are_uppercased() {
        let uppercased: Vec<&str> = STYLESHEET
            .split('}')
            .filter(|rule| rule.contains("text-transform:uppercase"))
            .map(|rule| rule.split('{').next().unwrap_or("").trim())
            .collect();
        assert_eq!(uppercased, [".section"], "uppercase must be opt-in by class, never by element");

        let html = page(Some(&[entry("https://github.com/o/r/pull/1")]));
        assert!(html.contains("<h2><a "), "a PR title is a plain h2 with a link in it");
        assert!(!html.contains("class=\"section\""), "and the PR page has no section headings");
        assert!(settings(&default_cfg()).contains("<h2 class=\"section\">"));
    }

    #[test]
    fn a_draft_is_marked() {
        let mut e = entry("https://github.com/o/r/pull/1");
        e.is_draft = true;
        assert!(page(Some(&[e])).contains("Draft"));
    }

    /// A null rollup means "no checks configured" just as often as it means "could not read it", and
    /// neither is a failure. Painting it red would invent a problem the user then goes looking for.
    #[test]
    fn an_unknown_check_rollup_is_not_shown_as_failing() {
        let mut e = entry("https://github.com/o/r/pull/1");
        e.checks = CheckRollup::Unknown;
        let html = page(Some(&[e]));
        // The pill on the card, not the stylesheet, which of course defines every class.
        assert!(html.contains(r#"<span class="pill checks-unknown">"#));
        assert!(!html.contains(r#"<span class="pill checks-failure">"#));
    }

    #[test]
    fn reviewer_verdicts_are_named() {
        let mut e = entry("https://github.com/o/r/pull/1");
        e.verdicts = vec![
            crate::portal::types::Verdict { login: "alice".into(), state: ReviewState::Approved },
            crate::portal::types::Verdict { login: "bob".into(), state: ReviewState::ChangesRequested },
        ];
        e.pending = vec![Reviewer::User("carol".into()), Reviewer::Team("backend".into())];
        let html = page(Some(&[e]));
        for who in ["alice", "bob", "carol", "backend"] {
            assert!(html.contains(who), "{who} should be named");
        }
    }

    /// The accent is the tray's own colour, read from `icons`, so the page you land on is
    /// recognisable as the bar you clicked.
    #[test]
    fn the_page_carries_the_axis_colour_the_icon_draws() {
        for (axis, color) in [
            (PrAxis::ReviewRequested, icons::REVIEW_DOT_COLOR),
            (PrAxis::ReadyToMerge, icons::MERGE_DOT_COLOR),
            (PrAxis::ChangesRequested, icons::CHANGES_DOT_COLOR),
        ] {
            let html = axis_page(axis, &[group(Some(&[]))], None, "tok", NOW, "n", None);
            assert!(html.contains(&icons::css_hex(color)), "{axis:?} should wear its own colour");
        }
    }

    /// Three pages, three headings, and no page wearing another bar's words — the page you land on
    /// has to be the bar you clicked.
    /// The sheet must not carry an `--accent` of its own. It used to, as a grey default written
    /// *before* the axis colour, so later-wins painted every page grey while the colour test above
    /// still passed — the right hex was in the string, just overridden.
    #[test]
    fn the_axis_accent_is_the_only_definition_and_comes_last() {
        assert!(!STYLESHEET.contains("--accent:"), "the sheet must not define the accent");
        let html = axis_page(PrAxis::ReadyToMerge, &[group(Some(&[]))], None, "t", NOW, "n", None);
        let hex = icons::css_hex(icons::MERGE_DOT_COLOR);
        assert_eq!(html.matches("--accent:").count(), 1, "exactly one definition");
        let accent_at = html.find(&format!("--accent:{hex}")).expect("the axis colour");
        let sheet_at = html.find("--bg:").expect("the sheet");
        assert!(accent_at > sheet_at, "the accent has to win, so it has to come last");
    }

    #[test]
    fn every_axis_renders_and_names_only_itself() {
        for axis in PrAxis::ALL {
            let html = axis_page(axis, &[group(Some(&[]))], None, "tok", NOW, "n", None);
            assert!(html.contains(heading(axis)), "{axis:?} should name itself");
            for other in PrAxis::ALL.into_iter().filter(|o| *o != axis) {
                assert!(!html.contains(heading(other)), "{axis:?} must not name {other:?}");
            }
        }
    }

    #[test]
    fn the_page_declares_utf8_and_a_language() {
        let html = page(Some(&[]));
        assert!(html.contains(r#"<meta charset="utf-8">"#));
        assert!(html.contains(r#"<html lang="en">"#));
    }

    /// The owl is served, not inlined, so the CSP can stay at `img-src 'self'`.
    #[test]
    fn the_logo_is_fetched_from_our_own_token_path() {
        assert!(page(Some(&[])).contains(r#"src="/deadbeef/owl.png""#));
    }

    // ── Live refresh ──────────────────────────────────────────────────────────

    fn live(entries: Option<&[PrEntry]>) -> String {
        items_json(&[group(entries)], Some(Duration::from_secs(47)), NOW, None)
    }

    /// The fragment must render the *same* cards the page does, or a refresh would quietly swap the
    /// list for a second renderer's idea of it. One function, used by both.
    #[test]
    fn the_fragment_renders_exactly_what_the_page_does() {
        let e = entry("https://github.com/o/r/pull/1");
        let page_html = page(Some(std::slice::from_ref(&e)));
        let fragment = live(Some(&[e]));
        let items = fragment
            .split(r#""items":"#)
            .nth(1)
            .expect("an items field")
            .trim_end_matches('}');
        // The JSON string is escaped; the card markup still has to be in the page verbatim.
        let unescaped = items.trim_matches('"').replace("\\\"", "\"").replace("\\n", "\n");
        assert!(page_html.contains(&unescaped), "fragment:\n{unescaped}\n\npage:\n{page_html}");
    }

    /// Three fields, on purpose. The **age is a number of seconds, never a rendered string** — that
    /// is what lets the page tick it locally and the server answer `304` while it does.
    #[test]
    fn the_fragment_sends_an_age_a_count_and_the_items() {
        let json = live(Some(&[entry("https://github.com/o/r/pull/1")]));
        assert!(json.contains(r#""age":47"#), "got {json}");
        assert!(json.contains(r#""count":"1 pull request(s) · ""#), "got {json}");
        assert!(json.contains(r#""items":"#), "got {json}");
        assert!(json.starts_with('{') && json.ends_with('}'));
    }

    /// Nothing in the payload may carry a rendered age, or it would change every second and no
    /// response could ever be unchanged — which is the whole premise of the conditional request.
    #[test]
    fn nothing_in_the_fragment_carries_a_rendered_age() {
        let json = live(Some(&[entry("https://github.com/o/r/pull/1")]));
        assert!(!json.contains("as of"), "got {json}");
        assert!(!json.contains("ago"), "got {json}");
    }

    /// A poll that never happened says so with a null rather than a zero age.
    #[test]
    fn an_unpolled_fragment_sends_a_null_age() {
        let json = items_json(&[group(None)], None, NOW, None);
        assert!(json.contains(r#""age":null"#), "got {json}");
    }

    #[test]
    fn the_page_wires_up_the_live_region_and_the_script() {
        let html = page(Some(&[entry("https://github.com/o/r/pull/1")]));
        assert!(html.contains(r#"id="asof""#));
        assert!(html.contains(r#"id="items""#));
        assert!(html.contains(r#"nonce="cafebabe""#), "the script must carry its CSP nonce");
        assert!(html.contains("/items"), "it has to know where to fetch from");
    }

    /// The settings page has a form and no list, so it gets no refresh loop.
    #[test]
    fn the_settings_page_has_no_refresh_script() {
        assert!(!settings_page(&default_cfg(), "tok", &[], &NAV).contains("<script"));
    }

    // ── The settings page ─────────────────────────────────────────────────────

    fn settings(cfg: &crate::config::Config) -> String {
        settings_page(cfg, "tok", &[], &NAV)
    }

    fn portal(auth: AuthStatus) -> PortalStatus {
        PortalStatus { info: GITHUB.clone(), auth }
    }

    fn view<'a>(portals: &'a [PortalStatus], signin: Option<&'a str>) -> PortalsView<'a> {
        PortalsView { portals, signin_started: signin, signed_out: None, now_unix: NOW, nonce: "n" }
    }

    // ── Integrations ──────────────────────────────────────────────────────────

    fn integration(installed: bool, missing: &'static [&'static str]) -> IntegrationView<'static> {
        IntegrationView {
            id: "herdr",
            name: "Herdr dispatcher",
            summary: "Starts agents.",
            installed,
            unsupported: None,
            missing,
            flash: None,
            dry_run: &[],
            settings: vec![SettingRow { key: "cloneRoot", label: "Clones live in", value: "/d/projects".into(), placeholder: "~/projects", flag: None }],
            body: "<p>its own part</p>".to_string(),
        }
    }

    /// Not installed is the default and must read as plainly that, offering Install on the spot. A
    /// page that explains what a thing does is the right page to turn it on from.
    #[test]
    fn an_integration_not_installed_offers_install_and_a_rehearsal() {
        let html = integration_page("tok", &integration(false, &[]), &NAV);
        assert!(html.contains("portal-status\">Not installed<"), "{html}");
        assert!(html.contains(r#"value="install""#) && html.contains(">Install<"));
        assert!(!html.contains(r#"value="remove""#), "nothing to remove while it is not installed");
        assert!(html.contains(">Dry run<"), "and a way to rehearse before committing to it");
        assert!(html.contains(r#"action="/tok/integrations/herdr""#));
    }

    #[test]
    fn an_installed_integration_offers_the_way_back_out() {
        let html = integration_page("tok", &integration(true, &[]), &NAV);
        assert!(html.contains("portal-status\">Installed<"), "{html}");
        assert!(html.contains(r#"value="remove""#) && !html.contains(r#"value="install""#), "one switch, not two");
    }

    /// Installed with a tool gone must not read as healthy: it is switched on and doing nothing.
    #[test]
    fn an_installed_integration_missing_a_tool_says_it_is_idle() {
        let html = integration_page("tok", &integration(true, &["herdr", "gh"]), &NAV);
        assert!(html.contains("Installed, but idle: missing herdr, gh"), "{html}");
        assert!(html.contains("<code>herdr</code>, <code>gh</code>"));
    }

    /// Where the build cannot run it, the page says why and offers nothing that could only fail.
    #[test]
    fn an_unsupported_integration_offers_no_install() {
        let v = IntegrationView { unsupported: Some("Needs Linux or Windows."), ..integration(false, &[]) };
        let html = integration_page("tok", &v, &NAV);
        assert!(html.contains("Not available here") && html.contains("Needs Linux or Windows."));
        assert!(!html.contains(r#"value="install""#) && !html.contains(">Dry run<"));
    }

    /// Setting values are the user's text going into an attribute.
    #[test]
    fn integration_settings_are_a_form_of_their_own_with_escaped_values() {
        let v = IntegrationView {
            settings: vec![SettingRow { key: "cloneRoot", label: "Clones live in", value: "\"><script>".into(), placeholder: "~/projects", flag: None }],
            ..integration(true, &[])
        };
        let html = integration_page("tok", &v, &NAV);
        assert!(html.contains(r#"name="action" value="settings""#));
        assert!(html.contains(r#"name="cloneRoot""#) && !html.contains("\"><script>"));
        assert!(!html.contains(r#"action="/tok/settings">"#), "never the general settings form");
    }

    /// A flag is a checkbox in the same form, ticked for on.
    #[test]
    fn integration_flags_are_checkboxes_in_the_settings_form() {
        let v = IntegrationView {
            settings: vec![
                SettingRow { key: "approved", label: "Approved", value: "off".into(), placeholder: "", flag: Some(false) },
                SettingRow { key: "workRequired", label: "Work required", value: "on".into(), placeholder: "", flag: Some(true) },
            ],
            ..integration(true, &[])
        };
        let html = integration_page("tok", &v, &NAV);
        assert!(html.contains(r#"<input type="checkbox" name="approved" value="on"> Approved"#), "{html}");
        assert!(html.contains(r#"<input type="checkbox" name="workRequired" value="on" checked> Work required"#));
        assert!(!html.contains(r#"type="text" name="approved""#));
    }

    #[test]
    fn an_integration_page_carries_its_own_part_and_a_flash_once_given() {
        let v = IntegrationView { flash: Some("Installed."), dry_run: &[], ..integration(true, &[]) };
        let html = integration_page("tok", &v, &NAV);
        assert!(html.contains("<p>its own part</p>") && html.contains("<strong>Installed.</strong>"));
    }

    #[test]
    fn the_integrations_tab_lists_each_with_its_state_and_page() {
        let rows = [IntegrationRow { id: "herdr", name: "Herdr dispatcher", summary: "Starts agents.", status: "Not installed".into() }];
        let html = integrations_page("tok", &rows, &Nav { current: Tab::Integrations, muted: 0 });
        assert!(html.contains(r#"href="/tok/integrations/herdr""#) && html.contains("Not installed"));
        assert!(html.contains(r#"aria-current="page">Integrations<"#));
    }

    // ── Mutes on the PR page ──────────────────────────────────────────────────

    fn bar_page(until: &dyn Fn(&str) -> Option<u64>) -> String {
        let a = PrEntry { id: Some("PR_a".into()), title: Some("Alpha".into()), ..entry("https://github.com/o/r/pull/1") };
        let b = PrEntry { id: Some("PR_b".into()), title: Some("Bravo".into()), ..entry("https://github.com/o/r/pull/2") };
        let m = Mutes { token: "tok", axis: PrAxis::ChangesRequested, now_unix: NOW, until };
        let list = [a, b];
        axis_page(PrAxis::ChangesRequested, &[group(Some(&list))], None, "tok", NOW, "n", Some(&m))
    }

    /// Every active row offers the three durations, as posts back to this axis.
    #[test]
    fn each_row_offers_three_mute_durations() {
        let html = bar_page(&|_| None);
        for days in [3, 7, 30] {
            assert!(html.contains(&format!(r#"name="days" value="{days}"><button class="link" type="submit">{days} days</button>"#)), "{days}");
        }
        assert!(html.contains(r#"action="/tok/work-required/mute""#));
        assert!(!html.contains("muted-head\">Muted"), "nothing muted, no section");
    }

    /// A muted pull request moves under "Muted" at the bottom, says how long is left, offers
    /// Unmute, and drops out of the count.
    #[test]
    fn a_muted_row_sits_below_a_muted_heading_and_offers_unmute() {
        let html = bar_page(&|k| (k == "PR_a").then_some(NOW + 5 * 86_400));
        let head = html.find("muted-head\">Muted").expect("a muted section");
        let alpha = html.find("Alpha").unwrap();
        let bravo = html.find("Bravo").unwrap();
        assert!(bravo < head && head < alpha, "active first, muted after the heading");
        assert!(html.contains("Muted, back in 5 days"));
        assert!(html.contains(r#"<a href="/tok/muted">all muted</a>"#), "a way to every muted PR");
        assert!(html.contains(r#"value="0"><button class="link" type="submit">Unmute</button>"#));
        assert!(html.contains("1 pull request(s)"), "the muted one is not counted");
    }

    /// Every link a page renders is caught by a rule that names its container: `.card a`,
    /// `footer a`, `.empty a`, `.tabs a`. A link in a `.section` heading was caught by none of
    /// them, so "all muted" took the browser's own `#0000EE`, underlined and purple once visited,
    /// glued to dim grey uppercase text. Against the dark sheet's `--bg` that is about 1.3:1,
    /// which is not a link so much as a rumour of one.
    ///
    /// The sheet styles by container and never by bare element, and that is not taste: a bare
    /// `h2` rule is what put every PR title in capitals from 2.0.0 to 2.0.3, which
    /// `only_settings_section_headings_are_uppercased` now pins. A bare `a` rule would not
    /// actually reach a card title — `.card a` outranks it — but it would silently decide the
    /// look of every link added after it, which is the same trap one specificity notch further
    /// along. So links stay opt-in by container, and this pins that too.
    #[test]
    fn the_muted_heading_link_is_coloured_by_the_sheet_not_the_browser() {
        let html = bar_page(&|k| (k == "PR_a").then_some(NOW + 5 * 86_400));
        assert!(html.contains(r#"<a href="/tok/muted">all muted</a>"#), "the link this is about");

        // A selector "colours a link" when it names an anchor and sets a colour on it.
        let colours = |wanted: &str| {
            STYLESHEET.split('}').any(|rule| {
                let (selector, body) = rule.split_once('{').unwrap_or(("", ""));
                selector.split(',').any(|s| s.trim() == wanted) && body.contains("color:")
            })
        };
        assert!(!colours("a"), "link colour is opt-in by container, never by bare element");
        assert!(colours(".section a"), "so the heading link needs a rule of its own");
    }

    /// Everything muted still says "Nothing here", with the muted section below it.
    #[test]
    fn a_bar_with_only_muted_prs_says_nothing_needs_you() {
        let html = bar_page(&|_| Some(NOW + 3 * 86_400));
        assert!(html.contains("Nothing here right now."));
        assert!(html.find("Nothing here").unwrap() < html.find("muted-head\">Muted").unwrap());
    }

    /// The muted page reaches every muted pull request, grouped by bar, with Unmute posting to
    /// itself; one no bar holds any more is still listed, by key, so it can be unmuted too.
    #[test]
    fn the_muted_page_lists_every_mute_with_unmute() {
        let a = PrEntry { id: Some("PR_a".into()), title: Some("Alpha".into()), ..entry("https://github.com/o/r/pull/1") };
        let rows = [
            MutedRow { key: "PR_a", until: NOW + 3 * 86_400, found: Some((PrAxis::ChangesRequested, &a, "https://github.com/")) },
            MutedRow { key: "PR_gone", until: NOW + 86_400, found: None },
        ];
        let html = muted_page(&rows, "tok", NOW, &NAV);
        assert!(html.contains(&format!(">{}</h2>", heading(PrAxis::ChangesRequested))));
        assert!(html.contains("Alpha") && html.contains("back in 3 days"));
        assert!(html.contains("No longer in any bar") && html.contains("<code>PR_gone</code>"));
        assert_eq!(html.matches(r#"action="/tok/muted""#).count(), 2, "one Unmute per row");
        assert!(html.contains(r#"<meta name="referrer" content="same-origin">"#));
        assert!(!muted_page(&[], "tok", NOW, &NAV).contains("Unmute"));
        assert!(muted_page(&[], "tok", NOW, &NAV).contains("Nothing is muted."));
    }

    /// Every one of GitHoot's own pages carries the same nav, so the muted page, the one route to a
    /// muted pull request whose bar is empty, is one click from anywhere; the current tab is not a link.
    #[test]
    fn every_own_page_carries_the_nav_and_marks_where_you_are() {
        let n = Nav { current: Tab::Settings, muted: 2 };
        let settings = settings_page(&default_cfg(), "tok", &[], &n);
        assert!(settings.contains(r#"<span class="on" aria-current="page">Settings</span>"#));
        for path in ["accounts", "muted", "integrations"] {
            assert!(settings.contains(&format!(r#"href="/tok/{path}""#)), "{path}");
        }
        assert!(settings.contains(">Muted (2)<"));
        let a = accounts_page("tok", &view(&[], None), &Nav { current: Tab::Accounts, ..n });
        assert!(a.contains(r#"aria-current="page">Accounts<"#) && a.contains(r#"href="/tok/settings""#));
        assert!(muted_page(&[], "tok", NOW, &Nav { current: Tab::Muted, ..n }).contains(r#"aria-current="page">Muted (2)<"#));
    }

    /// The point of the split: nothing that reloads by itself shares a page with the settings form.
    #[test]
    fn the_settings_form_shares_its_page_with_nothing_that_reloads_it() {
        let html = settings_page(&default_cfg(), "tok", &[], &NAV);
        assert!(html.contains(r#"<form method="post" action="/tok/settings">"#));
        assert!(!html.contains("http-equiv=\"refresh\""), "no auto-refresh on the page with your edits");
        assert!(!html.contains("Portals") && !html.contains("/tok/integrations/") && !html.contains("settings/authenticate"));
    }

    /// The page posts its mute links back to itself, so it must not null its own `Origin`.
    #[test]
    fn the_pr_page_keeps_an_origin_for_its_mute_links() {
        let html = bar_page(&|_| None);
        assert!(html.contains(r#"<meta name="referrer" content="same-origin">"#));
        assert!(!html.contains("no-referrer\""), "the document policy must not be no-referrer");
        assert!(html.contains(r#"rel="noreferrer""#), "outbound links still send nothing");
    }

    fn settings_with(portals: &[PortalStatus], signin: Option<&str>) -> String {
        accounts_page("tok", &view(portals, signin), &NAV)
    }

    fn default_cfg() -> crate::config::Config {
        let dir = std::env::temp_dir().join(format!("githoot-page-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let (cfg, _) = crate::config::Config::load(&dir);
        let _ = std::fs::remove_dir_all(&dir);
        cfg
    }

    /// Every writable key needs a control, or a setting exists the page silently cannot reach.
    #[test]
    fn every_writable_key_has_a_control() {
        let html = settings(&default_cfg());
        for (key, _) in crate::config::WRITABLE_KEYS {
            assert!(html.contains(&format!("name=\"{key}\"")), "no control for {key}");
        }
    }

    /// The form has to post back to us, or the CSP's `form-action 'self'` blocks it and nothing saves.
    #[test]
    fn the_form_posts_back_to_our_own_settings_route() {
        assert!(settings(&default_cfg()).contains(r#"<form method="post" action="/tok/settings">"#));
    }

    /// A checkbox that does not reflect what is on disk is worse than no page: it invites you to save
    /// a state you never chose.
    #[test]
    fn the_controls_show_what_is_actually_configured() {
        let mut cfg = default_cfg();
        cfg.sound = false;
        cfg.update_check = true;
        cfg.status_components = vec!["Issues".to_string()];
        let html = settings(&cfg);
        assert!(html.contains(r#"name="sound" value="on">"#), "an off box is not checked");
        assert!(html.contains(r#"name="updateCheck" value="on" checked>"#));
        assert!(html.contains(r#"value="Issues" checked>"#));
        assert!(html.contains(r#"value="Pages">"#), "an unlisted component is not checked");
    }

    /// Every component GitHub publishes is offered, not just the ones currently watched — the whole
    /// point is that adding one back is a tick rather than a trip to the status page.
    #[test]
    fn every_component_is_offered() {
        let html = settings(&default_cfg());
        for name in crate::config::all_components() {
            assert!(html.contains(&format!(r#"value="{name}""#)), "{name} is not offered");
        }
    }

    // ── Portals on the settings page ──────────────────────────────────────────

    /// The button appears exactly when a click would help: not signed in. It posts to its own
    /// route, names its portal, and stays outside the settings form so the two cannot nest.
    #[test]
    fn a_portal_waiting_for_sign_in_gets_a_button_that_names_it() {
        let html = settings_with(&[portal(AuthStatus::NotSignedIn)], None);
        assert!(html.contains("<h2 class=\"section\">Portals</h2>"), "got {html}");
        assert!(html.contains("<strong>GitHub</strong> · <span class=\"portal-status\">Not signed in."), "got {html}");
        assert!(html.contains(r#"<form method="post" action="/tok/settings/authenticate">"#));
        assert!(html.contains(r#"<input type="hidden" name="portal" value="github">"#));
        assert!(html.contains("Sign in to GitHub</button>"));
        assert!(html.contains("A code and a link"), "the device flow is explained before the click");
        assert!(!html.contains("http-equiv=\"refresh\""), "nothing running: the page sits still");
        assert!(!html.contains(r#"action="/tok/settings">"#), "the settings form lives on its own page now");
    }

    /// Signed in offers Sign out, which posts to the same route with `signout` set. The page after
    /// the redirect says so and reloads once so the card catches up with the poll thread.
    #[test]
    fn a_signed_in_portal_offers_to_sign_out() {
        let html = settings_with(&[portal(AuthStatus::SignedIn)], None);
        assert!(html.contains("<span class=\"portal-status\">Signed in</span>"), "got {html}");
        assert!(html.contains(r#"<input type="hidden" name="signout" value="1"><button type="submit">Sign out</button>"#));
        assert!(!html.contains("Cancel</button>") && !html.contains("Sign in to GitHub</button>"));
        assert!(!html.contains("http-equiv=\"refresh\""));

        let statuses = [portal(AuthStatus::NotSignedIn)];
        let html = accounts_page("tok", &PortalsView { signed_out: Some("github"), ..view(&statuses, None) }, &NAV);
        assert!(html.contains("<strong>Signed out of GitHub.</strong>"), "got {html}");
        assert!(html.contains(r#"<meta http-equiv="refresh" content="1;url=/tok/accounts">"#), "one quick reload to catch up, to the plain address: {html}");
    }

    /// A dead end the portal declared: the reason is said and no button is offered, because a
    /// sign-in would change nothing there.
    #[test]
    fn a_dead_end_shows_its_reason_without_a_button() {
        let html = settings_with(&[portal(AuthStatus::Off("PR status off: install the GitHub App to see your PRs".to_string()))], None);
        assert!(html.contains("install the GitHub App"), "got {html}");
        assert!(!html.contains("settings/authenticate"));
        assert!(!html.contains("http-equiv=\"refresh\""));
    }

    /// While the flow runs the page is the dialog: it shows the code and the link, says how long is
    /// left, offers Cancel and nothing else, and reloads itself so the outcome shows up on its own.
    #[test]
    fn a_running_sign_in_shows_the_code_the_link_the_deadline_and_cancel() {
        let html = settings_with(&[portal(AuthStatus::SigningIn(None))], None);
        assert!(html.contains("Starting sign-in"), "got {html}");
        assert!(html.contains(r#"<input type="hidden" name="cancel" value="1"><button type="submit">Cancel</button>"#));
        assert!(!html.contains("Sign in to GitHub</button>"), "no second sign-in while one runs");
        assert!(html.contains(r#"<meta http-equiv="refresh" content="3;url=/tok/accounts">"#), "reloads to the plain address");

        let prompt = SignInPrompt {
            code: "ABCD-1234".to_string(),
            url: "https://github.com/login/device".to_string(),
            expires_at: NOW + 14 * 60 + 30,
        };
        let html = settings_with(&[portal(AuthStatus::SigningIn(Some(prompt.clone())))], None);
        assert!(html.contains(r#"<input id="device-code" class="device-code" type="text" readonly value="ABCD-1234" aria-label="Device code">"#), "got {html}");
        assert!(html.contains(r#"<button type="button" id="copy-code" data-url="https://github.com/login/device">Copy and open</button>"#), "one button copies and opens: {html}");
        assert!(html.contains("<script nonce=\"n\">"), "the copy button needs the one script this page ever runs");
        assert!(!html.contains("<p class=\"row\">"), "the code row is a block, so the card must not wrap it in a paragraph");
        assert!(html.contains(r#"<a href="https://github.com/login/device" target="_blank" rel="noreferrer noopener">https://github.com/login/device</a>"#));
        assert!(html.contains("Expires in 15 min."), "rounded up, so it never claims less time than there is");
        assert!(html.contains("Cancel</button>"));

        let expired = SignInPrompt { expires_at: NOW - 1, ..prompt };
        let html = settings_with(&[portal(AuthStatus::SigningIn(Some(expired)))], None);
        assert!(html.contains("The code has expired"));
    }

    /// The code and the URL come from the portal's answer, so they are escaped like anything else
    /// that arrives over the network.
    #[test]
    fn the_prompt_is_escaped() {
        let prompt = SignInPrompt { code: "<b>".to_string(), url: "javascript:x".to_string(), expires_at: NOW + 60 };
        let html = settings_with(&[portal(AuthStatus::SigningIn(Some(prompt)))], None);
        assert!(html.contains("&lt;b&gt;") && !html.contains("<b>"));
        assert!(!html.contains("href=\"javascript"), "an address off the portal's own host is text, not a link");
        assert!(!html.contains("data-url="), "and the button will not open it either");
        assert!(html.contains("at javascript:x."), "shown, so nothing is hidden");
    }

    /// The redirect back from the button names the portal, and the page says so even if the poll
    /// thread has not yet published "in progress". An unknown id is ignored rather than rendered.
    #[test]
    fn the_sign_in_started_banner_names_the_portal_and_ignores_strangers() {
        let html = settings_with(&[portal(AuthStatus::NotSignedIn)], Some("github"));
        assert!(html.contains("<strong>Sign-in to GitHub started.</strong>"), "got {html}");
        assert!(html.contains("http-equiv=\"refresh\""), "and the page will catch up with the poll thread on its own");
        let html = settings_with(&[portal(AuthStatus::NotSignedIn)], Some("gitlab"));
        assert!(!html.contains("started."), "an id the page does not know renders nothing");
        assert!(!html.contains("gitlab"), "and is not echoed back");
    }

    /// Text on a portal card comes from the portal and from `config.txt`, so it is escaped like a
    /// PR title.
    #[test]
    fn portal_status_text_is_escaped() {
        let html = settings_with(&[portal(AuthStatus::Off("<b>x</b>".to_string()))], None);
        assert!(html.contains("&lt;b&gt;x&lt;/b&gt;"));
        assert!(!html.contains("<b>x</b>"));
    }

    /// No script unless a device code is on screen, and none at all without a nonce to name it.
    #[test]
    fn the_settings_page_runs_no_script_unless_a_code_is_on_screen() {
        assert!(!settings(&default_cfg()).contains("<script"));
        assert!(!settings_unavailable("tok").contains("<script"));
        assert!(!settings_with(&[portal(AuthStatus::SigningIn(None))], None).contains("<script"));
        let prompt = SignInPrompt { code: "X".to_string(), url: GITHUB.link_prefix.clone(), expires_at: NOW + 60 };
        let statuses = [portal(AuthStatus::SigningIn(Some(prompt)))];
        let without_nonce = accounts_page("tok", &PortalsView { nonce: "", ..view(&statuses, None) }, &NAV);
        assert!(!without_nonce.contains("<script"), "no nonce, no script: the CSP would block it anyway");
    }

    #[test]
    fn the_restart_banner_only_shows_after_a_save() {
        assert!(!settings(&default_cfg()).contains("Saved."));
        let banner = settings_page(&default_cfg(), "tok", &["logLevel"], &NAV);
        assert!(banner.contains("Saved."));
        assert!(banner.contains("logLevel"));
    }

    // ── Age ───────────────────────────────────────────────────────────────────

    #[test]
    fn an_iso_timestamp_becomes_a_relative_age() {
        const T: u64 = 1_789_463_553; // 2026-09-15T09:12:33Z
        let t = "2026-09-15T09:12:33Z";
        assert_eq!(age(t, T), "just now");
        assert_eq!(age(t, T + 90), "1 m");
        assert_eq!(age(t, T + 3 * 3600), "3 h");
        assert_eq!(age(t, T + 6 * 86_400), "6 d");
    }

    /// Never a panic and never a lie: an unreadable timestamp shows what GitHub actually said.
    #[test]
    fn an_unparseable_timestamp_falls_back_to_the_raw_text() {
        assert_eq!(age("not a date", NOW), "not a date");
        assert_eq!(age("", NOW), "");
    }

    #[test]
    fn a_leap_day_parses() {
        assert_eq!(age("2024-02-29T00:00:00Z", 1_709_164_800), "just now");
    }

    /// A clock a few seconds behind GitHub's must not render a negative age.
    #[test]
    fn a_future_timestamp_reads_as_just_now() {
        assert_eq!(age("2026-09-15T09:12:33Z", 1_789_463_553 - 500), "just now");
    }

    // ─── Golden markup ───────────────────────────────────────────────────────
    //
    // The refresh payload for one fully decorated card and for both empty states, verbatim. Same
    // purpose as `state`'s golden wording: the portal refactor promises a GitHub-only user sees no
    // difference, and the page is where most of the GitHub-shaped output lives.

    #[test]
    fn golden_items_json_for_a_decorated_card() {
        let mut e = entry("https://github.com/qumea/care-api/pull/2204");
        e.is_draft = true;
        e.conflicting = true;
        e.bot_review = Some(crate::portal::types::BotReview { name: "Copilot".into(), unresolved: 2 });
        e.checks = CheckRollup::Failure;
        e.verdicts = vec![
            crate::portal::types::Verdict { login: "alice".into(), state: ReviewState::Approved },
            crate::portal::types::Verdict { login: "bob".into(), state: ReviewState::ChangesRequested },
        ];
        e.pending = vec![Reviewer::User("carol".into()), Reviewer::Team("platform".into())];
        let json = items_json(&[group(Some(&[e]))], Some(Duration::from_secs(47)), NOW, None);
        assert_eq!(json, r#"{"age":47,"count":"1 pull request(s) · ","items":"<div class=\"card\"><h2><a href=\"https://github.com/qumea/care-api/pull/2204\" rel=\"noreferrer\">Fix the bed-exit debounce</a></h2><div class=\"meta\"><span>qumea/care-api #2204</span><span>octocat</span><span>updated just now</span><span class=\"pill draft\">Draft</span><span class=\"pill checks-failure\">Merge conflict</span><span class=\"pill checks-pending\">2 unresolved Copilot comments</span><span class=\"pill checks-failure\">Checks failing</span></div><ul class=\"who\"><li><span class=\"dot dot-ok\"></span>alice approved</li><li><span class=\"dot dot-no\"></span>bob requested changes</li><li><span class=\"dot dot-wait\"></span>carol re-review pending</li><li><span class=\"dot dot-wait\"></span>team platform re-review pending</li></ul></div>\n"}"#);
    }

    #[test]
    fn golden_items_json_for_both_empty_states() {
        assert_eq!(items_json(&[group(None)], None, NOW, None), r#"{"age":null,"count":"","items":"<div class=\"empty\"><p>This list is <strong>not known</strong> right now — GitHoot has no answer it still stands behind for this bar.</p><p><a href=\"https://github.com/pulls/inbox\" rel=\"noreferrer\">Open your pull requests on GitHub</a></p></div>\n"}"#);
        assert_eq!(items_json(&[group(Some(&[]))], Some(Duration::from_secs(5)), NOW, None), r#"{"age":5,"count":"0 pull request(s) · ","items":"<div class=\"empty\"><p>Nothing here right now.</p><p><a href=\"https://github.com/pulls/inbox\" rel=\"noreferrer\">Open your pull requests on GitHub</a></p></div>\n"}"#);
    }

    #[test]
    fn golden_unsafe_url_is_text_not_link() {
        let mut e = entry("https://github.com.evil.com/x");
        e.title = Some("t".into());
        let json = items_json(&[group(Some(&[e]))], None, NOW, None);
        assert_eq!(json, r#"{"age":null,"count":"1 pull request(s) · ","items":"<div class=\"card\"><h2>t</h2><div class=\"meta\"><span>qumea/care-api #2204</span><span>octocat</span><span>updated just now</span><span class=\"pill checks-success\">Checks passing</span></div></div>\n"}"#);
    }

    // ── Several portals ───────────────────────────────────────────────────────

    /// Two portals, two headings, and each group's own empty state and inbox: "nothing here" on one
    /// says nothing about the other. One portal keeps the old bare markup, which the goldens above
    /// pin down.
    #[test]
    fn several_portals_render_a_heading_and_an_empty_state_each() {
        let a = entry("https://github.com/o/r/pull/1");
        let groups = [
            PortalGroup { info: &GITHUB, entries: Some(&[a]) },
            PortalGroup { info: &GITLAB, entries: Some(&[]) },
        ];
        let html = axis_page(PrAxis::ReviewRequested, &groups, None, "tok", NOW, "n", None);
        assert!(html.contains("<h2 class=\"portal\">GitHub</h2>"), "got {html}");
        assert!(html.contains("<h2 class=\"portal\">GitLab</h2>"));
        assert!(html.contains("Open your pull requests on GitLab"), "GitLab's empty state names GitLab");
        assert!(html.contains(&GITLAB.inbox_url), "and links GitLab's inbox");
        assert!(html.contains("Your pull requests on GitHub") && html.contains("Your pull requests on GitLab"), "footer offers both inboxes");
        assert!(html.contains("1 pull request(s)"), "the count is the sum of confirmed lists");

        let one = page(Some(&[]));
        assert!(!one.contains("class=\"portal\""), "one portal gets no heading");
    }

    /// Which prefix makes a URL clickable is the group's, not a global: the same GitLab URL is a
    /// link under GitLab and text under GitHub, and no group's prefix admits the other's hosts.
    #[test]
    fn the_groups_link_prefix_decides_what_becomes_a_link() {
        let mut mr = entry("https://gitlab.example/g/p/-/merge_requests/7");
        mr.title = Some("Widen the door".to_string());
        let under_gitlab = items_json(&[PortalGroup { info: &GITLAB, entries: Some(std::slice::from_ref(&mr)) }], None, NOW, None);
        assert!(under_gitlab.contains("<a href=\\\"https://gitlab.example/g/p/-/merge_requests/7\\\""), "got {under_gitlab}");
        let under_github = items_json(&[PortalGroup { info: &GITHUB, entries: Some(std::slice::from_ref(&mr)) }], None, NOW, None);
        assert!(!under_github.contains("href=\\\"https://gitlab.example"), "got {under_github}");
        assert!(under_github.contains("Widen the door"), "shown as text, not hidden");
    }

    /// Before the poll loop has published anything there are no groups at all. That is the "not
    /// known" state with nowhere to send anyone, and it must not claim a count.
    #[test]
    fn no_groups_at_all_is_not_known_with_no_link() {
        let json = items_json(&[], None, NOW, None);
        assert!(json.contains("not known"), "got {json}");
        assert!(!json.contains("href"), "no portal, no inbox to offer");
        assert!(json.contains("\"count\":\"\""), "no confirmed list, no number");
    }
}


