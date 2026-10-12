# Mail Scrub

Keep Gmail organised without clicking through Gmail. A rule-based Rust CLI that labels mail by sender and type, writes exact Gmail filters, and learns from your corrections. No LLM.

Mail Scrub sorts your Gmail along two axes you actually think in: who it's from (Finance, Shopping, Travel, …) and what kind of mail it is (Receipt, Shipping, Security, Newsletter, …). Run it from cron or launchd. Each Run reads message metadata only and applies one `From/` label and one `Type/` label per message, using a fixed, explainable chain of sources and signals. Every decision can be explained as "because of X".

It then writes Gmail filters for every decision Gmail can reproduce exactly, so most new mail is labelled between Runs without the tool. Anything it can't decide is left alone and asked about in a short, capped review, and your answers and hand relabels become overrides. Each Run asks less than the last.

## Usage

```sh
cargo run -- run             # one Run against the in-memory fake
cargo run -- run --dry-run   # record a Run without advancing the cursor
cargo run -- run --full      # ignore the stored cursor
```

Global flag: `--config <path>`.

| File | Location |
| --- | --- |
| Config | `$XDG_CONFIG_HOME/mailscrub/mailscrub.toml` (else `~/.config/mailscrub/mailscrub.toml`); override with `--config` |
| Database | `$XDG_DATA_HOME/mailscrub/mailscrub.db` (else `~/.local/share/mailscrub/mailscrub.db`) |

A missing config at the default path means built-in defaults. An explicit `--config` that doesn't exist is an error.

## Config

Hand-edited TOML. Every key is optional, an empty file is valid, and unknown keys are rejected so typos fail loudly.

```toml
[gmail]
client_id = "..."          # from your own Google Cloud project

[sources]
cloudflare_account_id = "" # empty = Cloudflare source disabled
cloudflare_token_env  = "MAILSCRUB_CF_TOKEN"
ut1_path = "~/.local/share/mailscrub/ut1"

[review]
max_questions = 20
promote_after_pins = 2

[taxonomy]
sender = ["Finance", "Shopping", "Travel", "Software", "Social", "News",
          "Health", "Government", "Utilities", "Education", "Work", "People"]
type   = ["Receipt", "Statement", "Shipping", "Security", "Account",
          "Promotion", "Newsletter", "Notification", "Calendar", "Conversation"]

# Sender category Overrides: Sender domain -> category
[overrides.sender]
"paypal.com" = "Finance"

# Email type Overrides, keyed by sender address or subject pattern
[[overrides.type]]
address = "ship-confirm@amazon.com"
type    = "Shipping"

# External source category -> our Taxonomy
[map.ut1]
bank = "Finance"
```
