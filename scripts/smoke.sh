#!/usr/bin/env bash
# End-to-end smoke test for cocoon. Builds the binary, runs it against a
# throwaway database, exercises every route, and reports pass/fail.
set -u

cd "$(dirname "$0")/.."

PORT="${PORT:-3999}"
DB="${DB:-/tmp/cocoon-smoke.db}"
SECRET="${SECRET:-0123456789abcdef}"
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
COCOON_HMAC_SECRET="$SECRET" COCOON_BIND="127.0.0.1:${PORT}" COCOON_DB="$DB" \
  ./target/debug/cocoon >/tmp/cocoon-smoke-server.log 2>&1 &
SERVER_PID=$!
trap 'kill "$SERVER_PID" 2>/dev/null' EXIT

for _ in $(seq 1 50); do
  curl -sf "$BASE/healthz" >/dev/null 2>&1 && break
  sleep 0.1
done

# health
check "$(curl -sS -o /dev/null -w '%{http_code}' "$BASE/healthz")" "200" "healthz"

# immediate create + read
r=$(curl -sS -w '\n%{http_code}' -X POST "$BASE/api/paste" \
  -H 'content-type: application/json' -d '{"content":"hello world","title":"greeting"}')
code=$(echo "$r" | tail -1)
body=$(echo "$r" | sed '$d')
id=$(echo "$body" | jq -r .id)
check "$code" "201" "create immediate status"
check "$(echo "$body" | jq -r '.publish_at | endswith("Z")')" "true" "publish_at is UTC"
check "$(echo "$body" | jq -r .title)" "greeting" "create returns title"
check "${#id}" "22" "id length is 22"
rr=$(curl -sS -w '\n%{http_code}' "$BASE/p/$id")
check "$(echo "$rr" | tail -1)" "200" "read immediate status"
check "$(echo "$rr" | sed '$d')" "hello world" "read immediate content"

# a timestamp in the past is immediately public
pid=$(curl -sS -X POST "$BASE/api/paste" -H 'content-type: application/json' \
  -d '{"content":"archived","publish_at":"2000-01-01T00:00:00Z"}' | jq -r .id)
check "$(curl -sS -o /dev/null -w '%{http_code}' "$BASE/p/$pid")" "200" "read past timestamp -> 200"

# future paste is hidden until its timestamp
r=$(curl -sS -w '\n%{http_code}' -X POST "$BASE/api/paste" \
  -H 'content-type: application/json' \
  -d '{"content":"top secret","title":"classified","publish_at":"2030-01-01T00:00:00Z"}')
fid=$(echo "$r" | sed '$d' | jq -r .id)
check "$(echo "$r" | tail -1)" "201" "create future status"
check "$(curl -sS -o /dev/null -w '%{http_code}' "$BASE/p/$fid")" "425" "read future -> 425"

# exact duplicate is idempotent
r2=$(curl -sS -w '\n%{http_code}' -X POST "$BASE/api/paste" \
  -H 'content-type: application/json' \
  -d '{"content":"top secret","title":"classified","publish_at":"2030-01-01T00:00:00Z"}')
check "$(echo "$r2" | tail -1)" "200" "duplicate status"
check "$(echo "$r2" | sed '$d' | jq -r .id)" "$fid" "duplicate same id"

# same content + timestamp, different title -> different id
rt=$(curl -sS -X POST "$BASE/api/paste" -H 'content-type: application/json' \
  -d '{"content":"top secret","title":"other label","publish_at":"2030-01-01T00:00:00Z"}' | jq -r .id)
if [ "$rt" != "$fid" ]; then
  echo "PASS: different title -> different id"
  pass=$((pass + 1))
else
  echo "FAIL: title did not change id"
  fail=$((fail + 1))
fi

# same content, different timestamp -> different id
r3=$(curl -sS -X POST "$BASE/api/paste" -H 'content-type: application/json' \
  -d '{"content":"top secret","publish_at":"2031-01-01T00:00:00Z"}' | jq -r .id)
if [ "$r3" != "$fid" ]; then
  echo "PASS: different timestamp -> different id"
  pass=$((pass + 1))
else
  echo "FAIL: timestamp did not change id"
  fail=$((fail + 1))
fi

# listing (html + json)
check "$(curl -sS "$BASE/" | grep -c "$fid")" "1" "html listing contains id"
check "$(curl -sS "$BASE/" | grep -c 'greeting')" "1" "html listing shows title"
check "$(curl -sS "$BASE/" | grep -c 'classified')" "1" "html listing shows scheduled title"
check "$(curl -sS "$BASE/api/pastes?status=scheduled&sort=publish_at&order=asc" | jq -r '.pastes | length >= 2')" "true" "api list scheduled"
check "$(curl -sS "$BASE/api/pastes?status=scheduled&sort=publish_at&order=asc" | jq -r '[.pastes[].title] | index("classified") != null')" "true" "api list includes title"

# visibility toggles (revealed/private) round-trip through the URL
check "$(curl -sS "$BASE/?revealed=1&private=0" | grep -c "$id")" "1" "revealed filter includes revealed"
check "$(curl -sS "$BASE/?revealed=1&private=0" | grep -c "$fid")" "0" "revealed filter excludes private"
check "$(curl -sS "$BASE/?revealed=0&private=1" | grep -c "$fid")" "1" "private filter includes private"
check "$(curl -sS "$BASE/?revealed=0&private=1" | grep -c "$id")" "0" "private filter excludes revealed"
check "$(curl -sS "$BASE/?revealed=0&private=0" | grep -c 'no pastes match')" "1" "no filter shows none"

# sort arrows reflect the active column and direction
check "$(curl -sS "$BASE/?sort=publish_at&order=desc" | grep -c 'publish at (UTC) <span class="arrow">▼</span>')" "1" "sort desc arrow"
check "$(curl -sS "$BASE/?sort=created_at&order=asc" | grep -c 'created (UTC) <span class="arrow">▲</span>')" "1" "sort asc arrow"

# title search (web + api), case-insensitive, combinable with filters
curl -sS -o /dev/null -X POST "$BASE/api/paste" -H 'content-type: application/json' \
  -d '{"content":"needle content","title":"Searchable Needle"}'
check "$(curl -sS "$BASE/?q=needle" | grep -c 'Searchable Needle')" "1" "web search finds title"
check "$(curl -sS "$BASE/?q=NEEDLE" | grep -c 'Searchable Needle')" "1" "web search is case-insensitive"
check "$(curl -sS "$BASE/?q=absentterm" | grep -c 'no pastes match')" "1" "web search no match -> empty"
check "$(curl -sS "$BASE/?q=needle&revealed=0&private=1" | grep -c 'no pastes match')" "1" "search combines with filters"
check "$(curl -sS "$BASE/?q=needle" | grep -q 'q=needle' && echo yes || echo no)" "yes" "search preserved in links"
check "$(curl -sS "$BASE/api/pastes?q=needle" | jq -r '[.pastes[].title] | index("Searchable Needle") != null')" "true" "api search finds title"
check "$(curl -sS "$BASE/api/pastes?q=zzzz" | jq -r '.pastes | length')" "0" "api search no match -> empty"

# title validation
check "$(curl -sS -o /dev/null -w '%{http_code}' -X POST "$BASE/api/paste" \
  -H 'content-type: application/json' -d '{"content":"x","title":"a\nb"}')" "400" "title newline -> 400"
check "$(curl -sS -o /dev/null -w '%{http_code}' -X POST "$BASE/api/paste" \
  -H 'content-type: application/json' -d '{"content":"x","title":"a\u202eb"}')" "400" "title bidi override -> 400"
long_title=$(head -c 300 /dev/zero | tr '\0' a)
check "$(curl -sS -o /dev/null -w '%{http_code}' -X POST "$BASE/api/paste" \
  -H 'content-type: application/json' -d "$(jq -n --arg c x --arg t "$long_title" '{content:$c,title:$t}')")" "400" "title too long -> 400"
check "$(curl -sS -o /dev/null -w '%{http_code}' -X POST "$BASE/api/paste" \
  -H 'content-type: application/json' -d '{"content":"untitled","title":""}')" "201" "empty title -> 201"

# titles are rendered as escaped text, never markup
curl -sS -o /dev/null -X POST "$BASE/api/paste" \
  -H 'content-type: application/json' -d '{"content":"x","title":"<b>bold</b>"}'
check "$(curl -sS "$BASE/" | grep -c '<b>bold</b>')" "0" "html listing escapes raw title"
check "$(curl -sS "$BASE/" | grep -c '&#60;b&#62;bold&#60;/b&#62;')" "1" "html listing shows escaped title"

# validation and error paths
check "$(curl -sS -o /dev/null -w '%{http_code}' -X POST "$BASE/api/paste" \
  -H 'content-type: application/json' -d '{"content":"x","publish_at":"nope"}')" "400" "invalid timestamp -> 400"
check "$(curl -sS -o /dev/null -w '%{http_code}' "$BASE/api/pastes?status=bogus")" "400" "invalid status -> 400"
check "$(curl -sS -o /dev/null -w '%{http_code}' "$BASE/?sort=bogus")" "400" "html invalid sort -> 400"
check "$(curl -sS "$BASE/?sort=bogus" | grep -c 'back to listing')" "1" "html error page renders"
check "$(curl -sS "$BASE/?sort=bogus" | grep -c 'time-locked pastebin')" "1" "error page shares header"
check "$(curl -sS -o /dev/null -w '%{http_code}' "$BASE/p/AAAAAAAAAAAAAAAAAAAAAA")" "404" "unknown id -> 404"
check "$(curl -sS -o /dev/null -w '%{http_code}' "$BASE/p/notbase64!!")" "404" "malformed id -> 404"

big=$(jq -n --arg c "$(head -c 70000 /dev/zero | tr '\0' a)" '{content:$c}')
check "$(curl -sS -o /dev/null -w '%{http_code}' -X POST "$BASE/api/paste" \
  -H 'content-type: application/json' -d "$big")" "413" "70000 bytes -> 413"
check "$(curl -sS -o /dev/null -w '%{http_code}' -X POST "$BASE/api/paste" \
  -H 'content-type: application/json' -d '{"content":""}')" "201" "empty content -> 201"

echo "-----------------------------------"
echo "PASS=$pass FAIL=$fail"
if grep -q ERROR /tmp/cocoon-smoke-server.log; then
  echo "ERRORS FOUND IN SERVER LOG:"
  grep ERROR /tmp/cocoon-smoke-server.log
fi
[ "$fail" -eq 0 ] || exit 1
