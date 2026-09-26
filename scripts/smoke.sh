#!/usr/bin/env bash
# Smoke test for the cocoon service. Runs the binary with a fake bot token and
# webhook registration disabled, then exercises the HTTP surface: health, the
# Telegram webhook secret/dedup, the web pages, and the JSON API. No Telegram
# token or network required.
set -u

cd "$(dirname "$0")/.."

PORT="${PORT:-3999}"
DB="${DB:-/tmp/cocoon-smoke.db}"
SECRET="0123456789abcdef0123456789abcdef"
BASE="http://127.0.0.1:${PORT}"

pass=0
fail=0
check() { # check <actual> <expected> <label>
  if [ "$1" = "$2" ]; then
    echo "PASS: $3 ($1)"
    pass=$((pass + 1))
  else
    echo "FAIL: $3 expected=$2 got=$1"
    fail=$((fail + 1))
  fi
}

cargo build -q || exit 1
rm -f "$DB" "$DB"-*
COCOON_TELEGRAM_BOT_TOKEN="123456789:TESTTOKEN" \
  COCOON_TELEGRAM_WEBHOOK_SECRET="$SECRET" \
  COCOON_TELEGRAM_REGISTER_WEBHOOK=false \
  COCOON_BIND="127.0.0.1:${PORT}" \
  COCOON_DB="$DB" \
  ./target/debug/cocoon >/tmp/cocoon-smoke.log 2>&1 &
SERVER_PID=$!
trap 'kill "$SERVER_PID" 2>/dev/null' EXIT

for _ in $(seq 1 50); do
  curl -sf "$BASE/healthz" >/dev/null 2>&1 && break
  sleep 0.1
done

post_hook() { # post_hook <path> <header-secret> <body>
  curl -sS -o /dev/null -w '%{http_code}' -X POST "$BASE$1" \
    -H 'content-type: application/json' \
    -H "x-telegram-bot-api-secret-token: $2" \
    -d "$3"
}

# --- HTTP surface ---
check "$(curl -sS -o /dev/null -w '%{http_code}' "$BASE/healthz")" "200" "healthz"

# --- Telegram webhook ---
check "$(post_hook "/telegram/webhook/wrong" "wrong" '{"update_id":1}')" "401" "webhook rejects a bad secret"
check "$(post_hook "/telegram/webhook/$SECRET" "wrong" '{"update_id":2}')" "401" "webhook rejects a bad header"
check "$(post_hook "/telegram/webhook/$SECRET" "$SECRET" '{"update_id":10}')" "200" "webhook accepts a valid update"
check "$(post_hook "/telegram/webhook/$SECRET" "$SECRET" '{"update_id":10}')" "200" "duplicate update is acked"

# --- JSON API: create + read ---
body=$(curl -sS -X POST "$BASE/api/paste" -H 'content-type: application/json' \
  -d '{"content":"hello world","title":"greeting","publish_at":"2020-01-01T00:00:00Z"}')
id=$(echo "$body" | jq -r .id)
check "$(echo "$body" | jq -r .owner)" "anonymous" "api create is anonymous"
check "${#id}" "8" "api id is 8 characters"
check "$(curl -sS -o /dev/null -w '%{http_code}' "$BASE/p/$id")" "200" "read public paste"
check "$(curl -sS "$BASE/p/$id")" "hello world" "read returns the content"

# duplicate create is idempotent (200)
body2=$(curl -sS -w '\n%{http_code}' -X POST "$BASE/api/paste" -H 'content-type: application/json' \
  -d '{"content":"hello world","title":"greeting","publish_at":"2020-01-01T00:00:00Z"}')
check "$(echo "$body2" | tail -1)" "200" "duplicate create is idempotent"

# --- JSON API: a scheduled paste reads as 425 ---
future=$(curl -sS -X POST "$BASE/api/paste" -H 'content-type: application/json' \
  -d '{"content":"top secret","title":"secret","publish_at":"2030-01-01T00:00:00Z"}' | jq -r .id)
check "$(curl -sS -o /dev/null -w '%{http_code}' "$BASE/p/$future")" "425" "scheduled paste -> 425"

# --- JSON API: listing ---
check "$(curl -sS "$BASE/api/pastes" | jq -r --arg id "$id" '[.pastes[].id] | index($id) != null')" "true" "api listing includes the paste"

# --- Web pages ---
check "$(curl -sS -o /dev/null -w '%{http_code}' "$BASE/")" "200" "web listing"
check "$(curl -sS "$BASE/" | grep -c "$id")" "1" "web listing includes the paste"
check "$(curl -sS "$BASE/" | grep -c 'href="/new"')" "1" "web listing links to the form"
check "$(curl -sS -o /dev/null -w '%{http_code}' "$BASE/new")" "200" "web form"

loc=$(curl -sS -o /dev/null -w '%{http_code} %{redirect_url}' -X POST "$BASE/new" \
  --data-urlencode 'title=From the web' --data-urlencode 'content=web content')
check "$(echo "$loc" | cut -d' ' -f1)" "303" "web form create redirects"
created_path=$(echo "$loc" | cut -d' ' -f2)
check "$(curl -sS -o /dev/null -w '%{http_code}' "$created_path")" "200" "created page renders"
check "$(curl -sS "$created_path" | grep -c 'From the web')" "1" "created page shows the title"

echo "-----------------------------------"
echo "PASS=$pass FAIL=$fail"
if grep -q 'ERROR' /tmp/cocoon-smoke.log; then
  echo "ERRORS IN SERVER LOG:"
  grep 'ERROR' /tmp/cocoon-smoke.log
fi
[ "$fail" -eq 0 ] || exit 1
