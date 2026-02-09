#!/usr/bin/env node

/**
 * WebSocket Test Script for TS3 Bot
 *
 * Usage: node test-websocket.js
 */

const WebSocket = require('ws');

const WS_URL = 'ws://127.0.0.1:8080/ws';

console.log('🔌 Connecting to TS3 Bot WebSocket...');
console.log(`   URL: ${WS_URL}\n`);

const ws = new WebSocket(WS_URL);

ws.on('open', () => {
    console.log('✅ WebSocket connected successfully!\n');

    // Send a test ping after connection
    setTimeout(() => {
        console.log('📤 Sending test message...');
        ws.send(JSON.stringify({
            command: 'ping',
            command_id: 'test-1'
        }));
    }, 1000);
});

ws.on('message', (data) => {
    try {
        const message = JSON.parse(data.toString());
        console.log('📥 Received message:');
        console.log(JSON.stringify(message, null, 2));
        console.log('');
    } catch (e) {
        console.log('📥 Received raw message:', data.toString());
        console.log('');
    }
});

ws.on('error', (error) => {
    console.error('❌ WebSocket error:', error.message);
});

ws.on('close', (code, reason) => {
    console.log(`\n🔌 WebSocket closed`);
    console.log(`   Code: ${code}`);
    console.log(`   Reason: ${reason || 'No reason provided'}`);
    process.exit(0);
});

// Handle Ctrl+C
process.on('SIGINT', () => {
    console.log('\n\n⏹️  Closing connection...');
    ws.close();
});

// Keep alive - send ping every 30 seconds
setInterval(() => {
    if (ws.readyState === WebSocket.OPEN) {
        console.log('💓 Sending keepalive ping...');
        ws.ping();
    }
}, 30000);

console.log('Press Ctrl+C to exit\n');
console.log('─'.repeat(50));
