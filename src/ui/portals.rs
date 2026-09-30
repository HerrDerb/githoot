//! Portals: where pull requests come from. A list, and a page per portal with its sign-in and its
//! own settings.
//!
//! **Signing in happens in the card you pressed.** Every portal is one card, on the list and on its own
//! page alike, and the card carries its sign-in panel with it: pressing Sign in turns the card itself
//! into the sign-in, with the code, one button that copies it and opens the portal, how long the code
//! lasts, and Cancel. Nothing navigates and nothing reloads; one script asks in the background how the
//! sign-in stands and fills the card in place, and the page changes once, when the sign-in is over.
//! Without the script the forms still post and the page reloads until the sign-in lands.

use super::{layout, sections, settings_button, Place, Site};
use crate::page::{esc, safe_url};
use crate::portal::{AuthStatus, PortalKind, PortalStatus, SignInPrompt};
use crate::setting::Setting;

/// How often a page reloads while a sign-in is in flight, for the page that cannot run the script.
const SIGN_IN_REFRESH_SECS: u32 = 2;

/// Drives every portal card on the page. Delegated from the document, so it serves one card or many.
///
/// - Forms are posted to `getAttribute('action')`, never `form.action`: each form has a hidden field
///   named `action`, which shadows the property, and the post would go to `[object HTMLInputElement]`.
/// - **Sign in** (`form[data-signin]`) is posted in the background and the card turns into its
///   sign-in panel at once, in the shape it keeps: the code box pulses until the code arrives.
/// - The card then asks its `data-state-url` about once a second. A code is written into the box,
///   the button enabled and the deadline counted down, all in place. `not_signed_in` before any
///   `signing_in` is the moment before the server has started, and is waited out. Once the answer is
///   final the card goes back to Sign in with the reason, or the page reloads once to show Signed in.
/// - **Copy code and open** copies the code and opens the address in the same click, so the browser
///   lets the tab through. The address comes from the server only when it passed `safe_url`. Where
///   the clipboard is refused the code is selected instead; GitHoot has put it on the clipboard too.
/// - **Cancel** (`form[data-cancel]`) is posted in the background; the watcher sees the sign-in end.
/// - **Sign out** (`form[data-signout]`) is optimistic: the card turns to Not signed in at once and
///   the post goes in the background. It deletes a local file, so there is nothing worth waiting for,
///   and the server says the same on any page it renders meanwhile.
const SIGNIN_SCRIPT: &str = "(function(){\
function q(c,s){return c.querySelector(s);}\
function mmss(s){s=Math.max(0,Math.floor(s));return Math.floor(s/60)+':'+('0'+s%60).slice(-2);}\
function post(f){return fetch(f.getAttribute('action'),{method:'POST',body:new URLSearchParams(new FormData(f)),redirect:'manual'});}\
function signing(c){c.setAttribute('data-state','signing_in');q(c,'[data-idle]').hidden=true;q(c,'[data-signed]').hidden=true;q(c,'[data-signing]').hidden=false;\
var e=q(c,'.sign-in-error');if(e)e.hidden=true;q(c,'.portal-status').textContent='Waiting for you to authorize';}\
function idle(c,msg){c.setAttribute('data-state','not_signed_in');q(c,'[data-idle]').hidden=false;q(c,'[data-signed]').hidden=true;q(c,'[data-signing]').hidden=true;\
var st=document.querySelector('[data-settings]');if(st)st.hidden=true;\
q(c,'.portal-status').textContent='Not signed in';var o=q(c,'.device-code'),b=q(c,'.copy-open');o.textContent='····-····';o.classList.add('pending');\
b.disabled=true;b.removeAttribute('data-url');b.textContent='Copy code and open '+c.getAttribute('data-name');q(c,'.expiry').textContent='';\
q(c,'.sign-in-text').textContent='Getting a code from '+c.getAttribute('data-name')+'…';c.deadline=0;\
var e=q(c,'.sign-in-error');if(e&&msg){e.textContent=msg;e.hidden=false;}}\
function tick(c){var x=q(c,'.expiry');if(!x||!c.deadline)return;var s=(c.deadline-Date.now())/1000;x.textContent=s>0?'Expires in '+mmss(s):'Code expired';}\
function show(c,j){var o=q(c,'.device-code'),b=q(c,'.copy-open'),t=q(c,'.sign-in-text'),n=c.getAttribute('data-name');\
if(o.textContent!==j.code){o.textContent=j.code;o.classList.remove('pending');}b.disabled=false;\
if(j.url){b.setAttribute('data-url',j.url);b.textContent='Copy code and open '+n;t.textContent='Copy the code, then paste it on the '+n+' page that opens.';}\
else{b.removeAttribute('data-url');b.textContent='Copy code';t.textContent='Go to '+(j.url_text||'the sign-in page')+' and enter this code.';}\
c.deadline=Date.now()+j.expires_in*1000;tick(c);}\
function watch(c,seen){if(c.watching)return;c.watching=true;var url=c.getAttribute('data-state-url'),t0=Date.now();\
var x=q(c,'.expiry');if(x&&x.hasAttribute('data-expires-in')){c.deadline=Date.now()+x.getAttribute('data-expires-in')*1000;}\
var timer=setInterval(function(){tick(c);},1000);function stop(){c.watching=false;clearInterval(timer);}\
function t(){fetch(url,{cache:'no-store'}).then(function(r){return r.json();}).then(function(j){\
if(j.state==='signing_in'){seen=true;if(j.code)show(c,j);setTimeout(t,j.code?1500:400);}\
else if(j.state==='not_signed_in'&&!seen&&!j.error&&Date.now()-t0<20000){setTimeout(t,400);}\
else if(j.state==='not_signed_in'){stop();idle(c,j.error);}\
else{stop();location.reload();}}).catch(function(){setTimeout(t,2000);});}t();}\
document.addEventListener('submit',function(ev){var f=ev.target,c=f.closest('.portal-card');if(!c)return;\
if(f.hasAttribute('data-signin')){ev.preventDefault();signing(c);post(f).then(function(){watch(c,false);},function(){idle(c,'Could not reach GitHoot. Try again.');});}\
else if(f.hasAttribute('data-cancel')){ev.preventDefault();post(f);}\
else if(f.hasAttribute('data-signout')){ev.preventDefault();idle(c);post(f);}});\
document.addEventListener('click',function(ev){var b=ev.target.closest('.copy-open');if(!b||b.disabled)return;\
var c=b.closest('.portal-card'),o=q(c,'.device-code'),url=b.getAttribute('data-url'),label=b.textContent;\
var copied=navigator.clipboard&&navigator.clipboard.writeText?navigator.clipboard.writeText(o.textContent):Promise.reject();\
copied.catch(function(){var r=document.createRange();r.selectNodeContents(o);var s=getSelection();s.removeAllRanges();s.addRange(r);});\
if(url){window.open(url,'_blank','noopener,noreferrer');}\
b.textContent=url?'Copied. Paste it on the '+c.getAttribute('data-name')+' tab':'Copied';setTimeout(function(){b.textContent=label;},2500);});\
document.querySelectorAll('.portal-card[data-state=\"signing_in\"]').forEach(function(c){watch(c,true);});\
})();";

/// One word per sign-in state, for the card's `data-state` and the `?state=1` answer.
pub fn auth_state(auth: &AuthStatus) -> &'static str {
    match auth {
        AuthStatus::SignedIn => "signed_in",
        AuthStatus::NotSignedIn => "not_signed_in",
        AuthStatus::SigningIn(_) => "signing_in",
        AuthStatus::Off(_) => "off",
    }
}

/// What the card's script reads: the state; while a code is out, the code, its lifetime and the
/// address (`url` only when it is under `link_prefix`, the rule every link on these pages follows;
/// `url_text` always, to show as text otherwise); and why the last sign-in failed, if it did.
pub fn state_json(auth: &AuthStatus, link_prefix: &str, error: Option<&str>, now_unix: u64) -> String {
    let mut j = serde_json::json!({ "state": auth_state(auth) });
    if let AuthStatus::SigningIn(Some(prompt)) = auth {
        j["code"] = prompt.code.clone().into();
        j["url_text"] = prompt.url.clone().into();
        j["expires_in"] = prompt.expires_at.saturating_sub(now_unix).into();
        if let Some(url) = safe_url(&prompt.url, link_prefix) {
            j["url"] = url.into();
        }
    }
    if let (AuthStatus::NotSignedIn, Some(error)) = (auth, error) {
        j["error"] = error.into();
    }
    j.to_string()
}

/// The settings a portal kind has, as the core declares them. GitHub's keys are flat in
/// `config.txt`, as they always were; they are shown here because this is the portal they are about.
pub fn settings_for(kind: PortalKind) -> &'static [Setting] {
    match kind {
        PortalKind::GitHub => crate::config::GITHUB,
        // Its `portal.<name>.` keys are edited in config.txt for now: the pages can only write the
        // core's flat keys and an integration's, and a per-portal store is its own piece of work.
        PortalKind::GitLab => &[],
    }
}

/// One portal as its card shows it: a running portal, or a kind that is not running yet, which
/// reads as not signed in because to the person that is all it is.
#[derive(Clone, Debug)]
pub struct Card {
    pub id: String,
    pub name: String,
    pub kind: PortalKind,
    pub auth: AuthStatus,
    /// Only addresses under this become links.
    pub link_prefix: String,
    /// Why the last sign-in ended without a credential, shown until the next press.
    pub error: Option<String>,
}

impl Card {
    pub fn of(status: &PortalStatus, error: Option<String>) -> Card {
        Card {
            id: status.info.id.0.clone(),
            name: status.info.display_name.clone(),
            kind: status.info.kind,
            auth: status.auth.clone(),
            link_prefix: status.info.link_prefix.clone(),
            error,
        }
    }

    fn signing(&self) -> bool {
        matches!(self.auth, AuthStatus::SigningIn(_))
    }
}

/// `4:59`: minutes and seconds, the way a countdown is read.
fn mmss(secs: u64) -> String {
    format!("{}:{:02}", secs / 60, secs % 60)
}

/// What to expect after authorizing on the portal's site. GitLab answers a successful Authorize by
/// redirecting to its blank "enter your device code" form (Doorkeeper's index page, with a small
/// notice), which reads as being asked for a second code.
fn after_authorize(kind: PortalKind) -> &'static str {
    match kind {
        PortalKind::GitLab => "After you authorize, GitLab shows its code form again. That means it worked; close that tab.",
        PortalKind::GitHub => "",
    }
}

/// A form posting `action` for portal `id` to the list, the one place Sign in and Cancel go, so they
/// work for a portal that is not running yet.
fn list_form(token: &str, marker: &str, action: &str, c: &Card, button: &str) -> String {
    format!(
        "<form method=\"post\" action=\"/{}/portals\" {marker}><input type=\"hidden\" name=\"action\" value=\"{action}\">\
         <input type=\"hidden\" name=\"portal\" value=\"{}\"><input type=\"hidden\" name=\"kind\" value=\"{}\">{button}</form>",
        esc(token),
        esc(&c.id),
        c.kind.key(),
    )
}

/// Sign out posts to the portal's own page, where the running portal is.
fn sign_out_form(token: &str, c: &Card) -> String {
    format!(
        "<form method=\"post\" action=\"/{}/portals/{}\" data-signout><input type=\"hidden\" name=\"action\" value=\"signout\">\
         <button class=\"ghost\" type=\"submit\">Sign out</button></form>",
        esc(token),
        esc(&c.id)
    )
}

/// The sign-in panel, in the one shape it keeps from the press to the end, so the code arriving
/// moves nothing. Hidden until needed; rendered for every card that can sign in, so the script only
/// has to show it.
fn sign_in_panel(token: &str, c: &Card, now_unix: u64) -> String {
    let name = esc(&c.name);
    let prompt: Option<&SignInPrompt> = match &c.auth {
        AuthStatus::SigningIn(p) => p.as_ref(),
        _ => None,
    };
    let (text, code, button, expiry) = match prompt {
        Some(p) => {
            let left = p.expires_at.saturating_sub(now_unix);
            let expiry = format!(
                "<span class=\"expiry\" data-expires-in=\"{left}\">{}</span>",
                if left == 0 { "Code expired".to_string() } else { format!("Expires in {}", mmss(left)) }
            );
            let code = format!("<output class=\"device-code\" aria-live=\"polite\">{}</output>", esc(&p.code));
            match safe_url(&p.url, &c.link_prefix) {
                Some(url) => (
                    format!("Copy the code, then paste it on the {name} page that opens."),
                    code,
                    format!("<button type=\"button\" class=\"copy-open\" data-url=\"{}\">Copy code and open {name}</button>", esc(url)),
                    expiry,
                ),
                None => (
                    format!("Go to {} and enter this code.", esc(&p.url)),
                    code,
                    "<button type=\"button\" class=\"copy-open\">Copy code</button>".to_string(),
                    expiry,
                ),
            }
        }
        None => (
            format!("Getting a code from {name}…"),
            "<output class=\"device-code pending\" aria-live=\"polite\">····-····</output>".to_string(),
            format!("<button type=\"button\" class=\"copy-open\" disabled>Copy code and open {name}</button>"),
            "<span class=\"expiry\"></span>".to_string(),
        ),
    };
    let note = match after_authorize(c.kind) {
        "" => String::new(),
        line => format!("<p class=\"sub\">{}</p>", esc(line)),
    };
    let cancel = list_form(token, "data-cancel", "cancel", c, "<button class=\"link\" type=\"submit\">Cancel</button>");
    format!(
        "<div class=\"sign-in\" data-signing{}><p class=\"sign-in-text\">{text}</p>\
         <div class=\"code-row\">{code}{button}</div><div class=\"sign-in-foot\">{expiry}{cancel}</div>{note}</div>",
        if c.signing() { "" } else { " hidden" }
    )
}

/// One portal's card: name and state, its buttons, and for a portal that can sign in, its sign-in
/// panel. `with_settings` is the list; the portal's own page is the settings, so it leaves the
/// button out.
fn portal_card(token: &str, c: &Card, now_unix: u64, with_settings: bool) -> String {
    let name = esc(&c.name);
    let path = Place::Portal(c.id.clone()).path();
    let status = match &c.auth {
        AuthStatus::SignedIn => "Signed in".to_string(),
        AuthStatus::NotSignedIn => "Not signed in".to_string(),
        AuthStatus::SigningIn(_) => "Waiting for you to authorize".to_string(),
        AuthStatus::Off(reason) => esc(reason),
    };
    // Every row the card can need is rendered, the ones not in play hidden, so the script can move
    // between signed in, signing in and not signed in without asking the server for a new card.
    let signed = matches!(c.auth, AuthStatus::SignedIn | AuthStatus::Off(_));
    let sign_in = list_form(token, "data-signin", "signin", c, &format!("<button type=\"submit\">Sign in to {name}</button>"));
    let settings = if with_settings { settings_button(token, &path, &c.name) } else { String::new() };
    let signed_row = format!(
        "<div class=\"actions\" data-signed{}>{settings}{}</div>",
        if signed { "" } else { " hidden" },
        sign_out_form(token, c)
    );
    let error = match (&c.error, &c.auth) {
        (Some(line), AuthStatus::NotSignedIn) => format!("<p class=\"sign-in-error\" role=\"alert\">{}</p>", esc(line)),
        _ => "<p class=\"sign-in-error\" role=\"alert\" hidden></p>".to_string(),
    };
    let panel = sign_in_panel(token, c, now_unix);
    format!(
        "<div class=\"card portal-card\" id=\"portal-{id}\" data-name=\"{name}\" data-state=\"{state}\" \
         data-state-url=\"/{token}/{path}?state=1\"><div class=\"portal-head\">{title}\
         <span class=\"portal-status\">{status}</span></div>{error}<div class=\"actions\" data-idle{hidden}>{sign_in}</div>{signed_row}{panel}</div>\n",
        id = esc(&c.id),
        // On the list the card needs its name; on the portal's own page the title already says it.
        title = if with_settings { format!("<strong>{name}</strong>") } else { String::new() },
        state = auth_state(&c.auth),
        token = esc(token),
        hidden = if matches!(c.auth, AuthStatus::NotSignedIn) { "" } else { " hidden" },
    )
}

/// The script, named by the nonce, or the reload that stands in for it when there is no nonce and a
/// sign-in is in flight.
fn script_or_refresh(h: &mut String, nonce: &str, cards: &[&Card]) -> Option<u32> {
    if !nonce.is_empty() {
        h.push_str(&format!("<script nonce=\"{}\">{SIGNIN_SCRIPT}</script>\n", esc(nonce)));
        None
    } else if cards.iter().any(|c| c.signing()) {
        Some(SIGN_IN_REFRESH_SECS)
    } else {
        None
    }
}

/// The Portals list: every kind of portal, one card each. Signing in and out are the only things a
/// person does here, and they are also install and uninstall; see the module doc comment.
pub fn portals_page(site: &Site, cards: &[Card], nonce: &str, flash: Option<&str>, now_unix: u64) -> String {
    let token = site.token;
    let mut h = String::from("<p class=\"sub lead\">Where pull requests come from.</p>\n");
    if let Some(line) = flash {
        h.push_str(&format!("<div class=\"empty\"><strong>{}</strong></div>\n", esc(line)));
    }
    for c in cards {
        h.push_str(&portal_card(token, c, now_unix, true));
    }
    let refresh = script_or_refresh(&mut h, nonce, &cards.iter().collect::<Vec<_>>());
    layout(site, &Place::Portals, "Portals", refresh, &h)
}

/// What a portal's page needs besides the site.
pub struct PortalView<'a> {
    pub card: Card,
    pub now_unix: u64,
    /// For the card's script. Empty means no script, and the page reloads while a sign-in runs.
    pub nonce: &'a str,
    /// The text the file holds for a setting's key. `None` where settings are not available in this
    /// run, and then the page shows the card only.
    pub value: Option<&'a dyn Fn(&Setting) -> String>,
    pub flash: Option<&'a (usize, String)>,
}

/// One portal's page: its card, then, once signed in, its settings.
pub fn portal_page(site: &Site, v: &PortalView) -> String {
    let token = site.token;
    let place = Place::Portal(v.card.id.clone());
    let mut h = String::from("<section class=\"block\" id=\"sign-in\"><h2 class=\"section\">Sign-in</h2>\n");
    h.push_str(&portal_card(token, &v.card, v.now_unix, false));
    h.push_str("</section>\n");
    let refresh = script_or_refresh(&mut h, v.nonce, &[&v.card]);
    // Settings once signed in: nothing to configure about a portal that cannot be asked anything.
    if let (Some(value), AuthStatus::SignedIn | AuthStatus::Off(_)) = (v.value, &v.card.auth) {
        // One block, so Sign out can hide it at once along with the card's flip.
        h.push_str(&format!("<div data-settings>{}</div>", sections(token, &place, settings_for(v.card.kind), value, v.flash)));
    }
    layout(site, &place, &v.card.name, refresh, &h)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::tests::site;

    const NOW: u64 = 1_790_000_000;

    fn card(id: &'static str, kind: PortalKind, auth: AuthStatus) -> Card {
        Card {
            id: id.to_string(),
            name: kind.display_name().to_string(),
            kind,
            auth,
            link_prefix: format!("https://{id}.com/"),
            error: None,
        }
    }

    fn prompt(url: &str) -> SignInPrompt {
        SignInPrompt { code: "ABCD-1234".into(), url: url.into(), expires_at: NOW + 299 }
    }

    fn values(_: &Setting) -> String {
        "on".to_string()
    }

    fn list(cards: &[Card], nonce: &str) -> String {
        portals_page(&site(), cards, nonce, None, NOW)
    }

    fn page(c: Card) -> String {
        let value: &dyn Fn(&Setting) -> String = &values;
        portal_page(&site(), &PortalView { card: c, now_unix: NOW, nonce: "n", value: Some(value), flash: None })
    }

    // ── The card at rest ──────────────────────────────────────────────────────

    /// Not signed in: one button, which installs if need be and signs in. It posts to the list, so
    /// the same form works for a portal that is not running yet.
    #[test]
    fn not_signed_in_offers_one_button_that_signs_in() {
        let html = list(&[card("gitlab", PortalKind::GitLab, AuthStatus::NotSignedIn)], "n");
        assert!(html.contains(r#"<form method="post" action="/tok/portals" data-signin>"#), "{html}");
        assert!(html.contains(r#"name="action" value="signin""#) && html.contains(r#"name="portal" value="gitlab""#));
        assert!(html.contains(r#"name="kind" value="gitlab""#));
        assert!(html.contains(">Sign in to GitLab</button>"), "{html}");
        let hidden_row = html.find(r#"<div class="actions" data-signed hidden>"#).expect("the signed-in row, hidden");
        assert!(html.find("GitLab settings").is_some_and(|at| at > hidden_row), "Settings only inside the hidden row: nothing to configure before signing in");
        assert!(html.contains(r#"<div class="sign-in" data-signing hidden>"#), "the panel is ready, hidden");
    }

    /// Signed in: Settings and Sign out on show. The Sign in button and the panel are there too,
    /// hidden, so Sign out can flip the card at once without asking the server for a new one.
    #[test]
    fn signed_in_offers_settings_and_sign_out_with_sign_in_ready_behind_them() {
        let html = list(&[card("github", PortalKind::GitHub, AuthStatus::SignedIn)], "n");
        assert!(html.contains(r#"<div class="actions" data-signed>"#), "{html}");
        assert!(html.contains(r#"aria-label="GitHub settings""#), "{html}");
        assert!(html.contains(r#"<form method="post" action="/tok/portals/github" data-signout>"#), "{html}");
        assert!(html.contains(r#"value="signout"><button class="ghost" type="submit">Sign out</button>"#), "{html}");
        assert!(html.contains(r#"<div class="actions" data-idle hidden>"#), "{html}");
        assert!(html.contains(r#"<div class="sign-in" data-signing hidden>"#), "{html}");
        assert!(html.contains(r#"<span class="portal-status">Signed in</span>"#));
    }

    /// Not signed in: the signed-in buttons wait hidden, for the card to flip back after a sign-in.
    #[test]
    fn not_signed_in_keeps_the_signed_in_buttons_hidden() {
        let html = list(&[card("gitlab", PortalKind::GitLab, AuthStatus::NotSignedIn)], "n");
        assert!(html.contains(r#"<div class="actions" data-signed hidden>"#), "{html}");
    }

    /// On a portal's own page the settings sit in one block the script can hide on Sign out.
    #[test]
    fn the_portal_page_wraps_its_settings_so_sign_out_can_hide_them() {
        let html = page(card("github", PortalKind::GitHub, AuthStatus::SignedIn));
        assert!(html.contains("<div data-settings>"), "{html}");
    }

    // ── The card while signing in ─────────────────────────────────────────────

    /// With the code out, the panel shows it large, the one button that copies it and opens the
    /// portal, how long it lasts, and Cancel. The Sign in button is out of the way.
    #[test]
    fn signing_in_shows_the_code_the_button_the_deadline_and_cancel() {
        let c = card("github", PortalKind::GitHub, AuthStatus::SigningIn(Some(prompt("https://github.com/login/device"))));
        let html = list(&[c], "n");
        assert!(html.contains(r#"data-state="signing_in""#), "{html}");
        assert!(html.contains(r#"<span class="portal-status">Waiting for you to authorize</span>"#), "{html}");
        assert!(html.contains(r#"<div class="actions" data-idle hidden>"#), "{html}");
        assert!(html.contains(r#"<div class="sign-in" data-signing>"#), "{html}");
        assert!(html.contains(r#"<output class="device-code" aria-live="polite">ABCD-1234</output>"#), "{html}");
        assert!(html.contains(r#"<button type="button" class="copy-open" data-url="https://github.com/login/device">Copy code and open GitHub</button>"#), "{html}");
        assert!(html.contains(r#"<span class="expiry" data-expires-in="299">Expires in 4:59</span>"#), "{html}");
        assert!(html.contains(r#"<form method="post" action="/tok/portals" data-cancel>"#) && html.contains(r#"value="cancel""#), "{html}");
    }

    /// Before the code exists the panel already has its final shape, so the code arrives without
    /// anything moving.
    #[test]
    fn before_the_code_the_panel_already_has_its_final_shape() {
        let html = list(&[card("github", PortalKind::GitHub, AuthStatus::SigningIn(None))], "n");
        assert!(html.contains(r#"<output class="device-code pending" aria-live="polite">····-····</output>"#), "{html}");
        assert!(html.contains(r#"<button type="button" class="copy-open" disabled>Copy code and open GitHub</button>"#), "{html}");
        assert!(html.contains("Getting a code from GitHub…"), "{html}");
    }

    /// An address from the network is a link only under the portal's own prefix; otherwise the
    /// button still copies, and the address is shown as text to go to by hand.
    #[test]
    fn an_address_outside_the_portal_is_text_not_a_link() {
        let html = list(&[card("github", PortalKind::GitHub, AuthStatus::SigningIn(Some(prompt("https://evil.example/"))))], "n");
        assert!(!html.contains("data-url="), "{html}");
        assert!(html.contains("https://evil.example/"), "shown, as text");
        assert!(html.contains(">Copy code</button>"), "{html}");
    }

    #[test]
    fn gitlab_says_its_code_form_comes_back_after_authorizing() {
        let html = list(&[card("gitlab", PortalKind::GitLab, AuthStatus::NotSignedIn)], "n");
        assert!(html.contains("GitLab shows its code form again"), "{html}");
        let html = list(&[card("github", PortalKind::GitHub, AuthStatus::NotSignedIn)], "n");
        assert!(!html.contains("code form again"));
    }

    /// A sign-in that ended badly says so on the card, once, beside a fresh Sign in button.
    #[test]
    fn a_failed_sign_in_says_what_happened_beside_sign_in() {
        let mut c = card("gitlab", PortalKind::GitLab, AuthStatus::NotSignedIn);
        c.error = Some("The code expired before you authorized. Sign in again for a new one.".into());
        let html = list(&[c], "n");
        assert!(html.contains(r#"<p class="sign-in-error" role="alert">The code expired before you authorized. Sign in again for a new one.</p>"#), "{html}");
        assert!(html.contains(">Sign in to GitLab</button>"));
    }

    #[test]
    fn everything_from_outside_is_escaped() {
        let mut c = card("github", PortalKind::GitHub, AuthStatus::SigningIn(Some(SignInPrompt { code: "<b>".into(), url: "javascript:x".into(), expires_at: NOW + 60 })));
        c.error = Some("<i>".into());
        let html = list(&[c], "n");
        assert!(html.contains("&lt;b&gt;") && !html.contains("<b>") && !html.contains("<i>"));
        assert!(!html.contains("href=\"javascript") && !html.contains("data-url="));
        let html = list(&[card("github", PortalKind::GitHub, AuthStatus::Off("<b>x</b>".into()))], "n");
        assert!(html.contains("&lt;b&gt;x&lt;/b&gt;") && !html.contains("<b>x</b>"));
    }

    // ── The script and the pages ──────────────────────────────────────────────

    /// One script drives every card on the page, named by the nonce, and only with a nonce.
    #[test]
    fn one_script_drives_the_cards_and_only_with_a_nonce() {
        let cards = [card("github", PortalKind::GitHub, AuthStatus::SignedIn), card("gitlab", PortalKind::GitLab, AuthStatus::NotSignedIn)];
        let html = list(&cards, "n");
        assert_eq!(html.matches("<script nonce=\"n\">").count(), 1, "{html}");
        assert!(html.contains("data-signin") && html.contains("?state=1"));
        assert!(!list(&cards, "").contains("<script"));
        assert!(!html.contains("http-equiv=\"refresh\""), "nothing reloads on its own");
    }

    /// Without the script, a card caught mid sign-in still gets there by reloading.
    #[test]
    fn without_the_script_a_sign_in_in_flight_reloads() {
        let html = list(&[card("github", PortalKind::GitHub, AuthStatus::SigningIn(None))], "");
        assert!(html.contains("http-equiv=\"refresh\""), "{html}");
    }

    /// The portal's own page has the same card, without Settings (it is the settings), and the
    /// settings sections below it only once signed in and not mid sign-in.
    #[test]
    fn the_portal_page_has_the_same_card_and_settings_only_when_signed_in() {
        let html = page(card("github", PortalKind::GitHub, AuthStatus::SignedIn));
        assert!(html.contains(r#"class="card portal-card""#) && !html.contains("GitHub settings"), "{html}");
        assert!(!html.contains("<strong>GitHub</strong>"), "the page title already names it: {html}");
        assert!(html.contains(r#"name="statusComponents""#), "{html}");
        assert!(!html.contains(r#"name="copilotReviews""#), "Copilot's rule lives on General's green-bar line now");
        for name in crate::config::all_components() {
            assert!(html.contains(&format!(r#"value="{name}""#)), "{name} is not offered");
        }
        assert!(!page(card("github", PortalKind::GitHub, AuthStatus::NotSignedIn)).contains("name=\"section\""));
        assert!(!page(card("github", PortalKind::GitHub, AuthStatus::SigningIn(None))).contains("name=\"section\""));
    }

    #[test]
    fn each_sign_in_state_has_one_word_for_the_background_check() {
        assert_eq!(auth_state(&AuthStatus::SignedIn), "signed_in");
        assert_eq!(auth_state(&AuthStatus::NotSignedIn), "not_signed_in");
        assert_eq!(auth_state(&AuthStatus::SigningIn(None)), "signing_in");
        assert_eq!(auth_state(&AuthStatus::Off("x".into())), "off");
    }

    /// What the background check reads: state, code, lifetime, a link only under the prefix, and
    /// why the last sign-in failed.
    #[test]
    fn the_background_answer_carries_the_code_a_safe_link_and_the_failure() {
        let j: serde_json::Value = serde_json::from_str(&state_json(
            &AuthStatus::SigningIn(Some(prompt("https://github.com/login/device"))),
            "https://github.com/",
            None,
            NOW,
        ))
        .unwrap();
        assert_eq!(j["state"], "signing_in");
        assert_eq!(j["code"], "ABCD-1234");
        assert_eq!(j["url"], "https://github.com/login/device");
        assert_eq!(j["expires_in"], 299);
        let j: serde_json::Value =
            serde_json::from_str(&state_json(&AuthStatus::SigningIn(Some(prompt("https://evil.example/"))), "https://github.com/", None, NOW)).unwrap();
        assert!(j["url"].is_null());
        assert_eq!(j["url_text"], "https://evil.example/");
        let j: serde_json::Value = serde_json::from_str(&state_json(&AuthStatus::NotSignedIn, "https://github.com/", Some("expired"), NOW)).unwrap();
        assert_eq!(j, serde_json::json!({ "state": "not_signed_in", "error": "expired" }));
    }
}
