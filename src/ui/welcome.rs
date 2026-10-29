//! The welcome page: what every mark on the owl means, and where to act on it.
//!
//! Opened once, on the first start, and kept in the sidebar after that, so anyone who has used GitHoot
//! since before it existed can find it too.
//!
//! The marks only say that something needs you; acting on one is always a menu entry. So the page leads
//! with that, and every mark below says which entry acts on it, in the menu's own words: the labels are
//! the constants the menu is built from, never retyped here.

use super::{Place, Site};
use crate::page::esc;
use crate::state::{self, PrAxis};

/// The welcome page.
pub fn welcome_page(site: &Site) -> String {
    let t = esc(site.token);
    let icon = |name: &str| format!("/{t}/mark-{name}.png");
    let body = format!(
        "<p class=\"lead\">GitHoot sits in your tray and watches your pull requests, so you don't have to.</p>\n\
         {how}\
         <h2 class=\"section\">The three bars</h2>\n{review}{merge}{changes}\
         <h2 class=\"section\">The other marks</h2>\n{update}{alert}{all}\
         <h2 class=\"section\">Good to know</h2>\n\
         <div class=\"card\"><ul class=\"steps\">\
         <li><b>Once you have acted</b>, the next fetch will clear the bar. There is nothing to dismiss.</li>\
         <li><b>Mute</b> a pull request from its list to keep it off the bar for a while.</li>\
         <li><b>The hoot</b> plays when a new pull request lands on a bar. Leaving a bar is silent.</li>\
         <li><b>Choose what counts</b> in Settings, one section per bar.</li>\
         </ul></div>\n\
         <div class=\"actions welcome-next\"><a class=\"ghost\" href=\"/{t}/{general}\">Open the settings</a></div>\n",
        how = how_to_act(&icon("changes")),
        review = mark(
            &icon("review"),
            "intro-review",
            "Red bar: somebody wants your review",
            "A pull request waits for your review. Giving it, by approving, asking for changes or commenting, \
             clears the bar. Being asked again brings it back.",
            &act(&["Click the owl", PrAxis::ReviewRequested.menu_label(), "pick a pull request", "review it"]),
        ),
        merge = mark(
            &icon("merge"),
            "intro-merge",
            "Green bar: your pull request was approved",
            "Someone approved one of yours, nobody still objects, there is no conflict and its checks passed. \
             Good news, not a merge button: your repository's own rules are not checked.",
            &act(&["Click the owl", PrAxis::ReadyToMerge.menu_label(), "pick a pull request", "merge it"]),
        ),
        changes = mark(
            &icon("changes"),
            "intro-changes",
            "Amber bar: your pull request needs work",
            "A reviewer asked for changes, or a conflict or failed checks block someone reviewing it, or \
             Copilot left comments still open. The list says which, on each pull request.",
            &act(&["Click the owl", PrAxis::ChangesRequested.menu_label(), "pick a pull request", "fix it"]),
        ),
        update = mark(
            &icon("update"),
            "",
            "Green arrow: a new version is out",
            "GitHoot restarts itself on the new version.",
            &act(&["Click the owl", &format!("{}: …", state::UPDATE_MENU_LABEL)]),
        ),
        alert = mark(
            &icon("alert"),
            "",
            "Red exclamation: GitHoot cannot vouch for what it shows",
            "You need to sign in, a portal reports an outage, or asking failed. A failed answer never shows \
             as zero; the last known state stays.",
            &format!(
                "<div class=\"act\"><b>To act:</b><span class=\"k\">Hover the owl</span><span class=\"s\">to read \
                 why, then</span><span class=\"k\">Click the owl</span><span class=\"s\">›</span>\
                 <span class=\"k\">{}</span><span class=\"s\">or the status entry</span></div>",
                esc(&state::authenticate_menu_label("GitHub"))
            ),
        ),
        all = mark(
            &icon("all"),
            "",
            "Everything combines",
            "Each mark has a fixed place, so any mix can show at once. Here: all three bars, the update arrow \
             and the exclamation. A very bad Monday.",
            "",
        ),
        general = Place::General.path(),
    );
    let note = "This page opens once, on the first start. Find it again under Settings › Welcome.";
    super::layout_with_note(site, &Place::Welcome, "Welcome", None, &body, note)
}

/// The owl with one bar lit, and the menu exactly as it looks then: that bar's entry and the two that
/// are always there. A bar that is dark has no entry, so the drawing shows none.
fn how_to_act(icon: &str) -> String {
    format!(
        "<div class=\"card how\"><img class=\"big\" src=\"{icon}\" alt=\"\" width=\"88\" height=\"88\">\
         <span class=\"arrow\" aria-hidden=\"true\">→</span>\
         <div class=\"menu\" aria-hidden=\"true\"><div class=\"hi\">{} (2)</div><hr>\
         <div>{} ›</div><div>Quit</div></div>\
         <div class=\"how-text\"><h2>The owl shows. The menu acts.</h2>\
         <p>The marks tell you that something needs you. To do something about it, <b>click the owl</b>: \
         each lit bar gets its own entry, with the count. Here: the amber bar, and two of your pull requests \
         need work. A bar that is not lit has no entry.</p>\
         <p>A bar's entry opens the list behind it. Click a pull request there to go straight to it.</p></div></div>\n",
        esc(PrAxis::ChangesRequested.menu_label()),
        esc(state::SETTINGS_MENU_LABEL),
    )
}

/// One mark: its icon as the tray draws it, what it means, and how to act on it.
fn mark(icon: &str, bar: &str, title: &str, body: &str, act: &str) -> String {
    let class = if bar.is_empty() { "card intro".to_string() } else { format!("card intro {bar}") };
    format!(
        "<div class=\"{class}\"><img class=\"mark\" src=\"{icon}\" alt=\"\" width=\"56\" height=\"56\">\
         <div><h3>{}</h3><p>{}</p>{act}</div></div>\n",
        esc(title),
        esc(body)
    )
}

/// The clicks from the owl to the work, one step per chip.
fn act(steps: &[&str]) -> String {
    let steps: Vec<String> = steps.iter().map(|s| format!("<span class=\"k\">{}</span>", esc(s))).collect();
    format!("<div class=\"act\"><b>To act:</b>{}</div>", steps.join("<span class=\"s\">›</span>"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page() -> String {
        welcome_page(&super::super::tests::site())
    }

    /// The marks only say that something needs you. Acting is always in the menu, so that is said
    /// first, with the menu as it really looks for one lit bar: that bar's entry, and nothing for the
    /// bars that are dark.
    #[test]
    fn it_opens_with_the_menu_as_the_place_to_act() {
        let html = page();
        assert!(html.contains("The owl shows. The menu acts."), "{html}");
        let menu = concat!(
            r#"<div class="menu" aria-hidden="true"><div class="hi">Open Work Required (2)</div><hr>"#,
            r#"<div>Settings ›</div><div>Quit</div></div>"#,
        );
        assert!(html.contains(menu), "{html}");
        assert!(html.contains(r#"<img class="big" src="/tok/mark-changes.png""#), "{html}");
    }

    /// Every mark is shown as the tray draws it, served from this app rather than a picture of it.
    #[test]
    fn every_mark_is_shown_as_the_tray_draws_it() {
        let html = page();
        for name in crate::icons::MARKS {
            assert!(html.contains(&format!(r#"src="/tok/mark-{name}.png""#)), "{name}: {html}");
        }
    }

    /// Each bar names the menu entry that acts on it, in the menu's own words.
    #[test]
    fn each_bar_says_which_menu_entry_acts_on_it() {
        let html = page();
        for axis in PrAxis::ALL {
            let step = format!(r#"<span class="k">{}</span>"#, axis.menu_label());
            assert!(html.contains(&step), "{axis:?}: {html}");
        }
        assert!(html.contains(&format!(r#"<span class="k">{}: …</span>"#, crate::state::UPDATE_MENU_LABEL)), "{html}");
        assert!(html.contains(&format!(r#"<span class="k">{}</span>"#, crate::state::authenticate_menu_label("GitHub"))), "{html}");
    }

    #[test]
    fn it_says_a_bar_clears_on_the_next_fetch() {
        assert!(page().contains("the next fetch will clear the bar"));
    }

    /// One footer line: when the page opens and where it stays, then the owl.
    #[test]
    fn its_footer_says_where_to_find_it_again() {
        let html = page();
        assert!(html.contains("<footer>This page opens once, on the first start. Find it again under Settings › Welcome. · <a class=\"owl-link\""), "{html}");
    }
}
