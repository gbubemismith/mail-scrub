//! Classification as a pure function: metadata and config in, Decision out.

use mail_scrub::classifier::{Signal, Source, classify, sender_host};
use mail_scrub::client::MessageMeta;
use mail_scrub::config::Config;

const OVERRIDES: &str = r#"
[overrides.sender]
"chase.com" = "Finance"
"co.uk" = "Government"

[[overrides.type]]
address = "alerts@chase.com"
type = "Security"

[[overrides.type]]
address = "alerts@chase.com"
type = "Account"
"#;

fn meta(address: &str) -> MessageMeta {
    MessageMeta {
        id: "m1".to_owned(),
        thread_id: "t1".to_owned(),
        sender_address: address.to_owned(),
        subject: None,
        internal_date: 0,
        label_ids: Vec::new(),
    }
}

#[test]
fn sender_category_comes_from_the_nearest_domain_override() {
    let config: Config = OVERRIDES.parse().unwrap();
    let cases = [
        ("someone@chase.com", Some("Finance")),
        ("someone@email.chase.com", Some("Finance")),
        ("someone@a.b.email.chase.com", Some("Finance")),
        ("someone@CHASE.COM", Some("Finance")),
        ("someone@notchase.com", None),
        ("someone@chase.com.evil.org", None),
        ("someone@example.com", None),
        ("no-at-sign", None),
        ("someone@", None),
    ];

    for (address, expected) in cases {
        let got = classify(&config, &meta(address))
            .sender_category
            .map(|(category, source)| {
                assert_eq!(source, Source::Override, "{address}");
                category
            });
        assert_eq!(got.as_deref(), expected, "{address}");
    }
}

#[test]
fn email_type_comes_from_the_first_matching_address_override() {
    let config: Config = OVERRIDES.parse().unwrap();
    let cases = [
        ("alerts@chase.com", Some(("Security", "alerts@chase.com"))),
        ("Alerts@Chase.com", Some(("Security", "alerts@chase.com"))),
        ("alerts@email.chase.com", None),
        ("other@chase.com", None),
    ];

    for (address, expected) in cases {
        let got = classify(&config, &meta(address)).email_type;
        let got = got.as_ref().map(|t| {
            assert_eq!(t.signal, Signal::Address, "{address}");
            (t.email_type.as_str(), t.signal_key.as_str())
        });
        assert_eq!(got, expected, "{address}");
    }
}

#[test]
fn a_message_is_unknown_unless_both_dimensions_are_decided() {
    let config: Config = OVERRIDES.parse().unwrap();
    let cases = [
        ("alerts@chase.com", false),
        ("other@chase.com", true),
        ("alerts@email.chase.com", true),
        ("someone@example.com", true),
    ];

    for (address, unknown) in cases {
        assert_eq!(
            classify(&config, &meta(address)).is_unknown(),
            unknown,
            "{address}"
        );
    }
}

#[test]
fn sender_host_is_the_lowercased_part_after_the_last_at() {
    let cases = [
        ("Billing@Email.Chase.com", "email.chase.com"),
        ("a@b@c.com", "c.com"),
        ("no-at-sign", "no-at-sign"),
        ("someone@", ""),
    ];

    for (address, expected) in cases {
        assert_eq!(sender_host(address), expected, "{address}");
    }
}
