//! Integrations: where pull requests go. A list, and a page per integration.
//!
//! The page is the same for every integration: a header card with its state, Install or Uninstall
//! and a Dry run; then its declared settings as sections, each with its own Save; then whatever the
//! integration adds below, such as the dispatcher's prompts.

use super::{layout, settings_button, Place, Site};
use crate::page::esc;

/// One integration as the list shows it.
pub struct IntegrationRow<'a> {
    pub id: &'a str,
    pub name: &'a str,
    pub summary: &'a str,
    pub status: String,
    /// The button the row carries.
    pub switch: Switch,
    /// The line the last Install or Uninstall pressed here left, shown once.
    pub flash: Option<String>,
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
pub fn integration_status(installed: bool, unsupported: Option<&str>, missing: &[&str]) -> String {
    match (unsupported, installed, missing.is_empty()) {
        (Some(_), _, _) => "Not available here".to_string(),
        (None, false, _) => "Not installed".to_string(),
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
        h.push_str(&format!(
            "<div class=\"card\"><div class=\"row\"><strong><a href=\"/{}/{path}\">{}</a></strong> · \
             <span class=\"portal-status\">{}</span></div><p class=\"sub\">{}</p>{flash}<div class=\"actions\">{switch}{}</div></div>\n",
            esc(token),
            esc(row.name),
            esc(&row.status),
            esc(row.summary),
            settings_button(token, &path, row.name),
        ));
    }
    layout(site, &Place::Integrations, "Integrations", None, &h)
}

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
    let status = integration_status(v.installed, v.unsupported, v.missing);
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
        "<div class=\"card\"><div class=\"row\"><span class=\"portal-status\">{}</span></div><p class=\"sub\">{}</p>{notes}{flash}\
         <div class=\"actions\">{switch}{dry}</div>{said}</div>\n",
        esc(&status),
        esc(v.summary),
    )
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
        IntegrationRow { id: "herdr", name: "Herdr dispatcher", summary: "Starts agents.", status: status.into(), switch, flash: None }
    }

    /// Not installed means a button you cannot miss, right in the list. It says it came from the
    /// list, so the list is what reloads after.
    #[test]
    fn the_list_has_install_or_uninstall_and_a_settings_button_on_every_row() {
        let html = integrations_page(&site(), &[row("Not installed", Switch::Install)]);
        assert!(html.contains(r#"<form method="post" action="/tok/integrations/herdr"><input type="hidden" name="action" value="install"><input type="hidden" name="from" value="list"><button type="submit">Install</button></form>"#), "{html}");
        for switch in [Switch::Install, Switch::Uninstall, Switch::None] {
            let html = integrations_page(&site(), &[row("x", switch)]);
            let at = html.find(r#"<a class="ghost" href="/tok/integrations/herdr" aria-label="Herdr dispatcher settings">"#).expect("settings button");
            let button = &html[at..at + html[at..].find("</a>").unwrap()];
            assert!(button.contains("<svg") && button.contains("Settings"), "{button}");
        }
        let r = IntegrationRow { flash: Some("Uninstalled.".into()), ..row("Installed", Switch::Uninstall) };
        let html = integrations_page(&site(), &[r]);
        assert!(html.contains(r#"value="uninstall"><input type="hidden" name="from" value="list"><button class="ghost" type="submit">Uninstall</button>"#));
        assert!(html.contains("<strong>Uninstalled.</strong>"));
    }
}
