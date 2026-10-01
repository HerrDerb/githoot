//! Settings as declarations: what a setting is called, what it holds, and where it sits on a page.
//!
//! Everything the settings pages can change is declared here in one shape, whoever owns it: the core
//! (`config`), a portal, an integration. One renderer draws every declaration and one reader turns a
//! submitted form back into values, so adding a setting is a declaration, not a new form, a new route
//! and a new parser that each have to get the same details right.
//!
//! What a declaration does **not** decide is where its value lives. `config` stores the core's keys
//! flat and an integration's under `integration.<id>.`; both write through the same one-line edit, and
//! both check a value against its declaration first.

/// One setting, as a page shows it and a form submits it.
pub struct Setting {
    /// The key as the form names it. For the core that is the `config.txt` key; for an integration
    /// it is the part after `integration.<id>.`.
    pub key: &'static str,
    /// What the checkbox, box or list is labelled.
    pub label: &'static str,
    pub kind: Kind,
    /// One sentence: under the control on the page, and above the key in `config.txt`.
    pub help: &'static str,
    /// The section it belongs to on its page. Consecutive settings with the same group share one card
    /// and one Save, which is how a page is cut into sections without a second list to keep in step.
    pub group: &'static str,
    /// Whether it takes effect without a restart. The page says which ones need one, after a save.
    pub live: bool,
}

/// What a setting holds.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    /// Free text on one line. Empty means the setting's default, which `placeholder` shows greyed.
    /// Nothing ships one since the dispatcher's two roots became folders; kept, and drawn, for the
    /// next setting that is text.
    #[cfg_attr(not(test), allow(dead_code))]
    Text { placeholder: &'static str },
    /// A folder: text, with a Browse button that asks GitHoot for the native folder picker and fills
    /// the box with what was chosen. Stored, checked and read exactly like `Text`.
    Folder { placeholder: &'static str },
    /// `on` or `off`, a checkbox. Only an explicit value moves it off its default, so a typo leaves the
    /// default standing in either direction.
    Flag { default_on: bool },
    /// A flag drawn as a step on a rule line, with what happens to a pull request that fails it
    /// (`otherwise`: "else amber", "else waits"). Stored, checked and read exactly like `Flag`.
    Step { default_on: bool, otherwise: &'static str },
    /// Exactly one of `options`, as radio buttons: `(value, label)`.
    Choice { options: &'static [(&'static str, &'static str)] },
    /// A subset of `options` where empty means "all of it", stored comma-separated, shown as a choice
    /// between `whole` and `some` with the boxes under `some`. An unticked list that silently meant
    /// everything was a rule you could only learn from the help text.
    Parts { options: &'static [&'static str], whole: &'static str, some: &'static str },
}

/// Whether `value` is one `setting` may hold. The last check before a line is written into a file the
/// user owns, so it is strict: a flag is `on` or `off`, a choice one of its values, a list only names
/// it offers, and nothing may break the line.
pub fn check(setting: &Setting, value: &str) -> Result<(), String> {
    if value.contains(['\n', '\r']) {
        return Err(format!("{} must stay on one line", setting.key));
    }
    let ok = match setting.kind {
        Kind::Text { .. } | Kind::Folder { .. } => true,
        Kind::Flag { .. } | Kind::Step { .. } => matches!(value, "on" | "off"),
        Kind::Choice { options } => options.iter().any(|(v, _)| *v == value),
        Kind::Parts { options, .. } => split(value).all(|part| options.contains(&part)),
    };
    if ok { Ok(()) } else { Err(format!("{:?} is not a value {} can hold", value, setting.key)) }
}

/// A list setting's parts, trimmed, with empties dropped.
pub fn split(value: &str) -> impl Iterator<Item = &str> {
    value.split(',').map(str::trim).filter(|p| !p.is_empty())
}

/// A flag's answer from the text the file holds, its default when that is empty or a typo.
pub fn flag(kind: Kind, value: &str) -> bool {
    match kind {
        Kind::Flag { default_on: true } | Kind::Step { default_on: true, .. } => !crate::config::is_off(value),
        Kind::Flag { default_on: false } | Kind::Step { default_on: false, .. } => crate::config::is_on(value),
        _ => false,
    }
}

/// What a submitted form says for each of `settings`, in declaration order.
///
/// A checkbox posts nothing when unticked, which is how a form says off, so every flag gets an answer,
/// and a list with nothing ticked is an empty list. A text box or a choice absent from the form is left
/// alone; a text box submitted empty is a deliberate clear. Only names a list offers survive, so a
/// hand-made post cannot write one that would never match.
pub fn from_form(settings: &[&Setting], form: &crate::serve::Form) -> Vec<(&'static str, String)> {
    settings
        .iter()
        .filter_map(|s| match s.kind {
            Kind::Flag { .. } | Kind::Step { .. } => Some((s.key, if form.ticked(s.key) { "on" } else { "off" }.to_string())),
            // "The whole thing" chosen saves the empty list, whatever boxes are still ticked under
            // the other choice. With no choice posted the boxes decide, as a plain list does.
            Kind::Parts { options, .. } => {
                if form.get(&format!("{}.scope", s.key)).is_some_and(|v| v == "all") {
                    return Some((s.key, String::new()));
                }
                let ticked: Vec<&str> = form.all(s.key).into_iter().filter(|v| options.contains(v)).collect();
                Some((s.key, ticked.join(", ")))
            }
            Kind::Text { .. } | Kind::Folder { .. } | Kind::Choice { .. } => form.get(s.key).map(|v| (s.key, v.trim().to_string())),
        })
        .collect()
}

/// The sections of a page: consecutive settings with the same `group`, in order.
pub fn groups(settings: &'static [Setting]) -> Vec<(&'static str, Vec<&'static Setting>)> {
    let mut out: Vec<(&'static str, Vec<&'static Setting>)> = Vec::new();
    for setting in settings {
        match out.last_mut() {
            Some((group, members)) if *group == setting.group => members.push(setting),
            _ => out.push((setting.group, vec![setting])),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const fn s(key: &'static str, kind: Kind, group: &'static str) -> Setting {
        Setting { key, label: key, kind, help: "", group, live: true }
    }

    static ALL: [Setting; 5] = [
        s("loud", Kind::Flag { default_on: true }, "Noise"),
        s("quiet", Kind::Flag { default_on: false }, "Noise"),
        s("root", Kind::Text { placeholder: "~" }, "Paths"),
        s("level", Kind::Choice { options: &[("error", "Errors"), ("info", "Everything")] }, "Log"),
        s("parts", Kind::Parts { options: &["API", "Actions", "Pull Requests"], whole: "All", some: "Some" }, "Log"),
    ];

    fn form(body: &str) -> crate::serve::Form {
        crate::serve::parse_form(body)
    }

    #[test]
    fn a_page_is_cut_into_sections_by_consecutive_group() {
        let cut: Vec<(&str, Vec<&str>)> = groups(&ALL).into_iter().map(|(g, v)| (g, v.iter().map(|s| s.key).collect())).collect();
        assert_eq!(cut, [("Noise", vec!["loud", "quiet"]), ("Paths", vec!["root"]), ("Log", vec!["level", "parts"])]);
    }

    #[test]
    fn a_flag_is_its_default_until_the_file_says_otherwise() {
        let on = Kind::Flag { default_on: true };
        let off = Kind::Flag { default_on: false };
        assert!(flag(on, "") && !flag(off, ""));
        assert!(!flag(on, "off") && flag(off, "on"));
        assert!(flag(on, "offf") && !flag(off, "onn"), "a typo leaves the default");
    }

    #[test]
    fn only_values_a_setting_may_hold_pass() {
        let [loud, _, root, level, parts] = &ALL;
        assert!(check(loud, "on").is_ok() && check(loud, "off").is_ok() && check(loud, "yes").is_err());
        assert!(check(root, "/src").is_ok() && check(root, "").is_ok() && check(root, "a\nb=c").is_err());
        assert!(check(level, "info").is_ok() && check(level, "debug").is_err());
        assert!(check(parts, "API, Pull Requests").is_ok() && check(parts, "").is_ok());
        assert!(check(parts, "API, Everything").is_err(), "only names the list offers");
    }

    /// Unticked is off, nothing ticked is an empty list, and an absent text box is left alone.
    #[test]
    fn a_form_answers_for_every_flag_and_list_and_only_present_text() {
        let all: Vec<&Setting> = ALL.iter().collect();
        let got = from_form(&all, &form("loud=on&level=info&parts=API&parts=Bogus&parts=Pull+Requests"));
        assert_eq!(
            got,
            [
                ("loud", "on".to_string()),
                ("quiet", "off".to_string()),
                ("level", "info".to_string()),
                ("parts", "API, Pull Requests".to_string()),
            ]
        );
        let got = from_form(&all, &form("root=+%2Fsrc+"));
        assert!(got.contains(&("root", "/src".to_string())) && got.contains(&("parts", String::new())));
    }

    /// A folder is text with a Browse button: stored, checked and read off a form exactly as text is.
    #[test]
    fn a_folder_is_stored_and_read_like_text() {
        static FOLDER: Setting = s("root", Kind::Folder { placeholder: "~/src" }, "Paths");
        assert!(check(&FOLDER, r"D:\projects").is_ok() && check(&FOLDER, "").is_ok() && check(&FOLDER, "a\nb=c").is_err());
        assert_eq!(from_form(&[&FOLDER], &form("root=+D%3A%5Cprojects+")), [("root", r"D:\projects".to_string())]);
        assert_eq!(from_form(&[&FOLDER], &form("other=x")), [], "absent is left alone");
    }

    static SCOPED: Setting = Setting {
        key: "parts",
        label: "Parts",
        kind: Kind::Parts { options: &["API", "Actions"], whole: "Everything", some: "Only these" },
        help: "",
        group: "G",
        live: false,
    };

    /// "The whole thing" is an explicit choice now, not an unticked list. Choosing it saves the empty
    /// list, whatever boxes are still ticked; "Only these" saves what is ticked.
    #[test]
    fn a_parts_list_saves_the_whole_or_only_what_is_ticked() {
        let one = [&SCOPED];
        assert_eq!(from_form(&one, &form("parts.scope=all&parts=API")), [("parts", String::new())]);
        assert_eq!(from_form(&one, &form("parts.scope=some&parts=API&parts=Actions")), [("parts", "API, Actions".to_string())]);
        assert_eq!(from_form(&one, &form("parts=API")), [("parts", "API".to_string())], "no scope posted: the boxes decide, as before");
        assert!(check(&SCOPED, "API").is_ok() && check(&SCOPED, "").is_ok() && check(&SCOPED, "Bogus").is_err());
    }
}
