# Mail Scrub

Keep Gmail organised without clicking through Gmail. A rule-based Rust CLI that labels mail by sender and type, writes exact Gmail filters, and learns from your corrections. No LLM.

Mail Scrub sorts your Gmail along two axes you actually think in: who it's from (Finance, Shopping, Travel, …) and what kind of mail it is (Receipt, Shipping, Security, Newsletter, …). Run it from cron or launchd. Each Run reads message metadata only and applies one `From/` label and one `Type/` label per message, using a fixed, explainable chain of sources and signals. Every decision can be explained as "because of X".

It then writes Gmail filters for every decision Gmail can reproduce exactly, so most new mail is labelled between Runs without the tool. Anything it can't decide is left alone and asked about in a short, capped review, and your answers and hand relabels become overrides. Each Run asks less than the last.

## Usage

```sh
mailscrub setup              # one-off: three steps to create your Google Cloud OAuth client, then sign in and store tokens in the keychain
mailscrub run                # one Run; the very first one is a dry run
mailscrub run --apply        # write labels even on the first Run
mailscrub run --dry-run      # print planned label changes, write nothing, don't advance the cursor
mailscrub run --dry-run --limit 500   # preview only the 500 newest messages
mailscrub run --full         # ignore the stored cursor
cargo run --example demo -- examples/demo.toml [--dry-run]   # seeded fake mailbox: edit the TOML, rerun, see labels
```

`setup` takes three steps, opening the Google Cloud console page for each with a one-line instruction: create a project and enable the Gmail API, set up the consent screen and publish the app, then create a "Desktop app" OAuth client and click Download JSON. It reads the client ID and secret from that downloaded file (looking in `~/Downloads`, or asking for the path), then runs the consent flow. The OAuth client secret, refresh token and access token go in the OS keychain; the config file gets only the client ID, and is created readable by you alone. You can delete the downloaded JSON afterwards. No Cloudflare account is needed.

Because the app is your own and unverified, Google shows a one-time "hasn't verified this app" warning at consent. Click Advanced, then Go to your app.

Google requires a home page and privacy policy URL before you can publish. Any `https` URL works, such as your GitHub profile; add its domain under Authorized domains. Don't upload a logo, which triggers Google's verification review.

The app is published, so the refresh token persists. You only need to consent again if you revoke access or the token goes unused for six months; then an interactive `run` reopens the browser, and a `run` from cron or launchd exits with a message saying so.

Global flag: `--config <path>`.

| File | Location |
| --- | --- |
| Config | `$XDG_CONFIG_HOME/mailscrub/mailscrub.toml` (else `~/.config/mailscrub/mailscrub.toml`); override with `--config` |
| Database | `$XDG_DATA_HOME/mailscrub/mailscrub.db` (else `~/.local/share/mailscrub/mailscrub.db`); no flag, move it with `$XDG_DATA_HOME` |

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
