//! The onboarding pages: get connected, then what every mark on the owl means and where to act on it.
//!
//! Step 1 opens whenever no portal is signed in, and whenever GitHub has a sign-in but the App is
//! installed nowhere; it replaced the install dialog. Step 2 follows its Continue. Neither is a
//! setting, so they stand alone, outside the settings site.
//!
//! The marks only say that something needs you; acting on one is always a menu entry. So step 2 leads
//! with that, and every mark says which entry acts on it, in the menu's own words: the labels are the
//! constants the menu is built from, never retyped here.

use super::Place;
use crate::page::esc;
use crate::state::{self, PrAxis};

/// Where GitHub stands, for step 1. `None` on [`Onboarding::github`] when no GitHub portal is in play.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GitHubStep {
    /// No sign-in yet. `page` is the portal's settings page, where a sign-in shows its code.
    NotSignedIn { page: String },
    /// A sign-in is running; its code is on `page`.
    SigningIn { page: String },
    /// Signed in, but the App is installed nowhere, so nothing can be seen. `install` is GitHub's page.
    NotInstalled { install: String },
    /// Signed in and installed.
    Ready,
}

/// What step 1 shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Onboarding {
    pub github: Option<GitHubStep>,
    /// Whether Windows keeps the owl on the taskbar rather than behind the ^ arrow. `None` where that
    /// is not a question: every system but Windows.
    pub owl_visible: Option<bool>,
}

impl Onboarding {
    /// Whether Continue is offered. The owl's visibility is recommended, never required.
    pub fn ready(&self) -> bool {
        matches!(self.github, None | Some(GitHubStep::Ready))
    }
}

/// How often step 1 reloads while it waits for a sign-in or an install. Each reload while the App is
/// missing asks GitHub again, so this is also how quickly an install made in another tab shows.
pub const ONBOARDING_REFRESH_SECS: u32 = 10;

/// Step 1: get connected. Reloads itself until it is ready, so a sign-in or an install made in another
/// tab shows here without a click.
pub fn onboarding_page(token: &str, o: &Onboarding) -> String {
    let t = esc(token);
    let mut checks = String::new();
    if let Some(github) = &o.github {
        let (sign_in, install) = match github {
            GitHubStep::NotSignedIn { page } => (
                check(
                    "no",
                    "1",
                    "Sign in to GitHub",
                    "GitHoot reads your pull requests as you. Signing in shows a short code to enter on github.com.",
                    &format!("<a class=\"ghost\" href=\"/{t}/{}\">Sign in to GitHub</a>", esc(page)),
                ),
                waiting(),
            ),
            GitHubStep::SigningIn { page } => (
                check(
                    "no",
                    "1",
                    "Signing in to GitHub",
                    "Enter the code on github.com to finish.",
                    &format!("<a class=\"ghost\" href=\"/{t}/{}\">Show the code</a>", esc(page)),
                ),
                waiting(),
            ),
            GitHubStep::NotInstalled { install } => (
                signed_in(),
                check(
                    "no",
                    "2",
                    "Install the GitHub App",
                    "Signing in lets GitHoot ask as you; installing lets it see your repositories. Read-only: pull \
                     requests, checks and commit statuses. In an organisation, an owner may need to approve it.",
                    &format!(
                        "<a class=\"ghost\" href=\"{}\" rel=\"noreferrer\">Install the App on github.com</a>\
                         <span class=\"sub\">This page notices the install by itself.</span>",
                        esc(install)
                    ),
                ),
            ),
            GitHubStep::Ready => (
                signed_in(),
                check("ok", "✓", "GitHub App installed", "Read-only: pull requests, checks and commit statuses.", ""),
            ),
        };
        checks.push_str(&sign_in);
        checks.push_str(&install);
    }
    match o.owl_visible {
        Some(true) => checks.push_str(&check(
            "ok",
            "✓",
            "The owl stays visible",
            "Always shown on the taskbar, not behind the ^ arrow.",
            "",
        )),
        Some(false) => checks.push_str(&format!(
            "<li class=\"check rec\"><span class=\"st\">{WINDOWS_GLYPH}</span><div class=\"body\">\
             <h3><span class=\"rec-tag\">Recommended</span>Keep the owl visible</h3>\
             <p>Windows may hide new tray icons behind the ^ arrow next to the clock. GitHoot only helps when you \
             can see it: turn it on under Other system tray icons.</p>\
             <div class=\"do\"><a class=\"ghost\" href=\"{TASKBAR_SETTINGS}\">Open the taskbar settings</a></div></div></li>"
        )),
        None => {}
    }
    let next = if o.ready() {
        format!("<div class=\"next\"><a class=\"button\" href=\"/{t}/welcome/marks\">Continue</a></div>")
    } else {
        "<div class=\"next\"><span class=\"sub\">Continue once you are signed in and the App is installed.</span>\
         <button disabled>Continue</button></div>"
            .to_string()
    };
    let body = format!(
        "<p class=\"onboard-step\">Step 1 of 2 · Get connected</p>\n\
         <p class=\"lead\">GitHoot sits in your tray and tells you when a pull request needs you. Get it connected \
         first, then it can start watching.</p>\n\
         <ul class=\"checks\">{checks}</ul>\n{next}\n"
    );
    let refresh = (!o.ready()).then(|| (ONBOARDING_REFRESH_SECS, "welcome".to_string()));
    standalone(token, "Get connected", refresh, &body)
}

/// The Windows page that holds the switch. Opened by the browser, which hands it to Windows: GitHoot
/// does not write the shell's settings itself (see `docs/startup.md`).
const TASKBAR_SETTINGS: &str = "ms-settings:taskbar";

/// Four panes, for "this is about Windows". A plain shape, not Microsoft's logo.
const WINDOWS_GLYPH: &str = "<svg viewBox=\"0 0 16 16\" width=\"12\" height=\"12\" aria-hidden=\"true\" \
    fill=\"currentColor\"><rect x=\"1\" y=\"1\" width=\"6.5\" height=\"6.5\"/><rect x=\"8.5\" y=\"1\" \
    width=\"6.5\" height=\"6.5\"/><rect x=\"1\" y=\"8.5\" width=\"6.5\" height=\"6.5\"/><rect x=\"8.5\" \
    y=\"8.5\" width=\"6.5\" height=\"6.5\"/></svg>";

/// One line of step 1: its state, what it is, and what to do about it. `class` is `ok`, `no` or `wait`.
fn check(class: &str, mark: &str, title: &str, text: &str, action: &str) -> String {
    let action = if action.is_empty() { String::new() } else { format!("<div class=\"do\">{action}</div>") };
    format!(
        "<li class=\"check {class}\"><span class=\"st\">{}</span><div class=\"body\"><h3>{}</h3><p>{}</p>{action}</div></li>",
        esc(mark),
        esc(title),
        esc(text)
    )
}

fn signed_in() -> String {
    check("ok", "✓", "Signed in to GitHub", "GitHoot asks GitHub as you.", "")
}

/// The install, before there is a sign-in to install it with.
fn waiting() -> String {
    check("wait", "2", "Install the GitHub App", "Possible once you are signed in.", "")
}

/// Step 2: what the owl means.
pub fn marks_page(token: &str) -> String {
    let t = esc(token);
    let icon = |name: &str| format!("/{t}/mark-{name}.png");
    let body = format!(
        "<p class=\"onboard-step\">Step 2 of 2 · What the owl means</p>\n\
         <p class=\"lead\">GitHoot sits in your tray and watches your pull requests, so you don't have to.</p>\n\
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
    standalone(token, "What the owl means", None, &body)
}

/// A page outside the settings site: the shared head, the owl heading, `body`, the one footer line.
fn standalone(token: &str, tab: &str, refresh: Option<(u32, String)>, body: &str) -> String {
    let mut h = crate::page::shell_with(
        tab,
        crate::icons::css_hex(crate::icons::MERGE_DOT_COLOR),
        token,
        refresh,
        "",
        Some("<h1>Welcome to GitHoot</h1>"),
    );
    h.push_str(body);
    h.push_str(&format!("<footer>Served locally by {}, to this machine only.</footer>\n", crate::page::owl_link(token)));
    h.push_str(crate::page::PAGE_CLOSE);
    h
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
        marks_page("tok")
    }

    fn step1(github: Option<GitHubStep>, owl_visible: Option<bool>) -> String {
        onboarding_page("tok", &Onboarding { github, owl_visible })
    }

    fn not_signed_in() -> Option<GitHubStep> {
        Some(GitHubStep::NotSignedIn { page: "portals/github".into() })
    }

    // ── Step 1: get connected ───────────────────────────────────────────────

    /// Nothing done yet: sign in is the one thing to do, the install waits for it, and Continue is
    /// there but not usable, saying why.
    #[test]
    fn a_fresh_start_asks_for_the_sign_in_first() {
        let html = step1(not_signed_in(), None);
        assert!(html.contains("Step 1 of 2 · Get connected"), "{html}");
        assert!(html.contains(r#"<a class="ghost" href="/tok/portals/github">Sign in to GitHub</a>"#), "{html}");
        assert!(html.contains(r#"<li class="check wait">"#), "the install waits for the sign-in: {html}");
        assert!(html.contains("<button disabled>Continue</button>"), "{html}");
        assert!(!html.contains("/tok/welcome/marks"), "{html}");
    }

    /// Signed in, App nowhere: the install is the one thing left, on GitHub's own page.
    #[test]
    fn a_sign_in_without_the_app_offers_the_install() {
        let html = step1(Some(GitHubStep::NotInstalled { install: "https://github.com/apps/x/installations/new".into() }), None);
        assert!(html.contains("Signed in to GitHub"), "{html}");
        assert!(html.contains(r#"<a class="ghost" href="https://github.com/apps/x/installations/new" rel="noreferrer">Install the App on github.com</a>"#), "{html}");
        assert!(html.contains("This page notices the install by itself."), "{html}");
        assert!(html.contains("<button disabled>Continue</button>"), "{html}");
    }

    /// Both done: Continue leads to step 2, and the page stops reloading itself.
    #[test]
    fn all_good_continues_to_what_the_owl_means() {
        let html = step1(Some(GitHubStep::Ready), None);
        assert!(html.contains("GitHub App installed"), "{html}");
        assert!(html.contains(r#"<a class="button" href="/tok/welcome/marks">Continue</a>"#), "{html}");
        assert!(!html.contains("http-equiv=\"refresh\""), "{html}");
    }

    /// Waiting reloads the page, so a sign-in or an install made elsewhere shows by itself.
    #[test]
    fn waiting_reloads_the_page() {
        let html = step1(not_signed_in(), None);
        assert!(html.contains(&format!("content=\"{ONBOARDING_REFRESH_SECS};url=/tok/welcome\"")), "{html}");
    }

    /// The taskbar is a Windows question: elsewhere the item is not there at all. On Windows it is
    /// recommended, never required, and its button only opens the setting.
    #[test]
    fn keeping_the_owl_visible_is_a_recommended_windows_item() {
        assert!(!step1(Some(GitHubStep::Ready), None).contains("Keep the owl visible"));
        let hidden = step1(not_signed_in(), Some(false));
        assert!(hidden.contains(r#"<span class="rec-tag">Recommended</span>Keep the owl visible"#), "{hidden}");
        assert!(hidden.contains(r#"<a class="ghost" href="ms-settings:taskbar">Open the taskbar settings</a>"#), "{hidden}");
        let ready_but_hidden = step1(Some(GitHubStep::Ready), Some(false));
        assert!(ready_but_hidden.contains("/tok/welcome/marks"), "never required: {ready_but_hidden}");
        let shown = step1(Some(GitHubStep::Ready), Some(true));
        assert!(shown.contains("The owl stays visible"), "{shown}");
        assert!(!shown.contains("ms-settings:taskbar"), "{shown}");
    }

    /// No GitHub portal in play: there is nothing to connect, so Continue is straight there.
    #[test]
    fn without_github_there_is_nothing_to_connect() {
        let html = step1(None, None);
        assert!(!html.contains("Sign in to GitHub"), "{html}");
        assert!(html.contains("/tok/welcome/marks"), "{html}");
    }

    // ── Step 2: what the owl means ──────────────────────────────────────────

    /// It stands alone: no sidebar, no breadcrumb, nothing that makes it a setting.
    #[test]
    fn it_stands_alone_outside_the_settings() {
        let html = page();
        assert!(!html.contains("class=\"side\""), "{html}");
        assert!(!html.contains("class=\"crumbs\""), "{html}");
        assert!(html.contains("<h1>Welcome to GitHoot</h1>"), "{html}");
        assert!(html.contains("Step 2 of 2 · What the owl means"), "{html}");
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

    /// The same one footer line as every other page. Nothing about finding it again: it is not kept.
    #[test]
    fn its_footer_is_the_owl_line_and_nothing_more() {
        let html = page();
        assert!(html.contains("<footer>Served locally by <a class=\"owl-link\""), "{html}");
        assert!(!html.contains("Settings › Welcome"), "{html}");
        assert!(html.ends_with("</footer>\n</main>\n</body>\n</html>\n"), "{html}");
    }
}
