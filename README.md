# cocoon

A **time-locked pastebin**. Submit text together with a publication timestamp;
the content stays private until that moment, after which it is public forever.
Pastes are anonymous, immutable, and never expire.

Secrecy is **policy-based**: the server stores plaintext and simply refuses to
serve it before the timestamp. The server operator can always read it. There is
no encryption and no trust model to defeat a malicious operator.

## Behaviour

- Anonymous creation; no accounts.
- The listing can be searched by title (case-insensitive substring match).
- Each paste may carry an optional short `title` used as a public label in
  listings. Titles are visible **immediately**, including before a scheduled
  paste is revealed, so they must not contain secrets.
- Pastes can never be modified or deleted.
- Before `publish_at`, `GET /p/{id}` returns `425 Too Early` and the content is
  withheld. The **id itself is public** immediately and appears in the listing.
- A timestamp in the past simply produces an immediately public paste.
- Timestamps are UTC (RFC 3339); the timestamp is optional and defaults to now.
- Content is text only, at most 64 KiB, and empty content is allowed.

## Identifiers

`id = base64url(HMAC-SHA256(secret, "cocoon:v1\0" || publish_at || len(title) || title || len(content) || content)[..16])`

- Fixed 128 bits → 22 URL-safe characters.
- The `title` is part of the identity: the same content and timestamp submitted
  with a different title is a different paste.
- Deterministic for the server, but **not** computable by clients: the HMAC
  secret prevents an offline dictionary attack against low-entropy content
  before it is revealed.
- Submitting the exact same `(content, publish_at)` twice is idempotent:
  the first request returns `201`, later ones return `200` with the same id.
- A true 128-bit collision would be rejected with `409` and never overwrite.

## Endpoints

| Method | Path | Description |
| ------ | ---- | ----------- |
| `POST` | `/api/paste` | Create a paste. JSON `{"content": "...", "title": "...", "publish_at": "..."}` (title and timestamp optional). Returns JSON `{id, url, title, publish_at, created_at}`. |
| `GET`  | `/new`       | HTML form for creating a paste. |
| `POST` | `/new`       | Create from the web form and redirect to the listing; re-renders the form with an error on invalid input. |
| `GET`  | `/p/{id}`    | Raw `text/plain` content once public; `425` before; `404` if unknown. |
| `GET`  | `/`          | HTML listing (metadata incl. title; never content). |
| `GET`  | `/api/pastes`| JSON listing. |
| `GET`  | `/healthz`   | Liveness probe. |

The JSON listing (`/api/pastes`) accepts `page`, `per_page` (max 100),
`status` (`all`/`revealed`/`scheduled`), `sort` (`created_at`/`publish_at`),
`order` (`asc`/`desc`), `q` (case-insensitive title substring), and optional
`from`/`to` RFC 3339 bounds on `publish_at`.

The HTML listing uses the same parameters plus two independent visibility flags,
`revealed=0|1` and `private=0|1`, and exposes the title search as a box at the
top of the page. The visibility toggles and the sortable column headers are
plain links: clicking one rewrites the URL (preserving the search term) and
reloads — no JavaScript.

Titles are limited to 256 bytes, must be a single line, and reject control and
bidirectional-override characters. They are rendered HTML-escaped and never
linkified. The web form's publish time is entered with `datetime-local` and
interpreted as **UTC**; leaving it empty publishes immediately.

## Configuration

| Variable | Default | Required | Meaning |
| -------- | ------- | -------- | ------- |
| `COCOON_HMAC_SECRET` | — | one of these | HMAC key as a literal value, at least 16 bytes. |
| `COCOON_HMAC_SECRET_FILE` | — | one of these | Path to a file containing the HMAC key. |
| `COCOON_BIND` | `127.0.0.1:3000` | no | Listen address. |
| `COCOON_DB` | `cocoon.db` | no | SQLite database path. |

Exactly one of `COCOON_HMAC_SECRET` and `COCOON_HMAC_SECRET_FILE` must be set;
setting both is a startup error.

### Providing the secret from a file

For Docker secrets, Kubernetes mounted secrets, or systemd
`LoadCredential`/`$CREDENTIALS_DIRECTORY`, point `COCOON_HMAC_SECRET_FILE` at
the file instead of exporting the value:

```sh
openssl rand -hex 32 > /run/secrets/cocoon_hmac
# chmod 600 /run/secrets/cocoon_hmac
export COCOON_HMAC_SECRET_FILE=/run/secrets/cocoon_hmac
```

The file is read once at startup as raw bytes. A single trailing newline (`\n`
or `\r\n`) is stripped, so `echo -n`/`printf` and editor-written files behave
the same; all other bytes are significant. On Unix, a non-fatal warning is
logged if the file is readable by group or others. The secret is never logged.


## Running

```sh
export COCOON_HMAC_SECRET="$(openssl rand -hex 32)"
cargo run
```

The database and schema are created automatically on first start.

## Testing

```sh
cargo test          # unit tests (identifier derivation/encoding, config)
./scripts/smoke.sh  # end-to-end HTTP checks against a throwaway database

# licenses, advisories, bans, sources (also run in CI)
nix run nixpkgs#cargo-deny -- check
```

## NixOS module

The flake exposes `nixosModules.default`, under the namespace
`services.cocoon-paste` (`services.cocoon` is already taken by an unrelated
nixpkgs web app). The package defaults to this flake's build, but can be
overridden with `services.cocoon-paste.package`.

```nix
{
  inputs.cocoon.url = "github:serephus/cocoon";

  # in a NixOS configuration
  imports = [ cocoon.nixosModules.default ];

  services.cocoon-paste = {
    enable = true;
    settings = {
      COCOON_BIND = "0.0.0.0:3000";
      # COCOON_DB defaults to /var/lib/cocoon-paste/cocoon.db
    };
    secretFile = "/run/secrets/cocoon_hmac";
    openFirewall = true;
  };
}
```

The service runs as a `DynamicUser` with `StateDirectory = "cocoon-paste"`, so
the SQLite database lives in `/var/lib/cocoon-paste`. `secretFile` is delivered
as a systemd credential and exposed to the process as `%d/hmac`, so the secret
never enters the Nix store. `settings` is a free-form `COCOON_*` environment
map; keep secrets out of it and use `secretFile` (or `environmentFile`).

## Notes and caveats

- The **server clock is the sole source of truth** for "now". NTP drift or a
  clock change directly affects reveal times.
- Keep `COCOON_HMAC_SECRET` stable. Rotating it changes the id derived for new
  submissions; old rows still resolve, but a resubmission after rotation will
  not deduplicate against the old row.
- There is intentionally no rate limiting, captcha, or authentication.
- Storage grows without bound, both in row count and on-disk size.

## Stack

`axum` + `tokio`, `diesel` (SQLite, with bundled libsqlite3) behind an `r2d2`
pool, `askama` templates, `hmac`/`sha2`/`base64` for identifiers.
