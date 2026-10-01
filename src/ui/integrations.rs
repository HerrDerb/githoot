//! Integrations: where pull requests go. A list, and a page per integration.
//!
//! The page is the same for every integration: a header card with its state, Install or Uninstall
//! and a Dry run; then its declared settings as sections, each with its own Save; then whatever the
//! integration adds below, such as the dispatcher's prompts.

use super::{layout, settings_button, Place, Site};
use crate::integration::Problem;
use crate::page::esc;

/// One integration as the list shows it.
pub struct IntegrationRow<'a> {
    pub id: &'a str,
    pub name: &'a str,
    pub summary: &'a str,
    pub status: String,
    /// `" status-bad"` when it is installed and cannot do its job, else empty. See [`status_tone`].
    pub tone: &'static str,
    /// The button the row carries.
    pub switch: Switch,
    /// The line the last Install or Uninstall pressed here left, shown once.
    pub flash: Option<String>,
    /// What is wrong with its settings, said on the card as on its page.
    pub problems: Vec<Problem>,
}

/// Which way an integration's button goes, or that it has none: where the build cannot run an
/// integration that is not installed, there is nothing to offer.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Switch {
    Install,
    Uninstall,
    None,
}

/// The button an integration gets: Uninstall whenever it is installed, even where it cannot run, so a
/// hand-edited `config.txt` can always be undone from the page; Install only where it can run.
pub fn integration_switch(installed: bool, unsupported: Option<&str>) -> Switch {
    match (installed, unsupported) {
        (true, _) => Switch::Uninstall,
        (false, None) => Switch::Install,
        (false, Some(_)) => Switch::None,
    }
}

/// "Installed", "Not installed", and the two states that must not read as either.
pub fn integration_status(installed: bool, unsupported: Option<&str>, missing: &[&str], problems: usize) -> String {
    match (unsupported, installed, missing.is_empty()) {
        (Some(_), _, _) => "Not available here".to_string(),
        (None, false, _) => "Not installed".to_string(),
        (None, true, true) if problems > 0 => "Installed, but check its settings".to_string(),
        (None, true, true) => "Installed".to_string(),
        // Installed but unable to do anything is not "installed" in any sense that matters to the reader.
        (None, true, false) => format!("Installed, but idle: missing {}", missing.join(", ")),
    }
}

/// Install is the one thing a visitor to a page for something not installed came to do, so it is the
/// filled button. Uninstall is outlined, like Settings: the way out must not be the loudest thing on
/// the card. `from` names the page to reload afterwards.
fn switch_form(token: &str, id: &str, switch: Switch, from: Option<&str>) -> String {
    let (action, button) = match switch {
        Switch::Install => ("install", "<button type=\"submit\">Install</button>"),
        Switch::Uninstall => ("uninstall", "<button class=\"ghost\" type=\"submit\">Uninstall</button>"),
        Switch::None => return String::new(),
    };
    let from = from.map(|f| format!("<input type=\"hidden\" name=\"from\" value=\"{}\">", esc(f))).unwrap_or_default();
    format!(
        "<form method=\"post\" action=\"/{}/integrations/{}\"><input type=\"hidden\" name=\"action\" value=\"{action}\">{from}{button}</form>",
        esc(token),
        esc(id)
    )
}

/// The Integrations list: every integration this build knows, installed or not.
pub fn integrations_page(site: &Site, rows: &[IntegrationRow]) -> String {
    let token = site.token;
    let mut h = String::from(
        "<p class=\"sub lead\">What GitHoot may do with the pull requests it finds, beyond showing them. \
         Each is off until you install it, and uninstalling one keeps its settings and files.</p>\n",
    );
    for row in rows {
        let path = Place::Integration(crate::integration::find(row.id).map(|i| i.info().id).unwrap_or("")).path();
        let flash = row.flash.as_deref().map(|m| format!("<p class=\"sub\"><strong>{}</strong></p>", esc(m))).unwrap_or_default();
        let switch = switch_form(token, row.id, row.switch, Some("list"));
        let alert = problems_alert(matches!(row.switch, Switch::Uninstall), &row.problems, "alert", &format!("/{}/{path}", esc(token)));
        h.push_str(&format!(
            "<div class=\"card\"><div class=\"row\"><strong><a href=\"/{}/{path}\">{}</a></strong> · \
             <span class=\"portal-status{}\">{}</span></div><p class=\"sub\">{}</p>{alert}{flash}<div class=\"actions\">{switch}{}</div></div>\n",
            esc(token),
            esc(row.name),
            row.tone,
            esc(&row.status),
            esc(row.summary),
            // Installed, the page is its settings. Not installed, it is where the prompts are read and
            // a dry run shows what it would do, before deciding: a preview, and labelled as one.
            if matches!(row.switch, Switch::Uninstall) {
                settings_button(token, &path, row.name)
            } else {
                format!("<a class=\"ghost\" href=\"/{}/{path}\" aria-label=\"{} details\">Details</a>", esc(token), esc(row.name))
            },
        ));
    }
    layout(site, &Place::Integrations, "Integrations", None, &h)
}

/// What is wrong with its settings, as an alert: a card of its own on its page, a box inside its card
/// on the list. `page` is where the boxes are, empty when that is this page; each line's "Fix it" goes
/// to the box at fault there. Empty when nothing is wrong.
fn problems_alert(installed: bool, problems: &[Problem], class: &str, page: &str) -> String {
    if problems.is_empty() {
        return String::new();
    }
    let items: String = problems
        .iter()
        .map(|p| {
            let fix = if p.key.is_empty() { String::new() } else { format!("<a class=\"fix\" href=\"{page}#field-{}\">Fix it</a>", esc(p.key)) };
            format!("<li><span>{}</span>{fix}</li>", esc(&p.text))
        })
        .collect();
    let lead = if installed { "Until these are fixed it cannot start an agent." } else { "Fix these before you install it, or it will start no agents." };
    format!(
        "<div class=\"{class}\" role=\"alert\">{WARNING}<div class=\"alert-body\"><strong class=\"alert-title\">Check its settings</strong>\
         <p class=\"alert-lead\">{lead}</p><ul>{items}</ul></div></div>\n"
    )
}

/// A warning triangle, drawn in the alert's own colour.
const WARNING: &str = "<svg class=\"alert-icon\" viewBox=\"0 0 24 24\" width=\"20\" height=\"20\" aria-hidden=\"true\" fill=\"none\" \
stroke=\"currentColor\" stroke-width=\"2\" stroke-linecap=\"round\" stroke-linejoin=\"round\">\
<path d=\"M10.3 3.9 1.8 18a2 2 0 0 0 1.7 3h17a2 2 0 0 0 1.7-3L13.7 3.9a2 2 0 0 0-3.4 0z\"/><path d=\"M12 9v4\"/><path d=\"M12 17h.01\"/></svg>";

/// Everything the header card of an integration's page shows, as plain data.
pub struct IntegrationView<'a> {
    pub id: &'static str,
    pub name: &'a str,
    pub summary: &'a str,
    pub installed: bool,
    /// Why this build cannot run it. No Install button then, and the reason instead.
    pub unsupported: Option<&'a str>,
    /// Tools it needs that will not run. Only ever asked while installed.
    pub missing: &'a [&'a str],
    /// What is wrong with its settings on this machine, such as a folder that is not there.
    pub problems: &'a [Problem],
    /// The outcome of the last button press on the header, shown once.
    pub flash: Option<&'a str>,
    /// What the last Dry run said, in the order a pass produced it. Empty until one is asked for.
    pub dry_run: &'a [String],
    /// Its declared settings, already drawn as sections.
    pub sections: String,
    /// What the integration adds below. Already HTML, escaped by the integration.
    pub body: String,
}

/// One integration's page: the header card, its settings, then whatever it adds.
pub fn integration_page(site: &Site, v: &IntegrationView) -> String {
    let body = format!("{}{}{}", header_card(site.token, v), v.sections, v.body);
    layout(site, &Place::Integration(v.id), v.name, None, &body)
}

/// Whether it is installed, what it is missing, the switch and the rehearsal.
///
/// Dry run is offered installed or not, and that is the point: the only safe way to learn what
/// installing it would do is to ask first.
fn header_card(token: &str, v: &IntegrationView) -> String {
    let status = integration_status(v.installed, v.unsupported, v.missing, v.problems.len());
    let tone = status_tone(v.installed, v.unsupported, v.missing, v.problems.len());
    let mut notes = String::new();
    if let Some(why) = v.unsupported {
        notes.push_str(&format!("<p class=\"sub\">{}</p>", esc(why)));
    } else if v.installed && !v.missing.is_empty() {
        notes.push_str(&format!(
            "<p class=\"sub\">Put <code>{}</code> on your PATH. Until then it does nothing.</p>",
            v.missing.iter().map(|m| esc(m)).collect::<Vec<_>>().join("</code>, <code>")
        ));
    }
    // A card of its own, ahead of everything, and shown before Install too: that is the moment a wrong
    // folder is cheapest to fix. Each line jumps to the box at fault, which is marked as well.
    let alert = if v.unsupported.is_none() { problems_alert(v.installed, v.problems, "card alert", "") } else { String::new() };
    let flash = v.flash.map(|m| format!("<p class=\"sub\"><strong>{}</strong></p>", esc(m))).unwrap_or_default();
    let switch = switch_form(token, v.id, integration_switch(v.installed, v.unsupported), None);
    let dry = if v.unsupported.is_some() {
        String::new()
    } else {
        format!(
            "<form method=\"post\" action=\"/{}/integrations/{}\"><input type=\"hidden\" name=\"action\" value=\"dryrun\">\
             <button class=\"ghost\" type=\"submit\">Dry run</button></form>",
            esc(token),
            esc(v.id)
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
        esc(v.summary),
    )
}

/// The status in red when it is installed and cannot do its job, so it does not read as fine.
pub fn status_tone(installed: bool, unsupported: Option<&str>, missing: &[&str], problems: usize) -> &'static str {
    if installed && unsupported.is_none() && (!missing.is_empty() || problems > 0) { " status-bad" } else { "" }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::tests::site;

    fn view(installed: bool, missing: &'static [&'static str]) -> IntegrationView<'static> {
        IntegrationView {
            id: "herdr",
            name: "Herdr dispatcher",
            summary: "Starts agents.",
            installed,
            unsupported: None,
            missing,
            problems: &[],
            flash: None,
            dry_run: &[],
            sections: "<section class=\"block\" id=\"s0\">bars</section>".to_string(),
            body: "<p>its own part</p>".to_string(),
        }
    }

    fn page(v: &IntegrationView) -> String {
        integration_page(&site(), v)
    }

    #[test]
    fn not_installed_offers_a_full_size_install_and_a_rehearsal() {
        let html = page(&view(false, &[]));
        assert!(html.contains("portal-status\">Not installed<"), "{html}");
        assert!(html.contains(r#"value="install"><button type="submit">Install</button>"#));
        assert!(!html.contains(r#"value="uninstall""#));
        assert!(html.contains(r#"value="dryrun"><button class="ghost" type="submit">Dry run</button>"#), "the rehearsal stays secondary");
        assert!(html.contains(r#"action="/tok/integrations/herdr""#));
    }

    #[test]
    fn installed_offers_an_outlined_uninstall() {
        let html = page(&view(true, &[]));
        assert!(html.contains("portal-status\">Installed<"), "{html}");
        assert!(html.contains(r#"value="uninstall"><button class="ghost" type="submit">Uninstall</button>"#));
        assert!(!html.contains(r#"value="install""#));
    }

    /// Installed with a tool gone must not read as healthy: it is switched on and doing nothing.
    #[test]
    fn installed_but_missing_a_tool_says_it_is_idle() {
        let html = page(&view(true, &["herdr", "gh"]));
        assert!(html.contains("Installed, but idle: missing herdr, gh"), "{html}");
        assert!(html.contains("<code>herdr</code>, <code>gh</code>"));
    }

    /// A clone root pointing nowhere is installed and doing nothing, the same as a missing tool, and
    /// was found only by reading the log. The page says it, and says it before Install too.
    #[test]
    fn a_setting_that_points_nowhere_is_on_the_page_installed_or_not() {
        let problems = [Problem {
            key: "cloneRoot",
            text: "\"Clones live in\" is empty, so it looks in C:\\x, and that folder does not exist".to_string(),
        }];
        let v = IntegrationView { problems: &problems, ..view(true, &[]) };
        let html = page(&v);
        assert!(html.contains("<span class=\"portal-status status-bad\">Installed, but check its settings</span>"), "{html}");
        // Not a line among the others: a card of its own, announced, before the header's buttons.
        let alert = html.find("<div class=\"card alert\" role=\"alert\">").expect("an alert card");
        assert!(alert < html.find("value=\"uninstall\"").unwrap(), "{html}");
        assert!(html.contains("<li><span>&quot;Clones live in&quot; is empty, so it looks in C:\\x, and that folder does not exist</span>\
                               <a class=\"fix\" href=\"#field-cloneRoot\">Fix it</a></li>"), "{html}");
        // A title to scan and the consequence under it, with a mark that says "warning" without the colour.
        assert!(html.contains("<svg class=\"alert-icon\"") && html.contains("aria-hidden=\"true\""), "{html}");
        assert!(html.contains("<strong class=\"alert-title\">Check its settings</strong>\
                               <p class=\"alert-lead\">Until these are fixed it cannot start an agent.</p>"), "{html}");
        let html = page(&IntegrationView { problems: &problems, ..view(false, &[]) });
        assert!(html.contains("portal-status\">Not installed<") && html.contains("card alert"), "{html}");
        assert!(!page(&view(true, &[])).contains("card alert"), "nothing wrong, nothing shown");
        // A missing tool still comes first: nothing runs without it, whatever the folders say.
        assert_eq!(integration_status(true, None, &["gh"], 1), "Installed, but idle: missing gh");
    }

    #[test]
    fn unsupported_offers_no_install_and_says_why() {
        let v = IntegrationView { unsupported: Some("Needs Linux or Windows."), ..view(false, &[]) };
        let html = page(&v);
        assert!(html.contains("Not available here") && html.contains("Needs Linux or Windows."));
        assert!(!html.contains(r#"value="install""#) && !html.contains(">Dry run<"));
    }

    /// Header first, its settings sections next, its own part last; and the flash once given.
    #[test]
    fn the_page_is_header_then_sections_then_its_own_part() {
        let v = IntegrationView { flash: Some("Installed."), ..view(true, &[]) };
        let html = page(&v);
        let header = html.find("portal-status").unwrap();
        let bars = html.find(">bars<").unwrap();
        let own = html.find("<p>its own part</p>").unwrap();
        assert!(header < bars && bars < own, "{html}");
        assert!(html.contains("<strong>Installed.</strong>"));
    }

    fn row(status: &str, switch: Switch) -> IntegrationRow<'static> {
        IntegrationRow { id: "herdr", name: "Herdr dispatcher", summary: "Starts agents.", status: status.into(), tone: "", switch, flash: None, problems: Vec::new() }
    }

    /// The list is where you land, so a setting that stops it from working is said on its card too,
    /// with the same words as its page, and "Fix it" goes to that box on that page.
    #[test]
    fn the_list_card_says_a_setting_is_wrong_and_links_to_the_box() {
        let problems = vec![Problem { key: "cloneRoot", text: "that folder does not exist".into() }];
        let html = integrations_page(&site(), &[IntegrationRow { problems: problems.clone(), ..row("Installed, but check its settings", Switch::Uninstall) }]);
        let card = &html[html.find("<div class=\"card\">").unwrap()..];
        let alert = card.find("<div class=\"alert\" role=\"alert\">").expect("inside the card");
        assert!(alert < card.find("value=\"uninstall\"").unwrap(), "ahead of its buttons: {html}");
        assert!(card.contains("<p class=\"alert-lead\">Until these are fixed it cannot start an agent.</p>"), "{html}");
        assert!(card.contains("<li><span>that folder does not exist</span><a class=\"fix\" href=\"/tok/integrations/herdr#field-cloneRoot\">Fix it</a></li>"), "{html}");
        let html = integrations_page(&site(), &[IntegrationRow { problems, ..row("Not installed", Switch::Install) }]);
        assert!(html.contains("<p class=\"alert-lead\">Fix these before you install it, or it will start no agents.</p>"), "{html}");
        assert!(!integrations_page(&site(), &[row("Installed", Switch::Uninstall)]).contains("role=\"alert\""));
    }

    /// Not installed means a button you cannot miss, right in the list. It says it came from the
    /// list, so the list is what reloads after.
    #[test]
    fn the_list_has_install_or_uninstall_and_a_settings_button_on_every_row() {
        let html = integrations_page(&site(), &[row("Not installed", Switch::Install)]);
        assert!(html.contains(r#"<form method="post" action="/tok/integrations/herdr"><input type="hidden" name="action" value="install"><input type="hidden" name="from" value="list"><button type="submit">Install</button></form>"#), "{html}");
        // Installed, the page is its settings. Not installed, it is where you read the prompts and
        // dry-run it before deciding, which is a preview, and the button says so.
        let html = integrations_page(&site(), &[row("x", Switch::Uninstall)]);
        let at = html.find(r#"<a class="ghost" href="/tok/integrations/herdr" aria-label="Herdr dispatcher settings">"#).expect("settings button");
        let button = &html[at..at + html[at..].find("</a>").unwrap()];
        assert!(button.contains("<svg") && button.contains("Settings"), "{button}");
        for switch in [Switch::Install, Switch::None] {
            let html = integrations_page(&site(), &[row("x", switch)]);
            assert!(html.contains(r#"<a class="ghost" href="/tok/integrations/herdr" aria-label="Herdr dispatcher details">Details</a>"#), "{html}");
            assert!(!html.contains("aria-label=\"Herdr dispatcher settings\""));
        }
        let r = IntegrationRow { flash: Some("Uninstalled.".into()), ..row("Installed", Switch::Uninstall) };
        let html = integrations_page(&site(), &[r]);
        assert!(html.contains(r#"value="uninstall"><input type="hidden" name="from" value="list"><button class="ghost" type="submit">Uninstall</button>"#));
        assert!(html.contains("<strong>Uninstalled.</strong>"));
    }
}
