#!/bin/bash
# cyan-tunnel.sh — Simple persistent tunnel to Cyan GPU instance
# Usage: ./cyan-tunnel.sh start|stop|status|restart

BASTION="ec2-user@34.214.5.244"
KEY="$HOME/.ssh/cyan-infra.pem"
GPU_IP="10.0.48.183"
REGION="us-west-2"
SG_ID="sg-0ec4a3ba22dfdc854"

# Local port → Remote port mapping
# Local 9080 → GPU 8080 (Lens)
# Local 9000 → GPU 8000 (vLLM)
LOCAL_LENS=9080
LOCAL_VLLM=9000
REMOTE_LENS=8080
REMOTE_VLLM=8000

PID_FILE="$HOME/.cyan/tunnel.pid"
mkdir -p "$HOME/.cyan"

RED='\033[0;31m'
GREEN='\033[0;32m'
NC='\033[0m'

do_stop() {
    echo "🛑 Stopping tunnels..."
    [ -f "$PID_FILE" ] && kill $(cat "$PID_FILE") 2>/dev/null
    pkill -f "ssh.*${GPU_IP}" 2>/dev/null
    lsof -ti:${LOCAL_LENS} 2>/dev/null | xargs kill -9 2>/dev/null
    lsof -ti:${LOCAL_VLLM} 2>/dev/null | xargs kill -9 2>/dev/null
    rm -f ~/.ssh/sockets/*
    rm -f "$PID_FILE"
    sleep 2
    echo -e "${GREEN}✅ Stopped${NC}"
}

do_start() {
    echo "🚀 Starting Cyan tunnels..."
    do_stop 2>/dev/null

    # Update SG with current IP
    MY_IP=$(curl -s4 --connect-timeout 5 ifconfig.me)
    if [ -n "$MY_IP" ]; then
        echo "📡 IP: ${MY_IP} → updating security group"
        aws ec2 authorize-security-group-ingress \
            --group-id $SG_ID --protocol tcp --port 22 \
            --cidr "${MY_IP%.*}.0/24" --region $REGION 2>/dev/null || true
    fi

    # Start tunnel
    ssh -i $KEY \
        -o ControlMaster=no \
        -o ControlPath=none \
        -o ServerAliveInterval=30 \
        -o ServerAliveCountMax=3 \
        -o StrictHostKeyChecking=no \
        -o ConnectTimeout=10 \
        -L ${LOCAL_LENS}:${GPU_IP}:${REMOTE_LENS} \
        -L ${LOCAL_VLLM}:${GPU_IP}:${REMOTE_VLLM} \
        ${BASTION} -N &

    echo $! > "$PID_FILE"

    echo "⏳ Waiting for tunnels..."
    for i in $(seq 1 15); do
        sleep 2
        LENS=$(curl -sf --connect-timeout 2 http://localhost:${LOCAL_LENS}/health 2>/dev/null && echo "OK" || echo "")
        VLLM=$(curl -sf --connect-timeout 2 http://localhost:${LOCAL_VLLM}/v1/models 2>/dev/null && echo "OK" || echo "")
        if [ -n "$LENS" ] && [ -n "$VLLM" ]; then
            echo ""
            echo -e "${GREEN}✅ Tunnels ready${NC}"
            echo "   Lens:  http://localhost:${LOCAL_LENS}"
            echo "   vLLM:  http://localhost:${LOCAL_VLLM}"
            return 0
        fi
        echo -n "."
    done
    echo ""
    echo -e "${RED}❌ Tunnel timeout. Check bastion connectivity.${NC}"
    return 1
}

do_status() {
    echo "🔍 Cyan Tunnel Status"
    echo "━━━━━━━━━━━━━━━━━━━━━"
    LENS=$(curl -sf --connect-timeout 2 http://localhost:${LOCAL_LENS}/health 2>/dev/null && echo "OK" || echo "")
    VLLM=$(curl -sf --connect-timeout 2 http://localhost:${LOCAL_VLLM}/v1/models 2>/dev/null && echo "OK" || echo "")
    [ -n "$LENS" ] && echo -e "   Lens:  ${GREEN}✅ OK${NC} (localhost:${LOCAL_LENS})" || echo -e "   Lens:  ${RED}❌ DOWN${NC}"
    [ -n "$VLLM" ] && echo -e "   vLLM:  ${GREEN}✅ OK${NC} (localhost:${LOCAL_VLLM})" || echo -e "   vLLM:  ${RED}❌ DOWN${NC}"
    [ -f "$PID_FILE" ] && echo "   PID:   $(cat $PID_FILE)" || echo "   PID:   none"
}

case "${1:-status}" in
    start)   do_start ;;
    stop)    do_stop ;;
    status)  do_status ;;
    restart) do_stop; sleep 1; do_start ;;
    *)       echo "Usage: $0 {start|stop|status|restart}" ;;
esac
