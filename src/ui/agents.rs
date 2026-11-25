//! The Agents page: whether GitHoot starts agents, where they open, and what they are told.
//!
//! A header card with its state, the switch and a Dry run; then its settings as sections, each with its
//! own Save, the chosen client's card inside the Client section; then the folders and prompts below.

use super::{layout, Place, Site};
use crate::agents::Problem;
use crate::page::esc;

/// "On", "Off", and the two states that must not read as either.
pub fn agents_status(on: bool, missing: &[&str], problems: usize) -> String {
    match (on, missing.is_empty()) {
        (false, _) => "Off".to_string(),
        (true, true) if problems > 0 => "On, but check its settings".to_string(),
        (true, true) => "On".to_string(),
        // On but unable to do anything is not "on" in any sense that matters to the reader.
        (true, false) => format!("On, but idle: missing {}", missing.join(", ")),
    }
}

/// The status in red when it is on and cannot do its job, so it does not read as fine.
pub fn status_tone(on: bool, missing: &[&str], problems: usize) -> &'static str {
    if on && (!missing.is_empty() || problems > 0) { " status-bad" } else { "" }
}

/// Switch on is the one thing a visitor came to do while it is off, so it is the filled button. Switch
/// off is outlined, like Settings: the way out must not be the loudest thing on the card.
fn switch_form(token: &str, on: bool) -> String {
    let (action, button) = if on {
        ("off", "<button class=\"ghost\" type=\"submit\">Switch off</button>")
    } else {
        ("on", "<button type=\"submit\">Switch on</button>")
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
    /// Every pull request in the bars it acts on, with what happened to it.
    pub pulls: &'a [crate::agents::dispatch::PullRow],
    /// The last lines of its log, oldest first, each starting with its UTC timestamp.
    pub activity: &'a [String],
}

/// The Agents page: the header card, its settings with the client card in the Client section, then the
/// rest.
pub fn agents_page(site: &Site, v: &AgentsView) -> String {
    let sections = in_first_section(&v.sections, &v.client_card);
    let live = format!(
        "<div id=\"live\" data-live=\"/{}/{}?live=1\">{}</div>",
        esc(site.token),
        Place::Agents.path(),
        live_cards(site.token, v.on, v.pulls, v.activity)
    );
    let body = format!("{}{live}{sections}{}", header_card(site.token, v), v.body);
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
    let status = agents_status(v.on, v.missing, v.problems.len());
    let tone = status_tone(v.on, v.missing, v.problems.len());
    let mut notes = String::new();
    if v.on && !v.missing.is_empty() {
        notes.push_str(&format!(
            "<p class=\"sub\">Put <code>{}</code> on your PATH. Until then it does nothing.</p>",
            v.missing.iter().map(|m| esc(m)).collect::<Vec<_>>().join("</code>, <code>")
        ));
    }
    // A card of its own, ahead of everything, and shown while off too: that is the moment a wrong folder
    // is cheapest to fix. Each line jumps to the box at fault, which is marked as well.
    let alert = problems_alert(v.on, v.problems);
    let flash = v.flash.map(|m| format!("<p class=\"sub\"><strong>{}</strong></p>", esc(m))).unwrap_or_default();
    let switch = switch_form(token, v.on);
    let dry = format!(
        "<form method=\"post\" action=\"/{}/{}\"><input type=\"hidden\" name=\"action\" value=\"dryrun\">\
         <button class=\"ghost\" type=\"submit\">Dry run</button></form>",
        esc(token),
        Place::Agents.path()
    );
    let said = if v.dry_run.is_empty() {
        String::new()
    } else {
        format!("<pre class=\"dry\">{}</pre>", v.dry_run.iter().map(|l| esc(l)).collect::<Vec<_>>().join("\n"))
    };
    format!(
        "{alert}<div class=\"card\"><div class=\"row\"><span class=\"portal-status{tone}\">{}</span></div><p class=\"sub\">{}</p>\
         <p class=\"sub\">{EXPERIMENTAL}</p>{notes}{flash}\
         <div class=\"actions\">{switch}{dry}</div>{said}</div>\n",
        esc(&status),
        esc(crate::agents::SUMMARY),
    )
}

/// The two cards that change while you look: the pull requests (only while it is on) and the recent
/// activity. The page's script asks for them again every few seconds at `?live=1` and swaps them in.
pub fn live_cards(token: &str, on: bool, pulls: &[crate::agents::dispatch::PullRow], activity: &[String]) -> String {
    let pulls = if on { pulls_card(token, pulls) } else { String::new() };
    format!("{pulls}{}", activity_card(token, activity))
}

/// What "experimental" means here, on its card.
const EXPERIMENTAL: &str =
    "Experimental: it may miss a pull request or fail to start an agent. Recent activity, below, says what it did and why.";

/// A form posting one of its own actions, as a small outlined button.
fn small_action(token: &str, action: &str, fields: &[(&str, &str)], button: &str) -> String {
    let hidden: String = fields.iter().map(|(name, value)| format!("<input type=\"hidden\" name=\"{name}\" value=\"{}\">", esc(value))).collect();
    format!(
        "<form method=\"post\" action=\"/{}/{}\"><input type=\"hidden\" name=\"action\" value=\"{action}\">{hidden}\
         <button class=\"ghost small\" type=\"submit\">{button}</button></form>",
        esc(token),
        Place::Agents.path()
    )
}

/// A UTC timestamp the page's script turns into local time. Shown as it is without the script.
fn time(stamp: &str) -> String {
    format!("<time datetime=\"{0}\">{0}</time>", esc(stamp))
}

/// Each pull request in the bars it acts on: what happened, when, why, and Start agent or Retry where a
/// press can change something. Folded shut until asked for, so the page opens on its switch and settings.
fn pulls_card(token: &str, rows: &[crate::agents::dispatch::PullRow]) -> String {
    let failed = rows.iter().filter(|r| r.tone == "bad").count();
    let count = match (rows.len(), failed) {
        (0, _) => "none in its bars".to_string(),
        (n, 0) => format!("{n} in its bars"),
        (n, f) => format!("{n} in its bars, {f} failed"),
    };
    let body: String = rows
        .iter()
        .map(|r| {
            let when = if r.at == 0 { String::new() } else { format!("{} ", time(&crate::log::utc_stamp(r.at))) };
            let press = r.button.map(|b| small_action(token, "start", &[("bar", r.bar), ("key", &r.key)], b)).unwrap_or_default();
            format!(
                "<tr><td><a href=\"{}\">{}</a><span class=\"prnote\">{}</span></td><td class=\"bar\">{}</td>\
                 <td><span class=\"st {}\">{}</span><span class=\"prnote\">{when}{}</span></td><td class=\"press\">{press}</td></tr>",
                esc(&r.url),
                esc(&r.label),
                esc(&r.title),
                esc(r.bar_name),
                r.tone,
                esc(r.state),
                esc(&r.note),
            )
        })
        .collect();
    let table = if rows.is_empty() { String::new() } else { format!("<table class=\"prs\">{body}</table>") };
    format!(
        "<details class=\"card fold\" id=\"pulls\"><summary><h3>Pull request dispatch state</h3><span class=\"sub\">{count}</span></summary>{table}</details>\n"
    )
}

/// A log line's timestamp and the rest.
fn stamp_of(line: &str) -> (&str, String) {
    line.split_once(' ').map_or(("", line.to_string()), |(stamp, rest)| (stamp, rest.to_string()))
}

/// The last lines of its log. Debug information, so folded shut until asked for; absent until there is
/// a log.
fn activity_card(token: &str, lines: &[String]) -> String {
    let Some(last) = lines.last() else { return String::new() };
    let shown: Vec<String> = lines
        .iter()
        .rev()
        .map(|line| match stamp_of(line) {
            (stamp, rest) if stamp.ends_with('Z') => format!("{} {}", time(stamp), esc(&rest)),
            _ => esc(line),
        })
        .collect();
    let last_at = match stamp_of(last) {
        (stamp, _) if stamp.ends_with('Z') => format!(", the last at {}", time(stamp)),
        _ => String::new(),
    };
    format!(
        "<details class=\"card fold\" id=\"activity\"><summary><h3>Recent activity</h3><span class=\"sub\">{} lines{last_at}</span></summary>\
         <div class=\"actions\">{}</div><pre class=\"activity\">{}</pre></details>\n",
        lines.len(),
        small_action(token, "openlog", &[], "Open log"),
        shown.join("\n")
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::tests::site;

    fn view(on: bool, missing: &'static [&'static str]) -> AgentsView<'static> {
        AgentsView {
            on,
            missing,
            problems: &[],
            flash: None,
            dry_run: &[],
            sections: "<section class=\"block\" id=\"s0\"><h2>Client</h2>radio</section>\
                       <section class=\"block\" id=\"s1\"><h2>Agent</h2>claude</section>"
                .to_string(),
            client_card: "<div class=\"card\">the client card</div>".to_string(),
            body: "<p>its own part</p>".to_string(),
            pulls: &[],
            activity: &[],
        }
    }

    fn row(state: &'static str, tone: &'static str, button: Option<&'static str>) -> crate::agents::dispatch::PullRow {
        crate::agents::dispatch::PullRow {
            label: "o/r#5145".into(),
            url: "https://github.com/o/r/pull/5145".into(),
            title: "Bump <b>it</b>".into(),
            bar: "requested-reviews",
            bar_name: "Review requested",
            key: "PR_k1".into(),
            state,
            tone,
            note: "In the bar when Agents was switched on.".into(),
            at: 1_763_538_240,
            button,
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
        assert_eq!(agents_status(true, &["git"], 1), "On, but idle: missing git", "a missing tool still comes first");
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

    /// Experimental in one line on its card, saying what that means and where to look when it
    /// misbehaves. No pill beside its name, in the heading or the sidebar.
    #[test]
    fn it_says_it_is_experimental_on_its_card_only() {
        let html = page(&view(false, &[]));
        assert!(html.contains("<span>Agents</span></h1>"), "{html}");
        assert!(!html.contains("<span class=\"tag"), "{html}");
        assert!(html.contains("Experimental: it may miss a pull request or fail to start an agent."), "{html}");
    }

    /// Each pull request in its bars, folded shut until asked for, with what happened and the press
    /// that can change it. Shown only while it is on.
    #[test]
    fn the_pull_requests_card_is_folded_and_lists_what_happened() {
        let rows = [row("Taken as seen", "seen", Some("Start agent")), row("Failed", "bad", Some("Retry")), row("Agent started", "ok", None)];
        let html = page(&AgentsView { pulls: &rows, ..view(true, &[]) });
        assert!(html.contains(r#"<details class="card fold" id="pulls"><summary><h3>Pull request dispatch state</h3><span class="sub">3 in its bars, 1 failed</span></summary>"#), "{html}");
        assert!(html.contains(r#"<a href="https://github.com/o/r/pull/5145">o/r#5145</a>"#));
        assert!(html.contains("Bump &lt;b&gt;it&lt;/b&gt;"), "titles are other people's text");
        assert!(html.contains(r#"<span class="st seen">Taken as seen</span>"#), "{html}");
        assert!(html.contains(r#"<time datetime="2025-11-19T07:44:00Z">2025-11-19T07:44:00Z</time>"#), "{html}");
        assert!(html.contains(
            r#"<form method="post" action="/tok/agents"><input type="hidden" name="action" value="start"><input type="hidden" name="bar" value="requested-reviews"><input type="hidden" name="key" value="PR_k1"><button class="ghost small" type="submit">Start agent</button></form>"#
        ), "{html}");
        assert_eq!(html.matches("value=\"start\"").count(), 2, "Start agent and Retry, nothing for a started one");
        let off = page(&AgentsView { pulls: &rows, ..view(false, &[]) });
        assert!(!off.contains("id=\"pulls\""), "off, it acts on nothing");
        let none = page(&view(true, &[]));
        assert!(none.contains(r#"<span class="sub">none in its bars</span>"#), "{none}");
    }

    /// The card's classes are its own. A bare `.act` rule elsewhere once laid the button cell out as a
    /// flex box, shifting it off its row.
    #[test]
    fn the_pull_requests_card_classes_clash_with_no_other_rule() {
        let css = crate::page::STYLESHEET;
        for class in ["press", "st"] {
            for bare in [format!("}}.{class}{{"), format!("}}.{class} "), format!("}}.{class},"), format!("}}.{class}.")] {
                assert!(!css.contains(&bare), "a rule for .{class} outside the card: {bare}");
            }
        }
    }

    /// The two cards keep up by themselves: they sit in a wrapper the page's script refreshes from
    /// `?live=1`, which answers with exactly what the wrapper holds, so nothing else on the page, a
    /// half-typed setting included, is touched.
    #[test]
    fn the_cards_refresh_in_place_from_their_own_address() {
        let rows = [row("Taken as seen", "seen", Some("Start agent"))];
        let lines = ["2026-11-19T07:44:02Z one".to_string()];
        let html = page(&AgentsView { pulls: &rows, activity: &lines, ..view(true, &[]) });
        let inner = live_cards("tok", true, &rows, &lines);
        assert!(inner.contains("id=\"pulls\"") && inner.contains("id=\"activity\""), "{inner}");
        assert!(html.contains(&format!("<div id=\"live\" data-live=\"/tok/agents?live=1\">{inner}</div>")), "{html}");
        assert!(!live_cards("tok", false, &rows, &lines).contains("id=\"pulls\""), "off, no pull requests");
        assert!(crate::ui::SITE_SCRIPT.contains("data-live"), "the script that refreshes them");
    }

    /// A press reloads the page, and a card you opened stays open: the page's script keeps which cards
    /// are open for the tab, and opens them again on load.
    #[test]
    fn an_opened_card_stays_open_across_a_press() {
        let script = crate::ui::SITE_SCRIPT;
        assert!(script.contains("addEventListener('toggle'"), "it notices a card opening or closing");
        assert!(script.contains("sessionStorage"), "per tab, gone when the tab closes");
        assert!(script.contains("details.fold[id]"), "only these cards");
    }

    /// Its log is debug information: folded shut by default, the last lines with their times, and a
    /// way to open the whole file.
    #[test]
    fn recent_activity_is_folded_and_opens_the_log() {
        let lines = ["2026-11-19T07:44:02Z [requested-reviews] switched on: 1 pull request(s) taken as seen".to_string(), "2026-11-19T07:44:03Z <odd>".to_string()];
        let html = page(&AgentsView { activity: &lines, ..view(true, &[]) });
        assert!(html.contains(r#"<details class="card fold" id="activity"><summary><h3>Recent activity</h3><span class="sub">2 lines, the last at <time datetime="2026-11-19T07:44:03Z">2026-11-19T07:44:03Z</time></span></summary>"#), "{html}");
        assert!(html.contains(r#"<time datetime="2026-11-19T07:44:02Z">2026-11-19T07:44:02Z</time> [requested-reviews] switched on"#), "{html}");
        assert!(html.contains("&lt;odd&gt;"));
        assert!(html.contains(r#"value="openlog"><button class="ghost small" type="submit">Open log</button>"#), "{html}");
        assert!(!page(&view(true, &[])).contains("id=\"activity\""), "no log, no card");
        assert!(!html.contains("<details class=\"card fold\" id=\"activity\" open"), "closed until clicked");
    }
}
