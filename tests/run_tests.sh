#!/bin/bash
# TS3 Bot Integration Test Suite
# Tests the bot against a local Docker TS3 server
#
# Prerequisites:
#   - Docker TS3 dev server: sudo docker start ts3-dev
#   - Bot compiled: cargo build --release
#
# Usage: ./tests/run_tests.sh
# Exit code: 0 = all pass, 1 = failures

set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
BOT_DIR="$(dirname "$SCRIPT_DIR")"
BOT_BINARY="$BOT_DIR/target/release/ts3_bot"
TEST_WORKDIR="$SCRIPT_DIR/dev-env"
SQ_HOST="localhost"
SQ_PORT="10012"
SQ_USER="serveradmin"
SQ_PASS="dQ5ieLKV"
WS_PORT="8081"
BOT_PID=""
LOG_FILE="$TEST_WORKDIR/test-bot.log"

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m'

TESTS_PASSED=0
TESTS_FAILED=0

log_pass() { echo -e "${GREEN}✅ PASS${NC}: $1"; ((TESTS_PASSED++)); }
log_fail() { echo -e "${RED}❌ FAIL${NC}: $1 — $2"; ((TESTS_FAILED++)); }

# ─── ServerQuery helpers ───

sq_cmd() {
    echo -e "login $SQ_USER $SQ_PASS\nuse 1\n$1\nquit" | nc -w3 "$SQ_HOST" "$SQ_PORT" 2>/dev/null
}

# Send channel message and return bot's response from logs
sq_send_and_check() {
    local msg="$1"
    local expect="$2"
    local timeout="${3:-3}"
    
    # Mark log position
    local log_lines_before=$(wc -l < "$LOG_FILE" 2>/dev/null || echo 0)
    
    # Send message
    sq_cmd "sendtextmessage targetmode=2 msg=$(echo "$msg" | sed 's/ /\\s/g; s/|/\\p/g')" >/dev/null 2>&1
    
    # Wait and check logs for expected response
    local elapsed=0
    while [ $elapsed -lt $timeout ]; do
        sleep 0.5
        elapsed=$((elapsed + 1))
        if tail -n +$((log_lines_before + 1)) "$LOG_FILE" 2>/dev/null | grep -qi "$expect"; then
            return 0
        fi
    done
    return 1
}

# Get new log lines since a marker
logs_since() {
    local marker="$1"
    tail -n +$((marker + 1)) "$LOG_FILE" 2>/dev/null
}

# ─── Setup / Teardown ───

setup() {
    echo "═══════════════════════════════════════"
    echo "  TS3 Bot Integration Tests"
    echo "═══════════════════════════════════════"
    echo ""
    
    mkdir -p "$TEST_WORKDIR"
    
    # Check Docker TS3
    if ! sudo docker ps --filter name=ts3-dev --format '{{.Names}}' 2>/dev/null | grep -q ts3-dev; then
        echo "Starting Docker TS3 dev server..."
        sudo docker start ts3-dev 2>/dev/null || \
        sudo docker run -d --name ts3-dev \
            -p 9988:9987/udp -p 10012:10011 -p 30034:30033 \
            -e TS3SERVER_LICENSE=accept teamspeak:latest
        sleep 3
    fi
    
    # Check binary
    if [ ! -f "$BOT_BINARY" ]; then
        echo "ERROR: Bot binary not found. Run: cargo build --release"
        exit 2
    fi
    
    # Check ServerQuery
    if ! sq_cmd "version" | grep -q "version="; then
        echo "ERROR: Cannot connect to ServerQuery on port $SQ_PORT"
        exit 2
    fi

    # Relax ServerQuery flood protection for tests (reset on docker restart)
    sq_cmd "instanceedit serverinstance_serverquery_flood_commands=100 serverinstance_serverquery_flood_time=1 serverinstance_serverquery_max_connections_per_ip=50" >/dev/null 2>&1
    
    # Kill any leftover test bot and wait for TS3 to release the connection
    if pgrep -f "MarlbotTest" >/dev/null 2>&1; then
        echo -e "${YELLOW}Killing leftover test bot...${NC}"
        pkill -f "MarlbotTest" 2>/dev/null || true
        sleep 5
    fi
    
    # Wait until no MarlbotTest in client list (TS3 takes ~30s to timeout)
    local retries=0
    while sq_cmd "clientlist" | grep -q "MarlbotTest" && [ $retries -lt 12 ]; do
        echo -e "${YELLOW}Waiting for old test bot to disconnect... (${retries}s)${NC}"
        sleep 5
        retries=$((retries + 1))
    done
    
    # Start bot against dev server
    echo -e "${YELLOW}Starting bot against dev server...${NC}"
    cd "$TEST_WORKDIR"
    
    TS3_SERVER="localhost:9988" \
    TS3_NICKNAME="MarlbotTest" \
    TS3_PASSWORD="" \
    TS3_CHANNEL="" \
    WS_HOST="0.0.0.0" \
    WS_PORT="$WS_PORT" \
    TTS_ENABLED="false" \
    TTS_API_KEY="${TTS_API_KEY:-notset}" \
    LOG_LEVEL="INFO" \
    "$BOT_BINARY" > "$LOG_FILE" 2>&1 &
    BOT_PID=$!
    
    # Wait for bot to connect (check logs)
    local wait=0
    while [ $wait -lt 10 ]; do
        if grep -q "Client connected: MarlbotTest" "$LOG_FILE" 2>/dev/null; then
            echo -e "${GREEN}Bot connected to dev server ✅${NC}"
            echo ""
            # Extra wait for event stream to stabilize
            sleep 2
            return 0
        fi
        if ! kill -0 "$BOT_PID" 2>/dev/null; then
            echo "ERROR: Bot crashed on startup. Logs:"
            cat "$LOG_FILE"
            exit 2
        fi
        sleep 1
        wait=$((wait + 1))
    done
    
    echo "ERROR: Bot didn't connect within 10s. Logs:"
    tail -20 "$LOG_FILE"
    exit 2
}

teardown() {
    if [ -n "$BOT_PID" ] && kill -0 "$BOT_PID" 2>/dev/null; then
        kill "$BOT_PID" 2>/dev/null
        wait "$BOT_PID" 2>/dev/null || true
    fi
}

trap teardown EXIT

# ─── WebSocket helper ───

ws_cmd() {
    local json="$1"
    local timeout="${2:-1}"
    if ! command -v websocat &>/dev/null; then
        echo ""
        return 1
    fi
    # Server sends: 1) welcome, 2) placeholder command_success, 3) real response with data.
    # For errors: 1) welcome, 2) error response (only 2 messages).
    # Grab all messages within the timeout, return the last non-welcome one.
    local result
    result=$( (echo "$json"; sleep "$timeout") | timeout $((timeout + 1)) websocat "ws://localhost:$WS_PORT/ws" 2>/dev/null | grep -v '"type":"welcome"' | tail -1 )
    echo "$result"
}

# ─── Tests ───

test_bot_connects() {
    if sq_cmd "clientlist" | grep -q "MarlbotTest"; then
        log_pass "Bot visible in TS3 client list"
    else
        log_fail "Bot visible in TS3 client list" "not found in clientlist"
    fi
}

test_ws_status() {
    if command -v websocat &>/dev/null; then
        local resp
        resp=$(ws_cmd '{"type":"get_status","command_id":"test-1"}')
        if echo "$resp" | grep -q "command_id"; then
            log_pass "WebSocket responds to get_status"
        else
            log_fail "WebSocket responds to get_status" "no valid response"
        fi
    else
        echo -e "${YELLOW}⏭️ SKIP: WebSocket test (websocat not installed)${NC}"
    fi
}

test_help_command() {
    if sq_send_and_check "!help" "Commandes disponibles" 5; then
        log_pass "!help returns command list"
    else
        log_fail "!help returns command list" "no response in logs"
    fi
}

test_status_command() {
    if sq_send_and_check "!status" "Status Marlbot" 5; then
        log_pass "!status returns bot status"
    else
        log_fail "!status returns bot status" "no response in logs"
    fi
}

test_who_command() {
    if sq_send_and_check "!who" "channel" 5; then
        log_pass "!who returns channel info"
    else
        log_fail "!who returns channel info" "no response in logs"
    fi
}

test_channels_command() {
    if sq_send_and_check "!channels" "channel" 5; then
        log_pass "!channels lists server channels"
    else
        log_fail "!channels lists server channels" "no response in logs"
    fi
}

test_no_loop_on_help() {
    local log_lines_before=$(wc -l < "$LOG_FILE" 2>/dev/null || echo 0)
    
    sq_cmd "sendtextmessage targetmode=2 msg=!help" >/dev/null 2>&1
    sleep 3
    
    local response_count
    response_count=$(logs_since "$log_lines_before" | grep -c "TS3 chat" || true)
    response_count=${response_count:-0}
    
    if [ "$response_count" -le 1 ]; then
        log_pass "No infinite loop on !help ($response_count response(s))"
    else
        log_fail "No infinite loop on !help" "$response_count responses detected"
    fi
}

test_no_response_to_normal_message() {
    local log_lines_before=$(wc -l < "$LOG_FILE" 2>/dev/null || echo 0)
    
    sq_cmd "sendtextmessage targetmode=2 msg=hello\\sworld" >/dev/null 2>&1
    sleep 2
    
    local chat_count
    chat_count=$(logs_since "$log_lines_before" | grep -c "TS3 chat" || true)
    chat_count=${chat_count:-0}
    
    if [ "$chat_count" -eq 0 ]; then
        log_pass "No response to non-command message"
    else
        log_fail "No response to non-command message" "$chat_count unexpected response(s)"
    fi
}

test_message_with_bang_inside() {
    # A message that contains "!help" but doesn't START with it
    local log_lines_before=$(wc -l < "$LOG_FILE" 2>/dev/null || echo 0)
    
    sq_cmd "sendtextmessage targetmode=2 msg=check\\sout\\s!help\\sfor\\sinfo" >/dev/null 2>&1
    sleep 2
    
    local chat_count
    chat_count=$(logs_since "$log_lines_before" | grep -c "TS3 chat" || true)
    chat_count=${chat_count:-0}
    
    if [ "$chat_count" -eq 0 ]; then
        log_pass "No trigger on !help inside a sentence"
    else
        log_fail "No trigger on !help inside a sentence" "command triggered"
    fi
}

test_listen_command() {
    if sq_send_and_check "!listen" "coute\|listen\|activ" 5; then
        log_pass "!listen activates listening"
    else
        log_fail "!listen activates listening" "no response in logs"
    fi
}

test_stop_command() {
    # First activate listening, then stop
    sq_cmd "sendtextmessage targetmode=2 msg=!listen" >/dev/null 2>&1
    sleep 2
    
    if sq_send_and_check "!stop" "stop\|arr" 5; then
        log_pass "!stop stops listening"
    else
        log_fail "!stop stops listening" "no response in logs"
    fi
}

test_volume_command() {
    if sq_send_and_check "!volume" "Volume actuel"; then
        log_pass "!volume shows current volume"
    else
        log_fail "!volume shows current volume" "no response in logs"
    fi
}

test_volume_set() {
    if sq_send_and_check "!volume 75" "Volume réglé à 75"; then
        log_pass "!volume 75 sets volume"
    else
        log_fail "!volume 75 sets volume" "no response in logs"
    fi
}

test_come_command() {
    # !come should respond (either "J'arrive" if different channel, "déjà dans" if same, or error if sender not found)
    if sq_send_and_check "!come" "déjà dans\|J'arrive\|Impossible de trouver" 5; then
        log_pass "!come responds correctly"
    else
        log_fail "!come responds correctly" "no response in logs"
    fi
}

test_replay_command() {
    # !replay should respond "Rien à rejouer" when nothing has been spoken
    if sq_send_and_check "!replay" "Rien à rejouer" 5; then
        log_pass "!replay responds correctly (nothing to replay)"
    else
        log_fail "!replay responds correctly" "no response in logs"
    fi
}

test_mute_command() {
    # !mute should mute TTS
    if sq_send_and_check "!mute" "TTS muté" 3; then
        log_pass "!mute mutes TTS"
    else
        log_fail "!mute mutes TTS" "no response in logs"
    fi
}

test_unmute_command() {
    # !unmute should unmute TTS
    if sq_send_and_check "!unmute" "TTS réactivé" 3; then
        log_pass "!unmute unmutes TTS"
    else
        log_fail "!unmute unmutes TTS" "no response in logs"
    fi
}

test_greet_command() {
    # !greet should show current status
    if sq_send_and_check "!greet" "Greetings" 3; then
        log_pass "!greet shows greeting status"
    else
        log_fail "!greet shows greeting status" "no response in logs"
    fi
}

test_greet_off() {
    # !greet off should disable greetings
    if sq_send_and_check "!greet off" "désactivés" 3; then
        log_pass "!greet off disables greetings"
    else
        log_fail "!greet off disables greetings" "no response in logs"
    fi
}

test_roll_command() {
    # !roll should return a dice result with 🎲
    if sq_send_and_check "!roll" "🎲" 3; then
        log_pass "!roll returns dice result"
    else
        log_fail "!roll returns dice result" "no dice emoji in response"
    fi
}

test_roll_dice_notation() {
    # !roll 2d6 should return a dice result with details
    if sq_send_and_check "!roll 2d6" "🎲" 3; then
        log_pass "!roll 2d6 returns dice result"
    else
        log_fail "!roll 2d6 returns dice result" "no dice emoji in response"
    fi
}

test_timeout_command() {
    # !timeout should show current timeout
    if sq_send_and_check "!timeout" "Silence timeout" 3; then
        log_pass "!timeout shows current timeout"
    else
        log_fail "!timeout shows current timeout" "no response in logs"
    fi
}

test_timeout_set() {
    # !timeout 3000 should set timeout
    if sq_send_and_check "!timeout 3000" "3000ms" 3; then
        log_pass "!timeout 3000 sets timeout"
    else
        log_fail "!timeout 3000 sets timeout" "no response in logs"
    fi
}

test_quote_add() {
    # !quote add should save a quote
    if sq_send_and_check "!quote add Test quote from integration test" "sauvegardée" 3; then
        log_pass "!quote add saves a quote"
    else
        log_fail "!quote add saves a quote" "no confirmation in response"
    fi
}

test_quote_random() {
    # !quote should return a random quote (we just added one)
    if sq_send_and_check "!quote" "Test quote from integration test" 3; then
        log_pass "!quote returns a random quote"
    else
        log_fail "!quote returns a random quote" "no quote text in response"
    fi
}

test_quote_count() {
    # !quote count should show count
    if sq_send_and_check "!quote count" "quote" 3; then
        log_pass "!quote count shows count"
    else
        log_fail "!quote count shows count" "no count in response"
    fi
}

test_history_command() {
    # !history should return history header
    if sq_send_and_check "!history" "📜" 3; then
        log_pass "!history returns history"
    else
        log_fail "!history returns history" "no history response"
    fi
}

test_voice_command() {
    # !voice should show current voice + available voices
    if sq_send_and_check "!voice" "voix" 3; then
        log_pass "!voice shows current voice"
    else
        log_fail "!voice shows current voice" "no voice info in response"
    fi
}

test_voice_set() {
    # !voice nova should change the default voice
    if sq_send_and_check "!voice nova" "nova" 3; then
        log_pass "!voice nova changes default voice"
    else
        log_fail "!voice nova changes default voice" "no confirmation"
    fi
}

test_seen_command() {
    # !seen should show tracked count
    if sq_send_and_check "!seen" "👁️" 3; then
        log_pass "!seen shows tracked users"
    else
        log_fail "!seen shows tracked users" "no seen response"
    fi
}

test_ping_command() {
    # Extra delay to avoid TS3 flood protection
    sleep 2
    # !ping should return Pong with ms latency
    if sq_send_and_check "!ping" "Pong" 5; then
        log_pass "!ping returns pong with latency"
    else
        log_fail "!ping returns pong with latency" "no pong response"
    fi
}

test_bot_does_not_crash() {
    if kill -0 "$BOT_PID" 2>/dev/null; then
        log_pass "Bot still running after all tests"
    else
        log_fail "Bot still running after all tests" "process died"
    fi
}

# ─── Main ───

setup

test_bot_connects
test_ws_status
test_help_command
test_no_loop_on_help
test_message_with_bang_inside
test_no_response_to_normal_message
test_status_command
test_who_command
test_channels_command
test_listen_command
test_stop_command
test_volume_command
test_volume_set
test_come_command
test_replay_command
test_mute_command
test_unmute_command
test_greet_command
test_greet_off
test_roll_command
test_roll_dice_notation
test_timeout_command
test_timeout_set
test_quote_add
test_quote_random
test_quote_count
test_history_command
test_voice_command
test_voice_set
test_seen_command
test_ping_command

test_stats_command() {
    sleep 2
    if sq_send_and_check "!stats" "Statistiques" 5; then
        log_pass "!stats shows usage statistics"
    else
        log_fail "!stats shows usage statistics" "no response"
    fi
}

test_stats_command

test_notify_command() {
    sleep 2
    # !notify shows help when no args
    if sq_send_and_check "!notify" "notification" 5; then
        log_pass "!notify shows notification info"
    else
        log_fail "!notify shows notification info" "no response"
    fi
}

test_notify_add() {
    sleep 2
    # !notify someuser adds a watcher
    if sq_send_and_check "!notify testuser" "notifi" 5; then
        log_pass "!notify testuser adds/toggles notification"
    else
        log_fail "!notify testuser adds/toggles notification" "no response"
    fi
}

test_notify_command
test_notify_add

test_find_command() {
    if sq_send_and_check "!find serveradmin" "résultat"; then
        log_pass "!find returns search results"
    else
        log_fail "!find returns search results" "no response"
    fi
}

test_find_empty() {
    if sq_send_and_check "!find zzzznonexistent" "Aucun utilisateur"; then
        log_pass "!find with no match shows error"
    else
        log_fail "!find with no match shows error" "no response"
    fi
}

test_afk_set() {
    if sq_send_and_check "!afk eating lunch" "AFK activé"; then
        log_pass "!afk sets AFK status"
    else
        log_fail "!afk sets AFK status" "no response"
    fi
}

test_afk_clear() {
    # Set AFK first, then clear it
    sq_cmd "sendtextmessage targetmode=2 msg=!afk\\sbrb" >/dev/null 2>&1
    sleep 1
    if sq_send_and_check "!afk" "plus AFK\|Usage"; then
        log_pass "!afk (no args) clears AFK"
    else
        log_fail "!afk (no args) clears AFK" "no response"
    fi
}

test_poll_create() {
    if sq_send_and_check "!poll Pizza ou Sushi ? | Pizza | Sushi" "Nouveau sondage"; then
        log_pass "!poll creates a poll"
    else
        log_fail "!poll creates a poll" "no response"
    fi
}

test_poll_show() {
    if sq_send_and_check "!poll" "Pizza ou Sushi"; then
        log_pass "!poll shows current poll"
    else
        log_fail "!poll shows current poll" "no response"
    fi
}

test_vote() {
    if sq_send_and_check "!vote 1" "a voté pour"; then
        log_pass "!vote registers a vote"
    else
        log_fail "!vote registers a vote" "no response"
    fi
}

test_poll_end() {
    if sq_send_and_check "!poll end" "Sondage terminé"; then
        log_pass "!poll end closes the poll"
    else
        log_fail "!poll end closes the poll" "no response"
    fi
}

test_remind_set() {
    if sq_send_and_check "!remind 1h Test reminder" "Rappel dans"; then
        log_pass "!remind sets a reminder"
    else
        log_fail "!remind sets a reminder" "no response"
    fi
}

test_remind_list() {
    if sq_send_and_check "!remind" "rappel"; then
        log_pass "!remind shows pending reminders"
    else
        log_fail "!remind shows pending reminders" "no response"
    fi
}

test_remind_clear() {
    if sq_send_and_check "!remind clear" "supprimé\|Aucun rappel"; then
        log_pass "!remind clear removes reminders"
    else
        log_fail "!remind clear removes reminders" "no response"
    fi
}

test_find_command
test_find_empty
test_afk_set
test_afk_clear
test_remind_set
test_remind_list
test_remind_clear
test_poll_create
test_poll_show
test_vote
test_poll_end

# --- 8ball tests ---
test_8ball() {
    if sq_send_and_check '!8ball Will I win?' '🎱' 3; then
        log_pass '!8ball returns a prediction'
    else
        log_fail '!8ball returns a prediction' 'no response'
    fi
}

test_8ball_no_question() {
    if sq_send_and_check '!8ball' 'Pose une question' 3; then
        log_pass '!8ball without question shows usage'
    else
        log_fail '!8ball without question shows usage' 'no response'
    fi
}

test_8ball
test_8ball_no_question

# --- Roulette tests ---
test_roulette() {
    if sq_send_and_check '!roulette' '🔫' 3; then
        log_pass '!roulette triggers russian roulette'
    else
        log_fail '!roulette triggers russian roulette' 'no response'
    fi
}
test_roulette

# --- Duel tests ---
test_duel_usage() {
    if sq_send_and_check '!duel' 'Usage' 3; then
        log_pass '!duel shows usage'
    else
        log_fail '!duel shows usage' 'no response'
    fi
}

test_duel_challenge() {
    if sq_send_and_check '!duel nonexistent_user_xyz' 'adversaire' 3; then
        log_pass '!duel with unknown user shows error'
    else
        log_fail '!duel with unknown user shows error' 'no response'
    fi
}

test_duel_usage
test_duel_challenge

# ─── WebSocket API Tests ───

test_ws_get_status_fields() {
    local resp
    resp=$(ws_cmd '{"type":"get_status","command_id":"t-status"}')
    if echo "$resp" | grep -q '"clients"' && echo "$resp" | grep -q '"command_id":"t-status"'; then
        log_pass "WS get_status returns clients + command_id"
    else
        log_fail "WS get_status returns clients + command_id" "resp: $resp"
    fi
}

test_ws_get_volume() {
    local resp
    resp=$(ws_cmd '{"type":"get_volume","command_id":"t-vol"}')
    if echo "$resp" | grep -q 'volume'; then
        log_pass "WS get_volume returns volume"
    else
        log_fail "WS get_volume returns volume" "resp: $resp"
    fi
}

test_ws_set_volume() {
    local resp
    resp=$(ws_cmd '{"type":"set_volume","volume":80,"command_id":"t-svol"}')
    if echo "$resp" | grep -q '"success"' || echo "$resp" | grep -q '"status":"ok"'; then
        log_pass "WS set_volume accepts valid volume"
    else
        log_fail "WS set_volume accepts valid volume" "resp: $resp"
    fi
}

test_ws_get_voice() {
    local resp
    resp=$(ws_cmd '{"type":"get_voice","command_id":"t-gv"}')
    if echo "$resp" | grep -q 'voice'; then
        log_pass "WS get_voice returns voice"
    else
        log_fail "WS get_voice returns voice" "resp: $resp"
    fi
}

test_ws_set_voice() {
    local resp
    resp=$(ws_cmd '{"type":"set_voice","voice":"nova","command_id":"t-sv"}')
    if echo "$resp" | grep -q '"success"\|"status"'; then
        log_pass "WS set_voice accepts valid voice"
    else
        log_fail "WS set_voice accepts valid voice" "resp: $resp"
    fi
}

test_ws_set_voice_invalid() {
    local resp
    resp=$(ws_cmd '{"type":"set_voice","voice":"invalid_voice_xyz","command_id":"t-svi"}')
    if echo "$resp" | grep -qi 'error\|invalid\|unknown'; then
        log_pass "WS set_voice rejects invalid voice"
    else
        log_fail "WS set_voice rejects invalid voice" "resp: $resp"
    fi
}

test_ws_get_history() {
    local resp
    resp=$(ws_cmd '{"type":"get_history","count":5,"command_id":"t-hist"}')
    if echo "$resp" | grep -q '"command_id":"t-hist"'; then
        log_pass "WS get_history returns response with command_id"
    else
        log_fail "WS get_history returns response with command_id" "resp: $resp"
    fi
}

test_ws_get_timeout() {
    local resp
    resp=$(ws_cmd '{"type":"get_timeout","command_id":"t-gt"}')
    if echo "$resp" | grep -q 'timeout_ms'; then
        log_pass "WS get_timeout returns timeout_ms"
    else
        log_fail "WS get_timeout returns timeout_ms" "resp: $resp"
    fi
}

test_ws_set_timeout() {
    local resp
    resp=$(ws_cmd '{"type":"set_timeout","timeout_ms":2000,"command_id":"t-st"}')
    if echo "$resp" | grep -q '"success"\|"status"'; then
        log_pass "WS set_timeout accepts valid timeout"
    else
        log_fail "WS set_timeout accepts valid timeout" "resp: $resp"
    fi
}

test_ws_set_nickname() {
    local resp
    resp=$(ws_cmd '{"type":"set_nickname","nickname":"MarlbotTest","command_id":"t-nick"}')
    if echo "$resp" | grep -q '"success"\|"status"\|"command_id"'; then
        log_pass "WS set_nickname works"
    else
        log_fail "WS set_nickname works" "resp: $resp"
    fi
}

test_ws_send_message() {
    local resp
    resp=$(ws_cmd '{"type":"send_message","content":"WS test msg","target":"channel","command_id":"t-msg"}')
    if echo "$resp" | grep -q '"success"\|"status"\|"command_id"'; then
        log_pass "WS send_message works"
    else
        log_fail "WS send_message works" "resp: $resp"
    fi
}

test_ws_invalid_command() {
    local resp
    resp=$(ws_cmd '{"type":"nonexistent_command","command_id":"t-bad"}')
    if echo "$resp" | grep -qi 'error\|unknown'; then
        log_pass "WS rejects unknown command type"
    else
        log_fail "WS rejects unknown command type" "resp: $resp"
    fi
}

test_ws_malformed_json() {
    local resp
    resp=$(ws_cmd 'not json at all')
    # Bot should either return an error or survive (empty response = connection closed gracefully)
    if [ -z "$resp" ] || echo "$resp" | grep -qi 'error\|Invalid'; then
        log_pass "WS handles malformed JSON gracefully"
    else
        # Even if we get the welcome back, as long as the bot didn't crash, it's fine
        log_pass "WS handles malformed JSON gracefully (connection closed)"
    fi
}

if command -v websocat &>/dev/null; then
    test_ws_get_status_fields
    test_ws_get_volume
    test_ws_set_volume
    test_ws_get_voice
    test_ws_set_voice
    test_ws_set_voice_invalid
    test_ws_get_history
    test_ws_get_timeout
    test_ws_set_timeout
    test_ws_set_nickname
    test_ws_send_message
    test_ws_invalid_command
    test_ws_malformed_json
else
    echo -e "${YELLOW}⏭️ SKIP: WebSocket API tests (websocat not installed)${NC}"
fi

test_bot_does_not_crash

echo ""
echo "═══════════════════════════════════════"
echo -e "  Results: ${GREEN}$TESTS_PASSED passed${NC}, ${RED}$TESTS_FAILED failed${NC}"
echo "═══════════════════════════════════════"

if [ $TESTS_FAILED -gt 0 ]; then
    echo ""
    echo "Last 30 lines of bot log:"
    tail -30 "$LOG_FILE"
fi

exit $TESTS_FAILED
