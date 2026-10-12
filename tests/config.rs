use mail_scrub::config::{Config, TypeOverride};

#[test]
fn an_empty_file_gives_the_default_taxonomy_and_empty_overrides() {
    let config: Config = "".parse().unwrap();

    assert_eq!(
        config.taxonomy.sender_categories,
        [
            "Finance",
            "Shopping",
            "Travel",
            "Software",
            "Social",
            "News",
            "Health",
            "Government",
            "Utilities",
            "Education",
            "Work",
            "People"
        ]
    );
    assert_eq!(
        config.taxonomy.email_types,
        [
            "Receipt",
            "Statement",
            "Shipping",
            "Security",
            "Account",
            "Promotion",
            "Newsletter",
            "Notification",
            "Calendar",
            "Conversation"
        ]
    );
    assert!(config.overrides.sender.is_empty());
    assert!(config.overrides.email_types.is_empty());
    assert_eq!(config.review.max_questions, 20);
    assert_eq!(config.review.promote_after_pins, 2);
}

#[test]
fn reads_the_schema_note_example() {
    let config: Config = r#"
        [gmail]
        client_id = "abc.apps.googleusercontent.com"

        [sources]
        cloudflare_account_id = ""
        cloudflare_token_env  = "MAILSCRUB_CF_TOKEN"
        ut1_path = "~/.local/share/mailscrub/ut1"

        [review]
        max_questions = 10
        promote_after_pins = 3

        [taxonomy]
        sender = ["Finance", "People"]
        type   = ["Receipt"]

        [overrides.sender]
        "paypal.com" = "Finance"

        [[overrides.type]]
        address = "ship-confirm@amazon.com"
        type    = "Shipping"

        [[overrides.type]]
        subject = "^Your order #\\d+"
        type    = "Receipt"

        [map.ut1]
        bank = "Finance"
    "#
    .parse()
    .unwrap();

    assert_eq!(
        config.gmail.client_id.as_deref(),
        Some("abc.apps.googleusercontent.com")
    );
    assert_eq!(config.review.max_questions, 10);
    assert_eq!(config.taxonomy.sender_categories, ["Finance", "People"]);
    assert_eq!(config.taxonomy.email_types, ["Receipt"]);
    assert_eq!(config.overrides.sender["paypal.com"], "Finance");
    assert_eq!(
        config.overrides.email_types,
        [
            TypeOverride {
                address: Some("ship-confirm@amazon.com".to_owned()),
                subject: None,
                email_type: "Shipping".to_owned(),
            },
            TypeOverride {
                address: None,
                subject: Some(r"^Your order #\d+".to_owned()),
                email_type: "Receipt".to_owned(),
            },
        ]
    );
    assert_eq!(config.map["ut1"]["bank"], "Finance");
}

#[test]
fn a_misspelt_section_is_an_error_not_silently_ignored() {
    let err = "[overides.sender]\n\"paypal.com\" = \"Finance\"\n"
        .parse::<Config>()
        .unwrap_err();

    assert!(err.to_string().contains("overides"), "{err}");
}

#[test]
fn load_reads_a_file_and_reports_a_missing_one() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("mailscrub.toml");
    std::fs::write(&path, "[overrides.sender]\n\"notion.so\" = \"Software\"\n").unwrap();

    assert_eq!(
        Config::load(&path).unwrap().overrides.sender["notion.so"],
        "Software"
    );
    let err = Config::load(&dir.path().join("missing.toml")).unwrap_err();
    assert!(err.to_string().contains("missing.toml"), "{err}");
}
