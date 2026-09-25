//! Portals: where pull requests come from. A list, and a page per portal with its sign-in and its
//! own settings.
//!
//! **The page is the sign-in dialog while one runs.** A sign-in reloads its page every few seconds
//! until it lands, so while it is in flight, or has just been asked for, the page shows the sign-in
//! and nothing else. That is what lets a portal's settings live on the same page as its sign-in
//! without a reload ever throwing away a half-made edit.

use super::{layout, sections, settings_button, Place, Site};
use crate::page::{esc, safe_url};
use crate::portal::{AuthStatus, AuthStyle, PortalInfo, PortalKind, PortalStatus, SignInPrompt};
use crate::setting::Setting;

/// How often a running sign-in's page reloads. Fast enough that the code appears without anyone
/// reaching for F5, slow enough to cost nothing.
const SIGN_IN_REFRESH_SECS: u32 = 3;

/// How soon a page reloads after a button asked the poll thread for something, so it catches up as
/// soon as the poll thread plausibly has.
const CATCH_UP_REFRESH_SECS: u32 = 1;

/// Copies the device code and opens the sign-in address in a new tab when the button is clicked,
/// then says so on the button for a moment.
///
/// `navigator.clipboard` needs a secure context, which `*.localhost` is in every current browser;
/// where it is refused anyway the code is selected instead, one keystroke from copied. The address
/// comes from the button's own `data-url`, which the page sets only for an allowlisted URL, and the
/// tab is opened inside the click handler so popup blockers let it through.
const COPY_SCRIPT: &str = "(function(){var b=document.getElementById('copy-code'),i=document.getElementById('device-code');if(!b||!i)return;var url=b.getAttribute('data-url');function done(){b.textContent='Copied';setTimeout(function(){b.textContent='Copy and open';},1500);if(url){window.open(url,'_blank','noopener,noreferrer');}}function copy(){if(navigator.clipboard&&navigator.clipboard.writeText){return navigator.clipboard.writeText(i.value).catch(function(){i.select();});}i.select();try{document.execCommand('copy');}catch(e){}return Promise.resolve();}b.addEventListener('click',function(){copy().then(done,done);});})();";

/// The settings a portal kind has, as the core declares them. GitHub's keys are flat in
/// `config.txt`, as they always were; they are shown here because this is the portal they are about.
pub fn settings_for(kind: PortalKind) -> &'static [Setting] {
    match kind {
        PortalKind::GitHub => crate::config::GITHUB,
    }
}

/// A short state for the list: whether you are signed in, and nothing that needs a code on screen.
fn short_state(auth: &AuthStatus) -> String {
    match auth {
        AuthStatus::SignedIn => "Signed in".to_string(),
        AuthStatus::NotSignedIn => "Not signed in".to_string(),
        AuthStatus::SigningIn(_) => "Signing in…".to_string(),
        AuthStatus::Off(reason) => esc(reason),
    }
}

/// The Portals list: one card per portal, with Sign in where it would help and ⚙ Settings always.
pub fn portals_page(site: &Site, portals: &[PortalStatus]) -> String {
    let token = site.token;
    let mut h = String::from("<p class=\"sub lead\">Where pull requests come from. Today that is GitHub.</p>\n");
    for p in portals {
        let path = Place::Portal(p.info.id.0.clone()).path();
        let sign_in = if matches!(p.auth, AuthStatus::NotSignedIn) {
            action_form(token, &p.info, "signin", &format!("Sign in to {}", esc(&p.info.display_name)), "")
        } else {
            String::new()
        };
        h.push_str(&format!(
            "<div class=\"card\"><div class=\"row\"><strong><a href=\"/{}/{path}\">{}</a></strong> · \
             <span class=\"portal-status\">{}</span></div><div class=\"actions\">{sign_in}{}</div></div>\n",
            esc(token),
            esc(&p.info.display_name),
            short_state(&p.auth),
            settings_button(token, &path, &p.info.display_name),
        ));
    }
    if portals.is_empty() {
        h.push_str("<div class=\"empty\"><p>No portal has been set up yet in this run.</p></div>\n");
    }
    layout(site, &Place::Portals, "Portals", None, &h)
}

/// What a portal's page needs besides the portal.
pub struct PortalView<'a> {
    pub status: &'a PortalStatus,
    /// The redirect back from Sign in, before the poll thread may have published "in progress".
    pub signin_started: bool,
    /// The redirect back from Sign out.
    pub signed_out: bool,
    pub now_unix: u64,
    /// For the copy button's script. Empty means no script, which the CSP would block anyway.
    pub nonce: &'a str,
    /// The text the file holds for a setting's key. `None` where settings are not available in this
    /// run, and then the page shows the sign-in only.
    pub value: Option<&'a dyn Fn(&Setting) -> String>,
    pub flash: Option<&'a (usize, String)>,
}

/// One portal's page: its sign-in, then its settings. While a sign-in runs or was just asked for,
/// the sign-in only, reloading itself.
pub fn portal_page(site: &Site, v: &PortalView) -> String {
    let token = site.token;
    let info = &v.status.info;
    let place = Place::Portal(info.id.0.clone());
    let refresh = if v.signin_started || v.signed_out {
        Some(CATCH_UP_REFRESH_SECS)
    } else if matches!(v.status.auth, AuthStatus::SigningIn(_)) {
        Some(SIGN_IN_REFRESH_SECS)
    } else {
        None
    };

    let mut h = String::new();
    if v.signin_started {
        h.push_str(&format!(
            "<div class=\"empty\"><strong>Sign-in to {} started.</strong> This page refreshes itself; \
             what to do appears below in a moment.</div>\n",
            esc(&info.display_name)
        ));
    }
    if v.signed_out {
        h.push_str(&format!(
            "<div class=\"empty\"><strong>Signed out of {}.</strong> The saved credential was deleted; \
             sign in again whenever you like.</div>\n",
            esc(&info.display_name)
        ));
    }
    h.push_str("<section class=\"block\" id=\"sign-in\"><h2 class=\"section\">Sign-in</h2>\n");
    h.push_str(&sign_in_card(v.status, token, v.now_unix));
    h.push_str("</section>\n");

    // The one script this page ever carries, and only while there is a code to copy. Named by nonce,
    // so the CSP stays `default-src 'none'` otherwise.
    if matches!(v.status.auth, AuthStatus::SigningIn(Some(_))) && !v.nonce.is_empty() {
        h.push_str(&format!("<script nonce=\"{}\">{COPY_SCRIPT}</script>\n", esc(v.nonce)));
    }

    // Everything else waits while the page reloads itself: nothing editable may be on a page that
    // is about to reload.
    if let (None, Some(value)) = (refresh, v.value) {
        h.push_str(&sections(token, &place, settings_for(info.kind), value, v.flash));
    }
    layout(site, &place, &info.display_name, refresh, &h)
}

/// A button that posts one action to the portal's page.
fn action_form(token: &str, info: &PortalInfo, action: &str, label: &str, class: &str) -> String {
    let class = if class.is_empty() { String::new() } else { format!(" class=\"{class}\"") };
    format!(
        "<form method=\"post\" action=\"/{}/portals/{}\"><input type=\"hidden\" name=\"action\" value=\"{action}\">\
         <button{class} type=\"submit\">{label}</button></form>",
        esc(token),
        esc(&info.id.0)
    )
}

/// The sign-in card: how it stands, and one button. Sign in while not signed in; Sign out while
/// signed in, which deletes the saved credential; Cancel while a sign-in runs. Only a dead end the
/// portal declared gets no button, because a sign-in would change nothing there.
fn sign_in_card(portal: &PortalStatus, token: &str, now_unix: u64) -> String {
    let info = &portal.info;
    let name = esc(&info.display_name);
    let (status, form) = match &portal.auth {
        AuthStatus::SignedIn => ("Signed in".to_string(), action_form(token, info, "signout", "Sign out", "ghost")),
        AuthStatus::NotSignedIn => (
            format!("Not signed in. <span class=\"sub\">{}</span>", sign_in_hint(info)),
            action_form(token, info, "signin", &format!("Sign in to {name}"), ""),
        ),
        AuthStatus::SigningIn(None) => ("Starting sign-in…".to_string(), action_form(token, info, "cancel", "Cancel", "ghost")),
        AuthStatus::SigningIn(Some(prompt)) => {
            (sign_in_step(info, prompt, now_unix), action_form(token, info, "cancel", "Cancel", "ghost"))
        }
        AuthStatus::Off(reason) => (esc(reason), String::new()),
    };
    // A `div`, not a `p`: the running sign-in puts a block (the code row) inside the status.
    format!(
        "<div class=\"card\"><div class=\"row\"><strong>{name}</strong> · <span class=\"portal-status\">{status}</span></div>\
         <div class=\"actions\">{form}</div></div>\n"
    )
}

/// What the user has to do right now: the code, where to enter it, and how long they have. The link
/// opens in a new tab on purpose, the one place this app does that: this page has to stay open to
/// show the outcome, and the code is also on the clipboard.
///
/// The address came over the network, so it is a link only under the portal's own prefix, like every
/// other URL on these pages; anything else is shown as text.
fn sign_in_step(info: &PortalInfo, prompt: &SignInPrompt, now_unix: u64) -> String {
    let left = prompt.expires_at.saturating_sub(now_unix);
    let deadline = if left == 0 {
        "The code has expired; cancel and start again.".to_string()
    } else {
        format!("Expires in {} min.", left.div_ceil(60))
    };
    let safe = safe_url(&prompt.url, &info.link_prefix);
    let where_ = match safe {
        Some(url) => format!("<a href=\"{}\" target=\"_blank\" rel=\"noreferrer noopener\">{}</a>", esc(url), esc(url)),
        None => esc(&prompt.url),
    };
    let open = safe.map(|url| format!(" data-url=\"{}\"", esc(url))).unwrap_or_default();
    format!(
        "Enter this code at {where_}. {deadline}<div class=\"code-row\"><input id=\"device-code\" \
         class=\"device-code\" type=\"text\" readonly value=\"{}\" aria-label=\"Device code\">\
         <button type=\"button\" id=\"copy-code\"{open}>Copy and open</button></div><span class=\"sub\">\
         This page updates itself when you are done.</span>",
        esc(&prompt.code)
    )
}

/// What the sign-in will look like, so the button is not a surprise.
fn sign_in_hint(info: &PortalInfo) -> String {
    match info.capabilities.auth_style {
        AuthStyle::DeviceFlow => {
            format!("A code and a link to {} appear here; enter the code there to finish.", esc(&info.display_name))
        }
        AuthStyle::PastedToken => format!("You will be asked for a token you created on {}.", esc(&info.display_name)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::tests::site;

    const NOW: u64 = 1_790_000_000;

    /// The one portal every existing install has, as the page sees it.
    fn github() -> PortalInfo {
        PortalInfo {
            id: crate::portal::PortalId("github".to_string()),
            kind: PortalKind::GitHub,
            display_name: "GitHub".to_string(),
            link_prefix: "https://github.com/".to_string(),
            inbox_url: "https://github.com/pulls/inbox".to_string(),
            status_page: None,
            capabilities: crate::portal::Capabilities {
                auth_style: AuthStyle::DeviceFlow,
                conflict_state: true,
                rereview_pending: true,
                team_reviewers: true,
                bot_reviewer: Some("Copilot"),
            },
            min_poll_interval: std::time::Duration::from_secs(60),
        }
    }

    fn portal(auth: AuthStatus) -> PortalStatus {
        PortalStatus { info: github(), auth }
    }

    fn values(_: &Setting) -> String {
        "on".to_string()
    }

    fn page_for(status: &PortalStatus, signin_started: bool) -> String {
        let value: &dyn Fn(&Setting) -> String = &values;
        portal_page(
            &site(),
            &PortalView { status, signin_started, signed_out: false, now_unix: NOW, nonce: "n", value: Some(value), flash: None },
        )
    }

    /// The button appears exactly when a click would help: not signed in. It posts to the portal's
    /// own page, and the flow is explained before the click.
    #[test]
    fn a_portal_waiting_for_sign_in_gets_a_button_that_names_it() {
        let html = page_for(&portal(AuthStatus::NotSignedIn), false);
        assert!(html.contains("<strong>GitHub</strong> · <span class=\"portal-status\">Not signed in."), "got {html}");
        assert!(html.contains(r#"<form method="post" action="/tok/portals/github"><input type="hidden" name="action" value="signin"><button type="submit">Sign in to GitHub</button></form>"#), "{html}");
        assert!(html.contains("A code and a link"));
        assert!(!html.contains("http-equiv=\"refresh\""), "nothing running: the page sits still");
    }

    /// Its own settings sit on its own page, below the sign-in, each section its own form.
    #[test]
    fn the_github_settings_sit_on_the_github_page() {
        let html = page_for(&portal(AuthStatus::SignedIn), false);
        assert!(html.contains(r#"name="copilotReviews""#) && html.contains(r#"name="statusComponents""#), "{html}");
        assert!(html.contains("<h2 class=\"section\">Pull requests</h2>") && html.contains("<h2 class=\"section\">Outages</h2>"));
        for name in crate::config::all_components() {
            assert!(html.contains(&format!(r#"value="{name}""#)), "{name} is not offered");
        }
        assert!(html.find("id=\"sign-in\"").unwrap() < html.find("id=\"s0\"").unwrap(), "sign-in first");
    }

    #[test]
    fn a_signed_in_portal_offers_to_sign_out() {
        let html = page_for(&portal(AuthStatus::SignedIn), false);
        assert!(html.contains("<span class=\"portal-status\">Signed in</span>"), "got {html}");
        assert!(html.contains(r#"value="signout"><button class="ghost" type="submit">Sign out</button>"#));
        assert!(!html.contains("Cancel</button>") && !html.contains("Sign in to GitHub</button>"));
    }

    /// The page reloads while the sign-in runs, and while it does it shows the sign-in only, so no
    /// settings form is on screen to lose.
    #[test]
    fn a_running_sign_in_shows_only_itself_and_reloads() {
        let html = page_for(&portal(AuthStatus::SigningIn(None)), false);
        assert!(html.contains("Starting sign-in"), "got {html}");
        assert!(html.contains(r#"value="cancel"><button class="ghost" type="submit">Cancel</button>"#));
        assert!(html.contains(r#"<meta http-equiv="refresh" content="3;url=/tok/portals/github">"#), "{html}");
        assert!(!html.contains("name=\"section\""), "no settings form on a page that reloads itself");

        let prompt = SignInPrompt { code: "ABCD-1234".into(), url: "https://github.com/login/device".into(), expires_at: NOW + 14 * 60 + 30 };
        let html = page_for(&portal(AuthStatus::SigningIn(Some(prompt.clone()))), false);
        assert!(html.contains(r#"<input id="device-code" class="device-code" type="text" readonly value="ABCD-1234" aria-label="Device code">"#), "got {html}");
        assert!(html.contains(r#"<button type="button" id="copy-code" data-url="https://github.com/login/device">Copy and open</button>"#));
        assert!(html.contains("<script nonce=\"n\">"));
        assert!(html.contains("Expires in 15 min."));
        let expired = SignInPrompt { expires_at: NOW - 1, ..prompt };
        assert!(page_for(&portal(AuthStatus::SigningIn(Some(expired))), false).contains("The code has expired"));
    }

    /// The redirect back from Sign in says so at once and catches up quickly, sign-in only.
    #[test]
    fn the_sign_in_started_banner_catches_up_with_the_poll_thread() {
        let html = page_for(&portal(AuthStatus::NotSignedIn), true);
        assert!(html.contains("<strong>Sign-in to GitHub started.</strong>"), "got {html}");
        assert!(html.contains(r#"content="1;url=/tok/portals/github""#));
        assert!(!html.contains("name=\"section\""));
    }

    #[test]
    fn the_prompt_and_the_status_text_are_escaped() {
        let prompt = SignInPrompt { code: "<b>".into(), url: "javascript:x".into(), expires_at: NOW + 60 };
        let html = page_for(&portal(AuthStatus::SigningIn(Some(prompt))), false);
        assert!(html.contains("&lt;b&gt;") && !html.contains("<b>"));
        assert!(!html.contains("href=\"javascript") && !html.contains("data-url="));
        let html = page_for(&portal(AuthStatus::Off("<b>x</b>".into())), false);
        assert!(html.contains("&lt;b&gt;x&lt;/b&gt;") && !html.contains("<b>x</b>"));
        assert!(!html.contains("value=\"signin\""), "a dead end gets no button");
    }

    /// No script unless a device code is on screen, and none at all without a nonce to name it.
    #[test]
    fn no_script_unless_a_code_is_on_screen() {
        assert!(!page_for(&portal(AuthStatus::SignedIn), false).contains("<script"));
        assert!(!page_for(&portal(AuthStatus::SigningIn(None)), false).contains("<script"));
        let prompt = SignInPrompt { code: "X".into(), url: github().link_prefix.clone(), expires_at: NOW + 60 };
        let status = portal(AuthStatus::SigningIn(Some(prompt)));
        let html = portal_page(&site(), &PortalView { status: &status, signin_started: false, signed_out: false, now_unix: NOW, nonce: "", value: None, flash: None });
        assert!(!html.contains("<script"));
    }

    #[test]
    fn the_list_offers_sign_in_where_it_helps_and_settings_always() {
        let html = portals_page(&site(), &[portal(AuthStatus::NotSignedIn)]);
        assert!(html.contains(r#"value="signin"><button type="submit">Sign in to GitHub</button>"#), "{html}");
        assert!(html.contains(r#"<a class="ghost" href="/tok/portals/github" aria-label="GitHub settings">"#));
        let html = portals_page(&site(), &[portal(AuthStatus::SignedIn)]);
        assert!(!html.contains("value=\"signin\"") && html.contains("aria-label=\"GitHub settings\""));
    }
}
