# cocoon

A **time-locked pastebin** with two frontends: a Telegram bot and a web page.
Submit content with an optional publish time; it stays private until then, after
which it is public.

How the two frontends differ comes down to identity:

- **Web** = anonymous. You can create pastes and browse metadata, but you
  cannot read, edit, or delete a paste before it is revealed.
- **Telegram** = an authenticated owner. You can read, edit, and delete your
  own pastes until they are revealed, and you are notified when they are.
- **Anyone** can read a revealed paste by its id, and can subscribe (through the
  bot) to be notified when a particular paste reveals.

## Telegram commands

| Command | Description |
| --- | --- |
| `/new` | Create a paste: content → publish time (`/skip` = now) → title (`/skip` = none). |
| `/list [page]` | List every paste (inline prev/next buttons). |
| `/mine` | List your pastes. |
| `/show <id>` | Show a paste you may read (your own before reveal, or any revealed one). |
| `/edit <id>` | Edit one of your pastes before it is revealed (its id is recomputed). |
| `/delete <id>` | Delete one of your pastes before it is revealed (with confirmation). |
| `/share <id>` | Get a `t.me` deep link to the paste. |
| `/subscribe <id>` | Get a DM when the paste is revealed. |
| `/unsubscribe <id>` | Stop notifications for a paste. |
| `/subscriptions` | List your subscriptions. |
| `/start [id]` | Register; with a deep-link payload, open or subscribe to a paste. |
| `/help` | Show help. |

There are no roles: identity only distinguishes an owner from a non-owner.
Owners are subscribed to their own pastes automatically, so the reveal DM is
just the normal subscriber notification.

## Web pages and JSON API

| Method | Path | Description |
| --- | --- | --- |
| `GET` | `/` | HTML listing of every paste (metadata only). |
| `GET` | `/p/{id}` | Raw `text/plain` content once revealed; `425` before; `404` if unknown. |
| `GET`/`POST` | `/new` | Creation form for anonymous pastes. |
| `GET` | `/created/{id}` | Confirmation with the id, raw link, and a Telegram subscribe link. |
| `POST` | `/api/paste` | Create an anonymous paste (JSON; idempotent). |
| `GET` | `/api/pastes` | JSON listing. |
| `GET` | `/healthz` | Liveness probe. |

Listings accept `q` (title search), `revealed=0|1` and `private=0|1` (web
toggles; the API uses `status=all|revealed|scheduled`), `sort`
(`created_at`/`publish_at`), `order` (`asc`/`desc`), `page`, `per_page`, and
`from`/`to` bounds.

Content is limited to 4096 characters.

## Identity model

```rust
pub enum User { Anonymous, Telegram(i64) }
```

Stored as a single `owner` column (`"anonymous"` or `"telegram:<id>"`).
Authorization is two pure rules: a paste is readable once revealed, or by its
non-anonymous owner before then; and only a non-anonymous owner may edit or
delete it before reveal. New frontends can add a variant without touching those
rules.

## Subscriptions

`/subscribe <id>` records a row in `subscriptions`. A background task checks
every 30 seconds for newly-revealed pastes, DMs every subscriber the metadata
and content, then clears those subscriptions. `COCOON_MAX_SUBSCRIPTIONS` caps
how many pastes one user can subscribe to (default 100). The web listing and
confirmation page link to `https://t.me/<bot>?start=subscribe_<id>` so web
visitors can subscribe through the bot.

## Configuration

| Variable | Default | Required | Meaning |
| -------- | ------- | -------- | ------- |
| `COCOON_TELEGRAM_BOT_TOKEN` | — | yes | Bot token. |
| `COCOON_TELEGRAM_WEBHOOK_SECRET` | — | yes | Webhook secret (16–256 of `[A-Za-z0-9_-]`), used as both the `secret_token` header and a URL path segment. |
| `COCOON_PUBLIC_URL` | — | yes | Public HTTPS base URL. |
| `COCOON_TELEGRAM_WEBHOOK_PATH` | `/telegram/webhook` | no | Webhook path prefix. |
| `COCOON_TELEGRAM_REGISTER_WEBHOOK` | `true` | no | Register the webhook on startup. |
| `COCOON_TELEGRAM_PROXY` | — | no | Explicit outbound proxy (ambient `HTTPS_PROXY` is ignored). |
| `COCOON_TELEGRAM_API_URL` | Telegram | no | Override the Bot API base URL. |
| `COCOON_MAX_SUBSCRIPTIONS` | `100` | no | Per-user subscription cap. |
| `COCOON_BIND` | `127.0.0.1:3000` | no | Listen address. |
| `COCOON_DB` | `cocoon.db` | no | SQLite database path. |

The bot token and webhook secret each accept a `_FILE` variant that reads the
value from a file (a single trailing newline is stripped), for Docker/Kubernetes
secrets or systemd credentials.

## Running

```sh
export COCOON_TELEGRAM_BOT_TOKEN=...
export COCOON_TELEGRAM_WEBHOOK_SECRET="$(openssl rand -hex 32)"
export COCOON_PUBLIC_URL=https://paste.example.com
cargo run
```

Telegram requires HTTPS for webhooks, so run the bot behind a reverse proxy that
terminates TLS and forwards to `COCOON_BIND`:

```caddyfile
paste.example.com {
    reverse_proxy 127.0.0.1:3000
}
```

The webhook path is `/telegram/webhook/<secret>`; the bot registers it on
startup. Two instances sharing a token would fight over the webhook, so run a
single instance.

## NixOS module

The flake exposes `nixosModules.default` under `services.cocoon-paste`
(`services.cocoon` is an unrelated nixpkgs web app).

```nix
{
  inputs.cocoon.url = "github:serephus/cocoon";

  imports = [ cocoon.nixosModules.default ];

  services.cocoon-paste = {
    enable = true;
    settings.COCOON_PUBLIC_URL = "https://paste.example.com";
    # COCOON_BIND and COCOON_DB have sensible defaults.
    botTokenFile = "/run/secrets/cocoon_bot_token";
    webhookSecretFile = "/run/secrets/cocoon_webhook_secret";
  };
}
```

The service runs as a `DynamicUser` with `StateDirectory = "cocoon-paste"`; the
two secret files are delivered as systemd credentials and exposed via the
`_FILE` variables, so they never enter the Nix store.

## Development

```sh
cargo test          # unit + in-process command-flow tests (no token needed)
./scripts/smoke.sh  # starts the service and exercises web, API, and webhook
nix run nixpkgs#cargo-deny -- check
```

Tests use a fake messenger and a temporary database, so no Telegram token or
network access is required.
