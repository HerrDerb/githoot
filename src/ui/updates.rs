//! Updates: this version, the last check, Check now, and the automatic check.
//!
//! **Check now works whether or not the automatic check is on.** The setting decides whether GitHoot
//! asks by itself at startup and daily; asking by hand is always yours to do. The check runs on the
//! poll thread, like the automatic one, and while it runs the page shows only that and reloads
//! itself, by the same rule a running sign-in follows.

use super::{layout, sections, Place, Site};
use crate::page::esc;
use crate::setting::Setting;

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
    pub value: &'a dyn Fn(&Setting) -> String,
    pub flash: Option<&'a (usize, String)>,
}

pub fn updates_page(site: &Site, v: &UpdatesView) -> String {
    let token = site.token;
    let running = v.waiting.is_some_and(|secs| secs < WAIT_SECS);
    let mut h = String::new();
    h.push_str("<section class=\"block\" id=\"version\"><h2 class=\"section\">This version</h2><div class=\"card\">");
    h.push_str(&format!("<div class=\"row\"><strong>GitHoot {}</strong></div>", esc(v.installed)));
    if running {
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
            "Checked {}: <strong>{} is available.</strong> Install it from the tray menu.",
            ago_text(*ago),
            esc(newer)
        ),
        Some((ago, Err(why))) => format!("Checked {}: the check failed ({}).", ago_text(*ago), esc(why)),
    };
    h.push_str(&format!("<p class=\"sub\">{last}</p>"));
    h.push_str(&format!(
        "<div class=\"actions\"><form method=\"post\" action=\"/{}/updates\"><input type=\"hidden\" name=\"action\" value=\"check\">\
         <button type=\"submit\">Check now</button></form></div></div></section>\n",
        esc(token)
    ));
    h.push_str(&sections(token, &Place::Updates, crate::config::UPDATES, v.value, v.flash));
    layout(site, &Place::Updates, "Updates", None, &h)
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

    fn values(_: &Setting) -> String {
        "on".to_string()
    }

    fn page(waiting: Option<u64>, last: Option<(u64, Result<Option<String>, String>)>) -> String {
        updates_page(&site(), &UpdatesView { installed: "3.0.0", waiting, last, value: &values, flash: None })
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
}
