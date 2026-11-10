//! URL policy, web URLs and webhook signatures.

use stickpile_gitpack::{redact_url, validate_url, verify_hub_signature, verify_token, web_url, GitPackError};

fn schemes(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| s.to_string()).collect()
}

#[test]
fn url_policy_table() {
    let all = schemes(&["https", "http", "ssh", "file"]);
    let default = schemes(&["https", "ssh"]);
    let https_only = schemes(&["https"]);

    let ok: &[(&str, &[String])] = &[
        ("https://github.com/owner/repo.git", &default),
        ("https://github.com/owner/repo", &https_only),
        ("HTTPS://codeberg.org/owner/repo", &default),
        ("https://git.example.org:8443/a/b.git", &default),
        ("https://[2001:db8::1]/a/b.git", &default),
        ("http://gitea.lan/a/b.git", &all),
        ("ssh://git@github.com/owner/repo.git", &default),
        ("ssh://git@host.example:2222/owner/repo.git", &default),
        ("git@github.com:owner/repo.git", &default),
        ("github.com:owner/repo.git", &default),
        ("file:///srv/git/pack.git", &all),
        ("file://localhost/srv/git/pack.git", &all),
    ];
    for (url, allowed) in ok {
        assert!(
            validate_url(url, allowed).is_ok(),
            "{url} should be allowed: {:?}",
            validate_url(url, allowed)
        );
    }

    let bad: &[(&str, &[String])] = &[
        ("", &all),
        ("https://user:pass@github.com/a/b.git", &all),
        ("https://token@github.com/a/b.git", &all),
        ("ssh://git:secret@github.com/a/b.git", &all),
        ("ext::sh -c touch% /tmp/pwned", &all),
        ("ext::ssh", &all),
        ("fd::17", &all),
        ("/srv/git/pack.git", &all),
        ("./pack", &all),
        ("../pack", &all),
        ("~/pack", &all),
        ("pack", &all),
        ("C:\\repos\\pack", &all),
        ("C:/repos/pack", &all),
        ("-oProxyCommand=touch /tmp/x", &all),
        ("--upload-pack=touch /tmp/x", &all),
        ("ssh://-oProxyCommand=x/a/b", &all),
        ("ssh://-user@host/a/b", &all),
        ("-git@host:a/b", &all),
        ("git@-host:a/b", &all),
        ("git@host:-a/b", &all),
        ("git@host:", &all),
        ("user:pass@host:a/b", &all),
        ("http://gitea.lan/a/b.git", &default),
        ("file:///srv/git/pack.git", &default),
        ("file://otherhost/srv/pack.git", &all),
        ("git://example.org/a/b", &all),
        ("ftp://example.org/a/b", &all),
        ("https://github.com/a b", &all),
        ("https://github.com/a\nb", &all),
        ("git@github.com:owner/repo.git", &https_only),
    ];
    for (url, allowed) in bad {
        assert!(
            matches!(validate_url(url, allowed), Err(GitPackError::Url(_))),
            "{url:?} should be rejected"
        );
    }

    let long = format!("https://github.com/{}", "a".repeat(3000));
    assert!(validate_url(&long, &all).is_err());
}

#[test]
fn url_errors_do_not_leak_credentials() {
    let err = validate_url("https://user:hunter2@github.com/a/b", &schemes(&["https"])).unwrap_err();
    assert!(!err.to_string().contains("hunter2"), "{err}");
    assert_eq!(redact_url("https://user:pw@host/a"), "https://***@host/a");
    assert_eq!(redact_url("https://host/a@b"), "https://host/a@b");
    assert_eq!(redact_url("git@host:a/b"), "git@host:a/b");
}

#[test]
fn web_url_table() {
    // (remote URL, branch, subdir, expected web URL)
    type Case<'a> = (&'a str, Option<&'a str>, Option<&'a str>, Option<&'a str>);
    let cases: &[Case] = &[
        (
            "https://github.com/owner/repo.git",
            None,
            None,
            Some("https://github.com/owner/repo"),
        ),
        (
            "https://github.com/owner/repo",
            Some("main"),
            None,
            Some("https://github.com/owner/repo/tree/main"),
        ),
        (
            "https://github.com/owner/repo.git",
            Some("main"),
            Some("packs/cats"),
            Some("https://github.com/owner/repo/tree/main/packs/cats"),
        ),
        (
            "https://github.com/owner/repo/",
            None,
            Some("cats/"),
            Some("https://github.com/owner/repo/tree/HEAD/cats"),
        ),
        (
            "git@github.com:owner/repo.git",
            Some("dev"),
            None,
            Some("https://github.com/owner/repo/tree/dev"),
        ),
        (
            "ssh://git@github.com:22/owner/repo.git",
            Some("feature/x"),
            Some("a b"),
            Some("https://github.com/owner/repo/tree/feature/x/a%20b"),
        ),
        (
            "https://gitlab.com/group/sub/repo.git",
            Some("main"),
            Some("cats"),
            Some("https://gitlab.com/group/sub/repo/-/tree/main/cats"),
        ),
        (
            "git@gitlab.com:group/repo.git",
            None,
            None,
            Some("https://gitlab.com/group/repo"),
        ),
        (
            "git@gitlab.example.org:g/r.git",
            None,
            Some("x"),
            Some("https://gitlab.example.org/g/r/-/tree/HEAD/x"),
        ),
        (
            "https://codeberg.org/owner/repo.git",
            Some("main"),
            Some("cats"),
            Some("https://codeberg.org/owner/repo/src/branch/main/cats"),
        ),
        (
            "https://codeberg.org/owner/repo.git",
            None,
            Some("cats"),
            Some("https://codeberg.org/owner/repo/src/cats"),
        ),
        (
            "git@codeberg.org:owner/repo.git",
            None,
            None,
            Some("https://codeberg.org/owner/repo"),
        ),
        (
            "https://gitea.example.org/o/r",
            Some("main"),
            None,
            Some("https://gitea.example.org/o/r/src/branch/main"),
        ),
        (
            "https://forgejo.example.org/o/r",
            Some("main"),
            None,
            Some("https://forgejo.example.org/o/r/src/branch/main"),
        ),
        (
            "http://git.lan:3000/o/r.git",
            Some("main"),
            None,
            Some("http://git.lan:3000/o/r/tree/main"),
        ),
        ("file:///srv/git/pack.git", None, None, None),
        ("/srv/git/pack.git", None, None, None),
        ("https://github.com/owner", None, None, None),
        ("https://github.com/a/b/c", None, None, None),
        ("https://github.com/owner/repo", None, Some("../../etc"), None),
        ("not a url", None, None, None),
    ];
    for (url, branch, subdir, want) in cases {
        assert_eq!(
            web_url(url, *branch, *subdir).as_deref(),
            *want,
            "{url} {branch:?} {subdir:?}"
        );
    }
}

#[test]
fn hub_signature() {
    // Test vector from GitHub's webhook documentation.
    let secret = b"It's a Secret to Everybody";
    let body = b"Hello, World!";
    let good = "sha256=757107ea0eb2509fc211221cce984b8a37570b6d7586c22c46f4379c8b043e17";
    assert!(verify_hub_signature(secret, body, good));
    assert!(verify_hub_signature(
        secret,
        body,
        &good.to_uppercase().replace("SHA256=", "sha256=")
    ));
    assert!(verify_hub_signature(
        secret,
        body,
        "757107ea0eb2509fc211221cce984b8a37570b6d7586c22c46f4379c8b043e17"
    ));
    assert!(verify_hub_signature(secret, body, &format!(" {good} ")));

    assert!(!verify_hub_signature(secret, b"Hello, World?", good));
    assert!(!verify_hub_signature(b"wrong secret", body, good));
    assert!(!verify_hub_signature(b"", body, good));
    assert!(!verify_hub_signature(secret, body, "sha256="));
    assert!(!verify_hub_signature(secret, body, "sha256=zz"));
    assert!(!verify_hub_signature(
        secret,
        body,
        "sha1=757107ea0eb2509fc211221cce984b8a37570b6d7586c22c46f4379c8b043e17"
    ));
    assert!(!verify_hub_signature(secret, body, &good[..good.len() - 2]));
    assert!(!verify_hub_signature(secret, body, ""));
}

#[test]
fn shared_token() {
    assert!(verify_token(b"s3cret", "s3cret"));
    assert!(verify_token(b"s3cret", " s3cret "));
    assert!(!verify_token(b"s3cret", "s3cre"));
    assert!(!verify_token(b"s3cret", "s3cret!"));
    assert!(!verify_token(b"", ""));
}
