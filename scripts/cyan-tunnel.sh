#!/bin/bash
# cyan-tunnel.sh
# Persistent SSH tunnels to Cyan dev infrastructure
# Keeps Lens (8080) and vLLM (8000) tunnels alive
#
# Usage:
#   ./cyan-tunnel.sh start     # Start tunnels (foreground)
#   ./cyan-tunnel.sh daemon    # Install as launchd service
#   ./cyan-tunnel.sh stop      # Stop tunnels and unload service
#   ./cyan-tunnel.sh status    # Check tunnel status
#   ./cyan-tunnel.sh logs      # Tail logs

set -e

BASTION="cyan-dev-bastion"
LENS_HOST="10.0.33.168"
LENS_PORT=8080
VLLM_PORT=8000

PLIST_NAME="com.cyan.dev.tunnels"
PLIST_PATH="$HOME/Library/LaunchAgents/${PLIST_NAME}.plist"
LOG_DIR="$HOME/.cyan/logs"
PID_FILE="$HOME/.cyan/tunnel.pid"
SCRIPT_PATH="$(cd "$(dirname "$0")" && pwd)/$(basename "$0")"

mkdir -p "$LOG_DIR" "$HOME/.cyan"

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[0;33m'
NC='\033[0m'

check_tunnels() {
    local lens_ok=false
    local vllm_ok=false
    
    if curl -s --connect-timeout 2 http://localhost:${LENS_PORT}/health >/dev/null 2>&1; then
        lens_ok=true
    fi
    if curl -s --connect-timeout 2 http://localhost:${VLLM_PORT}/v1/models >/dev/null 2>&1; then
        vllm_ok=true
    fi
    
    if $lens_ok && $vllm_ok; then
        echo -e "${GREEN}✅ Both tunnels active${NC}"
        echo "   Lens:  http://localhost:${LENS_PORT}"
        echo "   vLLM:  http://localhost:${VLLM_PORT}"
        return 0
    else
        $lens_ok && echo -e "   Lens:  ${GREEN}✅ OK${NC}" || echo -e "   Lens:  ${RED}❌ DOWN${NC}"
        $vllm_ok && echo -e "   vLLM:  ${GREEN}✅ OK${NC}" || echo -e "   vLLM:  ${RED}❌ DOWN${NC}"
        return 1
    fi
}

kill_existing() {
    # Kill any existing tunnels on these ports
    lsof -ti:${LENS_PORT} 2>/dev/null | xargs kill -9 2>/dev/null || true
    lsof -ti:${VLLM_PORT} 2>/dev/null | xargs kill -9 2>/dev/null || true
    
    # Kill autossh processes
    pkill -f "autossh.*${LENS_HOST}" 2>/dev/null || true
    
    # Clean stale SSH sockets
    rm -f ~/.ssh/sockets/*${BASTION}* 2>/dev/null || true
    
    sleep 1
}

start_tunnels() {
    echo "🚀 Starting Cyan dev tunnels..."
    
    kill_existing
    
    # Ensure autossh is installed
    if ! command -v autossh &>/dev/null; then
        echo "Installing autossh..."
        brew install autossh
    fi
    
    # Update security group with current IP
    MY_IP=$(curl -s --connect-timeout 5 checkip.amazonaws.com 2>/dev/null)
    if [ -n "$MY_IP" ]; then
        echo "📡 Updating SG with IP: ${MY_IP}"
        aws ec2 authorize-security-group-ingress \
            --group-id sg-0ec4a3ba22dfdc854 \
            --protocol tcp --port 22 \
            --cidr "${MY_IP}/32" \
            --region us-west-2 2>/dev/null || true
    fi
    
    # Start autossh with both tunnels in one connection
    # -M 0 = no monitoring port (uses ServerAliveInterval instead)
    # -N = no remote command
    # -o ServerAliveInterval=30 = send keepalive every 30s
    # -o ServerAliveCountMax=3 = disconnect after 3 missed keepalives
    # -o ExitOnForwardFailure=yes = exit if port forward fails
    # -o StrictHostKeyChecking=no = auto-accept host keys
    AUTOSSH_PIDFILE="$PID_FILE" \
    AUTOSSH_LOGFILE="$LOG_DIR/autossh.log" \
    AUTOSSH_GATETIME=0 \
    autossh -M 0 \
        -N \
        -o "ServerAliveInterval=30" \
        -o "ServerAliveCountMax=3" \
        -o "ExitOnForwardFailure=yes" \
        -o "StrictHostKeyChecking=no" \
        -o "ConnectTimeout=10" \
        -o "TCPKeepAlive=yes" \
        -L ${LENS_PORT}:${LENS_HOST}:${LENS_PORT} \
        -L ${VLLM_PORT}:${LENS_HOST}:${VLLM_PORT} \
        ${BASTION} \
        >> "$LOG_DIR/tunnel.log" 2>&1 &
    
    local pid=$!
    echo "$pid" > "$PID_FILE"
    
    echo "⏳ Waiting for tunnels..."
    for i in {1..15}; do
        sleep 1
        if check_tunnels 2>/dev/null; then
            echo ""
            echo -e "${GREEN}🟢 Tunnels ready${NC}"
            check_tunnels
            return 0
        fi
        echo -n "."
    done
    
    echo ""
    echo -e "${RED}❌ Tunnels failed to start. Check: $LOG_DIR/autossh.log${NC}"
    return 1
}

install_daemon() {
    echo "📦 Installing launchd service..."
    
    # Stop existing service
    launchctl unload "$PLIST_PATH" 2>/dev/null || true
    
    cat > "$PLIST_PATH" << PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>${PLIST_NAME}</string>
    <key>ProgramArguments</key>
    <array>
        <string>${SCRIPT_PATH}</string>
        <string>start</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <dict>
        <key>NetworkState</key>
        <true/>
    </dict>
    <key>ThrottleInterval</key>
    <integer>30</integer>
    <key>StandardOutPath</key>
    <string>${LOG_DIR}/launchd-stdout.log</string>
    <key>StandardErrorPath</key>
    <string>${LOG_DIR}/launchd-stderr.log</string>
    <key>EnvironmentVariables</key>
    <dict>
        <key>PATH</key>
        <string>/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin</string>
        <key>HOME</key>
        <string>${HOME}</string>
        <key>SSH_AUTH_SOCK</key>
        <string>${SSH_AUTH_SOCK:-/tmp/ssh-agent.sock}</string>
    </dict>
</dict>
</plist>
PLIST
    
    chmod 644 "$PLIST_PATH"
    launchctl load "$PLIST_PATH"
    
    echo -e "${GREEN}✅ Service installed and started${NC}"
    echo "   Plist: $PLIST_PATH"
    echo "   Logs:  $LOG_DIR/"
    echo ""
    echo "   The tunnel will:"
    echo "   - Start automatically on login"
    echo "   - Restart if it dies"
    echo "   - Reconnect when network changes"
    echo ""
    
    sleep 3
    check_tunnels
}

stop_tunnels() {
    echo "🛑 Stopping tunnels..."
    
    # Unload launchd service
    launchctl unload "$PLIST_PATH" 2>/dev/null || true
    
    kill_existing
    
    # Kill by PID file
    if [ -f "$PID_FILE" ]; then
        kill $(cat "$PID_FILE") 2>/dev/null || true
        rm -f "$PID_FILE"
    fi
    
    echo -e "${GREEN}✅ Tunnels stopped${NC}"
}

show_logs() {
    echo "📋 Tunnel logs:"
    echo "━━━━━━━━━━━━━━━"
    tail -50 "$LOG_DIR/autossh.log" 2>/dev/null || echo "No autossh logs"
    echo ""
    echo "━━━━━━━━━━━━━━━"
    tail -20 "$LOG_DIR/tunnel.log" 2>/dev/null || echo "No tunnel logs"
}

# Main
case "${1:-status}" in
    start)
        start_tunnels
        ;;
    daemon)
        install_daemon
        ;;
    stop)
        stop_tunnels
        ;;
    status)
        echo "🔍 Cyan Tunnel Status"
        echo "━━━━━━━━━━━━━━━━━━━━━"
        check_tunnels
        echo ""
        if [ -f "$PID_FILE" ]; then
            local_pid=$(cat "$PID_FILE")
            if kill -0 "$local_pid" 2>/dev/null; then
                echo "   autossh PID: $local_pid (running)"
            else
                echo "   autossh PID: $local_pid (stale)"
            fi
        fi
        if launchctl list | grep -q "$PLIST_NAME"; then
            echo "   launchd: ✅ loaded"
        else
            echo "   launchd: not loaded (run: ./cyan-tunnel.sh daemon)"
        fi
        ;;
    logs)
        show_logs
        ;;
    restart)
        stop_tunnels
        sleep 2
        start_tunnels
        ;;
    *)
        echo "Usage: $0 {start|daemon|stop|status|logs|restart}"
        echo ""
        echo "  start    Start tunnels in foreground"
        echo "  daemon   Install as launchd service (auto-start, auto-reconnect)"
        echo "  stop     Stop tunnels and unload service"
        echo "  status   Check tunnel health"
        echo "  logs     Show tunnel logs"
        echo "  restart  Stop and restart tunnels"
        exit 1
        ;;
esac
