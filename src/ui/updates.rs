//! Updates: this version, the last check, Check now, the automatic check, and the update itself.
//!
//! **Check now works whether or not the automatic check is on.** The setting decides whether GitHoot
//! asks by itself at startup and daily; asking by hand is always yours to do. The check runs on the
//! poll thread, like the automatic one, and while it runs the page shows only that and reloads
//! itself, by the same rule a running sign-in follows.
//!
//! **The update is installed from here**, never from a dialog. A newer release shows every release
//! note between this version and it, and one button: pressing it is the consent. The install then runs
//! on its own thread and the page follows its steps in place, from `?progress=1`, until the app
//! restarts and this page's server goes with it.

use super::{layout, sections, Place, Site};
use crate::page::esc;
use crate::setting::Setting;
use crate::update::{Available, Progress, Step};

/// How long the page keeps reloading while it waits for a check it asked for. The check is one GET
/// with a timeout, but the poll thread may be busy first: a sign-in blocks it for as long as you take.
const WAIT_SECS: u64 = 60;

/// What the page knows about checks, as plain data.
pub struct UpdatesView<'a> {
    /// This build's version.
    pub installed: &'a str,
    /// Seconds since Check now was pressed, while that check has not landed.
    pub waiting: Option<u64>,
    /// The last check that landed: seconds ago, and what it found, a newer version or `None` for up to
    /// date, or why it failed.
    pub last: Option<(u64, Result<Option<String>, String>)>,
    /// The release that check found, with every skipped release's notes.
    pub available: Option<&'a Available>,
    /// Why this binary cannot update itself here, if it cannot: a local build, a folder it may not
    /// write, a platform with no release build.
    pub blocked: Option<String>,
    pub progress: &'a Progress,
    pub value: &'a dyn Fn(&Setting) -> String,
    pub flash: Option<&'a (usize, String)>,
}

/// Follows an install in place: polls `?progress=1`, marks the steps, counts the bytes, and says when
/// it failed or the app is restarting. Pressing Install posts in the background and starts following,
/// so the page never reloads; without the script the form posts and the page renders the same steps.
///
/// Once the app has restarted, every link and form here points at a port and token that are gone, so
/// the whole page gives way to one message. It names no version: if the new binary would not start,
/// the previous one is restarted instead, and from here the two look the same.
const PROGRESS_SCRIPT: &str = "(function(){\
var card=document.querySelector('[data-progress-url]');if(!card)return;\
var url=card.getAttribute('data-progress-url'),order=['signature','download','digest','test','replace'];\
function q(s){return card.querySelector(s);}\
function mb(n){return (n/1e6).toFixed(1);}\
function mark(step){var i=order.indexOf(step);card.querySelectorAll('[data-step]').forEach(function(li){\
var j=order.indexOf(li.getAttribute('data-step'));li.className=step===null?'':(j<i?'done':(j===i?'now':''));});}\
function note(t){q('[data-note]').textContent=t;}\
function show(j){card.setAttribute('data-state',j.state);\
if(j.state==='running'){mark(j.step);var b=q('.bytes');b.textContent=j.step==='download'&&j.done?mb(j.done)+(j.total?' / '+mb(j.total):'')+' MB':'';\
note('Installing '+j.version+'. GitHoot keeps working meanwhile.');return 700;}\
if(j.state==='restarting'){mark('replace');q('[data-step=\"replace\"]').className='done';note('Restarting into '+j.version+'. This tab loses its connection when GitHoot restarts.');return 1000;}\
if(j.state==='failed'){mark(null);q('.bytes').textContent='';note('The update was not installed and the current version is untouched: '+j.message);\
var r=document.querySelector('[data-retry]');if(r)r.hidden=false;return 0;}\
return 0;}\
var restarting=false;\
function gone(){document.title='GitHoot restarted';var m=document.querySelector('main');\
var h=document.createElement('h1');h.textContent='GitHoot restarted';\
var p=document.createElement('p');p.textContent='This tab belongs to the previous run and no longer works. Close it and open GitHoot from the tray.';\
m.replaceChildren(h,p);}\
function tick(){fetch(url,{cache:'no-store'}).then(function(r){return r.json();}).then(function(j){\
restarting=j.state==='restarting';var next=show(j);if(next)setTimeout(tick,next);})\
.catch(function(){if(restarting)gone();else setTimeout(tick,2000);});}\
document.addEventListener('submit',function(ev){var f=ev.target;if(!f.hasAttribute('data-install')&&!f.hasAttribute('data-retry'))return;\
ev.preventDefault();document.querySelectorAll('#version form,[data-retry]').forEach(function(x){x.hidden=true;});\
card.closest('section').hidden=false;mark(null);note('Starting…');card.setAttribute('data-state','running');\
fetch(f.getAttribute('action'),{method:'POST',body:new URLSearchParams(new FormData(f)),redirect:'manual'}).then(tick,tick);});\
if(card.getAttribute('data-state')==='running'||card.getAttribute('data-state')==='restarting')tick();\
})();";

/// The page's own rules: the step list and the notes. Static text with nothing interpolated, which is
/// the condition the CSP's `style-src 'unsafe-inline'` rests on.
const STYLE: &str = "<style>\
.steps{list-style:none;margin:.2rem 0 .6rem;padding:0;display:grid;gap:.3rem}\
.steps li{color:var(--dim);padding-left:1.6rem;position:relative}\
.steps li::before{content:'·';position:absolute;left:.45rem}\
.steps li.now{color:var(--ink);font-weight:600}\
.steps li.now::before{content:'⟳';left:.3rem}\
.steps li.done{color:var(--ok)}\
.steps li.done::before{content:'✓';left:.3rem}\
.steps .bytes{margin-left:.5rem;font-weight:400;color:var(--dim)}\
[data-state=failed] [data-note]{color:var(--bad)}\
.notes h3{margin:1rem 0 .3rem;font-size:1.05rem}\
.notes h3:first-child{margin-top:0}\
.notes h4{margin:.4rem 0 .2rem;font-size:.9rem;color:var(--dim);font-weight:600}\
.notes p{margin:.2rem 0;overflow-wrap:anywhere}\
.notes .release-link{font-size:.85rem}\
.notes a{color:var(--ink);text-decoration:underline;text-decoration-color:var(--accent);text-underline-offset:.2em}\
.notes a:hover{text-decoration-thickness:2px}\
</style>";

pub fn updates_page(site: &Site, v: &UpdatesView) -> String {
    let token = site.token;
    let running = v.waiting.is_some_and(|secs| secs < WAIT_SECS);
    let installing = matches!(v.progress, Progress::Running { .. } | Progress::Restarting { .. });
    let mut h = String::new();
    h.push_str("<section class=\"block\" id=\"version\"><h2 class=\"section\">This version</h2><div class=\"card\">");
    h.push_str(&format!("<div class=\"row\"><strong>GitHoot {}</strong></div>", esc(v.installed)));
    if running && !installing {
        h.push_str("<p class=\"sub\"><strong>Checking for a newer release…</strong> This page updates itself.</p></div></section>\n");
        return layout(site, &Place::Updates, "Updates", Some(1), &h);
    }
    if v.waiting.is_some() {
        h.push_str(
            "<p class=\"sub\"><strong>Still waiting for the check.</strong> GitHoot is busy with something \
             else first, such as a sign-in. It will run as soon as that is done.</p>",
        );
    }
    let last = match &v.last {
        None => "Not checked yet since GitHoot started.".to_string(),
        Some((ago, Ok(None))) => format!("Checked {}: this is the newest release.", ago_text(*ago)),
        Some((ago, Ok(Some(newer)))) => format!(
            "Checked {}: <strong>{} is available.</strong>",
            ago_text(*ago),
            esc(newer)
        ),
        Some((ago, Err(why))) => format!("Checked {}: the check failed ({}).", ago_text(*ago), esc(why)),
    };
    h.push_str(&format!("<p class=\"sub\">{last}</p>"));
    let newer = v.available;
    if let (Some(a), Some(why)) = (newer, &v.blocked) {
        h.push_str(&format!(
            "<p class=\"sub\">GitHoot cannot update itself here: {}. \
             <a href=\"{}\" rel=\"noreferrer\" target=\"_blank\">Download {} from its release page</a>.</p>",
            esc(why),
            esc(&a.url),
            esc(&a.version.to_string())
        ));
    }
    // A newer release found: Install is what the visitor came for, so it is the filled button and
    // Check now steps back. Pressing it is the consent; nothing asks again.
    let offer = newer.filter(|_| v.blocked.is_none() && !installing);
    let install = offer
        .map(|a| install_form(token, &a.version.to_string(), "data-install", &format!("Install {} and restart", a.version)))
        .unwrap_or_default();
    let check_class = if newer.is_some() { " class=\"ghost\"" } else { "" };
    let check = if installing {
        String::new()
    } else {
        format!(
            "<form method=\"post\" action=\"/{}/updates\"><input type=\"hidden\" name=\"action\" value=\"check\">\
             <button{check_class} type=\"submit\">Check now</button></form>",
            esc(token)
        )
    };
    h.push_str(&format!("<div class=\"actions\">{install}{check}</div></div></section>\n"));

    if newer.is_some_and(|_| v.blocked.is_none()) || !matches!(v.progress, Progress::Idle) {
        h.push_str(&progress_section(token, v.progress, newer));
    }
    if let Some(a) = newer {
        h.push_str(&whats_new(a));
    }
    h.push_str(&sections(token, &Place::Updates, crate::config::UPDATES, v.value, v.flash));
    h.push_str(STYLE);
    if !site.nonce.is_empty() {
        h.push_str(&format!("<script nonce=\"{}\">{PROGRESS_SCRIPT}</script>\n", esc(site.nonce)));
    }
    layout(site, &Place::Updates, "Updates", None, &h)
}

fn install_form(token: &str, version: &str, marker: &str, label: &str) -> String {
    format!(
        "<form method=\"post\" action=\"/{}/updates\" {marker}><input type=\"hidden\" name=\"action\" value=\"install\">\
         <input type=\"hidden\" name=\"version\" value=\"{}\"><button type=\"submit\">{}</button></form>",
        esc(token),
        esc(version),
        esc(label)
    )
}

/// The install's steps, the current one marked. Rendered hidden while nothing runs, so the script can
/// reveal it in place when Install is pressed.
fn progress_section(token: &str, progress: &Progress, newer: Option<&Available>) -> String {
    let (state, version, current, bytes, note) = match progress {
        Progress::Idle => ("idle", newer.map(|a| a.version.to_string()).unwrap_or_default(), None, String::new(), String::new()),
        Progress::Running { version, step, done, total } => {
            let bytes = match (step, done, total) {
                (Some(Step::Download), d, Some(t)) if *d > 0 => format!("{} / {} MB", mb(*d), mb(*t)),
                (Some(Step::Download), d, None) if *d > 0 => format!("{} MB", mb(*d)),
                _ => String::new(),
            };
            ("running", version.clone(), Some(step.map_or(-1, |s| index(s) as i32)), bytes, format!("Installing {}. GitHoot keeps working meanwhile.", esc(version)))
        }
        Progress::Restarting { version } => (
            "restarting",
            version.clone(),
            Some(Step::ALL.len() as i32),
            String::new(),
            format!("Restarting into {}. This tab loses its connection when GitHoot restarts.", esc(version)),
        ),
        Progress::Failed { version, message } => (
            "failed",
            version.clone(),
            None,
            String::new(),
            format!("The update was not installed and the current version is untouched: {}", esc(message)),
        ),
    };
    let steps: String = Step::ALL
        .iter()
        .enumerate()
        .map(|(i, step)| {
            let class = match current {
                Some(c) if (i as i32) < c => " class=\"done\"",
                Some(c) if (i as i32) == c => " class=\"now\"",
                _ => "",
            };
            let extra = if *step == Step::Download { format!("<span class=\"bytes\">{bytes}</span>") } else { String::new() };
            format!("<li data-step=\"{}\"{class}>{}{extra}</li>", step.key(), esc(step.label()))
        })
        .collect();
    let failed = matches!(progress, Progress::Failed { .. });
    let retry = if version.is_empty() {
        String::new()
    } else {
        let form = install_form(token, &version, "data-retry", "Try again");
        if failed { form } else { form.replacen("data-retry>", "data-retry hidden>", 1) }
    };
    let hidden = if state == "idle" { " hidden" } else { "" };
    format!(
        "<section class=\"block\" id=\"install\"{hidden}><h2 class=\"section\">Install</h2>\
         <div class=\"card\" data-progress-url=\"/{}/updates?progress=1\" data-state=\"{state}\">\
         <ol class=\"steps\">{steps}</ol><p class=\"sub\" data-note>{note}</p>\
         <div class=\"actions\">{retry}</div></div></section>\n",
        esc(token)
    )
}

fn index(step: Step) -> usize {
    Step::ALL.iter().position(|s| *s == step).unwrap_or(0)
}

/// Megabytes of a million bytes, the way GitHub states an asset's size.
fn mb(bytes: u64) -> String {
    format!("{:.1}", bytes as f64 / 1_000_000.0)
}

/// Every release between this version and the newest, newest first, each under its version.
fn whats_new(a: &Available) -> String {
    let mut h = String::from("<section class=\"block\" id=\"whats-new\"><h2 class=\"section\">What's new</h2><div class=\"card notes\">");
    for r in &a.releases {
        h.push_str(&format!("<h3>{}</h3>", esc(&r.version.to_string())));
        match &r.body {
            Some(body) => h.push_str(&notes_html(body)),
            None => h.push_str("<p class=\"sub\">No notes for this release.</p>"),
        }
        h.push_str(&format!(
            "<p class=\"release-link\"><a href=\"{}\" rel=\"noreferrer\" target=\"_blank\">Release page</a></p>",
            esc(&r.url)
        ));
    }
    h.push_str("</div></section>\n");
    h
}

/// Renders one release body's Markdown with every way out closed.
///
/// Release bodies are text from GitHub, so they are treated as untrusted: raw HTML is shown as text,
/// a link survives only with an `http` or `https` target and opens in a new tab with no referrer, and
/// an image is reduced to its alt text (the CSP would block it anyway). Headings sit under the
/// version's own `<h3>`, and a single line break is kept, because the bodies are one commit per line.
pub fn notes_html(markdown: &str) -> String {
    use pulldown_cmark::{CowStr, Event, Options, Parser, Tag, TagEnd, TextMergeStream};

    let safe = |url: &str| {
        let lower = url.trim().to_ascii_lowercase();
        lower.starts_with("https://") || lower.starts_with("http://")
    };
    // Which link starts were kept, so each end matches its start. Text inside a link is not linked again.
    let mut links: Vec<bool> = Vec::new();
    // Inside an HTML comment that has not closed yet. A block comment arrives one line per event.
    let mut in_comment = false;
    // Merged, because the parser splits text at entities and some punctuation, and an address cut in
    // two would be linked as two halves.
    let parsed = TextMergeStream::new(Parser::new_ext(markdown, Options::ENABLE_STRIKETHROUGH));
    let events = parsed.filter_map(|event| match event {
        // A comment is dropped: GitHub's generated notes open with one, invisible there. Any other raw
        // HTML is shown as the text it is.
        Event::Html(raw) | Event::InlineHtml(raw) if in_comment || raw.trim_start().starts_with("<!--") => {
            in_comment = !raw.contains("-->");
            None
        }
        Event::Html(raw) | Event::InlineHtml(raw) => Some(Event::Text(raw)),
        Event::Text(text) if links.is_empty() => Some(Event::Html(CowStr::from(autolink(&text)))),
        Event::SoftBreak => Some(Event::HardBreak),
        // The alt text arrives as the image's own text events, so dropping the tag keeps the words.
        Event::Start(Tag::Image { .. }) | Event::End(TagEnd::Image) => None,
        Event::Start(Tag::Link { dest_url, .. }) => {
            let keep = safe(&dest_url);
            links.push(keep);
            keep.then(|| {
                Event::Html(CowStr::from(format!(
                    "<a href=\"{}\" rel=\"noreferrer\" target=\"_blank\">",
                    esc(&dest_url)
                )))
            })
        }
        Event::End(TagEnd::Link) => links.pop().unwrap_or(false).then(|| Event::Html(CowStr::from("</a>"))),
        Event::Start(Tag::Heading { .. }) => Some(Event::Html(CowStr::from("<h4>"))),
        Event::End(TagEnd::Heading(_)) => Some(Event::Html(CowStr::from("</h4>\n"))),
        Event::Start(Tag::HtmlBlock) | Event::End(TagEnd::HtmlBlock) => None,
        other => Some(other),
    });
    let mut out = String::new();
    pulldown_cmark::html::push_html(&mut out, events);
    out
}

/// Escapes `text` and links what GitHub's generated notes leave bare: an address, shortened to `#21`
/// for a pull request or issue and to its range for a compare, and an `@name`, linked to that profile.
///
/// Hand-rolled rather than a regex crate: two shapes, scanned once. An address ends at whitespace or a
/// quote or angle bracket, and sentence punctuation at its end stays outside the link. A mention needs
/// a start of text or a space or bracket before the `@`, which keeps an email address plain.
fn autolink(text: &str) -> String {
    let link = |href: &str, label: &str| {
        format!("<a href=\"{}\" rel=\"noreferrer\" target=\"_blank\">{}</a>", esc(href), esc(label))
    };
    let mut out = String::new();
    let mut rest = text;
    let mut prev: Option<char> = None;
    while let Some(c) = rest.chars().next() {
        let at_word_start = prev.is_none_or(|p| p.is_whitespace() || "([{".contains(p));
        if rest.starts_with("https://") || rest.starts_with("http://") {
            let end = rest.find(|c: char| c.is_whitespace() || "\"'<>".contains(c)).unwrap_or(rest.len());
            let mut url = &rest[..end];
            while let Some(last) = url.chars().last().filter(|l| ".,;:!?)]".contains(*l)) {
                // A closing bracket the address itself opened is part of it.
                if (last == ')' && url.matches('(').count() >= url.matches(')').count())
                    || (last == ']' && url.matches('[').count() >= url.matches(']').count())
                {
                    break;
                }
                url = &url[..url.len() - last.len_utf8()];
            }
            out.push_str(&link(url, &short_label(url)));
            prev = url.chars().last();
            rest = &rest[url.len()..];
            continue;
        }
        if c == '@' && at_word_start {
            let name: String = rest[1..].chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '-').collect();
            let name = name.trim_end_matches('-');
            if !name.is_empty() && !name.starts_with('-') && name.len() <= 39 {
                out.push_str(&link(&format!("https://github.com/{name}"), &format!("@{name}")));
                prev = name.chars().last();
                rest = &rest[1 + name.len()..];
                continue;
            }
        }
        out.push_str(&esc(&c.to_string()));
        prev = Some(c);
        rest = &rest[c.len_utf8()..];
    }
    out
}

/// What an address reads as: `#21` for a GitHub pull request or issue, the range for a compare, and
/// the address itself for anything else.
fn short_label(url: &str) -> String {
    let parts: Vec<&str> = url.trim_end_matches('/').split('/').collect();
    match parts.as_slice() {
        ["https:", "", "github.com", _, _, "pull" | "issues", n] if !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()) => format!("#{n}"),
        ["https:", "", "github.com", _, _, "compare", range] if !range.is_empty() => range.to_string(),
        _ => url.to_string(),
    }
}

fn ago_text(secs: u64) -> String {
    match secs {
        0..=59 => "just now".to_string(),
        60..=3599 => format!("{} min ago", secs / 60),
        3600..=86_399 => format!("{} h ago", secs / 3600),
        _ => format!("{} d ago", secs / 86_400),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::tests::site;
    use crate::update::{Available, Progress, Step};

    fn values(_: &Setting) -> String {
        "on".to_string()
    }

    fn page(waiting: Option<u64>, last: Option<(u64, Result<Option<String>, String>)>) -> String {
        let available = available_of(&last);
        view(UpdatesView { installed: "3.0.0", waiting, last, available: available.as_ref(), blocked: None, progress: &Progress::Idle, value: &values, flash: None })
    }

    fn view(v: UpdatesView) -> String {
        updates_page(&Site { nonce: "n", ..site() }, &v)
    }

    /// The release a check found, with notes, as the poll thread would hold it.
    fn available_of(last: &Option<(u64, Result<Option<String>, String>)>) -> Option<Available> {
        match last {
            Some((_, Ok(Some(version)))) => Some(found(version, &[(version, Some("## Changes\n\nabc1234 Did a thing"))])),
            _ => None,
        }
    }

    fn found(version: &str, notes: &[(&str, Option<&str>)]) -> Available {
        Available {
            version: crate::version::Version::parse(version).unwrap(),
            tag: format!("v{version}"),
            releases: notes
                .iter()
                .map(|(v, body)| crate::update::ReleaseNotes {
                    version: crate::version::Version::parse(v).unwrap(),
                    body: body.map(str::to_string),
                    url: format!("https://github.com/HerrDerb/githoot/releases/tag/v{v}"),
                })
                .collect(),
            url: format!("https://github.com/HerrDerb/githoot/releases/tag/v{version}"),
        }
    }

    fn with(available: &Available, progress: &Progress, blocked: Option<String>) -> String {
        let last = Some((5, Ok(Some(available.version.to_string()))));
        view(UpdatesView { installed: "3.0.0", waiting: None, last, available: Some(available), blocked, progress, value: &values, flash: None })
    }

    #[test]
    fn it_names_the_version_and_offers_check_now_and_the_automatic_switch() {
        let html = page(None, None);
        assert!(html.contains("<strong>GitHoot 3.0.0</strong>") && html.contains("Not checked yet"));
        assert!(html.contains(r#"<form method="post" action="/tok/updates"><input type="hidden" name="action" value="check"><button type="submit">Check now</button></form>"#), "{html}");
        assert!(html.contains(r#"name="updateCheck""#), "the automatic check is a setting on the same page");
        assert!(!html.contains("http-equiv=\"refresh\""));
    }

    #[test]
    fn each_outcome_is_said_plainly() {
        assert!(page(None, Some((30, Ok(None)))).contains("Checked just now: this is the newest release."));
        let html = page(None, Some((120, Ok(Some("3.1.0".into())))));
        assert!(html.contains("Checked 2 min ago: <strong>3.1.0 is available.</strong>"), "{html}");
        assert!(!html.contains("from the tray menu"), "the button is right there: {html}");
        let html = page(None, Some((7200, Err("<timeout>".into()))));
        assert!(html.contains("Checked 2 h ago: the check failed (&lt;timeout&gt;)."), "{html}");
    }

    /// While a check it asked for runs, the page shows that and nothing else, and reloads itself.
    #[test]
    fn a_running_check_shows_only_itself_and_reloads() {
        let html = page(Some(3), None);
        assert!(html.contains("Checking for a newer release"));
        assert!(html.contains(r#"<meta http-equiv="refresh" content="1;url=/tok/updates">"#), "{html}");
        assert!(!html.contains("<form"), "nothing to press or edit on a page that reloads itself");
    }

    /// A check that has not landed within a minute stops the reloading and says why it may be late.
    #[test]
    fn a_late_check_stops_reloading_and_says_so() {
        let html = page(Some(WAIT_SECS), None);
        assert!(!html.contains("http-equiv=\"refresh\"") && html.contains("Still waiting for the check"));
        assert!(html.contains(">Check now<"));
    }

    /// A newer release found: an Update button beside Check now, the filled one because it is what
    /// the visitor came for, running the same install as the tray's "Install update". Check now steps
    /// back to outlined.
    #[test]
    fn a_newer_release_is_offered_as_an_update_button() {
        let html = page(None, Some((120, Ok(Some("3.2.1".into())))));
        assert!(html.contains(r#"<form method="post" action="/tok/updates" data-install><input type="hidden" name="action" value="install"><input type="hidden" name="version" value="3.2.1"><button type="submit">Install 3.2.1 and restart</button></form>"#), "{html}");
        assert!(html.contains(r#"value="check"><button class="ghost" type="submit">Check now</button>"#), "{html}");
        for last in [None, Some((30, Ok(None))), Some((30, Err("x".to_string())))] {
            assert!(!page(None, last).contains(r#"value="install""#), "no newer release, no Update button");
        }
    }

    // ── What's new ─────────────────────────────────────────────────────────

    /// Every release between this one and the newest, newest first, each under its version and with a
    /// link to its own page. Nothing cut: the page scrolls.
    #[test]
    fn a_newer_release_shows_every_skipped_release_newest_first() {
        let long = (0..60).map(|i| format!("line {i}")).collect::<Vec<_>>().join("\n");
        let a = found("3.2.0", &[("3.2.0", Some(&long)), ("3.1.0", None)]);
        let html = with(&a, &Progress::Idle, None);
        let newer = html.find("<h3>3.2.0</h3>").expect("3.2.0 heading");
        let older = html.find("<h3>3.1.0</h3>").expect("3.1.0 heading");
        assert!(newer < older, "{html}");
        assert!(html.contains("line 59"), "nothing cut");
        assert!(html.contains("<p class=\"sub\">No notes for this release.</p>"), "{html}");
        assert!(html.contains(r#"href="https://github.com/HerrDerb/githoot/releases/tag/v3.1.0""#), "{html}");
    }

    /// Release bodies come from outside, so the Markdown is rendered with every way out closed: raw
    /// HTML is shown as text, only http and https links survive, and an image is its alt text.
    #[test]
    fn notes_are_rendered_with_no_way_to_inject_anything() {
        let html = notes_html("<script>alert(1)</script>\n\n<b>bold</b> and <img src=x onerror=alert(1)>");
        assert!(!html.contains("<script") && !html.contains("<b>") && !html.contains("<img"), "{html}");
        assert!(html.contains("&lt;script&gt;"), "{html}");

        let html = notes_html("[bad](javascript:alert(1)) [data](data:text/html,x) [ok](https://github.com/o/r)");
        assert!(!html.contains("javascript:") && !html.contains("data:text"), "{html}");
        assert!(html.contains("bad") && html.contains("data"), "the words stay: {html}");
        assert!(html.contains(r#"<a href="https://github.com/o/r" rel="noreferrer" target="_blank">ok</a>"#), "{html}");

        let html = notes_html("![a screenshot](https://example.com/x.png)");
        assert!(!html.contains("<img") && html.contains("a screenshot"), "{html}");
    }

    /// The release bodies are `git log --oneline` under a heading: one commit per line, which Markdown
    /// would run together into one paragraph. Each keeps its own line, and the heading sits below the
    /// version's own.
    #[test]
    fn one_commit_per_line_stays_one_per_line() {
        let html = notes_html("## Changes since v3.5.0\n\n72d892a Bump to 3.5.1\n58f00aa Fix the thing");
        assert!(html.contains("72d892a Bump to 3.5.1<br />"), "{html}");
        assert!(html.contains("<h4>Changes since v3.5.0</h4>"), "{html}");
        assert!(!html.contains("<h2"), "{html}");
    }

    /// The installer cannot run here (a local build, a folder it cannot write): the page says why and
    /// points at the release page instead of offering a button that would fail.
    #[test]
    fn a_blocked_install_says_why_and_offers_no_button() {
        let a = found("3.2.0", &[("3.2.0", Some("x"))]);
        let html = with(&a, &Progress::Idle, Some("this looks like a local build".into()));
        assert!(html.contains("this looks like a local build"), "{html}");
        assert!(!html.contains(r#"value="install""#), "{html}");
        assert!(html.contains(r#"href="https://github.com/HerrDerb/githoot/releases/tag/v3.2.0""#), "{html}");
    }

    // ── Progress ───────────────────────────────────────────────────────────

    /// While an install runs, the page lists every step with the current one marked, carries the script
    /// that follows it in place, and offers nothing to press. No reloading.
    #[test]
    fn a_running_install_shows_its_steps_and_follows_them_in_place() {
        let a = found("3.2.0", &[("3.2.0", Some("x"))]);
        let p = Progress::Running { version: "3.2.0".into(), step: Some(Step::Download), done: 1_000_000, total: Some(11_800_000) };
        let html = with(&a, &p, None);
        assert!(html.contains(r#"data-progress-url="/tok/updates?progress=1""#), "{html}");
        for step in Step::ALL {
            assert!(html.contains(&format!(r#"data-step="{}""#, step.key())), "{step:?}: {html}");
        }
        assert!(html.contains(r#"<li data-step="signature" class="done">"#), "{html}");
        assert!(html.contains(r#"<li data-step="download" class="now">"#), "{html}");
        assert!(html.contains("1.0 / 11.8 MB"), "megabytes of a million, as GitHub counts them: {html}");
        assert!(!PROGRESS_SCRIPT.contains("1048576") && PROGRESS_SCRIPT.contains("1e6"), "the script counts the same way");
        assert!(!html.contains("data-install>"), "nothing to press while it runs");
        assert!(html.contains("data-retry hidden>"), "the retry waits, hidden, for a failure: {html}");
        assert!(html.contains("<script nonce=\"n\">") && html.contains("progress"), "{html}");
        assert!(!html.contains("http-equiv=\"refresh\""), "in place, never a reload");
    }

    /// A failed install says what went wrong, that nothing changed, and offers the button again.
    #[test]
    fn a_failed_install_says_why_and_offers_a_retry() {
        let a = found("3.2.0", &[("3.2.0", Some("x"))]);
        let html = with(&a, &Progress::Failed { version: "3.2.0".into(), message: "digest <mismatch>".into() }, None);
        assert!(html.contains("digest &lt;mismatch&gt;") && html.contains("current version is untouched"), "{html}");
        assert!(html.contains(r#"value="install""#), "{html}");
    }

    /// Restarting: the page says so, and the script knows the tab is about to lose its server.
    #[test]
    fn a_restart_says_the_tab_can_be_closed() {
        let a = found("3.2.0", &[("3.2.0", Some("x"))]);
        let html = with(&a, &Progress::Restarting { version: "3.2.0".into() }, None);
        assert!(html.contains("Restarting into 3.2.0"), "{html}");
        assert!(!html.contains("data-install>") && html.contains("data-retry hidden>"), "{html}");
    }

    /// GitHub's generated notes end each line with the pull request's address and name its author with
    /// an `@`. Both become links, the address shortened to the number GitHub itself shows.
    #[test]
    fn pull_requests_and_people_in_the_notes_become_short_links() {
        let html = notes_html("* Let Agents run on macOS by @HerrDerb in https://github.com/HerrDerb/githoot/pull/21");
        assert!(html.contains(r#"<a href="https://github.com/HerrDerb/githoot/pull/21" rel="noreferrer" target="_blank">#21</a>"#), "{html}");
        assert!(html.contains(r#"by <a href="https://github.com/HerrDerb" rel="noreferrer" target="_blank">@HerrDerb</a> in"#), "{html}");
        assert!(!html.contains(">https://github.com/HerrDerb/githoot/pull/21<"), "{html}");
        // Underscores and brackets are where the parser splits text, which must not split an address.
        let html = notes_html("in https://github.com/my_org/a_repo/pull/7 by @some-one_x");
        assert!(html.contains(r#"<a href="https://github.com/my_org/a_repo/pull/7" rel="noreferrer" target="_blank">#7</a>"#), "{html}");
        let html = notes_html("see https://example.com/a?x=1&amp;y=2 now");
        assert!(html.contains(r#"<a href="https://example.com/a?x=1&amp;y=2" rel="noreferrer" target="_blank">https://example.com/a?x=1&amp;y=2</a> now"#), "{html}");
    }

    /// A compare address names its range, any other address is linked as written, and punctuation that
    /// ends a sentence stays outside the link.
    #[test]
    fn other_addresses_become_links_and_keep_their_punctuation_outside() {
        let html = notes_html("**Full Changelog**: https://github.com/HerrDerb/githoot/compare/v3.4.0...v3.5.0");
        assert!(html.contains(r#"<a href="https://github.com/HerrDerb/githoot/compare/v3.4.0...v3.5.0" rel="noreferrer" target="_blank">v3.4.0...v3.5.0</a>"#), "{html}");
        let html = notes_html("See https://example.com/docs. And (https://example.com/x), too.");
        assert!(html.contains(r#"<a href="https://example.com/docs" rel="noreferrer" target="_blank">https://example.com/docs</a>. And"#), "{html}");
        assert!(html.contains(r#"(<a href="https://example.com/x" rel="noreferrer" target="_blank">https://example.com/x</a>), too."#), "{html}");
    }

    /// Only a real mention is one: an address with an `@` in it is not, nor is text inside a link or a
    /// code span, and nothing here opens a way to inject anything.
    #[test]
    fn only_real_mentions_and_addresses_are_linked() {
        let html = notes_html("Mail ugg@example.com, run `curl https://x.test/@me`, read [the docs](https://example.com/@a).");
        assert!(!html.contains(r#"href="https://github.com/example"#), "an email is not a mention: {html}");
        assert!(html.contains("<code>curl https://x.test/@me</code>"), "code stays code: {html}");
        assert_eq!(html.matches("<a ").count(), 1, "the one written link, nothing nested in it: {html}");
        let html = notes_html("https://evil.test/\"onmouseover=\"x and @a\"b");
        assert!(!html.contains("\"onmouseover"), "{html}");
    }

    /// The release page link looks like a link, which the card's own rule for links would otherwise undo.
    #[test]
    fn links_in_the_notes_look_like_links() {
        assert!(STYLE.contains(".notes a{color:var(--ink);text-decoration:underline"), "{STYLE}");
    }

    /// GitHub's generated notes open with an HTML comment naming the config they came from. On GitHub it
    /// is invisible; here raw HTML is shown as text, so a comment would sit on top of the notes. Comments
    /// are dropped, on one line or several, while other raw HTML still shows as text.
    #[test]
    fn html_comments_in_the_notes_are_dropped_not_shown() {
        let html = notes_html(
            "<!-- Release notes generated using configuration in .github/release.yml at main -->\n\n## What's Changed\n* A thing",
        );
        assert!(!html.contains("Release notes generated") && !html.contains("&lt;!--"), "{html}");
        assert!(html.contains("<h4>What&#39;s Changed</h4>") && html.contains("A thing"), "{html}");
        let html = notes_html("<!--\nspread over\nlines\n-->\n\nafter");
        assert!(!html.contains("spread over") && !html.contains("--&gt;") && html.contains("after"), "{html}");
        let html = notes_html("before <!-- inline --> after");
        assert!(!html.contains("inline") && html.contains("before") && html.contains("after"), "{html}");
        assert!(notes_html("<b>x</b>").contains("&lt;b&gt;"), "other raw HTML still shows as text");
    }
}

