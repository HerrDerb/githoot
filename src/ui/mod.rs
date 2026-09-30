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
document.addEventListener('change',function(ev){parts();var f=ev.target.form;if(!f||!f.hasAttribute('data-autosave'))return;\
var line=f.querySelector('.save-line');line.textContent='Saving…';clearTimeout(f.fade);\
var body=new URLSearchParams(new FormData(f));body.append('reply','json');\
fetch(f.getAttribute('action'),{method:'POST',body:body}).then(function(r){return r.json();}).then(function(j){\
line.textContent=j.line;if(j.fade){f.fade=setTimeout(function(){line.textContent='';},2000);}\
}).catch(function(){line.textContent='Not saved: GitHoot did not answer. Is it still running?';});});\
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
                rule_line(&members, value)
            } else {
                members.iter().map(|s| field(s, &value(s))).collect()
            };
            let line = match flash {
                Some((at, line)) if *at == i => esc(line),
                _ => String::new(),
            };
            // No text box in it: the section saves as it changes, and the script hides its button.
            let auto = if members.iter().any(|s| matches!(s.kind, Kind::Text { .. })) { "" } else { " data-autosave" };
            format!(
                "<section class=\"block\" id=\"s{i}\"><h2 class=\"section\">{}</h2><div class=\"card\">\
                 <form method=\"post\" action=\"/{}/{}\"{auto}><input type=\"hidden\" name=\"section\" value=\"{i}\">{fields}\
                 <div class=\"actions\"><button class=\"small save\" type=\"submit\">Save</button>\
                 <span class=\"save-line\" aria-live=\"polite\">{line}</span></div></form></div></section>\n",
                esc(title),
                esc(token),
                place.path(),
            )
        })
        .collect()
}

/// The green bar's rules as the path a pull request of yours walks to green, top to bottom: the two
/// steps that define the bar (drawn as dots, not switches), then one switch per rule with what happens
/// to a pull request that fails it, ending at the green bar itself.
///
/// The order is who decides: people, then git, then CI, then bots. It is not a ladder: every switch
/// is independent, because nothing makes one rule depend on another. A rule switched off fades and
/// its stretch of the line turns dashed, so a skipped gate is visible at a glance.
fn rule_line(steps: &[&'static Setting], value: &dyn Fn(&Setting) -> String) -> String {
    let fixed = |text: &str, otherwise: &str| {
        let chip = if otherwise.is_empty() { String::new() } else { format!("<span class=\"gate-else else-amber\">{otherwise}</span>") };
        format!("<li class=\"gate fixed\"><span class=\"gate-node\" aria-hidden=\"true\"></span><span class=\"gate-text\">{text}</span>{chip}</li>")
    };
    let mut h = String::from(
        "<p class=\"sub gate-lead\">What one of your pull requests has to pass to light the green bar. \
         A step you switch off is ignored on both bars.</p><ol class=\"gates\">",
    );
    h.push_str(&fixed("Somebody approved it", ""));
    h.push_str(&fixed("Nobody's objection stands", "else amber"));
    for s in steps {
        let Kind::Step { otherwise, .. } = s.kind else { continue };
        let on = crate::setting::flag(s.kind, &value(s));
        let tone = if otherwise.contains("amber") { "else-amber" } else { "else-wait" };
        // A step only some portal kinds can judge names them, so a switch that does nothing for the
        // portal you use says so rather than looking broken.
        let badge = crate::config::step_portals(s.key)
            .map(|kinds| {
                let names: Vec<&str> = kinds.iter().map(|k| k.display_name()).collect();
                format!("<span class=\"gate-portal\">{}</span>", esc(&names.join(" · ")))
            })
            .unwrap_or_default();
        h.push_str(&format!(
            "<li class=\"gate\"><label class=\"gate-row\"><input type=\"checkbox\" name=\"{}\" value=\"on\"{}>\
             <span class=\"gate-title\"><span class=\"gate-text\">{}</span>{badge}</span></label>\
             <span class=\"gate-else {tone}\">{}</span><p class=\"sub gate-help\">{}</p></li>",
            esc(s.key),
            if on { " checked" } else { "" },
            esc(s.label),
            esc(otherwise),
            esc(s.help),
        ));
    }
    h.push_str("<li class=\"gate end\"><span class=\"gate-node\" aria-hidden=\"true\"></span><span class=\"gate-text\">Green bar</span></li></ol>");
    h
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
    let body = format!(
        "<p class=\"sub lead\">What the tray shows and does. Changes save as you make them.</p>\n{}",
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
        assert!(html.contains(r#"<form method="post" action="/tok/general" data-autosave>"#), "{html}");
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
        let at = html.find(r#"<ol class="gates">"#).expect("the line");
        let line = &html[at..at + html[at..].find("</ol>").unwrap()];
        let order: Vec<usize> = ["Somebody approved it", "Nobody's objection stands", "No merge conflict", "Checks did not fail", "Checks have finished", "Automatic reviewer&#39;s comments resolved", "Green bar"]
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
        assert_eq!(html.matches(r#"<form method="post" action="/tok/general""#).count(), 3);
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
