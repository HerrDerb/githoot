//! The Agents page: whether GitHoot starts agents, where they open, and what they are told.
//!
//! A header card with its state, the switch and a Dry run; then its settings as sections, each with its
//! own Save, the chosen client's card inside the Client section; then the folders and prompts below.

use super::{layout, Place, Site};
use crate::agents::Problem;
use crate::page::esc;

/// Which way the switch goes, or that there is none: where the build cannot run agents and they are
/// off, there is nothing to offer.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Switch {
    On,
    Off,
    None,
}

/// Switch off whenever it is on, even where it cannot run, so a hand-edited `config.txt` can always be
/// undone from the page; switch on only where it can run.
pub fn agents_switch(on: bool, unsupported: Option<&str>) -> Switch {
    match (on, unsupported) {
        (true, _) => Switch::Off,
        (false, None) => Switch::On,
        (false, Some(_)) => Switch::None,
    }
}

/// "On", "Off", and the two states that must not read as either.
pub fn agents_status(on: bool, unsupported: Option<&str>, missing: &[&str], problems: usize) -> String {
    match (unsupported, on, missing.is_empty()) {
        (Some(_), _, _) => "Not available here".to_string(),
        (None, false, _) => "Off".to_string(),
        (None, true, true) if problems > 0 => "On, but check its settings".to_string(),
        (None, true, true) => "On".to_string(),
        // On but unable to do anything is not "on" in any sense that matters to the reader.
        (None, true, false) => format!("On, but idle: missing {}", missing.join(", ")),
    }
}

/// The status in red when it is on and cannot do its job, so it does not read as fine.
pub fn status_tone(on: bool, unsupported: Option<&str>, missing: &[&str], problems: usize) -> &'static str {
    if on && unsupported.is_none() && (!missing.is_empty() || problems > 0) { " status-bad" } else { "" }
}

/// Switch on is the one thing a visitor came to do while it is off, so it is the filled button. Switch
/// off is outlined, like Settings: the way out must not be the loudest thing on the card.
fn switch_form(token: &str, switch: Switch) -> String {
    let (action, button) = match switch {
        Switch::On => ("on", "<button type=\"submit\">Switch on</button>"),
        Switch::Off => ("off", "<button class=\"ghost\" type=\"submit\">Switch off</button>"),
        Switch::None => return String::new(),
    };
    format!(
        "<form method=\"post\" action=\"/{}/{}\"><input type=\"hidden\" name=\"action\" value=\"{action}\">{button}</form>",
        esc(token),
        Place::Agents.path()
    )
}

/// What is wrong with its settings, as an alert card of its own. Each line's "Fix it" goes to the box at
/// fault. Empty when nothing is wrong.
fn problems_alert(on: bool, problems: &[Problem]) -> String {
    if problems.is_empty() {
        return String::new();
    }
    let items: String = problems
        .iter()
        .map(|p| {
            let fix = if p.key.is_empty() { String::new() } else { format!("<a class=\"fix\" href=\"#field-{}\">Fix it</a>", esc(p.key)) };
            format!("<li><span>{}</span>{fix}</li>", esc(&p.text))
        })
        .collect();
    let lead = if on { "Until these are fixed it cannot start an agent." } else { "Fix these before you switch it on, or it will start no agents." };
    format!(
        "<div class=\"card alert\" role=\"alert\">{WARNING}<div class=\"alert-body\"><strong class=\"alert-title\">Check its settings</strong>\
         <p class=\"alert-lead\">{lead}</p><ul>{items}</ul></div></div>\n"
    )
}

/// A warning triangle, drawn in the alert's own colour.
const WARNING: &str = "<svg class=\"alert-icon\" viewBox=\"0 0 24 24\" width=\"20\" height=\"20\" aria-hidden=\"true\" fill=\"none\" \
stroke=\"currentColor\" stroke-width=\"2\" stroke-linecap=\"round\" stroke-linejoin=\"round\">\
<path d=\"M10.3 3.9 1.8 18a2 2 0 0 0 1.7 3h17a2 2 0 0 0 1.7-3L13.7 3.9a2 2 0 0 0-3.4 0z\"/><path d=\"M12 9v4\"/><path d=\"M12 17h.01\"/></svg>";

/// Everything the Agents page shows, as plain data.
pub struct AgentsView<'a> {
    pub on: bool,
    /// Why this build cannot run it. No switch then, and the reason instead.
    pub unsupported: Option<&'a str>,
    /// Tools the chosen client needs that will not run. Only ever asked while it is on.
    pub missing: &'a [&'a str],
    /// What is wrong with its settings on this machine, such as a folder that is not there.
    pub problems: &'a [Problem],
    /// The outcome of the last button press on the header, shown once.
    pub flash: Option<&'a str>,
    /// What the last Dry run said, in the order a pass produced it. Empty until one is asked for.
    pub dry_run: &'a [String],
    /// Its settings, already drawn as sections, the first one being the Client section.
    pub sections: String,
    /// The chosen client's card, drawn inside the Client section, under its radio.
    pub client_card: String,
    /// What goes below the sections: where it looks, and the prompts. Already HTML, escaped.
    pub body: String,
}

/// The Agents page: the header card, its settings with the client card in the Client section, then the
/// rest.
pub fn agents_page(site: &Site, v: &AgentsView) -> String {
    let sections = in_first_section(&v.sections, &v.client_card);
    let body = format!("{}{sections}{}", header_card(site.token, v), v.body);
    layout(site, &Place::Agents, crate::agents::NAME, None, &body)
}

/// `card` placed at the end of the first section, inside it, so the chosen client's card sits under the
/// radio that chose it. The sections are numbered by group and posted by number, so the card is put in
/// after drawing rather than by splitting the settings, which would number two sections `s0`.
fn in_first_section(sections: &str, card: &str) -> String {
    let second = sections.find("<section class=\"block\" id=\"s1\"").unwrap_or(sections.len());
    let Some(end) = sections[..second].rfind("</section>") else { return format!("{sections}{card}") };
    format!("{}{card}{}", &sections[..end], &sections[end..])
}

/// Whether it is on, what it is missing, the switch and the rehearsal.
///
/// Dry run is offered on or off, and that is the point: the only safe way to learn what switching it on
/// would do is to ask first.
fn header_card(token: &str, v: &AgentsView) -> String {
    let status = agents_status(v.on, v.unsupported, v.missing, v.problems.len());
    let tone = status_tone(v.on, v.unsupported, v.missing, v.problems.len());
    let mut notes = String::new();
    if let Some(why) = v.unsupported {
        notes.push_str(&format!("<p class=\"sub\">{}</p>", esc(why)));
    } else if v.on && !v.missing.is_empty() {
        notes.push_str(&format!(
            "<p class=\"sub\">Put <code>{}</code> on your PATH. Until then it does nothing.</p>",
            v.missing.iter().map(|m| esc(m)).collect::<Vec<_>>().join("</code>, <code>")
        ));
    }
    // A card of its own, ahead of everything, and shown while off too: that is the moment a wrong folder
    // is cheapest to fix. Each line jumps to the box at fault, which is marked as well.
    let alert = if v.unsupported.is_none() { problems_alert(v.on, v.problems) } else { String::new() };
    let flash = v.flash.map(|m| format!("<p class=\"sub\"><strong>{}</strong></p>", esc(m))).unwrap_or_default();
    let switch = switch_form(token, agents_switch(v.on, v.unsupported));
    let dry = if v.unsupported.is_some() {
        String::new()
    } else {
        format!(
            "<form method=\"post\" action=\"/{}/{}\"><input type=\"hidden\" name=\"action\" value=\"dryrun\">\
             <button class=\"ghost\" type=\"submit\">Dry run</button></form>",
            esc(token),
            Place::Agents.path()
        )
    };
    let said = if v.dry_run.is_empty() {
        String::new()
    } else {
        format!("<pre class=\"dry\">{}</pre>", v.dry_run.iter().map(|l| esc(l)).collect::<Vec<_>>().join("\n"))
    };
    format!(
        "{alert}<div class=\"card\"><div class=\"row\"><span class=\"portal-status{tone}\">{}</span></div><p class=\"sub\">{}</p>{notes}{flash}\
         <div class=\"actions\">{switch}{dry}</div>{said}</div>\n",
        esc(&status),
        esc(crate::agents::SUMMARY),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::tests::site;

    fn view(on: bool, missing: &'static [&'static str]) -> AgentsView<'static> {
        AgentsView {
            on,
            unsupported: None,
            missing,
            problems: &[],
            flash: None,
            dry_run: &[],
            sections: "<section class=\"block\" id=\"s0\"><h2>Client</h2>radio</section>\
                       <section class=\"block\" id=\"s1\"><h2>Agent</h2>claude</section>"
                .to_string(),
            client_card: "<div class=\"card\">the client card</div>".to_string(),
            body: "<p>its own part</p>".to_string(),
        }
    }

    fn page(v: &AgentsView) -> String {
        agents_page(&site(), v)
    }

    #[test]
    fn off_offers_a_full_size_switch_on_and_a_rehearsal() {
        let html = page(&view(false, &[]));
        assert!(html.contains("portal-status\">Off<"), "{html}");
        assert!(html.contains(r#"<form method="post" action="/tok/agents"><input type="hidden" name="action" value="on"><button type="submit">Switch on</button></form>"#), "{html}");
        assert!(!html.contains(r#"value="off""#));
        assert!(html.contains(r#"value="dryrun"><button class="ghost" type="submit">Dry run</button>"#), "the rehearsal stays secondary");
    }

    #[test]
    fn on_offers_an_outlined_switch_off() {
        let html = page(&view(true, &[]));
        assert!(html.contains("portal-status\">On<"), "{html}");
        assert!(html.contains(r#"value="off"><button class="ghost" type="submit">Switch off</button>"#));
        assert!(!html.contains(r#"value="on""#));
    }

    /// On with a tool gone must not read as healthy: it is switched on and doing nothing.
    #[test]
    fn on_but_missing_a_tool_says_it_is_idle() {
        let html = page(&view(true, &["herdr", "git"]));
        assert!(html.contains("On, but idle: missing herdr, git"), "{html}");
        assert!(html.contains("<code>herdr</code>, <code>git</code>"));
    }

    /// A clone root pointing nowhere is on and doing nothing, the same as a missing tool. The page says
    /// it, and says it while off too.
    #[test]
    fn a_setting_that_points_nowhere_is_on_the_page_on_or_off() {
        let problems = [Problem { key: "agentsCloneRoot", text: "that folder does not exist".to_string() }];
        let html = page(&AgentsView { problems: &problems, ..view(true, &[]) });
        assert!(html.contains("<span class=\"portal-status status-bad\">On, but check its settings</span>"), "{html}");
        let alert = html.find("<div class=\"card alert\" role=\"alert\">").expect("an alert card");
        assert!(alert < html.find("value=\"off\"").unwrap(), "{html}");
        assert!(html.contains("<li><span>that folder does not exist</span><a class=\"fix\" href=\"#field-agentsCloneRoot\">Fix it</a></li>"), "{html}");
        assert!(html.contains("<p class=\"alert-lead\">Until these are fixed it cannot start an agent.</p>"), "{html}");
        let html = page(&AgentsView { problems: &problems, ..view(false, &[]) });
        assert!(html.contains("portal-status\">Off<") && html.contains("Fix these before you switch it on"), "{html}");
        assert!(!page(&view(true, &[])).contains("card alert"), "nothing wrong, nothing shown");
        assert_eq!(agents_status(true, None, &["git"], 1), "On, but idle: missing git", "a missing tool still comes first");
    }

    #[test]
    fn unsupported_offers_no_switch_and_says_why() {
        let html = page(&AgentsView { unsupported: Some("Agents need Linux or Windows."), ..view(false, &[]) });
        assert!(html.contains("Not available here") && html.contains("Agents need Linux or Windows."));
        assert!(!html.contains(r#"value="on""#) && !html.contains(">Dry run<"));
    }

    /// The chosen client's card sits in the Client section, under its radio, and before the Agent
    /// section; the rest of the page follows the sections.
    #[test]
    fn the_client_card_sits_under_the_client_radio() {
        let html = page(&AgentsView { flash: Some("Switched on."), ..view(true, &[]) });
        let header = html.find("portal-status").unwrap();
        let radio = html.find(">radio<").unwrap();
        let card = html.find("the client card").unwrap();
        let client_end = html[radio..].find("</section>").unwrap() + radio;
        let agent = html.find(">claude<").unwrap();
        let own = html.find("<p>its own part</p>").unwrap();
        assert!(header < radio && radio < card && card < client_end && client_end < agent && agent < own, "{html}");
        assert!(html.contains("<strong>Switched on.</strong>"));
        assert_eq!(in_first_section("<p>no sections</p>", "C"), "<p>no sections</p>C");
    }

    /// Its place in the trail and the sidebar: a page of its own, not inside a collection.
    #[test]
    fn it_is_a_page_of_its_own() {
        let html = page(&view(false, &[]));
        assert!(html.contains("<title>Settings › Agents — GitHoot</title>"), "{html}");
        assert!(html.contains(r#"<a class="on" aria-current="page" href="/tok/agents">Agents</a>"#), "{html}");
    }
}
