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
    echo -e "login $SQ_USER $SQ_PASS\nuse 1\n$1\nquit" | nc -q2 "$SQ_HOST" "$SQ_PORT" 2>/dev/null
}

# Send channel message and return bot's response from logs
sq_send_and_check() {
    local msg="$1"
    local expect="$2"
    local timeout="${3:-3}"
    
    # Mark log position
    local log_lines_before=$(wc -l < "$LOG_FILE" 2>/dev/null || echo 0)
    
    # Send message
    sq_cmd "sendtextmessage targetmode=2 msg=$(echo "$msg" | sed 's/ /\\s/g')" >/dev/null 2>&1
    
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
        resp=$(echo '{"type":"get_status","command_id":"test-1"}' | timeout 3 websocat -1 "ws://localhost:$WS_PORT/ws" 2>/dev/null || echo "")
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
