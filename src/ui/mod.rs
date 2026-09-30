//! GitHoot's own pages: one small site with a sidebar, organised by what you manage.
//!
//! ```text
//! General        what the tray shows and does: the three bars, the hoot
//! Portals        where pull requests come from   ▸ GitHub
//! Integrations   where pull requests go          ▸ Herdr dispatcher
//! Muted          pull requests you silenced
//! Updates        this version, Check now, the automatic check
//! Advanced       the local API, log detail
//! ```
//!
//! Portals and Integrations are symmetric on purpose: they are the two seams of the code,
//! `crate::portal` and `crate::integration`, shown as two collections. Each collection has a list
//! page (one card per item, its state, its main action, ⚙ Settings) and a page per item.
//!
//! ## Two page shapes, and one rule
//!
//! Every page is a header, then **sections**. A section is a card with its own form and its own Save,
//! drawn from declarations (`crate::setting`), so a Save writes that section and nothing else, and
//! comes back to the same page with one line saying what it did.
//!
//! The rule that used to be enforced by splitting pages apart: **a page that reloads itself shows
//! only the thing that is running.** A sign-in in flight or an update check under way reloads its
//! page until it lands, and while it does, the page shows that and nothing else. No half-filled form
//! can be on screen to lose.
//!
//! The PR pages the tray opens are not part of this site. They are `crate::page`'s, and they link to
//! the muted page, nothing else.

pub mod integrations;
pub mod portals;
pub mod updates;

use crate::page::esc;
use crate::setting::{Kind, Setting};
use std::collections::BTreeMap;

/// A page of the site.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Place {
    General,
    Portals,
    /// A portal by its id. Whether one by that id is configured is the handler's question; the shape
    /// is checked here, so the segment can never be more than a word.
    Portal(String),
    Integrations,
    /// Only an id the registry knows.
    Integration(&'static str),
    Muted,
    Updates,
    Advanced,
}

impl Place {
    /// The place a path names, or `None`. Pure, so the router's whole idea of the site is tested
    /// without a socket.
    pub fn parse(leaf: &str, tail: Option<&str>) -> Option<Place> {
        let word = |id: &str| !id.is_empty() && id.len() <= 32 && id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
        Some(match (leaf, tail) {
            ("general", None) => Place::General,
            ("portals", None) => Place::Portals,
            ("portals", Some(id)) if word(id) => Place::Portal(id.to_string()),
            ("integrations", None) => Place::Integrations,
            ("integrations", Some(id)) => Place::Integration(crate::integration::find(id)?.info().id),
            ("muted", None) => Place::Muted,
            ("updates", None) => Place::Updates,
            ("advanced", None) => Place::Advanced,
            _ => return None,
        })
    }

    /// The path after the token, as links and redirects use it.
    pub fn path(&self) -> String {
        match self {
            Place::General => "general".to_string(),
            Place::Portals => "portals".to_string(),
            Place::Portal(id) => format!("portals/{id}"),
            Place::Integrations => "integrations".to_string(),
            Place::Integration(id) => format!("integrations/{id}"),
            Place::Muted => "muted".to_string(),
            Place::Updates => "updates".to_string(),
            Place::Advanced => "advanced".to_string(),
        }
    }

    /// The collection a page sits in, for the sidebar to mark.
    fn parent(&self) -> Option<Place> {
        match self {
            Place::Portal(_) => Some(Place::Portals),
            Place::Integration(_) => Some(Place::Integrations),
            _ => None,
        }
    }

    /// Whether a POST here is something the page offers. The integrations list carries buttons that
    /// post to the integration they are about, so a POST to it is the wrong method. The portals list
    /// takes one: Install is for a portal that has no page of its own yet.
    pub fn takes_posts(&self) -> bool {
        !matches!(self, Place::Integrations)
    }
}

/// What the sidebar needs to know that is not fixed: which portals you are signed in to, which
/// integrations are installed, how many pull requests are muted, and whether a newer release is waiting.
pub struct Site<'a> {
    pub token: &'a str,
    /// This response's script nonce. Empty means no script on the page, and every control still
    /// works by posting its form.
    pub nonce: &'a str,
    /// `(id, display name, signed in)`, in configuration order. The sidebar lists only the portals
    /// you are signed in to; the list page is where the rest are found and signed in to.
    pub portals: Vec<(String, String, bool)>,
    /// The ids of the integrations that are installed. The sidebar lists only these; the list page
    /// is where the rest are found and installed.
    pub installed: Vec<&'static str>,
    pub muted: usize,
    /// The version a check found, when it is newer than this one.
    pub update: Option<String>,
}

/// The sidebar: every page, collections with their items under them, where you are marked.
///
/// The current page is `aria-current` and highlighted; the collection it sits in is marked too, so a
/// page three clicks deep still says which part of the site it belongs to.
pub fn sidebar(site: &Site, current: &Place) -> String {
    let parent = current.parent();
    let link = |place: Place, label: &str, child: bool| {
        let mut classes = Vec::new();
        if child {
            classes.push("child");
        }
        let here = place == *current;
        if here {
            classes.push("on");
        } else if parent.as_ref() == Some(&place) {
            classes.push("up");
        }
        let class = if classes.is_empty() { String::new() } else { format!(" class=\"{}\"", classes.join(" ")) };
        let aria = if here { " aria-current=\"page\"" } else { "" };
        // An item names its collection for the narrow window, where the sidebar folds into one row of
        // pills and "GitHub" would otherwise read as an equal of "General". Hidden when wide.
        let up = match (&place, child) {
            (Place::Portal(_), true) => "<span class=\"up-name\">Portals › </span>",
            (Place::Integration(_), true) => "<span class=\"up-name\">Integrations › </span>",
            _ => "",
        };
        // The label is already HTML where it carries a badge; every part of it was escaped on the way in.
        format!("<a{class}{aria} href=\"/{}/{}\">{up}{label}</a>", esc(site.token), place.path())
    };
    let mut items = vec![link(Place::General, "General", false), link(Place::Portals, "Portals", false)];
    for (id, name, signed_in) in &site.portals {
        let place = Place::Portal(id.clone());
        // Signed in, or the page you are on, for the same reason as the integrations below.
        if *signed_in || place == *current {
            items.push(link(place, &esc(name), true));
        }
    }
    items.push(link(Place::Integrations, "Integrations", false));
    for integration in crate::integration::all() {
        let info = integration.info();
        // Installed, or the page you are on: a sidebar that cannot show where you are is lost.
        if site.installed.contains(&info.id) || *current == Place::Integration(info.id) {
            items.push(link(Place::Integration(info.id), &esc(info.name), true));
        }
    }
    let muted = if site.muted > 0 { format!("Muted ({})", site.muted) } else { "Muted".to_string() };
    items.push(link(Place::Muted, &muted, false));
    let updates = match &site.update {
        Some(version) => format!("Updates <span class=\"badge\">{}</span>", esc(version)),
        None => "Updates".to_string(),
    };
    items.push(link(Place::Updates, &updates, false));
    items.push(link(Place::Advanced, "Advanced", false));
    format!("<nav class=\"side\" aria-label=\"GitHoot settings\">{}</nav>", items.join(""))
}

/// A whole page: the shared head, the sidebar, and `body`.
///
/// `refresh` makes it reload itself to its own plain address every so many seconds. Only a page
/// showing something that is running asks for it; see the module comment.
///
/// The title is a breadcrumb trail from Settings to `title`, the page's own name: every step a link
/// but the last, so a page deep in a collection says where it is and how to get back.
pub fn layout(site: &Site, place: &Place, title: &str, refresh: Option<u32>, body: &str) -> String {
    let mut trail: Vec<(String, Option<String>)> = vec![("Settings".to_string(), Some(Place::General.path()))];
    if let Some(parent) = place.parent() {
        let label = match parent {
            Place::Portals => "Portals",
            _ => "Integrations",
        };
        trail.push((label.to_string(), Some(parent.path())));
    }
    trail.push((title.to_string(), None));
    let sep = "<span class=\"sep\" aria-hidden=\"true\">›</span>";
    let crumbs: Vec<String> = trail
        .iter()
        .map(|(label, path)| match path {
            Some(path) => format!("<a href=\"/{}/{path}\">{}</a>", esc(site.token), esc(label)),
            None => format!("<span>{}</span>", esc(label)),
        })
        .collect();
    let heading = format!("<h1 class=\"crumbs\">{}</h1>", crumbs.join(sep));
    let tab: Vec<&str> = trail.iter().map(|(label, _)| label.as_str()).collect();
    let mut h = crate::page::shell_with(
        &tab.join(" › "),
        crate::icons::css_hex(crate::icons::MERGE_DOT_COLOR),
        site.token,
        refresh.map(|secs| (secs, place.path())),
        "site-main",
        Some(&heading),
    );
    h.push_str(&format!("<div class=\"site\">{}<div class=\"pane\">\n{body}</div></div>\n", sidebar(site, place)));
    if !site.nonce.is_empty() {
        h.push_str(&format!("<script nonce=\"{}\">{SITE_SCRIPT}</script>\n", esc(site.nonce)));
    }
    h.push_str("</main>\n</body>\n</html>\n");
    h
}

// ── Sections drawn from declarations ─────────────────────────────────────────

/// What every settings page runs, when it has a nonce.
///
/// **Checkboxes and choices save as they change.** A section with no text box is marked
/// `data-autosave`; this hides its Save button and posts the section in the background on every
/// change, asking for a JSON reply, and writes the server's line beside the controls: "Saved." fades
/// after two seconds, a line about a restart stays. Text is still saved on purpose with its button,
/// since half-typed text should not save itself. Without the script every Save button is there and
/// posts as it always did.
///
/// **A "whole or only these" list** (`[data-parts]`) shows its boxes only while "Only these" is chosen.
const SITE_SCRIPT: &str = "(function(){\
document.querySelectorAll('form[data-autosave]').forEach(function(f){f.classList.add('auto');});\
function parts(){document.querySelectorAll('[data-parts]').forEach(function(p){var some=p.querySelector('input[value=\"some\"]');\
var boxes=p.querySelector('.boxes');if(some&&boxes)boxes.hidden=!some.checked;});}parts();\
function counts(){document.querySelectorAll('[data-gates-count]').forEach(function(c){var boxes=c.closest('details').querySelectorAll('.gates input[type=checkbox]');\
var on=0;boxes.forEach(function(b){if(b.checked)on++;});c.textContent=on+' of '+boxes.length+' '+(c.getAttribute('data-noun')||'rules')+' on';});}\
function mirror(t){if(t.type!=='checkbox'||!t.form)return;document.querySelectorAll('input[type=checkbox]').forEach(function(o){\
if(o!==t&&o.form===t.form&&o.name===t.name)o.checked=t.checked;});}/* same name, same form: a data-mirror box and its original */\
document.addEventListener('change',function(ev){mirror(ev.target);parts();counts();var f=ev.target.form;if(!f||!f.hasAttribute('data-autosave'))return;\
var lines=[f.querySelector('.save-line')];document.querySelectorAll('[data-save-for=\"'+f.id+'\"]').forEach(function(l){lines.push(l);});\
function say(t){lines.forEach(function(l){if(l)l.textContent=t;});}say('Saving…');clearTimeout(f.fade);\
var body=new URLSearchParams(new FormData(f));body.append('reply','json');\
fetch(f.getAttribute('action'),{method:'POST',body:body}).then(function(r){return r.json();}).then(function(j){\
say(j.line);if(j.fade){f.fade=setTimeout(function(){say('');},2000);}\
}).catch(function(){say('Not saved: GitHoot did not answer. Is it still running?');});});\
})();";

/// Every section `settings` is cut into, each its own form posting to `place` with its index.
///
/// `value` gives the text the file holds for a key. `flash` is the line the last Save on this page
/// left, with the index of the section it belongs under.
pub fn sections(
    token: &str,
    place: &Place,
    settings: &'static [Setting],
    value: &dyn Fn(&Setting) -> String,
    flash: Option<&(usize, String)>,
) -> String {
    crate::setting::groups(settings)
        .into_iter()
        .enumerate()
        .map(|(i, (title, members))| {
            // A section of steps is drawn as a rule line rather than as a list of checkboxes.
            let fields: String = if members.iter().all(|s| matches!(s.kind, Kind::Step { .. })) {
                rule_line(title, &members, value)
            } else {
                members.iter().map(|s| field(s, &value(s))).collect()
            };
            let line = match flash {
                Some((at, line)) if *at == i => esc(line),
                _ => String::new(),
            };
            // No text box in it: the section saves as it changes, and the script hides its button.
            let auto = if members.iter().any(|s| matches!(s.kind, Kind::Text { .. })) { "" } else { " data-autosave" };
            // The green bar's rules wear the bar itself in front of their title.
            let mark = if title == crate::config::GREEN_RULES_GROUP {
                "<span class=\"bar-mark bar-merge\" aria-hidden=\"true\"></span>"
            } else if title == crate::config::RED_RULES_GROUP {
                "<span class=\"bar-mark bar-review\" aria-hidden=\"true\"></span>"
            } else {
                ""
            };
            format!(
                "<section class=\"block\" id=\"s{i}\"><h2 class=\"section\">{mark}{}</h2><div class=\"card\">\
                 <form id=\"form-s{i}\" method=\"post\" action=\"/{}/{}\"{auto}><input type=\"hidden\" name=\"section\" value=\"{i}\">{fields}\
                 <div class=\"actions\"><button class=\"small save\" type=\"submit\">Save</button>\
                 <span class=\"save-line\" aria-live=\"polite\">{line}</span></div></form></div></section>\n",
                esc(title),
                esc(token),
                place.path(),
            )
        })
        .collect()
}

/// How one bar's line reads: what its folded row says, the fixed steps it opens with, the colour of
/// its end node and what the end is called. Red and green are paths a pull request walks to the bar;
/// see `amber_line` for the one bar that is a list of reasons instead.
struct Line {
    summary: &'static str,
    /// The line under the summary. Red's rules act on red only; green's act on green and amber.
    lead: &'static str,
    fixed: &'static [(&'static str, &'static str)],
    end: &'static str,
    bar: &'static str,
}

fn line_for(group: &str) -> Line {
    if group == crate::config::RED_RULES_GROUP {
        Line {
            summary: "What a review request has to pass to light the red bar",
            lead: "A step you switch off is no longer checked.",
            fixed: &[("A review was asked of you", ""), ("You have not given it yet", "")],
            end: "Red bar",
            bar: "review",
        }
    } else {
        Line {
            summary: "What one of your pull requests has to pass to light the green bar",
            lead: "A step you switch off is ignored on both bars.",
            fixed: &[("Somebody approved it", ""), ("Nobody's objection stands", "else amber")],
            end: "Green bar",
            bar: "merge",
        }
    }
}

/// The badge naming the portals a step works on, when not all of them. A switch that does nothing
/// for the portal you use says so rather than looking broken.
fn portal_badge(key: &str) -> String {
    crate::config::step_portals(key)
        .map(|kinds| {
            let names: Vec<&str> = kinds.iter().map(|k| k.display_name()).collect();
            format!("<span class=\"gate-portal\">{}</span>", esc(&names.join(" · ")))
        })
        .unwrap_or_default()
}

/// The fold every bar's line sits in: one row that says how it is set, opened on a click. The count
/// is kept in step by the site script as boxes are ticked, so the closed row never lies.
fn fold(summary: &str, count: String, noun: &str, lead: &str, bar: &str, items: String, end: &str) -> String {
    // "rules" is what the script assumes; any other word travels with the count so a recount keeps it.
    let noun_attr = if noun == "rules" { String::new() } else { format!(" data-noun=\"{noun}\"") };
    format!(
        "<details class=\"gates-fold\"><summary><span class=\"gates-sum\">{summary}</span>\
         <span class=\"gates-count\" data-gates-count{noun_attr}>{count}</span></summary>\
         <p class=\"sub gate-lead\">{lead}</p><ol class=\"gates gates-{bar}\">{items}\
         <li class=\"gate end\"><span class=\"gate-node\" aria-hidden=\"true\"></span><span class=\"gate-text\">{end}</span></li></ol></details>"
    )
}

fn fixed_step(text: &str, otherwise: &str) -> String {
    let chip = if otherwise.is_empty() { String::new() } else { format!("<span class=\"gate-else else-amber\">{otherwise}</span>") };
    format!(
        "<li class=\"gate fixed\"><span class=\"gate-node\" aria-hidden=\"true\"></span><span class=\"gate-text\">{}</span>{chip}</li>",
        esc(text)
    )
}

fn switch_step(key: &str, label: &str, on: bool, form: Option<&str>, otherwise: &str, help: &str) -> String {
    let tone = if otherwise.contains("amber") { "else-amber" } else { "else-wait" };
    let bound = form.map(|id| format!(" form=\"{}\" data-mirror", esc(id))).unwrap_or_default();
    format!(
        "<li class=\"gate\"><label class=\"gate-row\"><input type=\"checkbox\" name=\"{}\" value=\"on\"{bound}{}>\
         <span class=\"gate-title\"><span class=\"gate-text\">{}</span>{}</span></label>\
         <span class=\"gate-else {tone}\">{}</span><p class=\"sub gate-help\">{}</p></li>",
        esc(key),
        if on { " checked" } else { "" },
        esc(label),
        portal_badge(key),
        esc(otherwise),
        esc(help),
    )
}

/// A bar's rules as the path a pull request walks to it, top to bottom: the fixed steps that define
/// the bar (dots, not switches), then one switch per rule with what happens to a pull request that
/// fails it, ending at the bar itself.
///
/// The order is who decides: people, then git, then CI, then bots. It is not a ladder: every switch
/// is independent. A rule switched off fades and its stretch of the line turns dashed.
fn rule_line(group: &str, steps: &[&'static Setting], value: &dyn Fn(&Setting) -> String) -> String {
    let line = line_for(group);
    let on = steps.iter().filter(|s| crate::setting::flag(s.kind, &value(s))).count();
    let mut items: String = line.fixed.iter().map(|(text, otherwise)| fixed_step(text, otherwise)).collect();
    for s in steps {
        let Kind::Step { otherwise, .. } = s.kind else { continue };
        items.push_str(&switch_step(s.key, s.label, crate::setting::flag(s.kind, &value(s)), None, otherwise, s.help));
    }
    fold(
        line.summary,
        format!("{on} of {} rules on", steps.len()),
        "rules",
        line.lead,
        line.bar,
        items,
        line.end,
    )
}

/// The amber bar is a list of reasons, not a path: any one puts a pull request there. Three of them
/// are the green bar's own switches, because each rule acts on both bars, so they are drawn here as
/// the very same inputs, bound to the green section's form (`form="…"`). One setting, one answer,
/// shown in both places; the site script keeps the two boxes in step.
fn amber_line(green_form: &str, value: &dyn Fn(&str) -> String) -> String {
    const REASONS: [(&str, &str, &str); 3] = [
        ("ruleConflicts", "A merge conflict while somebody reviews it", "A conflict nobody is waiting on blocks nobody."),
        ("ruleFailedChecks", "Approved, but its checks failed", "Red CI on an approved pull request is work, not good news."),
        ("copilotReviews", "Automatic reviewer's comments open", "Copilot reviews by commenting, never by a verdict. Open, current comments count."),
    ];
    let on = REASONS.iter().filter(|(key, _, _)| crate::config::is_on_or_default(&value(key))).count();
    let mut items = fixed_step("A reviewer's request for changes stands", "");
    for (key, label, help) in REASONS {
        items.push_str(&switch_step(key, label, crate::config::is_on_or_default(&value(key)), Some(green_form), "also off green", help));
    }
    fold(
        "Any one of these puts one of your pull requests on the amber bar",
        format!("{on} of {} reasons on", REASONS.len()),
        "reasons",
        "A request for changes stands until you ask that reviewer for their review again. The switches here are the green bar's own: one setting, both bars.",
        "changes",
        items,
        "Amber bar",
    )
}

/// One declared setting as its control, showing `value`, with its help underneath.
fn field(s: &Setting, value: &str) -> String {
    let help = if s.help.is_empty() { String::new() } else { format!("<p class=\"sub help\">{}</p>", esc(s.help)) };
    let checked = |on: bool| if on { " checked" } else { "" };
    match s.kind {
        Kind::Flag { .. } | Kind::Step { .. } => format!(
            "<label class=\"row\"><input type=\"checkbox\" name=\"{}\" value=\"on\"{}> {}</label>{help}",
            esc(s.key),
            checked(crate::setting::flag(s.kind, value)),
            esc(s.label)
        ),
        Kind::Text { placeholder } => format!(
            "<label class=\"path\"><span>{}</span><input type=\"text\" name=\"{}\" value=\"{}\" placeholder=\"{}\" spellcheck=\"false\"></label>{help}",
            esc(s.label),
            esc(s.key),
            esc(value),
            esc(placeholder)
        ),
        Kind::Choice { options } => {
            let radios: String = options
                .iter()
                .map(|(v, label)| {
                    format!(
                        "<label class=\"row\"><input type=\"radio\" name=\"{}\" value=\"{}\"{}> {}</label>",
                        esc(s.key),
                        esc(v),
                        checked(*v == value),
                        esc(label)
                    )
                })
                .collect();
            format!("<fieldset><legend>{}</legend>{radios}</fieldset>{help}", esc(s.label))
        }
        Kind::Parts { options, whole, some } => {
            let ticked: Vec<&str> = crate::setting::split(value).collect();
            let boxes: String = options
                .iter()
                .map(|o| {
                    format!(
                        "<label class=\"row\"><input type=\"checkbox\" name=\"{}\" value=\"{}\"{}> {}</label>",
                        esc(s.key),
                        esc(o),
                        checked(ticked.contains(o)),
                        esc(o)
                    )
                })
                .collect();
            let scope = |v: &str, label: &str, on: bool| {
                format!(
                    "<label class=\"row\"><input type=\"radio\" name=\"{}.scope\" value=\"{v}\"{}> {}</label>",
                    esc(s.key),
                    checked(on),
                    esc(label)
                )
            };
            format!(
                "<fieldset data-parts><legend>{}</legend>{help}{}{}<div class=\"boxes\">{boxes}</div></fieldset>",
                esc(s.label),
                scope("all", whole, ticked.is_empty()),
                scope("some", some, !ticked.is_empty()),
            )
        }
    }
}

// ── One line after a press ───────────────────────────────────────────────────

/// The line a press left for its page, by page path: which section it belongs under, and what it
/// says. Taken once, so a reload does not repeat it.
static FLASH: std::sync::Mutex<BTreeMap<String, (usize, String)>> = std::sync::Mutex::new(BTreeMap::new());

pub fn flash(place: &Place, section: usize, line: String) {
    if let Ok(mut m) = FLASH.lock() {
        m.insert(place.path(), (section, line));
    }
}

pub fn take_flash(place: &Place) -> Option<(usize, String)> {
    FLASH.lock().ok().and_then(|mut m| m.remove(&place.path()))
}

// ── The plain pages ──────────────────────────────────────────────────────────

/// General: what the tray shows and does.
pub fn general_page(site: &Site, cfg: &crate::config::Config, flash: Option<&(usize, String)>) -> String {
    let value = |s: &Setting| crate::config::value_of(cfg, s.key);
    // The amber bar's reasons follow the green bar's section, bound to its form: see `amber_line`.
    let green = crate::setting::groups(crate::config::GENERAL)
        .iter()
        .position(|(title, _)| *title == crate::config::GREEN_RULES_GROUP)
        .expect("General declares the green bar's rules");
    let amber = amber_line(&format!("form-s{green}"), &|key| crate::config::value_of(cfg, key));
    let body = format!(
        "<p class=\"sub lead\">What the tray shows and does. Changes save as you make them.</p>\n{}\
         <section class=\"block\" id=\"amber\"><h2 class=\"section\"><span class=\"bar-mark bar-changes\" aria-hidden=\"true\"></span>\
         What makes a pull request amber</h2><div class=\"card\">{amber}<div class=\"actions auto\">\
         <span class=\"save-line\" aria-live=\"polite\" data-save-for=\"form-s{green}\"></span></div></div></section>\n",
        sections(site.token, &Place::General, crate::config::GENERAL, &value, flash)
    );
    layout(site, &Place::General, "General", None, &body)
}

/// Advanced: the settings that open something or change what is written, rather than what is shown.
pub fn advanced_page(site: &Site, cfg: &crate::config::Config, flash: Option<&(usize, String)>) -> String {
    let value = |s: &Setting| crate::config::value_of(cfg, s.key);
    let body = format!(
        "<p class=\"sub lead\">Settings you rarely need. Both take effect after a restart.</p>\n{}\
         <footer>Written to <code>config.txt</code>, one line per changed setting — your comments and any \
         keys this version has never heard of are left alone.</footer>\n",
        sections(site.token, &Place::Advanced, crate::config::ADVANCED, &value, flash)
    );
    layout(site, &Place::Advanced, "Advanced", None, &body)
}

/// One row of the muted page: a live mute, and the pull request it names if a bar still holds it.
pub struct MutedRow<'a> {
    pub key: &'a str,
    pub until: u64,
    /// The bar it was found in, the entry, and that portal's link prefix. `None` when no bar holds it
    /// any more, typically because it was merged or closed while muted.
    pub found: Option<(crate::state::PrAxis, &'a crate::portal::types::PrEntry, &'a str)>,
}

/// Every muted pull request in one place, across all three bars, each with Unmute.
///
/// This exists because a muted pull request can otherwise be unreachable: an empty bar hides its
/// menu entry, so a bar whose only pull request is muted has no page to unmute it from. Reached from
/// the sidebar, and from the muted section of any bar.
pub fn muted_page(site: &Site, rows: &[MutedRow], now_unix: u64) -> String {
    use crate::state::PrAxis;
    let token = site.token;
    let mut h = String::from(
        "<p class=\"sub lead\">A muted pull request stays out of its bar until the mute ends, then comes back as new.</p>\n",
    );
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
        h.push_str(&format!("<h2 class=\"section\">{}</h2>\n", esc(crate::page::heading(axis))));
        for r in here {
            let (_, e, prefix) = r.found.expect("filtered to found rows");
            h.push_str(&crate::page::card(e, prefix, now_unix, &unmute(r.key, r.until)));
        }
    }
    let gone: Vec<_> = rows.iter().filter(|r| r.found.is_none()).collect();
    if !gone.is_empty() {
        h.push_str("<h2 class=\"section\">No longer in any bar</h2>\n");
        h.push_str(
            "<p class=\"sub\">Merged, closed, or simply not waiting on you right now. The mute ends by itself; \
             unmute it to have it count as new the next time it turns up.</p>\n",
        );
        for r in gone {
            h.push_str(&format!("<div class=\"card\"><h2><code>{}</code></h2>{}</div>\n", esc(r.key), unmute(r.key, r.until)));
        }
    }
    layout(site, &Place::Muted, "Muted pull requests", None, &h)
}

/// A gear, drawn in `currentColor` so it follows the text in both themes. Inline, because the page
/// loads nothing it does not carry, and hidden from screen readers, which get the link's label.
pub const GEAR: &str = "<svg viewBox=\"0 0 24 24\" aria-hidden=\"true\" fill=\"none\" stroke=\"currentColor\" \
stroke-width=\"2\" stroke-linecap=\"round\" stroke-linejoin=\"round\"><circle cx=\"12\" cy=\"12\" r=\"3\"/>\
<path d=\"M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 1 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 \
1.65 1.65 0 0 0-1 1.51V21a2 2 0 1 1-4 0v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 1 1-2.83-2.83\
l.06-.06A1.65 1.65 0 0 0 4.68 15a1.65 1.65 0 0 0-1.51-1H3a2 2 0 1 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82\
l-.06-.06a2 2 0 1 1 2.83-2.83l.06.06A1.65 1.65 0 0 0 9 4.68a1.65 1.65 0 0 0 1-1.51V3a2 2 0 1 1 4 0v.09a1.65 1.65 0 0 0 1 \
1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 1 1 2.83 2.83l-.06.06A1.65 1.65 0 0 0 19.4 9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 \
1 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z\"/></svg>";

/// The ⚙ Settings button on a collection's card: a button, not the name as a link, because nothing
/// about a bold word says "click here to set this up".
pub fn settings_button(token: &str, path: &str, name: &str) -> String {
    format!(
        "<a class=\"ghost\" href=\"/{}/{}\" aria-label=\"{} settings\">{GEAR}Settings</a>",
        esc(token),
        esc(path),
        esc(name)
    )
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub fn site() -> Site<'static> {
        Site { token: "tok", nonce: "", portals: vec![("github".into(), "GitHub".into(), true)], installed: vec!["herdr"], muted: 2, update: None }
    }

    /// A section of checkboxes and choices saves as it changes: the form says so, its Save button is
    /// marked for the script to hide, and the line that reports the save is always there to fill.
    #[test]
    fn a_section_without_text_saves_as_it_changes() {
        let value = |_: &Setting| "on".to_string();
        let html = sections("tok", &Place::General, crate::config::GENERAL, &value, None);
        assert!(html.contains(r#"<form id="form-s0" method="post" action="/tok/general" data-autosave>"#), "{html}");
        assert!(html.contains(r#"<button class="small save" type="submit">Save</button>"#), "{html}");
        assert!(html.contains(r#"<span class="save-line" aria-live="polite"></span>"#), "{html}");
    }

    /// Text is saved on purpose, not on every keystroke: a section with a text box keeps its button.
    #[test]
    fn a_section_with_text_keeps_its_save_button() {
        static TEXTY: [Setting; 2] = [
            Setting { key: "a", label: "A", kind: Kind::Flag { default_on: true }, help: "", group: "G", live: true },
            Setting { key: "b", label: "B", kind: Kind::Text { placeholder: "" }, help: "", group: "G", live: true },
        ];
        let value = |_: &Setting| String::new();
        let html = sections("tok", &Place::General, &TEXTY, &value, None);
        assert!(!html.contains("data-autosave"), "{html}");
    }

    /// The outage list says what empty means as a choice, and lays its boxes out in columns under
    /// "Only these", which the script shows only while it is chosen.
    #[test]
    fn a_parts_list_offers_the_whole_or_only_these() {
        static SCOPED: [Setting; 1] = [Setting {
            key: "parts",
            label: "Parts",
            kind: Kind::Parts { options: &["API", "Pages"], whole: "Everything", some: "Only these" },
            help: "",
            group: "G",
            live: false,
        }];
        let whole = sections("tok", &Place::General, &SCOPED, &|_| String::new(), None);
        assert!(whole.contains("<fieldset data-parts>"), "{whole}");
        assert!(whole.contains(r#"<input type="radio" name="parts.scope" value="all" checked> Everything"#), "{whole}");
        assert!(whole.contains(r#"<input type="radio" name="parts.scope" value="some"> Only these"#), "{whole}");
        assert!(whole.contains(r#"<div class="boxes">"#), "{whole}");
        let some = sections("tok", &Place::General, &SCOPED, &|_| "Pages".to_string(), None);
        assert!(some.contains(r#"value="some" checked> Only these"#), "{some}");
        assert!(some.contains(r#"<input type="checkbox" name="parts" value="Pages" checked> Pages"#), "{some}");
    }

    /// On a narrow window the sidebar folds into a row of pills, where an item would read as an equal
    /// of the pages. Each item carries its collection's name for that row, hidden on a wide one.
    #[test]
    fn an_item_in_the_sidebar_carries_its_collections_name_for_the_narrow_row() {
        let html = sidebar(&site(), &Place::General);
        assert!(html.contains(r#"href="/tok/portals/github"><span class="up-name">Portals › </span>GitHub</a>"#), "{html}");
        assert!(html.contains(r#"<span class="up-name">Integrations › </span>Herdr dispatcher</a>"#), "{html}");
    }

    /// The green bar's rules are drawn as the path a pull request walks to green: two fixed steps,
    /// then one switch per rule with what happens when it fails, ending at the green bar. One form
    /// that saves as it changes.
    #[test]
    fn the_green_bar_rules_are_drawn_as_a_line_to_green() {
        let value = |s: &Setting| if s.key == "ruleRunningChecks" { "off".to_string() } else { "on".to_string() };
        let html = sections("tok", &Place::General, crate::config::GENERAL, &value, None);
        let at = html.find(r#"<ol class="gates gates-merge">"#).expect("the green line");
        let line = &html[at..at + html[at..].find("</ol>").unwrap()];
        let order: Vec<usize> = ["Somebody approved it", "Nobody&#39;s objection stands", "No merge conflict", "Checks did not fail", "Checks have finished", "Automatic reviewer&#39;s comments resolved", "Green bar"]
            .iter()
            .map(|label| line.find(label).unwrap_or_else(|| panic!("{label} missing: {line}")))
            .collect();
        assert!(order.windows(2).all(|w| w[0] < w[1]), "in order: {order:?}");
        assert!(line.contains(r#"<li class="gate fixed">"#), "fixed steps are dots, not switches");
        assert!(line.contains(r#"<input type="checkbox" name="ruleConflicts" value="on" checked>"#), "{line}");
        assert!(line.contains(r#"<input type="checkbox" name="ruleRunningChecks" value="on">"#), "off: unticked");
        assert!(line.contains(r#"<span class="gate-else else-amber">else amber</span>"#), "{line}");
        assert!(line.contains(r#"<span class="gate-else else-wait">else waits</span>"#), "{line}");
        assert!(line.contains(r#"<li class="gate end">"#), "{line}");
        // A step only some portals can judge carries a badge naming them; the others carry none.
        assert!(line.contains(r#"comments resolved</span><span class="gate-portal">GitHub</span>"#), "{line}");
        assert_eq!(line.matches("gate-portal").count(), 1, "only the Copilot step is portal-specific today");
        let form = &html[..at];
        assert!(form[form.rfind("<form").unwrap()..].contains("data-autosave"), "saves as it changes");
    }

    /// The green bar's line is folded away by default, to one row that still says how it is set,
    /// and opens on a click. The count is kept in step by the site script as boxes are ticked.
    #[test]
    fn the_green_bar_rules_are_folded_away_by_default_with_a_count() {
        let value = |s: &Setting| if s.key == "ruleRunningChecks" { "off".to_string() } else { "on".to_string() };
        let html = sections("tok", &Place::General, crate::config::GENERAL, &value, None);
        let green = html.find("bar-mark bar-merge").expect("the green section");
        let at = green + html[green..].find(r#"<details class="gates-fold">"#).expect("folded, and closed: no open attribute");
        let fold = &html[at..at + html[at..].find("</details>").unwrap()];
        assert!(fold.contains(r#"<span class="gates-count" data-gates-count>3 of 4 rules on</span>"#), "{fold}");
        assert!(fold.find("<summary").unwrap() < fold.find(r#"<ol class="gates gates-merge">"#).unwrap(), "the line is inside the fold");
        let html_all_on = sections("tok", &Place::General, crate::config::GENERAL, &|_| "on".to_string(), None);
        assert!(html_all_on.contains(">4 of 4 rules on<"));
        assert!(SITE_SCRIPT.contains("data-gates-count"), "the count follows the boxes");
        // The section is about the green bar, so its title wears it: the tray's own pill, in its green.
        assert!(html.contains(r#"<h2 class="section"><span class="bar-mark bar-merge" aria-hidden="true"></span>What makes a pull request green</h2>"#), "{html}");
    }

    /// All three bars get their line, in bar order, each folded, each wearing its bar. Red is a path
    /// like green; amber is a list of reasons whose switches are the very same inputs as green's,
    /// bound to green's form, so one setting never shows two answers.
    #[test]
    fn each_bar_has_its_folded_line_in_bar_order() {
        let cfg = crate::config::Config::from_text("ruleSkipBots=off\n");
        let html = general_page(&site(), &cfg, None);
        let red = html.find("bar-mark bar-review").expect("red");
        let green = html.find("bar-mark bar-merge").expect("green");
        let amber = html.find("bar-mark bar-changes").expect("amber");
        assert!(red < green && green < amber, "bar order");
        assert_eq!(html.matches(r#"<details class="gates-fold">"#).count(), 3, "all folded");
        assert!(html.contains(">1 of 2 rules on<"), "red counts its own switches: {html}");
        for text in ["A review was asked of you", "You have not given it yet", "Team requests count", "Not opened by a bot", "Red bar"] {
            assert!(html[red..green].contains(text), "{text}");
        }
        let amber_html = &html[amber..];
        for text in ["A reviewer&#39;s request for changes stands", "Amber bar"] {
            assert!(amber_html.contains(text), "{text}: {amber_html}");
        }
        let green_form = html[green..amber].find("<form").map(|i| &html[green + i..]).expect("green's form");
        let id = green_form.split("id=\"").nth(1).and_then(|r| r.split('"').next()).expect("the form has an id");
        for key in ["ruleConflicts", "ruleFailedChecks", "copilotReviews"] {
            assert!(amber_html.contains(&format!(r#"name="{key}" value="on" form="{id}""#)), "{key} bound to green's form: {amber_html}");
        }
        assert!(amber_html.contains(r#"<span class="gate-else else-wait">also off green</span>"#));
        // The amber count keeps its own word when the script recounts it.
        assert!(amber_html.contains(r#"data-gates-count data-noun="reasons">3 of 3 reasons on<"#), "{amber_html}");
        assert!(SITE_SCRIPT.contains("data-noun"));
        // Red's rules act on red only, so its lead says so; amber reports saves in its own card.
        assert!(html[red..green].contains("A step you switch off is no longer checked."), "red's own lead");
        assert!(!html[red..green].contains("both bars"), "red's rules do not touch the other bars");
        assert!(amber_html.contains(&format!(r#"<span class="save-line" aria-live="polite" data-save-for="{id}"></span>"#)), "{amber_html}");
        assert!(SITE_SCRIPT.contains("data-save-for"));
        assert!(SITE_SCRIPT.contains("data-mirror") || SITE_SCRIPT.contains("same name"), "shared switches stay in step");
    }

    /// Every page carries the site script when it has a nonce, and nothing without one.
    #[test]
    fn every_page_carries_the_site_script_with_a_nonce() {
        let html = layout(&Site { nonce: "n", ..site() }, &Place::Muted, "Muted", None, "");
        assert!(html.contains("<script nonce=\"n\">") && html.contains("form[data-autosave]"), "{html}");
        assert!(!layout(&site(), &Place::Muted, "Muted", None, "").contains("<script"));
    }

    #[test]
    fn every_page_has_one_path_and_parses_back_from_it() {
        let places = [
            Place::General,
            Place::Portals,
            Place::Portal("github".into()),
            Place::Integrations,
            Place::Integration("herdr"),
            Place::Muted,
            Place::Updates,
            Place::Advanced,
        ];
        for place in places {
            let path = place.path();
            let (leaf, tail) = match path.split_once('/') {
                Some((l, t)) => (l.to_string(), Some(t.to_string())),
                None => (path.clone(), None),
            };
            assert_eq!(Place::parse(&leaf, tail.as_deref()), Some(place), "{path}");
        }
    }

    /// Only words, only ids the registry knows, and nothing the old pages were called.
    #[test]
    fn anything_else_is_not_a_page() {
        for (leaf, tail) in [
            ("settings", None),
            ("accounts", None),
            ("dispatcher", None),
            ("integrations", Some("nope")),
            ("integrations", Some("HERDR")),
            ("portals", Some("../x")),
            ("portals", Some("")),
            ("portals", Some("Git Hub")),
            ("general", Some("x")),
            ("muted", Some("x")),
        ] {
            assert_eq!(Place::parse(leaf, tail), None, "{leaf}/{tail:?}");
        }
    }

    #[test]
    fn the_sidebar_lists_every_page_with_items_under_their_collection() {
        let html = sidebar(&site(), &Place::General);
        let order = ["/tok/general\"", "/tok/portals\"", "/tok/portals/github\"", "/tok/integrations\"", "/tok/integrations/herdr\"", "/tok/muted\"", "/tok/updates\"", "/tok/advanced\""];
        let mut at = 0;
        for href in order {
            let found = html[at..].find(href).unwrap_or_else(|| panic!("{href} missing or out of order in {html}"));
            at += found + href.len();
        }
        assert!(html.contains(r#"class="child""#), "items sit under their collection");
        assert!(html.contains(">Muted (2)<"));
    }

    /// Where you are is marked twice: the page itself, and the collection it sits in.
    #[test]
    fn the_sidebar_marks_the_page_and_the_collection_it_sits_in() {
        let html = sidebar(&site(), &Place::Integration("herdr"));
        assert!(html.contains(r#"<a class="child on" aria-current="page" href="/tok/integrations/herdr">"#), "{html}");
        assert!(html.contains(r#"<a class="up" href="/tok/integrations">Integrations</a>"#), "{html}");
        assert_eq!(html.matches("aria-current").count(), 1);
    }

    /// Under Integrations only what is installed: the list page shows everything else. The one
    /// exception is the page you are on, so the sidebar can always say where you are.
    #[test]
    fn the_sidebar_lists_only_installed_integrations_and_the_one_you_are_on() {
        let none = Site { installed: vec![], ..site() };
        assert!(!sidebar(&none, &Place::General).contains("/tok/integrations/herdr"));
        assert!(sidebar(&none, &Place::Integrations).contains(r#"href="/tok/integrations""#), "the collection itself always shows");
        let here = sidebar(&none, &Place::Integration("herdr"));
        assert!(here.contains(r#"<a class="child on" aria-current="page" href="/tok/integrations/herdr">"#), "{here}");
        assert!(sidebar(&site(), &Place::General).contains("/tok/integrations/herdr"), "installed shows everywhere");
    }

    /// The same for portals: only those you are signed in to, plus the page you are on. The list page
    /// is where a portal waiting for a sign-in is found and signed in to.
    #[test]
    fn the_sidebar_lists_only_signed_in_portals_and_the_one_you_are_on() {
        let out = Site { portals: vec![("github".into(), "GitHub".into(), false)], ..site() };
        assert!(!sidebar(&out, &Place::General).contains("/tok/portals/github"));
        assert!(sidebar(&out, &Place::General).contains(r#"href="/tok/portals""#), "the collection itself always shows");
        let here = sidebar(&out, &Place::Portal("github".into()));
        assert!(here.contains(r#"<a class="child on" aria-current="page" href="/tok/portals/github">"#), "{here}");
        assert!(sidebar(&site(), &Place::General).contains("/tok/portals/github"), "signed in shows everywhere");
    }

    #[test]
    fn a_waiting_release_is_named_in_the_sidebar() {
        let html = sidebar(&Site { update: Some("3.1.0".into()), ..site() }, &Place::General);
        assert!(html.contains("Updates <span class=\"badge\">3.1.0</span>"), "{html}");
    }

    const DECLARED: &[Setting] = &[
        Setting { key: "loud", label: "Loud", kind: Kind::Flag { default_on: true }, help: "Makes noise.", group: "Noise", live: true },
        Setting { key: "root", label: "Root", kind: Kind::Text { placeholder: "~/src" }, help: "", group: "Paths", live: true },
        Setting { key: "level", label: "Level", kind: Kind::Choice { options: &[("error", "Errors"), ("info", "All")] }, help: "", group: "Paths", live: false },
        Setting { key: "parts", label: "Parts", kind: Kind::Parts { options: &["API", "Pages"], whole: "All", some: "Some" }, help: "", group: "Parts", live: false },
    ];

    fn values(s: &Setting) -> String {
        match s.key {
            "loud" => "off".into(),
            "root" => "\"><b>".into(),
            "level" => "info".into(),
            "parts" => "Pages".into(),
            _ => String::new(),
        }
    }

    /// Each section is its own form, posting its index to the page it is on, with its own Save.
    #[test]
    fn each_section_is_its_own_form_back_to_its_page() {
        let html = sections("tok", &Place::General, DECLARED, &values, None);
        for i in 0..3 {
            assert!(html.contains(&format!(r#"<section class="block" id="s{i}">"#)), "{html}");
            assert!(html.contains(&format!(r#"<input type="hidden" name="section" value="{i}">"#)));
        }
        assert_eq!(html.matches(r#"method="post" action="/tok/general""#).count(), 3);
        assert_eq!(html.matches(">Save</button>").count(), 3);
        assert!(html.contains("<h2 class=\"section\">Noise</h2>") && html.contains("<h2 class=\"section\">Parts</h2>"));
    }

    /// Every kind draws as what it is, showing what the file holds, and user text is escaped.
    #[test]
    fn each_kind_draws_its_control_with_the_current_value() {
        let html = sections("tok", &Place::General, DECLARED, &values, None);
        assert!(html.contains(r#"<input type="checkbox" name="loud" value="on"> Loud"#), "off is unticked: {html}");
        assert!(html.contains("Makes noise."), "help sits with its control");
        assert!(html.contains(r#"name="root" value="&quot;&gt;&lt;b&gt;" placeholder="~/src""#), "{html}");
        assert!(html.contains(r#"<input type="radio" name="level" value="info" checked> All"#));
        assert!(html.contains(r#"<input type="radio" name="level" value="error"> Errors"#));
        assert!(html.contains(r#"<input type="checkbox" name="parts" value="Pages" checked> Pages"#));
        assert!(html.contains(r#"<input type="checkbox" name="parts" value="API"> API"#));
    }

    #[test]
    fn a_saves_line_shows_under_the_section_it_belongs_to_only() {
        let flash = (1, "Saved.".to_string());
        let html = sections("tok", &Place::General, DECLARED, &values, Some(&flash));
        let s1 = html.find("id=\"s1\"").unwrap();
        let s2 = html.find("id=\"s2\"").unwrap();
        let line = html.find(r#"<span class="save-line" aria-live="polite">Saved.</span>"#).expect("the line is shown");
        assert!(s1 < line && line < s2, "under section 1: {html}");
        assert_eq!(html.matches("Saved.").count(), 1);
    }

    /// The title is where you are, as a trail from Settings: every step a link, except the page itself.
    #[test]
    fn the_title_is_a_breadcrumb_trail_from_settings() {
        let html = layout(&site(), &Place::Integration("herdr"), "Herdr dispatcher", None, "");
        assert!(html.contains(
            r#"<h1 class="crumbs"><a href="/tok/general">Settings</a><span class="sep" aria-hidden="true">›</span><a href="/tok/integrations">Integrations</a><span class="sep" aria-hidden="true">›</span><span>Herdr dispatcher</span></h1>"#
        ), "{html}");
        assert!(html.contains("<title>Settings › Integrations › Herdr dispatcher — GitHoot</title>"), "{html}");
        let html = layout(&site(), &Place::General, "General", None, "");
        assert!(html.contains(r#"<h1 class="crumbs"><a href="/tok/general">Settings</a><span class="sep" aria-hidden="true">›</span><span>General</span></h1>"#), "{html}");
        let html = layout(&site(), &Place::Portal("github".into()), "<GitHub>", None, "");
        assert!(html.contains(r#"<a href="/tok/portals">Portals</a><span class="sep" aria-hidden="true">›</span><span>&lt;GitHub&gt;</span>"#), "escaped: {html}");
    }

    #[test]
    fn a_page_carries_the_sidebar_and_reloads_only_when_asked() {
        let html = layout(&site(), &Place::Updates, "Updates", None, "<p>body</p>");
        assert!(html.contains("<nav class=\"side\"") && html.contains("<p>body</p>"));
        assert!(!html.contains("http-equiv=\"refresh\""));
        let html = layout(&site(), &Place::Updates, "Updates", Some(1), "");
        assert!(html.contains(r#"<meta http-equiv="refresh" content="1;url=/tok/updates">"#), "{html}");
    }
}
