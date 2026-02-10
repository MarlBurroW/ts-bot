#!/bin/bash
# TS3 Bot Integration Test Suite
# Requires: ts3-dev docker container running on port 9988 (TS3) / 10012 (ServerQuery)
#
# Usage: ./tests/integration_test.sh [test_name]
# If no test_name, runs all tests

set -euo pipefail

SQ_HOST="localhost"
SQ_PORT="10012"
SQ_USER="serveradmin"
SQ_PASS="dQ5ieLKV"
TS3_PORT="9988"
BOT_BINARY="./target/release/ts3_bot"
BOT_PID=""
WS_PORT="8081"  # Different from prod (8080)

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m'

TESTS_PASSED=0
TESTS_FAILED=0
TESTS_SKIPPED=0

# Send ServerQuery command and return response
sq_cmd() {
    echo -e "login $SQ_USER $SQ_PASS\nuse 1\n$1\nquit" | nc -q2 "$SQ_HOST" "$SQ_PORT" 2>/dev/null
}

# Send a channel text message via ServerQuery
sq_send_message() {
    local msg="$1"
    sq_cmd "sendtextmessage targetmode=2 msg=$msg"
}

# Get bot's client ID from the client list
get_bot_clid() {
    sq_cmd "clientlist" | grep -oP 'clid=\d+(?=.*client_nickname=Marlbot\b)' | head -1 | cut -d= -f2
}

# Check if bot is connected to TS3
bot_connected() {
    sq_cmd "clientlist" | grep -q "client_nickname=Marlbot"
}

# Check if WebSocket is responding
ws_connected() {
    echo '{"type":"get_status","command_id":"test-1"}' | timeout 3 websocat -1 "ws://localhost:$WS_PORT/ws" 2>/dev/null | grep -q "status"
}

log_pass() {
    echo -e "${GREEN}✅ PASS${NC}: $1"
    ((TESTS_PASSED++))
}

log_fail() {
    echo -e "${RED}❌ FAIL${NC}: $1"
    ((TESTS_FAILED++))
}

log_skip() {
    echo -e "${YELLOW}⏭️ SKIP${NC}: $1"
    ((TESTS_SKIPPED++))
}

log_info() {
    echo -e "${YELLOW}ℹ️${NC} $1"
}

# ============== SETUP ==============

setup() {
    log_info "Checking prerequisites..."
    
    # Check Docker TS3 is running
    if ! sudo docker ps --filter name=ts3-dev --format '{{.Names}}' | grep -q ts3-dev; then
        echo "ERROR: ts3-dev container not running. Start with:"
        echo "  sudo docker run -d --name ts3-dev -p 9988:9987/udp -p 10012:10011 -p 30034:30033 -e TS3SERVER_LICENSE=accept teamspeak:latest"
        exit 1
    fi
    
    # Check binary exists
    if [ ! -f "$BOT_BINARY" ]; then
        echo "ERROR: Bot binary not found at $BOT_BINARY. Run: cargo build --release"
        exit 1
    fi
    
    # Check websocat is available (for WS tests)
    if ! command -v websocat &>/dev/null; then
        log_info "websocat not found — installing..."
        cargo install websocat 2>/dev/null || log_info "Failed to install websocat, WS tests will be skipped"
    fi

    # Start bot pointed at dev server
    log_info "Starting bot against dev server (port $TS3_PORT, WS port $WS_PORT)..."
    
    # Create a test .env or pass env vars
    TS3_SERVER="localhost:$TS3_PORT" \
    WS_PORT="$WS_PORT" \
    TTS_ENABLED="false" \
    LOG_LEVEL="DEBUG" \
    "$BOT_BINARY" &
    BOT_PID=$!
    
    # Wait for bot to connect
    sleep 3
    
    if ! kill -0 "$BOT_PID" 2>/dev/null; then
        echo "ERROR: Bot crashed on startup"
        exit 1
    fi
    
    if bot_connected; then
        log_info "Bot connected to dev TS3 server ✅"
    else
        echo "ERROR: Bot not visible in TS3 client list"
        kill "$BOT_PID" 2>/dev/null
        exit 1
    fi
}

teardown() {
    log_info "Cleaning up..."
    if [ -n "$BOT_PID" ] && kill -0 "$BOT_PID" 2>/dev/null; then
        kill "$BOT_PID" 2>/dev/null
        wait "$BOT_PID" 2>/dev/null || true
    fi
}

trap teardown EXIT

# ============== TESTS ==============

test_bot_connects() {
    if bot_connected; then
        log_pass "Bot connects to TS3 server"
    else
        log_fail "Bot connects to TS3 server"
    fi
}

test_ws_responds() {
    if ! command -v websocat &>/dev/null; then
        log_skip "WebSocket responds (websocat not installed)"
        return
    fi
    
    local response
    response=$(echo '{"type":"get_status","command_id":"test-ws"}' | timeout 3 websocat -1 "ws://localhost:$WS_PORT/ws" 2>/dev/null || echo "")
    
    if echo "$response" | grep -q "command_id"; then
        log_pass "WebSocket responds to get_status"
    else
        log_fail "WebSocket responds to get_status (got: $response)"
    fi
}

test_no_self_message_loop() {
    # Send a message that contains "!help" as part of a longer text (like the bot's own response)
    # The bot should NOT respond to it because it comes from ServerQuery (different user)
    # But this tests that the bot doesn't loop on its own output
    
    # First, send !help and check bot responds
    sq_send_message "!help"
    sleep 2
    
    # Check logs for infinite loop indicators (multiple "TS3 Message" lines in quick succession)
    local msg_count
    msg_count=$(sudo journalctl -u ts3-bot --since '5 sec ago' --no-pager 2>/dev/null | grep -c "TS3 chat.*Commandes disponibles" || echo "0")
    
    if [ "$msg_count" -le 1 ]; then
        log_pass "No self-message loop on !help"
    else
        log_fail "Self-message loop detected ($msg_count responses)"
    fi
}

test_help_command() {
    sq_send_message "!help"
    sleep 2
    
    # Check if the bot sent a response (via logs since we can't easily read channel chat via SQ)
    if sudo journalctl -u ts3-bot --since '3 sec ago' --no-pager 2>/dev/null | grep -q "Commandes disponibles"; then
        log_pass "!help command responds"
    else
        log_fail "!help command responds (no response in logs)"
    fi
}

test_status_command() {
    sq_send_message "!status"
    sleep 2
    
    if sudo journalctl -u ts3-bot --since '3 sec ago' --no-pager 2>/dev/null | grep -q "Status Marlbot"; then
        log_pass "!status command responds"
    else
        log_fail "!status command responds"
    fi
}

test_who_command() {
    sq_send_message "!who"
    sleep 2
    
    if sudo journalctl -u ts3-bot --since '3 sec ago' --no-pager 2>/dev/null | grep -q "channel\|Channel\|who"; then
        log_pass "!who command responds"
    else
        log_fail "!who command responds"
    fi
}

test_unknown_command_ignored() {
    sq_send_message "hello this is just a regular message"
    sleep 1
    
    # Bot should NOT respond to non-command messages
    local response_count
    response_count=$(sudo journalctl -u ts3-bot --since '2 sec ago' --no-pager 2>/dev/null | grep -c "TS3 chat" || echo "0")
    
    if [ "$response_count" -eq 0 ]; then
        log_pass "Non-command messages are ignored"
    else
        log_fail "Non-command messages triggered a response"
    fi
}

# ============== MAIN ==============

echo "═══════════════════════════════════════"
echo "  TS3 Bot Integration Tests"
echo "═══════════════════════════════════════"
echo ""

# Note: for now these tests work against the RUNNING prod bot
# TODO: start a separate bot instance against ts3-dev

if [ "${1:-all}" = "all" ]; then
    # For now, test against prod bot with ServerQuery on the dev server
    # The real tests would start a bot against ts3-dev
    
    log_info "Running basic tests against dev TS3 server..."
    
    # Test ServerQuery connectivity
    if sq_cmd "version" | grep -q "version="; then
        log_pass "ServerQuery connection to dev server"
    else
        log_fail "ServerQuery connection to dev server"
        exit 1
    fi
    
    # Test we can send messages
    result=$(sq_send_message "test_message" 2>&1)
    if echo "$result" | grep -q "id=0"; then
        log_pass "Can send messages via ServerQuery"
    else
        log_fail "Can send messages via ServerQuery"
    fi
    
    echo ""
    echo "═══════════════════════════════════════"
    echo -e "  Results: ${GREEN}$TESTS_PASSED passed${NC}, ${RED}$TESTS_FAILED failed${NC}, ${YELLOW}$TESTS_SKIPPED skipped${NC}"
    echo "═══════════════════════════════════════"
else
    # Run specific test
    "test_$1"
fi

exit $TESTS_FAILED
