//! Who the settings page suggests while a login is typed into Ignored authors.
//!
//! **People you work with first.** The App cannot list an organisation's members: that takes the
//! *Members* permission, and adding one makes every installation, organisation owners included,
//! approve the App again, for an autocomplete. What it can read is the repositories it is installed
//! on, and each one's `mentionableUsers`, the people GitHub itself would offer after an `@` there. So
//! the first group is those people, on the repositories of your recent pull requests, labelled with
//! the repository's owner. Then well-known bots, since neither source ever returns one: GitHub's user
//! search skips bot accounts and `mentionableUsers` lists none, so a bot taken off the list could not
//! be found again. Last, GitHub's user search, minus anyone already shown.
//!
//! **Read-only credential.** The page asks on the server's thread, not the poll thread that owns the
//! credential. A refresh from here could race the poll's own refresh, and a GitHub App's refresh token
//! is single-use, so the loser would be signed out. Only a saved token that is still valid is used;
//! otherwise there are no suggestions, and typing a login by hand still works.

use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};

/// How many of each group the dropdown shows.
const PER_GROUP: usize = 8;

/// How many of your pull requests the first group's repositories come from.
const RECENT_PRS: u32 = 30;

/// One suggestion.
#[derive(Clone, PartialEq, Eq, Debug, Serialize)]
pub struct Person {
    /// As search's `-author:` reads it: `octocat`, or `app/dependabot` for a bot.
    pub login: String,
    /// The display name, when the account has one.
    pub name: Option<String>,
    /// Where they were found: the repository owner for the first group, empty for the second.
    pub org: Option<String>,
}

/// The groups, in the order the dropdown draws them (and the JSON lists them).
#[derive(Clone, PartialEq, Eq, Debug, Default, Serialize)]
pub struct People {
    pub near: Vec<Person>,
    pub bots: Vec<Person>,
    pub everyone: Vec<Person>,
}

/// GitHub Apps that open pull requests, as `(slug, name)`. Search's `app/<slug>` reads them. Short on
/// purpose: these are the ones a review request commonly comes from, and anything else can be typed.
const KNOWN_BOTS: [(&str, &str); 7] = [
    ("dependabot", "Dependabot"),
    ("renovate", "Renovate"),
    ("github-actions", "GitHub Actions"),
    ("pre-commit-ci", "pre-commit.ci"),
    ("mergify", "Mergify"),
    ("imgbot", "ImgBot"),
    ("allcontributors", "All Contributors"),
];

/// The well-known bots whose slug or name contains what was typed, `app/` and case aside.
pub fn bots(typed: &str) -> Vec<Person> {
    let typed = typed.trim().to_ascii_lowercase();
    let typed = typed.strip_prefix("app/").unwrap_or(&typed);
    KNOWN_BOTS
        .iter()
        .filter(|(slug, name)| slug.contains(typed) || name.to_ascii_lowercase().contains(typed))
        .map(|(slug, name)| Person { login: format!("app/{slug}"), name: Some(name.to_string()), org: None })
        .collect()
}

/// The first group's document: you, and the people mentionable on the repositories of your recent
/// pull requests, filtered by GitHub on what was typed.
const NEAR_DOCUMENT: &str = "query($q: String!, $prs: Int!, $per: Int!) { viewer { login } \
search(query: \"is:pr involves:@me\", type: ISSUE, first: $prs) { nodes { ... on PullRequest { \
repository { owner { login } mentionableUsers(query: $q, first: $per) { nodes { login name } } } } } } }";

#[derive(Deserialize)]
struct NearBody {
    data: Option<NearData>,
}
#[derive(Deserialize)]
struct NearData {
    viewer: Viewer,
    search: NearSearch,
}
#[derive(Deserialize)]
struct Viewer {
    login: String,
}
#[derive(Deserialize)]
struct NearSearch {
    nodes: Vec<Option<NearNode>>,
}
#[derive(Deserialize)]
struct NearNode {
    repository: Option<NearRepo>,
}
#[derive(Deserialize)]
struct NearRepo {
    owner: Viewer,
    #[serde(rename = "mentionableUsers")]
    mentionable: Users,
}
#[derive(Deserialize)]
struct Users {
    nodes: Vec<Option<User>>,
}
#[derive(Deserialize)]
struct User {
    login: String,
    name: Option<String>,
}

/// The people of the first group: each once, never you, the ones whose login starts with what was
/// typed ahead of the rest.
pub fn parse_near(body: &str, typed: &str) -> Vec<Person> {
    let Some(data) = serde_json::from_str::<NearBody>(body).ok().and_then(|b| b.data) else {
        return Vec::new();
    };
    let me = data.viewer.login;
    let mut out: Vec<Person> = Vec::new();
    for repo in data.search.nodes.into_iter().flatten().filter_map(|n| n.repository) {
        for user in repo.mentionable.nodes.into_iter().flatten() {
            if user.login.eq_ignore_ascii_case(&me) || out.iter().any(|p| p.login.eq_ignore_ascii_case(&user.login)) {
                continue;
            }
            let name = user.name.filter(|n| !n.trim().is_empty());
            out.push(Person { login: user.login, name, org: Some(repo.owner.login.clone()) });
        }
    }
    let typed = typed.to_ascii_lowercase();
    out.sort_by_key(|p| {
        let login = p.login.to_ascii_lowercase();
        (!login.starts_with(&typed), login)
    });
    out
}

#[derive(Deserialize)]
struct EveryoneBody {
    items: Vec<SearchUser>,
}
#[derive(Deserialize)]
struct SearchUser {
    login: String,
    #[serde(rename = "type")]
    kind: String,
}

/// The second group, from `search/users`: people, and bots as search's `app/<name>`. Organisations
/// are left out, since an organisation never opens a pull request.
pub fn parse_everyone(body: &str) -> Vec<Person> {
    let Ok(body) = serde_json::from_str::<EveryoneBody>(body) else {
        return Vec::new();
    };
    body.items
        .into_iter()
        .filter_map(|u| match u.kind.as_str() {
            "User" => Some(u.login),
            "Bot" => Some(format!("app/{}", u.login.strip_suffix("[bot]").unwrap_or(&u.login))),
            _ => None,
        })
        .map(|login| Person { login, name: None, org: None })
        .collect()
}

/// The groups, the last without anyone the others already show, each cut to `PER_GROUP`.
pub fn merge(mut near: Vec<Person>, mut bots: Vec<Person>, everyone: Vec<Person>) -> People {
    let shown = |p: &Person| near.iter().chain(&bots).any(|n| n.login.eq_ignore_ascii_case(&p.login));
    let everyone = everyone.into_iter().filter(|p| !shown(p)).take(PER_GROUP).collect();
    near.truncate(PER_GROUP);
    bots.truncate(PER_GROUP);
    People { near, bots, everyone }
}

/// Asks GitHub for both groups. Whatever fails leaves its group empty: a suggestion list is a
/// convenience, and a login typed by hand is saved either way.
pub fn suggest(client: &Client, graphql_url: &str, user_search_url: &str, token: &str, typed: &str) -> People {
    let typed = typed.trim();
    if typed.is_empty() {
        return People::default();
    }
    let near_body = serde_json::json!({
        "query": NEAR_DOCUMENT,
        "variables": { "q": typed, "prs": RECENT_PRS, "per": PER_GROUP },
    });
    let near = text(authorized(client.post(graphql_url).json(&near_body), token)).map(|b| parse_near(&b, typed));
    let everyone = text(authorized(
        client.get(user_search_url).query(&[("q", typed), ("per_page", &PER_GROUP.to_string())]),
        token,
    ))
    .map(|b| parse_everyone(&b));
    merge(near.unwrap_or_default(), bots(typed), everyone.unwrap_or_default())
}

fn authorized(request: reqwest::blocking::RequestBuilder, token: &str) -> reqwest::blocking::RequestBuilder {
    request
        .header(reqwest::header::ACCEPT, "application/vnd.github+json")
        .header(reqwest::header::AUTHORIZATION, format!("Bearer {token}"))
        .header("X-GitHub-Api-Version", "2022-11-28")
        .header(reqwest::header::USER_AGENT, "githoot")
}

/// The body of a successful answer; anything else is logged once and suggests nobody.
fn text(request: reqwest::blocking::RequestBuilder) -> Option<String> {
    match request.send() {
        Ok(r) if r.status().is_success() => r.text().ok(),
        Ok(r) => {
            crate::infoln!("login suggestions: GitHub answered {}", r.status());
            None
        }
        Err(e) => {
            crate::infoln!("login suggestions: {e}");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn person(login: &str, name: Option<&str>, org: Option<&str>) -> Person {
        Person { login: login.into(), name: name.map(Into::into), org: org.map(Into::into) }
    }

    /// Two repositories naming the same person show them once, under the first owner; you are never
    /// suggested; and a login starting with what was typed comes before one that only contains it.
    #[test]
    fn people_you_work_with_are_listed_once_without_you_prefix_first() {
        let body = r#"{"data":{"viewer":{"login":"HerrDerb"},"search":{"nodes":[
            {"repository":{"owner":{"login":"acme"},"mentionableUsers":{"nodes":[
                {"login":"samantha","name":"Samantha Ruiz"},{"login":"HerrDerb","name":""},{"login":"marta","name":"Marta Keller"}]}}},
            {},
            {"repository":{"owner":{"login":"acme-labs"},"mentionableUsers":{"nodes":[
                {"login":"Marta","name":null},{"login":"max-ops","name":null}]}}}
        ]}}}"#;
        assert_eq!(
            parse_near(body, "ma"),
            [
                person("marta", Some("Marta Keller"), Some("acme")),
                person("max-ops", None, Some("acme-labs")),
                person("samantha", Some("Samantha Ruiz"), Some("acme")),
            ]
        );
    }

    #[test]
    fn a_broken_or_refused_answer_suggests_nobody() {
        assert!(parse_near("not json", "ma").is_empty());
        assert!(parse_near(r#"{"errors":[{"message":"nope"}]}"#, "ma").is_empty());
        assert!(parse_everyone("not json").is_empty());
    }

    /// Search names a bot `dependabot[bot]`, `-author:` wants `app/dependabot`.
    #[test]
    fn everyone_on_github_is_people_and_bots_as_search_reads_them() {
        let body = r#"{"total_count":3,"items":[
            {"login":"markus","type":"User"},{"login":"mastodon","type":"Organization"},{"login":"mabl[bot]","type":"Bot"}]}"#;
        assert_eq!(parse_everyone(body), [person("markus", None, None), person("app/mabl", None, None)]);
    }

    #[test]
    fn everyone_leaves_out_who_is_already_near_and_both_are_capped() {
        let near: Vec<Person> = (0..12).map(|i| person(&format!("near{i}"), None, Some("acme"))).collect();
        let everyone = vec![person("NEAR0", None, None), person("far", None, None)];
        let merged = merge(near, Vec::new(), everyone);
        assert_eq!(merged.near.len(), PER_GROUP);
        assert_eq!(merged.everyone, [person("far", None, None)], "near0 is already shown, whatever its case");
    }

    /// GitHub's user search never returns a bot, and `mentionableUsers` lists none, so a bot taken off
    /// the list could not be found again. The well-known ones are offered from here, matched on their
    /// login or their name, as search's `app/<name>`.
    #[test]
    fn well_known_bots_are_found_by_login_or_name() {
        let logins = |typed: &str| bots(typed).into_iter().map(|p| p.login).collect::<Vec<_>>();
        assert_eq!(logins("dep"), ["app/dependabot"]);
        assert_eq!(logins("RENO"), ["app/renovate"]);
        assert_eq!(logins("app/renovate"), ["app/renovate"], "typed the way the chip shows it");
        assert_eq!(logins("actions"), ["app/github-actions"]);
        assert!(logins("octocat").is_empty());
        assert_eq!(bots("dep")[0].name.as_deref(), Some("Dependabot"));
    }

    /// Bots sit between the people you work with and everyone else.
    #[test]
    fn bots_are_their_own_group_between_near_and_everyone() {
        let merged = merge(vec![person("marta", None, Some("acme"))], bots("dep"), vec![person("dependabot-bot", None, None)]);
        assert_eq!(merged.bots.iter().map(|p| p.login.as_str()).collect::<Vec<_>>(), ["app/dependabot"]);
        let json = serde_json::to_string(&merged).expect("serialises");
        let at = |k: &str| json.find(k).expect(k);
        assert!(at("\"near\"") < at("\"bots\"") && at("\"bots\"") < at("\"everyone\""), "{json}");
    }
}
